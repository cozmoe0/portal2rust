//! Floor turrets (`npc_portal_turret_floor`, `docs/re/turret.md`): physics
//! props that think every 0.1 s. Closed, a turret watches for the player;
//! on sight it deploys and fires once its wind-up, longer the further the
//! player is, has passed, turning its head toward them and shooting when
//! it points within 10° of them. Losing sight it scans for a few seconds,
//! then folds up. It talks throughout; carrying, lasers and disabling stop
//! it.
//!
//! Each shot plays `fire` or `fire2` (the first or second pair of guns),
//! whose model events flash their muzzles (`effects::muzzle_flash`), and
//! leaves from the next of the four muzzles in turn.
//!
//! Portals don't hide the player: a turret looking into a linked portal
//! sees them beyond the other one (aiming at where they appear through it),
//! its bullets and their tracers carry on through, and so does its laser
//! sight.
//!
//! Knocked over (server.dll's tipped think), it sounds its alarm and thrashes
//! for 2 to 2.5 s, thinking every 0.05 s: firing each think and jerking its
//! head toward a new random aim up to 60° away. Then it turns to face ahead,
//! says its last line (`NPC_FloorTurret.Die` and the disabled line), folds up
//! (`retract`, `NPC_FloorTurret.Retract`) and, once folded, plays its dying
//! `eye_off` and its eye goes out. A turret started on uneven ground stays
//! put until something moves it (`physics`: it starts at rest).
//!
//! A laser that stops on a turret, even a dead one, sets it on fire
//! (server.dll: the laser's hit `sub_103B27D0` ignites it; `sub_10325D10`,
//! `sub_103248D0`; also the `Ignite` input): it stops
//! thinking for itself, burns for `sv_portal_turret_min_burn_time` to
//! `sv_portal_turret_max_burn_time` (1 to 1.5 s), crying out once in four
//! as it catches (`TalkStartBurning`) and every 0.5 to 0.75 s after
//! (`TalkBurned`), then blows apart (`turret_break`), firing `OnExplode`.

use avian3d::prelude::LinearVelocity;
use bevy::prelude::*;
use source_formats::bsp::contents;

use bevy::camera::visibility::NoFrustumCulling;

use super::laser::{self, LaserBeamMaterial, LaserHitTurret};
use super::{BeamSegment, EffectMaterial, linked_frames, trace_beam};
use crate::entities::SpawnHandlers;
use crate::entities::io::{FireOutput, InputCall, InputHandlers};
use crate::entities::props::PhysicsProp;
use crate::entities::spawner::{ModelOptions, attach_studio_model};
use crate::player::Player;
use crate::player::health::DamagePlayer;
use crate::portals::{Portal, Portals};
use crate::visuals::sprites::{SpriteMaterial, SpriteParams, load_sprite_source, sprite_quad};
use portal2_audio::SoundRequest;
use portal2_core::clock::GameClock;
use portal2_core::entity::MapEntity;
use portal2_core::sets::ThinkSet;
use portal2_core::vfs::GameFs;
use portal2_render::models::StudioAnimator;
use portal2_render::textures::TextureCache;
use portal2_world::MapObject;
use portal2_world::collision::portal::{PortalFrame, transfer};
use portal2_world::collision::{Physics, mask};

const THINK: f64 = 0.1;
/// `PortalTurretBullet` damage, of which players take half
/// (`sk_dmg_take_scale1`).
const BULLET_DAMAGE: f32 = 3.0;
const PLAYER_DAMAGE_SCALE: f32 = 0.5;
/// Fires when pointing within 10° of the player (45° when closer than
/// `CLOSE_RANGE`).
const FIRE_COS: f32 = 0.984_807_7;
#[allow(clippy::approx_constant, reason = "the game's value, not 1/sqrt(2)")]
const CLOSE_FIRE_COS: f32 = 0.7071;
const CLOSE_RANGE: f32 = 60.0;
/// The view cone: within 60° to the sides and
/// `sv_portal_turret_fire_cone_z_tolerance` (45°) up or down.
const VIEW_COS: f32 = 0.5;
const VIEW_Z_DEGREES: f32 = 45.0;
/// Head aim limits (the model's pose parameters) and step per think.
const AIM_YAW_LIMIT: f32 = 60.0;
const AIM_PITCH_LIMIT: f32 = 15.0;
const AIM_STEP: f32 = 12.0;
/// Seconds of scanning after losing the player (spawn flag 128: shorter).
const SEARCH_TIME: f64 = 4.0;
const SEARCH_TIME_SHORT: f64 = 1.5;
const SHORT_SEARCH_FLAG: u32 = 128;
/// Wind-up before the first shot at the edge of the range.
const MAX_WIND_UP: f64 = 2.5;
/// Seconds a tipped turret thrashes before giving up (random in between),
/// thinking and firing every `TIPPED_THINK`, with random aims up to
/// `THRASH_AIM` degrees away.
const THRASH_TIME: (f32, f32) = (2.0, 2.5);
const TIPPED_THINK: f64 = 0.05;
const THRASH_AIM: f32 = 60.0;
/// The four muzzles, fired in turn: the first pair plays `fire`, the
/// second `fire2`.
const MUZZLES: [&str; 4] = ["LFT_Gun1_Muzzle", "RT_Gun1_Muzzle", "LFT_Gun2_Muzzle", "RT_Gun2_Muzzle"];
/// Up axis further from vertical than 60° counts as knocked over.
const UPRIGHT_COS: f32 = 0.5;
/// Seconds a burning turret lasts (`sv_portal_turret_min_burn_time`,
/// `sv_portal_turret_max_burn_time`), and between its cries.
const BURN_TIME: (f32, f32) = (1.0, 1.5);
const BURN_TALK_GAP: (f32, f32) = (0.5, 0.75);
/// Walls, glass, props and light bridges block a turret's view.
const SIGHT_MASK: u32 = mask::SOLID_ALL | contents::BLOCKLOS;
/// The server sets the model from `ModelIndex` whatever the map says (the
/// entity definition's `models/props/turret_01.mdl` is a self-lit leftover);
/// 1 is unused and keeps the normal model.
const MODELS: [&str; 5] = [
    "models/npcs/turret/turret.mdl",
    "models/npcs/turret/turret.mdl",
    "models/npcs/turret/turret_boxed.mdl",
    "models/npcs/turret/turret_backwards.mdl",
    "models/npcs/turret/turret_skeleton.mdl",
];
/// The laser sight (client.dll `sub_10271940`): a beam of `effects/redlaser1`
/// 0.75 units wide, coloured (255, 32, 32) at full brightness, with
/// `sprites/redlaserglow` (the server's halo, scale 4) where it ends.
const SIGHT_MATERIAL: &str = "effects/redlaser1";
const SIGHT_WIDTH: f32 = 0.75;
const SIGHT_COLOR: [f32; 3] = [255.0, 32.0, 32.0];
const SIGHT_HALO: &str = "sprites/redlaserglow";
const SIGHT_HALO_SCALE: f32 = 4.0;

