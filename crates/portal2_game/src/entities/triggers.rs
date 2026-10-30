//! Brush triggers driven by player touch: once / multiple, hurt, teleport and
//! push, with spawnflag and filter checks. Also presses floor buttons the
//! player stands on.

use bevy::prelude::*;

use super::io::{FireOutput, InputCall, InputHandlers, fire_output};
use super::props::FloorButton;
use super::{EntityRegistry, MapEntity, SpawnHandlers};
use crate::player::Player;
use crate::player::health::DamagePlayer;
use portal2_core::clock::GameClock;
use portal2_core::sets::PlayerMoveSet;
use portal2_render::models::StudioAnimator;
use portal2_world::CurrentMap;
use portal2_world::collision::Physics;

pub fn register(app: &mut App) {
    app.add_message::<PlayerDied>()
        .add_systems(FixedUpdate, (touch_triggers, press_floor_buttons).after(PlayerMoveSet))
        .add_systems(Update, respawn_after_death);
    let world = app.world_mut();
    {
        let mut s = world.resource_mut::<SpawnHandlers>();
        for class in TRIGGER_CLASSES {
            s.register(class, spawn_trigger);
        }
    }
    let mut h = world.resource_mut::<InputHandlers>();
    for class in TRIGGER_CLASSES {
        h.register(class, trigger_input);
    }
}

const TRIGGER_CLASSES: [&str; 9] = [
    "trigger_autosave",
    "trigger_once",
    "trigger_multiple",
    "trigger_hurt",
    "trigger_teleport",
    "trigger_push",
    "trigger_look",
    "trigger_portal_cleanser",
    "trigger_paint_cleanser",
];

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum TriggerKind {
    Plain,
    /// Deals `damage` every half second while touched.
    Hurt {
        damage: f32,
        next: f64,
    },
    Teleport {
        target: String,
        landmark: Option<String>,
    },
    Push {
        dir: Vec3,
        speed: f32,
    },
    /// Emancipation field: removes the player's portals when crossed.
    Cleanser,
    /// Washes paint off props passing through (see paint/props.rs).
    PaintCleanser,
    /// Autosaves when the player enters, then goes away; a dangerous
    /// timer makes it a dangerous autosave.
    Autosave {
        dangerous: f32,
        min_health: f32,
    },
}

#[derive(Component, serde::Serialize, serde::Deserialize)]
pub struct Trigger {
    pub enabled: bool,
    pub once: bool,
    /// Seconds before OnTrigger may fire again (-1 = never again).
    pub wait: f32,
    pub next_allowed: f64,
    #[serde(with = "portal2_core::save::ids::entities")]
    pub touching: Vec<Entity>,
    pub allow_players: bool,
    pub filter: Option<String>,
    pub kind: TriggerKind,
}

/// spawnflags: 1 = clients, 64 = everything.
const SF_CLIENTS: u32 = 1;
const SF_EVERYTHING: u32 = 64;

fn spawn_trigger(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e).cloned() else { return };
    let kind = match me.classname.as_str() {
        "trigger_hurt" => TriggerKind::Hurt { damage: me.get_f32("damage").unwrap_or(10.0), next: 0.0 },
        "trigger_teleport" => TriggerKind::Teleport {
            target: me.get("target").unwrap_or("").to_ascii_lowercase(),
            landmark: me.get("landmark").filter(|s| !s.is_empty()).map(|s| s.to_ascii_lowercase()),
        },
        "trigger_portal_cleanser" => TriggerKind::Cleanser,
        "trigger_paint_cleanser" => TriggerKind::PaintCleanser,
        "trigger_autosave" => TriggerKind::Autosave {
            dangerous: me.get_f32("dangeroustimer").unwrap_or(0.0),
            min_health: me.get_f32("minimumhitpoints").unwrap_or(0.0),
        },
        "trigger_push" => TriggerKind::Push {
            dir: portal2_core::coords::angles_forward(me.get_vec3("pushdir").unwrap_or(Vec3::ZERO)),
            speed: me.get_f32("speed").unwrap_or(40.0),
        },
        _ => TriggerKind::Plain,
    };
    let flags = me.spawnflags();
    let t = Trigger {
        enabled: !me.get_bool("startdisabled"),
        once: me.classname == "trigger_once",
        wait: me.get_f32("wait").unwrap_or(if me.classname == "trigger_multiple" { 1.0 } else { -1.0 }),
        next_allowed: 0.0,
        touching: Vec::new(),
        allow_players: flags & (SF_CLIENTS | SF_EVERYTHING) != 0
            || matches!(me.classname.as_str(), "trigger_portal_cleanser" | "trigger_autosave"),
        filter: me.get("filtername").filter(|s| !s.is_empty()).map(|s| s.to_ascii_lowercase()),
        kind,
    };
    world.entity_mut(e).insert(t);
}

