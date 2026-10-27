//! Brush triggers driven by player touch: once / multiple, hurt, teleport and
//! push, with spawnflag and filter checks. Also presses floor buttons the
//! player stands on.

use bevy::prelude::*;

use super::io::{FireOutput, InputCall, InputHandlers, fire_output};
use super::props::FloorButton;
use super::{EntityRegistry, MapEntity, SpawnHandlers};
use crate::collision::Physics;
use crate::models::StudioAnimator;
use crate::player::health::DamagePlayer;
use crate::player::{Player, PlayerMoveSet};
use crate::world::{CurrentMap, LoadMap};

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

const TRIGGER_CLASSES: [&str; 7] = [
    "trigger_once",
    "trigger_multiple",
    "trigger_hurt",
    "trigger_teleport",
    "trigger_push",
    "trigger_look",
    "trigger_portal_cleanser",
];

#[derive(Clone, Debug)]
pub enum TriggerKind {
    Plain,
    /// Deals `damage` every half second while touched.
    Hurt { damage: f32, next: f64 },
    Teleport { target: String, landmark: Option<String> },
    Push { dir: Vec3, speed: f32 },
    /// Emancipation field: removes the player's portals when crossed.
    Cleanser,
}

#[derive(Component)]
pub struct Trigger {
    pub enabled: bool,
    pub once: bool,
    /// Seconds before OnTrigger may fire again (-1 = never again).
    pub wait: f32,
    pub next_allowed: f64,
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
        "trigger_push" => TriggerKind::Push {
            dir: crate::coords::angles_forward(me.get_vec3("pushdir").unwrap_or(Vec3::ZERO)),
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
        allow_players: flags & (SF_CLIENTS | SF_EVERYTHING) != 0 || me.classname == "trigger_portal_cleanser",
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

/// Evaluates `filter_activator_name` / `filter_activator_class` style filters.
fn passes_filter(
    filter: &Option<String>,
    activator: Entity,
    registry: &EntityRegistry,
    entities: &Query<&MapEntity>,
    is_player: bool,
) -> bool {
    let Some(name) = filter else { return true };
    let Some(&f) = registry.by_name.get(name).and_then(|l| l.first()) else { return true };
    let Ok(fe) = entities.get(f) else { return true };
    let negate = fe.get_bool("negated");
    let result = match fe.classname.as_str() {
        "filter_activator_class" => {
            let class = fe.get("filterclass").unwrap_or("").to_ascii_lowercase();
            let act_class = if is_player { "player".to_owned() } else { entities.get(activator).map(|m| m.classname.clone()).unwrap_or_default() };
            class == act_class
        }
        "filter_activator_name" => {
            let wanted = fe.get("filtername").unwrap_or("").to_ascii_lowercase();
            if is_player {
                wanted == "!player" || wanted == "player"
            } else {
                entities.get(activator).ok().and_then(|m| m.targetname.clone()).is_some_and(|n| n == wanted)
            }
        }
        _ => true,
    };
    result != negate
}

#[allow(clippy::too_many_arguments)]
fn touch_triggers(
    time: Res<Time>,
    physics: Physics,
    registry: Res<EntityRegistry>,
    entities: Query<&MapEntity>,
    mut players: Query<(Entity, &mut Player)>,
    mut triggers: Query<(Entity, &mut Trigger)>,
    transforms: Query<&GlobalTransform>,
    mut out: MessageWriter<FireOutput>,
    mut hurt: MessageWriter<DamagePlayer>,
    mut fizzle: MessageWriter<crate::portals::FizzlePortals>,
) {
    let now = time.elapsed_secs_f64();
    let Ok((pe, mut player)) = players.single_mut() else { return };
    if player.noclip {
        return;
    }
    let (mins, maxs) = player.state.hull();
    let origin = player.state.origin;
    let mut teleport: Option<(Vec3, Option<Vec3>)> = None;
    for (te, mut t) in &mut triggers {
        if !t.enabled {
            continue;
        }
        let inside = t.allow_players
            && physics.touches(te, origin, mins, maxs)
            && passes_filter(&t.filter, pe, &registry, &entities, true);
        let was = t.touching.contains(&pe);
        if inside && !was {
            if t.touching.is_empty() {
                out.write(FireOutput::new(te, "OnStartTouchAll").with_activator(Some(pe)));
            }
            t.touching.push(pe);
            out.write(FireOutput::new(te, "OnStartTouch").with_activator(Some(pe)));
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
                        teleport = Some((dt.translation() + offset.unwrap_or(Vec3::ZERO), None));
                    }
                }
                TriggerKind::Cleanser => {
                    fizzle.write(crate::portals::FizzlePortals { slot: None });
                    out.write(FireOutput::new(te, "OnFizzle").with_activator(Some(pe)));
                }
                _ => {}
            }
        } else if !inside && was {
            t.touching.retain(|e| *e != pe);
            out.write(FireOutput::new(te, "OnEndTouch").with_activator(Some(pe)));
            if t.touching.is_empty() {
                out.write(FireOutput::new(te, "OnEndTouchAll").with_activator(Some(pe)));
            }
        }
        if inside {
            if let TriggerKind::Push { dir, speed } = t.kind {
                player.state.velocity += dir * speed * time.delta_secs();
            }
            if let TriggerKind::Hurt { damage, next } = &mut t.kind
                && *damage > 0.0
                && now >= *next
            {
                *next = now + 0.5;
                hurt.write(DamagePlayer { amount: *damage, push: Vec3::ZERO });
                out.write(FireOutput::new(te, "OnHurtPlayer").with_activator(Some(pe)));
            }
            if now >= t.next_allowed && t.next_allowed >= 0.0 {
                out.write(FireOutput::new(te, "OnTrigger").with_activator(Some(pe)));
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
    }
    if let Some((pos, _)) = teleport {
        player.state.origin = pos;
        player.previous_origin = pos;
        player.state.velocity = Vec3::ZERO;
    }
}

/// Floor buttons press while the player or a suitable prop rests on them.
#[allow(clippy::type_complexity)]
fn press_floor_buttons(
    time: Res<Time>,
    players: Query<&Player>,
    mut buttons: Query<(Entity, &mut FloorButton, &GlobalTransform, Option<&mut StudioAnimator>, &MapEntity)>,
    props: Query<(&MapEntity, &crate::collision::Collider, &GlobalTransform), (With<crate::entities::props::PhysicsProp>, Without<FloorButton>)>,
    mut out: MessageWriter<FireOutput>,
) {
    let Ok(player) = players.single() else { return };
    let (pmins, pmaxs) = player.state.hull();
    let now = time.elapsed_secs_f64();
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
        let player_on = !cube_only
            && !ball_only
            && boxes_overlap(player.state.origin + (pmins + pmaxs) * 0.5, (pmaxs - pmins) * 0.5);
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

/// Reloads the current map shortly after death.
fn respawn_after_death(
    time: Res<Time>,
    mut deaths: MessageReader<PlayerDied>,
    mut pending: Local<Option<f64>>,
    map: Option<Res<CurrentMap>>,
    mut load: MessageWriter<LoadMap>,
) {
    let now = time.elapsed_secs_f64();
    if deaths.read().last().is_some() && pending.is_none() {
        info!("player died");
        *pending = Some(now + 1.5);
    }
    if let (Some(at), Some(map)) = (*pending, map)
        && now >= at
    {
        *pending = None;
        load.write(LoadMap(map.name.clone()));
    }
}