pub struct TurretPlugin;

impl Plugin for TurretPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<TurretExploded>()
            .add_systems(FixedUpdate, run_turrets.in_set(ThinkSet))
            .add_systems(Update, (fade_tracers, show_aim, draw_lasers))
            // After everything else this frame has had its say about it.
            .add_systems(Last, remove_exploded);
        let world = app.world_mut();
        world.resource_mut::<SpawnHandlers>().register("npc_portal_turret_floor", spawn_turret);
        world.resource_mut::<InputHandlers>().register("npc_portal_turret_floor", turret_input);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TurretState {
    /// Closed, watching for the player.
    #[serde(alias = "Searching")]
    Idle,
    /// Open: winding up, then firing at the player.
    #[serde(alias = "WindingUp", alias = "Firing")]
    Active,
    /// Lost the player: scanning until the search time runs out.
    Scanning,
    /// Knocked over: thrashing and spraying bullets until it gives up,
    /// then turning to face ahead.
    Tipped,
    /// Given up: folding (`retract`) before it dies.
    Closing,
    /// Set on fire by a laser: burning until it blows apart.
    Burning,
    Dead,
}

/// A turret blew apart: where it stood, how it was moving and its model
/// (for the pieces).
#[derive(Message, Clone, Debug)]
pub struct TurretExploded {
    pub entity: Entity,
    pub at: Transform,
    pub velocity: Vec3,
    pub model: Option<String>,
}

/// What the eye shows (server.dll's eye states): bright on its target,
/// blinking while searching, dim closed, out when dead or disabled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum EyeState {
    SeeTarget,
    Seeking,
    #[default]
    Dormant,
    Dead,
    Disabled,
}

/// What the turret last talked about (index into the game's line table).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum Talk {
    Search,
    Active,
    Deploy,
    Retire,
    Tipped,
    Disabled,
    Pickup,
}

impl Talk {
    fn sound(self) -> &'static str {
        match self {
            Talk::Search => "NPC_FloorTurret.TalkSearch",
            Talk::Active => "NPC_FloorTurret.TalkActive",
            Talk::Deploy => "NPC_FloorTurret.TalkDeploy",
            Talk::Retire => "NPC_FloorTurret.TalkRetire",
            Talk::Tipped => "NPC_FloorTurret.TalkTipped",
            Talk::Disabled => "NPC_FloorTurret.TalkDisabled",
            Talk::Pickup => "NPC_FloorTurret.TalkPickup",
        }
    }

    /// Quiet time after the line.
    fn gap(self) -> f64 {
        match self {
            Talk::Retire => 3.5,
            Talk::Tipped => 1.15,
            Talk::Pickup => 2.25,
            _ => 1.75,
        }
    }

    /// States that may speak at once when entered.
    fn urgent(self) -> bool {
        matches!(self, Talk::Retire | Talk::Tipped | Talk::Pickup)
    }
}

#[derive(Component, serde::Serialize, serde::Deserialize)]
pub struct Turret {
    pub enabled: bool,
    pub state: TurretState,
    pub range: f32,
    /// Eye position in the turret's local space.
    pub eye: Vec3,
    #[serde(default)]
    next_think: f64,
    #[serde(default)]
    shot_time: f64,
    #[serde(default)]
    search_until: f64,
    /// Head (yaw, pitch) in degrees.
    #[serde(default)]
    aim: Vec2,
    #[serde(default)]
    muzzle: usize,
    #[serde(default)]
    talk: Option<Talk>,
    #[serde(default)]
    next_talk: f64,
    #[serde(default)]
    next_active_talk: f64,
    #[serde(default)]
    next_activate_sound: f64,
    #[serde(default)]
    pub tipped_since: Option<f64>,
    #[serde(default)]
    gagged: bool,
    #[serde(default)]
    held: bool,
    /// When a tipped turret stops thrashing.
    #[serde(default)]
    thrash_until: f64,
    #[serde(default)]
    pub eye_state: EyeState,
    /// The seeking eye blinks between two looks, one per think.
    #[serde(default)]
    pub eye_blink: bool,
    /// When a burning turret blows apart, and when it next cries out.
    #[serde(default)]
    pub burn_until: f64,
    #[serde(default)]
    next_burn_talk: f64,
}