fn trigger_input(world: &mut World, call: &InputCall) -> bool {
    let e = call.target;
    let Some(mut t) = world.get_mut::<Trigger>(e) else { return false };
    match call.input.as_str() {
        "enable" => t.enabled = true,
        "disable" => {
            t.enabled = false;
            t.touching.clear();
        }
        "toggle" => t.enabled = !t.enabled,
        "touchtest" => {
            let touching = !t.touching.is_empty();
            fire_output(world, e, if touching { "OnTouching" } else { "OnNotTouching" }, call.activator, None);
        }
        "starttouch" | "endtouch" => {}
        _ => return false,
    }
    true
}

/// Something that can touch a trigger this tick.
struct Toucher {
    entity: Entity,
    is_player: bool,
    center: Vec3,
    half: Vec3,
}

/// Filter entity evaluation: class, name and model filters, held-object
/// filters and `filter_multi` combinations, each optionally negated.
fn passes_filter(
    filter: &Option<String>,
    activator: Entity,
    is_player: bool,
    registry: &EntityRegistry,
    entities: &Query<&MapEntity>,
    held: Option<Entity>,
) -> bool {
    let Some(name) = filter else { return true };
    filter_named(name, activator, is_player, registry, entities, held, 0)
}

fn filter_named(
    name: &str,
    activator: Entity,
    is_player: bool,
    registry: &EntityRegistry,
    entities: &Query<&MapEntity>,
    held: Option<Entity>,
    depth: u32,
) -> bool {
    let Some(&f) = registry.by_name.get(name).and_then(|l| l.first()) else { return true };
    let Ok(fe) = entities.get(f) else { return true };
    let negate = fe.get_bool("negated");
    let act = entities.get(activator).ok();
    let result = match fe.classname.as_str() {
        "filter_activator_class" => {
            let class = fe.get("filterclass").unwrap_or("").to_ascii_lowercase();
            let act_class = if is_player { "player".to_owned() } else { act.map(|m| m.classname.clone()).unwrap_or_default() };
            class == act_class
        }
        "filter_activator_name" => {
            let wanted = fe.get("filtername").unwrap_or("").to_ascii_lowercase();
            if is_player {
                wanted == "!player" || wanted == "player"
            } else {
                act.and_then(|m| m.targetname.clone()).is_some_and(|n| match wanted.strip_suffix('*') {
                    Some(prefix) => n.starts_with(prefix),
                    None => n == wanted,
                })
            }
        }
        "filter_activator_model" => {
            let wanted = fe.get("model").unwrap_or("").to_ascii_lowercase();
            act.and_then(|m| m.get("model")).is_some_and(|m| m.eq_ignore_ascii_case(&wanted))
        }
        "filter_player_held" => held == Some(activator),
        "filter_multi" if depth < 8 => {
            let any = fe.get_i32("filtertype") == Some(1);
            let subs: Vec<String> =
                (1..=10).filter_map(|i| fe.get(&format!("filter{i:02}")).map(str::to_ascii_lowercase)).filter(|s| !s.is_empty()).collect();
            let mut results = subs.iter().map(|s| filter_named(s, activator, is_player, registry, entities, held, depth + 1));
            if any { results.any(|r| r) } else { results.all(|r| r) }
        }
        _ => true,
    };
    result != negate
}

/// spawnflags that admit physics objects.
const SF_PHYSICS: u32 = 8;
/// trigger_teleport: keep the toucher's view angles.
const SF_PRESERVE_ANGLES: u32 = 32;