fn spawn_turret(world: &mut World, e: Entity) {
    let Some(me) = world.get_mut::<MapEntity>(e).map(|mut me| {
        let index = me.get_i32("modelindex").unwrap_or(0).clamp(0, MODELS.len() as i32 - 1) as usize;
        me.set("model", MODELS[index]);
        me.clone()
    }) else {
        return;
    };
    let opts = ModelOptions { skin: me.get_i32("skinnumber").unwrap_or(0).max(0) as usize, ..default() };
    let model = me.studio_model().and_then(|p| attach_studio_model(world, e, &p, opts));
    // Eye from the `eyes` attachment in the rest pose, else above the centre.
    let eye = model
        .as_ref()
        .and_then(|m| {
            let mdl = &m.studio.mdl;
            let a = mdl.attachments.iter().find(|a| a.name.to_ascii_lowercase().contains("eye"))?;
            let bone = m.default_pose.get(a.bone.max(0) as usize).copied().unwrap_or(Mat4::IDENTITY);
            Some((bone * a.local).transform_point3(Vec3::ZERO))
        })
        .or_else(|| model.as_ref().map(|m| (m.studio.mdl.hull_min + m.studio.mdl.hull_max) * 0.5 + Vec3::Z * 12.0))
        .unwrap_or(Vec3::Z * 40.0);
    if let Some(mut anim) = world.get_mut::<StudioAnimator>(e) {
        anim.set_default_sequence("idle");
        anim.set_sequence("idle");
    }
    let now = world.resource::<GameClock>().curtime;
    world.entity_mut(e).insert((
        PhysicsProp,
        Turret {
            enabled: true,
            state: TurretState::Idle,
            range: me.get_f32("turretrange").unwrap_or(1024.0),
            eye,
            next_think: now + THINK,
            shot_time: 0.0,
            search_until: 0.0,
            aim: Vec2::ZERO,
            muzzle: 0,
            talk: None,
            next_talk: 0.0,
            next_active_talk: 0.0,
            next_activate_sound: 0.0,
            tipped_since: None,
            gagged: me.get_i32("gagged").unwrap_or(0) != 0,
            held: false,
            thrash_until: 0.0,
            eye_state: EyeState::Dormant,
            eye_blink: false,
            burn_until: 0.0,
            next_burn_talk: 0.0,
        },
    ));
    // Placed in the map (spawned as it loads), it stands where it was put
    // until something moves it; one a dropper makes later falls as usual.
    if now < MAP_START_SPAWN {
        world.entity_mut(e).insert(crate::physics::StartsAtRest::default());
    }
}

/// Entities spawned before this much map time came with the map.
const MAP_START_SPAWN: f64 = 0.05;

fn turret_input(world: &mut World, call: &InputCall) -> bool {
    // `SetModel <index>`: one of the models `ModelIndex` picks from
    // (`sp_a2_bts4`'s conveyor makes malfunctioning turrets of some as they
    // pass its reject trigger). The turret is set up again on the new model,
    // keeping whether it is enabled and gagged.
    if call.input == "setmodel" {
        let e = call.target;
        let Ok(index) = call.param.trim().parse::<i32>() else { return true };
        let Some(kept) = world.get::<Turret>(e).map(|t| (t.enabled, t.gagged)) else { return false };
        if let Some(mut me) = world.get_mut::<MapEntity>(e) {
            me.set("modelindex", &index.to_string());
        }
        crate::entities::spawner::clear_render_children(world, e);
        spawn_turret(world, e);
        if let Some(mut t) = world.get_mut::<Turret>(e) {
            (t.enabled, t.gagged) = kept;
        }
        return true;
    }
    let now = world.resource::<GameClock>().curtime;
    let Some(mut t) = world.get_mut::<Turret>(call.target) else { return false };
    match call.input.as_str() {
        // `CBaseAnimating`'s inputs: set on fire as a laser would.
        "ignite" | "ignitelifetime" | "ignitenumhitboxfires" | "ignitehitboxfires" => {
            if t.state != TurretState::Burning {
                t.ignite(now);
            }
        }
        "enable" => t.enabled = true,
        "disable" => t.enabled = false,
        "toggle" => t.enabled = !t.enabled,
        "enablegagging" => t.gagged = true,
        "disablegagging" => t.gagged = false,
        "selfdestruct" | "selfdestructimmediately" => {
            t.state = TurretState::Dead;
            t.eye_state = EyeState::Dead;
        }
        "firebullet" | "enablepickup" | "disablepickup" => {}
        _ => return false,
    }
    true
}

/// A bullet's tracer: a short streak racing from the muzzle to where the
/// bullet went, as the game's line-and-whiz tracers fly (64 to 128 units
/// long, 0.5 to 1.5 wide, thousands of units a second).
#[derive(Component)]
struct Tracer {
    start: Vec3,
    dir: Vec3,
    distance: f32,
    length: f32,
    width: f32,
    spawned: f64,
}

const TRACER_SPEED: f32 = 6000.0;

/// What a turret does in one think, applied after the per-turret logic.
struct Effects<'a> {
    now: f64,
    sounds: Vec<SoundRequest>,
    outputs: Vec<FireOutput>,
    sequences: Vec<(&'a str, &'a str)>,
}

/// World placement of a model attachment from its joints' current
/// placement (the head's aim and the deployed guns included).
fn attachment_placement(anim: &StudioAnimator, joints: &Query<&GlobalTransform>, name: &str) -> Option<Transform> {
    let mdl = &anim.model.studio.mdl;
    let a = mdl.attachments.get(mdl.find_attachment(name)?)?;
    let joint = joints.get(*anim.joints.get(a.bone.max(0) as usize)?).ok()?;
    Some(joint.compute_transform() * Transform::from_matrix(a.local))
}

impl Turret {
    /// Sets it on fire; true when it cries out as it catches (one time in
    /// four, unless gagged).
    fn ignite(&mut self, now: f64) -> bool {
        self.state = TurretState::Burning;
        self.burn_until = now + portal2_core::rng::random_range(BURN_TIME.0, BURN_TIME.1) as f64;
        self.next_think = now + THINK;
        debug!("turret ignited, burning until {:.2}", self.burn_until);
        if portal2_core::rng::random_range(0.0, 4.0) < 1.0 {
            self.next_burn_talk = now + 1.0;
            return !self.gagged;
        }
        false
    }

    /// Speaks the line for `talk` when it changed and the quiet time is over.
    fn say(&mut self, e: Entity, talk: Talk, fx: &mut Effects) {
        if self.talk == Some(talk) {
            return;
        }
        if talk.urgent() {
            self.next_talk = fx.now - 1.0;
        }
        if fx.now <= self.next_talk {
            return;
        }
        self.talk = Some(talk);
        self.next_talk = fx.now + talk.gap();
        if !self.gagged {
            fx.sounds.push(SoundRequest::on(talk.sound(), e));
        }
    }

    /// Turns the head toward local (yaw, pitch) by at most one step.
    fn turn_toward(&mut self, goal: Vec2) {
        let goal = Vec2::new(goal.x.clamp(-AIM_YAW_LIMIT, AIM_YAW_LIMIT), goal.y.clamp(-AIM_PITCH_LIMIT, AIM_PITCH_LIMIT));
        let d = (goal - self.aim).clamp(Vec2::splat(-AIM_STEP), Vec2::splat(AIM_STEP));
        self.aim += d;
    }

    /// The muzzle direction in the turret's local space.
    fn aim_dir(&self) -> Vec3 {
        let (y, p) = (self.aim.x.to_radians(), self.aim.y.to_radians());
        Vec3::new(y.cos() * p.cos(), y.sin() * p.cos(), -p.sin())
    }
}

/// Local (yaw, pitch) of a direction in the turret's frame, pitch positive
/// downward like view angles.
fn local_angles(dir: Vec3) -> Vec2 {
    Vec2::new(dir.y.atan2(dir.x).to_degrees(), (-dir.z).atan2(dir.truncate().length()).to_degrees())
}