/// The player (unless in noclip) and physics props touch triggers; props
/// only those that admit physics objects. Filters see each toucher as the
/// activator.
fn touch_triggers(
    clock: Res<GameClock>,
    physics: Physics,
    registry: Res<EntityRegistry>,
    entities: Query<&MapEntity>,
    holding: Option<Res<crate::physics::carry::Holding>>,
    props: Query<(Entity, &portal2_world::collision::Collider, &GlobalTransform), With<crate::entities::props::PhysicsProp>>,
    mut players: Query<(Entity, &mut Player)>,
    mut input: ResMut<crate::player::PlayerInput>,
    mut triggers: Query<(Entity, &mut Trigger)>,
    transforms: Query<&GlobalTransform>,
    mut out: MessageWriter<FireOutput>,
    mut hurt: MessageWriter<DamagePlayer>,
    mut fizzle: MessageWriter<crate::portals::FizzlePortals>,
    mut autosave: MessageWriter<portal2_core::save::AutosaveRequest>,
) {
    let now = clock.curtime;
    let held = holding.and_then(|h| h.entity);
    let Ok((pe, mut player)) = players.single_mut() else { return };
    let (mins, maxs) = player.state.hull();
    let origin = player.state.origin;
    let mut touchers = Vec::new();
    if !player.noclip {
        touchers.push(Toucher { entity: pe, is_player: true, center: origin + (mins + maxs) * 0.5, half: (maxs - mins) * 0.5 });
    }
    for (e, col, gt) in &props {
        let t = gt.compute_transform();
        let (lo, hi) = crate::physics::world_bounds(&col.shape, t.translation, t.rotation);
        touchers.push(Toucher { entity: e, is_player: false, center: (lo + hi) * 0.5, half: (hi - lo) * 0.5 });
    }
    // Destination, view angles to snap to, whether velocity is kept.
    let mut teleport: Option<(Vec3, Option<Vec3>, bool)> = None;
    for (te, mut t) in &mut triggers {
        if !t.enabled {
            continue;
        }
        let flags = entities.get(te).map(|m| m.spawnflags()).unwrap_or(0);
        let allow_props = flags & (SF_PHYSICS | SF_EVERYTHING) != 0;
        for toucher in &touchers {
            let allowed = if toucher.is_player { t.allow_players } else { allow_props };
            let inside = allowed
                && physics.touches(te, toucher.center - toucher.half, Vec3::ZERO, toucher.half * 2.0)
                && passes_filter(&t.filter, toucher.entity, toucher.is_player, &registry, &entities, held);
            let who = toucher.entity;
            let was = t.touching.contains(&who);
            if inside && !was {
                if t.touching.is_empty() {
                    out.write(FireOutput::new(te, "OnStartTouchAll").with_activator(Some(who)));
                }
                t.touching.push(who);
                out.write(FireOutput::new(te, "OnStartTouch").with_activator(Some(who)));
                if toucher.is_player {
                    match &t.kind {
                        TriggerKind::Teleport { target, landmark } => {
                            if let Some(&dest) = registry.by_name.get(target).and_then(|l| l.first())
                                && let Ok(dt) = transforms.get(dest)
                            {
                                let offset = landmark
                                    .as_ref()
                                    .and_then(|l| registry.by_name.get(l).and_then(|v| v.first()))
                                    .and_then(|&l| transforms.get(l).ok())
                                    .map(|lt| origin - lt.translation());
                                // Without a landmark the player faces the
                                // destination's way and stops, unless the
                                // trigger preserves angles; with one both
                                // carry over.
                                let reorient = offset.is_none() && flags & SF_PRESERVE_ANGLES == 0;
                                let angles = reorient.then(|| portal2_core::coords::quat_to_angles(dt.compute_transform().rotation));
                                teleport = Some((dt.translation() + offset.unwrap_or(Vec3::ZERO), angles, !reorient));
                            }
                        }
                        TriggerKind::Autosave { dangerous, min_health } => {
                            autosave.write(portal2_core::save::AutosaveRequest {
                                dangerous: *dangerous,
                                min_health: *min_health,
                                commit_health: 0.0,
                                remove: Some(te),
                            });
                        }
                        TriggerKind::Cleanser => {
                            fizzle.write(crate::portals::FizzlePortals { slot: None });
                            out.write(FireOutput::new(te, "OnFizzle").with_activator(Some(who)));
                        }
                        _ => {}
                    }
                }
            } else if !inside && was {
                t.touching.retain(|e| *e != who);
                out.write(FireOutput::new(te, "OnEndTouch").with_activator(Some(who)));
                if t.touching.is_empty() {
                    out.write(FireOutput::new(te, "OnEndTouchAll").with_activator(Some(who)));
                }
            }
            if !inside {
                continue;
            }
            if toucher.is_player {
                if let TriggerKind::Push { dir, speed } = t.kind {
                    player.state.velocity += dir * speed * clock.frametime();
                }
                if let TriggerKind::Hurt { damage, next } = &mut t.kind
                    && *damage > 0.0
                    && now >= *next
                {
                    *next = now + 0.5;
                    hurt.write(DamagePlayer { amount: *damage, push: Vec3::ZERO, source: Some(te) });
                    out.write(FireOutput::new(te, "OnHurtPlayer").with_activator(Some(who)));
                }
            }
            if now >= t.next_allowed && t.next_allowed >= 0.0 {
                out.write(FireOutput::new(te, "OnTrigger").with_activator(Some(who)));
                if t.once {
                    t.enabled = false;
                    t.next_allowed = -1.0;
                } else if t.wait >= 0.0 {
                    t.next_allowed = now + t.wait.max(0.05) as f64;
                } else {
                    t.next_allowed = -1.0;
                }
            }
        }
        // Forget touchers that no longer exist.
        t.touching.retain(|e| touchers.iter().any(|t| t.entity == *e));
    }
    if let Some((pos, angles, keep_velocity)) = teleport {
        player.state.origin = pos;
        player.previous_origin = pos;
        player.ground_ref = None;
        if !keep_velocity {
            player.state.velocity = Vec3::ZERO;
        }
        if let Some(a) = angles {
            input.input.angles = a;
        }
    }
}