fn run_turrets(
    clock: Res<GameClock>,
    physics: Physics,
    mut turrets: Query<(Entity, &mut Turret, &GlobalTransform, &MapEntity, Option<&LinearVelocity>, Option<&mut StudioAnimator>)>,
    joints: Query<&GlobalTransform>,
    players: Query<(Entity, &Player)>,
    holding: Option<Res<crate::physics::carry::Holding>>,
    (portals, portal_query): (Res<Portals>, Query<&Portal>),
    mut lasered: MessageReader<LaserHitTurret>,
    mut exploded: MessageWriter<TurretExploded>,
    mut damage: MessageWriter<DamagePlayer>,
    mut out: MessageWriter<FireOutput>,
    mut sound_out: MessageWriter<SoundRequest>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<EffectMaterial>>,
    mut shared: Local<Option<(Handle<Mesh>, Handle<EffectMaterial>)>>,
) {
    let now = clock.curtime;
    let held = holding.and_then(|h| h.entity);
    // A laser on a turret sets it on fire.
    for hit in lasered.read() {
        if let Ok((e, mut t, ..)) = turrets.get_mut(hit.0)
            && t.state != TurretState::Burning
            && t.ignite(now)
        {
            sound_out.write(SoundRequest::on("NPC_FloorTurret.TalkStartBurning", e));
        }
    }
    // The game's turrets shoot at a player flying with noclip too.
    let player = players.single().ok();
    let frames = linked_frames(&portals, &portal_query);
    for (e, mut t, gt, me, velocity, anim) in &mut turrets {
        if t.state == TurretState::Dead || now < t.next_think {
            continue;
        }
        t.next_think = now + THINK;
        // On fire: cries out until it blows apart.
        if t.state == TurretState::Burning {
            if now > t.burn_until {
                t.state = TurretState::Dead;
                t.eye_state = EyeState::Dead;
                sound_out.write(SoundRequest::stop("NPC_FloorTurret.TalkBurned", e));
                exploded.write(TurretExploded {
                    entity: e,
                    at: gt.compute_transform(),
                    velocity: velocity.map_or(Vec3::ZERO, |v| v.0),
                    model: me.studio_model(),
                });
                out.write(FireOutput::new(e, "OnExplode"));
            } else if now > t.next_burn_talk {
                if !t.gagged {
                    sound_out.write(SoundRequest::on("NPC_FloorTurret.TalkBurned", e));
                }
                t.next_burn_talk = now + portal2_core::rng::random_range(BURN_TALK_GAP.0, BURN_TALK_GAP.1) as f64;
            }
            continue;
        }
        let mut fx = Effects { now, sounds: Vec::new(), outputs: Vec::new(), sequences: Vec::new() };
        let tr = gt.compute_transform();
        let eye = tr.transform_point(t.eye);
        let up = tr.rotation * Vec3::Z;
        // The muzzle the next shot leaves from, and its direction.
        let muzzle = |t: &Turret, anim: &Option<Mut<StudioAnimator>>| {
            anim.as_deref().and_then(|a| attachment_placement(a, &joints, MUZZLES[(t.muzzle + 1) % 4]))
        };

        // Carried: no shooting, complains once.
        let carried = held == Some(e);
        if carried != t.held {
            t.held = carried;
            if carried {
                t.say(e, Talk::Pickup, &mut fx);
            }
        }

        // Folding up after giving up: dies once folded.
        if t.state == TurretState::Closing {
            if anim.as_ref().is_none_or(|a| a.finished) {
                t.state = TurretState::Dead;
                t.eye_state = EyeState::Dead;
                fx.sequences.push(("eye_off", "idle_eye_off"));
            }
            flush(fx, &mut sound_out, &mut out, anim);
            continue;
        }

        // Knocked over: thrashes and sprays bullets for a while, then faces
        // ahead and gives up.
        if up.z < UPRIGHT_COS && !carried {
            t.next_think = now + TIPPED_THINK;
            if t.state != TurretState::Tipped {
                t.state = TurretState::Tipped;
                t.tipped_since = Some(now);
                t.thrash_until = now + portal2_core::rng::random_range(THRASH_TIME.0, THRASH_TIME.1) as f64;
                t.eye_state = EyeState::SeeTarget;
                if !t.gagged {
                    fx.sounds.push(SoundRequest::on("NPC_FloorTurret.Alarm", e));
                }
                t.say(e, Talk::Tipped, &mut fx);
            }
            if now < t.thrash_until {
                if now >= t.shot_time {
                    let (start, dir) = match muzzle(&t, &anim) {
                        Some(m) => (m.translation, m.rotation * Vec3::X),
                        None => (eye, tr.rotation * t.aim_dir()),
                    };
                    fire(
                        &mut t,
                        e,
                        start,
                        dir,
                        None,
                        &physics,
                        frames,
                        &mut fx,
                        &mut commands,
                        &mut damage,
                        &mut meshes,
                        &mut materials,
                        &mut shared,
                    );
                    t.shot_time = now + TIPPED_THINK;
                }
                let r = || portal2_core::rng::random_range(-THRASH_AIM, THRASH_AIM);
                let goal = Vec2::new(r(), r());
                t.turn_toward(goal);
            } else if t.aim.length_squared() > 1e-4 {
                // Faces ahead first, its guns still out.
                t.turn_toward(Vec2::ZERO);
                fx.sequences.push(("idlealert", "idlealert"));
            } else {
                t.state = TurretState::Closing;
                if !t.gagged {
                    fx.sounds.push(SoundRequest::on("NPC_FloorTurret.Die", e));
                    fx.sounds.push(SoundRequest::on(Talk::Disabled.sound(), e));
                    fx.sounds.push(SoundRequest::on("NPC_FloorTurret.Retract", e));
                }
                fx.sequences.push(("retract", "retract"));
                fx.outputs.push(FireOutput::new(e, "OnTipped"));
            }
            flush(fx, &mut sound_out, &mut out, anim);
            continue;
        }
        t.tipped_since = None;
        if t.state == TurretState::Tipped {
            t.state = TurretState::Idle;
            t.aim = Vec2::ZERO;
            fx.sequences.push(("idle", "idle"));
        }

        // The player, when this turret can see them inside its cone:
        // straight there, or through a linked portal.
        let range = t.range;
        let target = player.and_then(|(pe, p)| {
            let (mins, maxs) = p.state.hull();
            let center = p.state.origin + (mins + maxs) * 0.5;
            // How far away a point is, and its direction in the turret's
            // frame, when it lies within the view cone.
            let in_cone = |aim: Vec3| {
                let to = aim - eye;
                let dist = to.length();
                if dist > range || dist < 1.0 {
                    return None;
                }
                let local = tr.rotation.inverse() * (to / dist);
                let flat = local.truncate().normalize_or_zero();
                (flat.x >= VIEW_COS && local_angles(local).y.abs() <= VIEW_Z_DEGREES).then_some((dist, local))
            };
            if let Some((dist, local)) = in_cone(center)
                && physics.trace(eye, center, Vec3::ZERO, Vec3::ZERO, SIGHT_MASK, &[e, pe]).fraction >= 1.0
            {
                return Some((pe, center, center, dist, local));
            }
            let frames = frames?;
            (0..2).find_map(|slot| {
                let aim = through_portal(&physics, &frames, slot, eye, center, &[e, pe])?;
                let (dist, local) = in_cone(aim)?;
                Some((pe, center, aim, dist, local))
            })
        });
        let target = target.filter(|_| t.enabled && !carried);

        match (t.state, target) {
            (TurretState::Idle | TurretState::Scanning, Some((_, center, aim, dist, _))) => {
                debug!("turret {e} found the player{}", if aim == center { "" } else { " through a portal" });
                // Deploy and wind up, longer the further away the player is.
                t.shot_time = now + ((dist / t.range).powi(2) as f64) * MAX_WIND_UP;
                if t.state == TurretState::Idle {
                    fx.sequences.push(("deploy", "idlealert"));
                }
                t.state = TurretState::Active;
                t.eye_state = EyeState::SeeTarget;
                if now > t.next_activate_sound {
                    fx.sounds.push(SoundRequest::on("NPC_FloorTurret.Activate", e));
                    t.next_activate_sound = now + 3.0;
                }
                t.say(e, Talk::Deploy, &mut fx);
                fx.outputs.push(FireOutput::new(e, "OnFoundTarget"));
            }
            (TurretState::Active, Some((pe, center, aim, dist, local))) => {
                t.turn_toward(local_angles(local));
                let muzzle_dir = tr.rotation * t.aim_dir();
                let to = (aim - eye).normalize_or_zero();
                let cos = if dist < CLOSE_RANGE { CLOSE_FIRE_COS } else { FIRE_COS };
                if now > t.shot_time && muzzle_dir.dot(to) >= cos {
                    let start = muzzle(&t, &anim).map_or(eye, |m| m.translation);
                    fire(
                        &mut t,
                        e,
                        start,
                        (aim - start).normalize_or(to),
                        Some((pe, center)),
                        &physics,
                        frames,
                        &mut fx,
                        &mut commands,
                        &mut damage,
                        &mut meshes,
                        &mut materials,
                        &mut shared,
                    );
                    if now > t.next_active_talk {
                        t.next_active_talk = now + 2.5;
                        if !t.gagged {
                            fx.sounds.push(SoundRequest::on(Talk::Active.sound(), e));
                        }
                    }
                }
            }
            (TurretState::Active, None) => {
                let short = me.spawnflags() & SHORT_SEARCH_FLAG != 0;
                t.search_until = now + if short { SEARCH_TIME_SHORT } else { SEARCH_TIME };
                t.state = TurretState::Scanning;
                t.eye_state = EyeState::Seeking;
                let line = if t.enabled { Talk::Search } else { Talk::Disabled };
                t.say(e, line, &mut fx);
                fx.outputs.push(FireOutput::new(e, "OnLostTarget"));
            }
            (TurretState::Scanning, None) => {
                if now > t.search_until {
                    // Folds back up.
                    t.state = TurretState::Idle;
                    t.eye_state = EyeState::Dormant;
                    t.aim = Vec2::ZERO;
                    fx.sequences.push(("retract", "idle"));
                    fx.sounds.push(SoundRequest::on("NPC_FloorTurret.Retire", e));
                    t.say(e, Talk::Retire, &mut fx);
                } else {
                    t.eye_blink = !t.eye_blink;
                    // Sweeps its head around while searching.
                    let s = now as f32;
                    t.turn_toward(Vec2::new((s * 2.0).sin() * 20.0, (s * 1.3).sin() * 20.0));
                }
            }
            _ => {}
        }
        // Switched off, its eye goes out until it is switched back on.
        if !t.enabled {
            t.eye_state = EyeState::Disabled;
        } else if t.eye_state == EyeState::Disabled {
            t.eye_state = EyeState::Dormant;
        }
        flush(fx, &mut sound_out, &mut out, anim);
    }
}

/// Plays a think's sounds, outputs and animation changes, and shows the
/// head aim.
fn flush(fx: Effects, sounds: &mut MessageWriter<SoundRequest>, out: &mut MessageWriter<FireOutput>, anim: Option<Mut<StudioAnimator>>) {
    for s in fx.sounds {
        sounds.write(s);
    }
    for o in fx.outputs {
        out.write(o);
    }
    if let Some(mut anim) = anim {
        for (play, then) in fx.sequences {
            anim.set_sequence(play);
            anim.set_default_sequence(then);
        }
    }
}

/// Where the player at `center` appears to a turret at `eye` looking into
/// portal `slot`, when it can see them that way: the line to that point
/// crosses the portal's opening, nothing blocks it up to the portal, and
/// nothing blocks the rest of the way out of the other one.
fn through_portal(physics: &Physics, frames: &[PortalFrame; 2], slot: usize, eye: Vec3, center: Vec3, ignore: &[Entity]) -> Option<Vec3> {
    let (entry, exit) = (&frames[slot], &frames[1 - slot]);
    if exit.to_local(center).x <= 0.0 {
        return None;
    }
    let aim = transfer(exit, entry).transform_point3(center);
    let (from, to) = (entry.to_local(eye), entry.to_local(aim));
    if from.x <= 1.0 || to.x >= 0.0 {
        return None;
    }
    let crossing = from + (to - from) * (from.x / (from.x - to.x));
    if !entry.in_opening(crossing, 0.0) {
        return None;
    }
    let crossing = entry.to_world(crossing);
    let dir = (aim - eye).normalize_or_zero();
    if physics.trace(eye, crossing - dir * 2.0, Vec3::ZERO, Vec3::ZERO, SIGHT_MASK, ignore).fraction < 1.0 {
        return None;
    }
    let m = transfer(entry, exit);
    let out = m.transform_point3(crossing) + m.transform_vector3(dir) * 2.0;
    (physics.trace(out, center, Vec3::ZERO, Vec3::ZERO, SIGHT_MASK, ignore).fraction >= 1.0).then_some(aim)
}

/// One bullet along `dir` from the next muzzle (at `start`), on through
/// any linked portal it enters: plays that pair's firing sequence (whose
/// events flash the muzzles), damages the player when it reaches them and
/// leaves a tracer along its way.
#[allow(clippy::too_many_arguments)]
fn fire(
    t: &mut Turret,
    e: Entity,
    start: Vec3,
    dir: Vec3,
    target: Option<(Entity, Vec3)>,
    physics: &Physics,
    frames: Option<[PortalFrame; 2]>,
    fx: &mut Effects,
    commands: &mut Commands,
    damage: &mut MessageWriter<DamagePlayer>,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<EffectMaterial>,
    shared: &mut Option<(Handle<Mesh>, Handle<EffectMaterial>)>,
) {
    t.muzzle = (t.muzzle + 1) % 4;
    fx.sequences.push((if t.muzzle < 2 { "fire" } else { "fire2" }, "idlealert"));
    let mut path = trace_beam(physics, frames, start, dir, mask::SOLID_ALL, &[e], 4, |_| None);
    let through = path.len() > 1;
    if let Some(last) = path.last_mut()
        && let Some((pe, center)) = target
        && (last.hit_entity == Some(pe) || last.start.distance(last.end) + 1.0 >= last.start.distance(center))
    {
        last.end = center;
        debug!("turret {e} shot the player{}", if through { " through a portal" } else { "" });
        let push = last.dir.with_z(0.0).normalize_or_zero() * 4.0;
        damage.write(DamagePlayer { amount: BULLET_DAMAGE * PLAYER_DAMAGE_SCALE, push, source: Some(e) });
    }
    fx.sounds.push(SoundRequest::on("NPC_FloorTurret.ShotSounds", e));
    let (mesh, material) = shared
        .get_or_insert_with(|| {
            (meshes.add(super::tube_mesh(4)), materials.add(EffectMaterial::glow(Vec3::new(1.0, 0.82, 0.5) * 1.2, 6.0, 0.02)))
        })
        .clone();
    // One streak racing along the whole path, a piece per stretch, each
    // setting off as the one before reaches its portal.
    let (length, width) = (portal2_core::rng::random_range(64.0, 128.0), portal2_core::rng::random_range(0.5, 1.5));
    let mut delay = 0.0;
    for seg in &path {
        let distance = seg.start.distance(seg.end);
        let tracer =
            Tracer { start: seg.start, dir: (seg.end - seg.start).normalize_or(seg.dir), distance, length, width, spawned: fx.now + delay };
        delay += f64::from(distance / TRACER_SPEED);
        let place = tracer.place(fx.now);
        commands.spawn((tracer, MapObject, Mesh3d(mesh.clone()), MeshMaterial3d(material.clone()), place.unwrap_or_default()));
    }
}