/// Floor buttons press while the player or a suitable prop rests on them.
fn press_floor_buttons(
    clock: Res<GameClock>,
    players: Query<&Player>,
    mut buttons: Query<(Entity, &mut FloorButton, &GlobalTransform, Option<&mut StudioAnimator>, &MapEntity)>,
    props: Query<
        (&MapEntity, &portal2_world::collision::Collider, &GlobalTransform),
        (With<crate::entities::props::PhysicsProp>, Without<FloorButton>),
    >,
    mut out: MessageWriter<FireOutput>,
) {
    let Ok(player) = players.single() else { return };
    let (pmins, pmaxs) = player.state.hull();
    let now = clock.curtime;
    for (e, mut b, gt, anim, me) in &mut buttons {
        // Cube buttons take cubes only, ball buttons spheres only; the player
        // presses the ordinary kinds.
        let cube_only = me.classname == "prop_floor_cube_button";
        let ball_only = me.classname == "prop_floor_ball_button";
        let accepts = |prop: &MapEntity| -> bool {
            let is_cube = prop.classname == "prop_weighted_cube";
            let is_ball = is_cube && prop.get_i32("cubetype") == Some(3);
            if ball_only {
                is_ball
            } else if cube_only {
                is_cube && !is_ball
            } else {
                true
            }
        };
        let (lo, hi) = match anim.as_ref() {
            Some(a) => {
                let m = &a.model.studio.mdl;
                let lo = m.hull_min.min(m.view_bbmin);
                let hi = m.hull_max.max(m.view_bbmax);
                (Vec3::new(lo.x + 4.0, lo.y + 4.0, hi.z - 8.0), Vec3::new(hi.x - 4.0, hi.y - 4.0, hi.z + 24.0))
            }
            None => (Vec3::new(-24.0, -24.0, 0.0), Vec3::new(24.0, 24.0, 32.0)),
        };
        let inv = gt.affine().inverse();
        let boxes_overlap = |c: Vec3, half: Vec3| {
            let p = inv.transform_point3(c);
            p.x + half.x > lo.x
                && p.x - half.x < hi.x
                && p.y + half.y > lo.y
                && p.y - half.y < hi.y
                && p.z + half.z > lo.z
                && p.z - half.z < hi.z
        };
        let player_on = !cube_only && !ball_only && boxes_overlap(player.state.origin + (pmins + pmaxs) * 0.5, (pmaxs - pmins) * 0.5);
        let prop_on = props.iter().any(|(pm, col, pgt)| {
            if !accepts(pm) {
                return false;
            }
            let t = pgt.compute_transform();
            let (mins, maxs) = crate::physics::world_bounds(&col.shape, t.translation, t.rotation);
            boxes_overlap((mins + maxs) * 0.5, (maxs - mins) * 0.5)
        });
        let overlap = player_on || prop_on;
        if overlap {
            b.last_contact = now;
        }
        let pressed = overlap || now - b.last_contact < 0.1;
        if pressed != b.pressed {
            b.pressed = pressed;
            if let Some(mut a) = anim {
                a.set_sequence(if pressed { "down" } else { "up" });
            }
            out.write(FireOutput::new(e, if pressed { "OnPressed" } else { "OnUnPressed" }));
        }
    }
}

/// Raised when the player touches something lethal.
#[derive(Message, Clone, Copy, Debug)]
pub struct PlayerDied;

/// After death the most recent save is reloaded (`reload`) once
/// `sk_auto_reload_time` seconds have passed.
fn respawn_after_death(
    time: Res<Time>,
    mut deaths: MessageReader<PlayerDied>,
    mut pending: Local<Option<(f64, String)>>,
    map: Option<Res<CurrentMap>>,
    mut console: ResMut<portal2_core::console::Console>,
) {
    let now = time.elapsed_secs_f64();
    let Some(map) = map else { return };
    if deaths.read().last().is_some() && pending.is_none() {
        info!("player died");
        *pending = Some((now + console.f32("sk_auto_reload_time") as f64, map.name.clone()));
    }
    // A level change in the meantime cancels the reload.
    if let Some((at, died_on)) = pending.clone()
        && now >= at
    {
        *pending = None;
        if died_on == map.name {
            console.add_text("reload\n");
        }
    }
}

impl portal2_core::save::Restore for Trigger {}