impl Tracer {
    /// The streak's placement at `now` (nothing yet before it sets off);
    /// None once it has arrived.
    fn place(&self, now: f64) -> Option<Transform> {
        if now < self.spawned {
            return Some(Transform::from_scale(Vec3::ZERO));
        }
        let head = ((now - self.spawned) as f32 * TRACER_SPEED).clamp(0.0, self.distance);
        let tail = (head - self.length).max(0.0);
        if tail >= self.distance - 0.01 || (now - self.spawned) as f32 * TRACER_SPEED > self.distance + self.length {
            return None;
        }
        let w = self.width * 0.5;
        Some(super::segment_transform(self.start + self.dir * tail, self.start + self.dir * head.max(tail + 1.0), w, w))
    }
}

fn fade_tracers(clock: Res<GameClock>, mut tracers: Query<(Entity, &Tracer, &mut Transform)>, mut commands: Commands) {
    for (e, t, mut place) in &mut tracers {
        match t.place(clock.curtime) {
            Some(p) => *place = p,
            None => commands.entity(e).despawn(),
        }
    }
}

/// Shows the head aim through the model's pose parameters.
pub fn show_aim(mut turrets: Query<(&Turret, &mut StudioAnimator)>) {
    for (t, mut anim) in &mut turrets {
        anim.set_pose_parameter("aim_yaw", t.aim.x);
        anim.set_pose_parameter("aim_pitch", t.aim.y);
    }
}

/// A turret's laser sight: a beam (and, with the visual overhaul, a soft
/// red halo along it) for each stretch of it, on through linked portals,
/// and the glow where it ends.
#[derive(Component)]
struct LaserSight {
    beams: Vec<(Entity, Entity)>,
    halo: Entity,
}

impl LaserSight {
    fn parts(&self) -> impl Iterator<Item = Entity> + '_ {
        self.beams.iter().flat_map(|&(beam, glow)| [beam, glow]).chain(std::iter::once(self.halo))
    }
}

/// Where a turret's laser sight runs this frame (for its light).
#[derive(Component, Clone, Debug, Default)]
pub struct LaserSightPath {
    pub segments: Vec<BeamSegment>,
}

struct SightAssets {
    beam_mesh: Handle<Mesh>,
    beam: Handle<LaserBeamMaterial>,
    glow: Handle<LaserBeamMaterial>,
    halo_mesh: Handle<Mesh>,
    halo: Handle<SpriteMaterial>,
}

/// The overhaul's halo along a laser sight: units each side of the beam,
/// and its colour (HDR, so the bloom takes it).
const SIGHT_GLOW_HALF_WIDTH: f32 = 1.6;
const SIGHT_GLOW_COLOR: Vec3 = Vec3::new(1.0, 0.035, 0.03);
/// Stretches a laser sight may run through portals.
const SIGHT_SEGMENTS: usize = 4;

/// Draws each turret's laser sight from the eye along the head's aim to
/// whatever it meets, on through linked portals.
#[allow(clippy::too_many_arguments)]
fn draw_lasers(
    physics: Physics,
    mut commands: Commands,
    mut turrets: Query<(Entity, &Turret, &GlobalTransform, Option<&mut LaserSight>)>,
    settings: Res<crate::visuals::fx::FxSettings>,
    (portals, portal_query): (Res<Portals>, Query<&Portal>),
    mut placed: Query<(&mut Transform, &mut Visibility), Without<Turret>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut beams: ResMut<Assets<LaserBeamMaterial>>,
    mut sprites: ResMut<Assets<SpriteMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut textures: ResMut<TextureCache>,
    fs: Res<GameFs>,
    mut shared: Local<Option<SightAssets>>,
) {
    let frames = linked_frames(&portals, &portal_query);
    for (e, t, gt, sight) in &mut turrets {
        let on = t.enabled && !matches!(t.state, TurretState::Dead | TurretState::Closing);
        let tr = gt.compute_transform();
        let eye = tr.transform_point(t.eye);
        let dir = tr.rotation * t.aim_dir();
        let start = eye + dir * 2.0;
        let path = trace_beam(&physics, frames, start, dir, mask::SOLID_ALL, &[e], SIGHT_SEGMENTS, |_| None);
        let Some(last) = path.last().copied() else { continue };
        // Pulled back a little so the wall it lies on does not cut it.
        let halo_place = Transform::from_translation(last.end - last.dir);
        let glow_on = on && settings.has(crate::visuals::fx::features::EMISSIVE);
        let mut show = |entity: Entity, place: Transform, visible: bool| {
            if let Ok((mut t, mut vis)) = placed.get_mut(entity) {
                *t = place;
                let want = if visible { Visibility::Visible } else { Visibility::Hidden };
                if *vis != want {
                    *vis = want;
                }
            }
        };
        if !on && sight.is_none() {
            continue;
        }
        let assets = shared.get_or_insert_with(|| {
            let color = Vec3::from_array(SIGHT_COLOR.map(|c| laser::srgb_to_linear(c / 255.0)));
            let halo = load_sprite_source(&fs, &mut textures, &mut images, SIGHT_HALO);
            SightAssets {
                beam_mesh: meshes.add(laser::beam_quad()),
                beam: beams.add(laser::beam_material(&fs, &mut textures, &mut images, SIGHT_MATERIAL, SIGHT_WIDTH, color)),
                glow: beams.add(laser::glow_material(SIGHT_GLOW_HALF_WIDTH, SIGHT_GLOW_COLOR)),
                halo_mesh: meshes.add(sprite_quad()),
                halo: sprites.add(SpriteMaterial {
                    params: SpriteParams {
                        color: color.extend(1.0),
                        shape: Vec4::new(SIGHT_HALO_SCALE * 0.5, SIGHT_HALO_SCALE * 0.5, halo.orientation, 5.0),
                        state: Vec4::new(1.0, 1.0, 0.0, 0.0),
                    },
                    base: halo.base,
                    alpha_mode: AlphaMode::Add,
                    no_depth_test: false,
                }),
            }
        });
        // Both are placed in their shaders; their mesh bounds say nothing.
        let spawn_beam = |commands: &mut Commands| {
            let part = |commands: &mut Commands, material: Handle<LaserBeamMaterial>| {
                commands
                    .spawn((
                        MapObject,
                        Mesh3d(assets.beam_mesh.clone()),
                        MeshMaterial3d(material),
                        Transform::default(),
                        Visibility::Hidden,
                        NoFrustumCulling,
                    ))
                    .id()
            };
            (part(commands, assets.beam.clone()), part(commands, assets.glow.clone()))
        };
        let mut new_sight = None;
        let sight = match sight {
            Some(s) => s.into_inner(),
            None => {
                let halo = commands
                    .spawn((
                        MapObject,
                        Mesh3d(assets.halo_mesh.clone()),
                        MeshMaterial3d(assets.halo.clone()),
                        halo_place,
                        Visibility::Hidden,
                        NoFrustumCulling,
                    ))
                    .id();
                new_sight.insert(LaserSight { beams: Vec::new(), halo })
            }
        };
        while sight.beams.len() < path.len() {
            let pair = spawn_beam(&mut commands);
            sight.beams.push(pair);
        }
        for (i, &(beam, glow)) in sight.beams.iter().enumerate() {
            match path.get(i) {
                Some(seg) => {
                    let place = laser::beam_transform(seg.start, seg.end);
                    show(beam, place, on);
                    show(glow, place, glow_on);
                }
                None => {
                    show(beam, Transform::default(), false);
                    show(glow, Transform::default(), false);
                }
            }
        }
        show(sight.halo, halo_place, on && !last.through_portal);
        if let Some(s) = new_sight {
            commands.entity(e).try_insert(s);
        }
        commands.entity(e).try_insert(LaserSightPath { segments: path });
    }
}

/// A blown-apart turret goes, with its eye glow and laser sight.
fn remove_exploded(
    mut exploded: MessageReader<TurretExploded>,
    turrets: Query<(Option<&LaserSight>, Option<&super::turret_visual::TurretEye>)>,
    mut commands: Commands,
) {
    for ev in exploded.read() {
        let Ok((sight, eye)) = turrets.get(ev.entity) else { continue };
        let parts: Vec<Entity> = sight.map(|s| s.parts().collect()).unwrap_or_default();
        for part in parts.into_iter().chain(eye.map(|e| e.sprite())) {
            commands.entity(part).try_despawn();
        }
        let e = ev.entity;
        commands.queue(move |world: &mut World| crate::entities::despawn_entity(world, e));
    }
}

impl portal2_core::save::Restore for Turret {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_angles_match_directions() {
        let a = local_angles(Vec3::new(1.0, 1.0, 0.0).normalize());
        assert!((a.x - 45.0).abs() < 1e-3 && a.y.abs() < 1e-3);
        let a = local_angles(Vec3::new(1.0, 0.0, -1.0).normalize());
        assert!((a.y - 45.0).abs() < 1e-3, "{a}");
    }

    #[test]
    fn head_turns_in_steps_within_limits() {
        let mut t = Turret {
            enabled: true,
            state: TurretState::Active,
            range: 1024.0,
            eye: Vec3::ZERO,
            next_think: 0.0,
            shot_time: 0.0,
            search_until: 0.0,
            aim: Vec2::ZERO,
            muzzle: 0,
            talk: None,
            next_talk: 0.0,
            next_active_talk: 0.0,
            next_activate_sound: 0.0,
            tipped_since: None,
            gagged: false,
            held: false,
            thrash_until: 0.0,
            eye_state: EyeState::Dormant,
            eye_blink: false,
            burn_until: 0.0,
            next_burn_talk: 0.0,
        };
        t.turn_toward(Vec2::new(90.0, -40.0));
        assert_eq!(t.aim, Vec2::new(12.0, -12.0));
        for _ in 0..10 {
            t.turn_toward(Vec2::new(90.0, -40.0));
        }
        assert_eq!(t.aim, Vec2::new(60.0, -15.0));
    }

    /// A tracer's piece beyond a portal waits, unseen, until the streak
    /// reaches it, then races on.
    #[test]
    fn tracer_pieces_set_off_in_turn() {
        let tracer = Tracer { start: Vec3::ZERO, dir: Vec3::X, distance: 600.0, length: 100.0, width: 1.0, spawned: 1.0 };
        assert_eq!(tracer.place(0.5).map(|t| t.scale), Some(Vec3::ZERO));
        let place = tracer.place(1.05).expect("under way");
        assert!(place.scale.z > 1.0 && place.translation.x > 0.0, "{place:?}");
        assert!(tracer.place(1.2).is_none());
    }
}
