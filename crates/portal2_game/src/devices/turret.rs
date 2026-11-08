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
//! Knocked over (server.dll's tipped think), it sounds its alarm and thrashes
//! for 2 to 2.5 s, thinking every 0.05 s: firing each think and jerking its
//! head toward a new random aim up to 60° away. Then it turns to face ahead,
//! says its last line (`NPC_FloorTurret.Die` and the disabled line), folds up
//! (`retract`, `NPC_FloorTurret.Retract`) and, once folded, plays its dying
//! `eye_off` and its eye goes out. A turret started on uneven ground stays
//! put until something moves it (`physics`: it starts at rest).

use avian3d::prelude::RigidBody;
use bevy::prelude::*;
use source_formats::bsp::contents;

use bevy::camera::visibility::NoFrustumCulling;

use super::EffectMaterial;
use super::laser::{self, LaserBeamMaterial, LaserHitTurret};
use crate::entities::SpawnHandlers;
use crate::entities::io::{FireOutput, InputCall, InputHandlers};
use crate::entities::props::PhysicsProp;
use crate::entities::spawner::{ModelOptions, attach_studio_model};
use crate::player::Player;
use crate::player::health::DamagePlayer;
use crate::visuals::sprites::{SpriteMaterial, SpriteParams, load_sprite_source, sprite_quad};
use portal2_audio::SoundRequest;
use portal2_core::clock::GameClock;
use portal2_core::entity::MapEntity;
use portal2_core::sets::ThinkSet;
use portal2_core::vfs::GameFs;
use portal2_render::models::StudioAnimator;
use portal2_render::textures::TextureCache;
use portal2_world::MapObject;
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
        app.add_systems(FixedUpdate, run_turrets.in_set(ThinkSet)).add_systems(Update, (fade_tracers, show_aim, draw_lasers));
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
    Dead,
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
    let Some(mut t) = world.get_mut::<Turret>(call.target) else { return false };
    match call.input.as_str() {
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
    mut turrets: Query<(Entity, &mut Turret, &GlobalTransform, &MapEntity, Option<&RigidBody>, Option<&mut StudioAnimator>)>,
    joints: Query<&GlobalTransform>,
    players: Query<(Entity, &Player)>,
    holding: Option<Res<crate::physics::carry::Holding>>,
    mut lasered: MessageReader<LaserHitTurret>,
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
    for hit in lasered.read() {
        if let Ok((e, mut t, ..)) = turrets.get_mut(hit.0)
            && t.state != TurretState::Dead
        {
            t.state = TurretState::Dead;
            t.eye_state = EyeState::Dead;
            out.write(FireOutput::new(e, "OnDeath"));
        }
    }
    // The game's turrets shoot at a player flying with noclip too.
    let player = players.single().ok();
    for (e, mut t, gt, me, _, anim) in &mut turrets {
        if t.state == TurretState::Dead || now < t.next_think {
            continue;
        }
        t.next_think = now + THINK;
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

        // The player, when this turret can see them inside its cone.
        let target = player.and_then(|(pe, p)| {
            let (mins, maxs) = p.state.hull();
            let center = p.state.origin + (mins + maxs) * 0.5;
            let to = center - eye;
            let dist = to.length();
            if dist > t.range || dist < 1.0 {
                return None;
            }
            let local = tr.rotation.inverse() * (to / dist);
            let flat = local.truncate().normalize_or_zero();
            if flat.x < VIEW_COS || local_angles(local).y.abs() > VIEW_Z_DEGREES {
                return None;
            }
            (physics.trace(eye, center, Vec3::ZERO, Vec3::ZERO, SIGHT_MASK, &[e, pe]).fraction >= 1.0).then_some((pe, center, dist, local))
        });
        let target = target.filter(|_| t.enabled && !carried);

        match (t.state, target) {
            (TurretState::Idle | TurretState::Scanning, Some((_, _, dist, _))) => {
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
            (TurretState::Active, Some((pe, center, dist, local))) => {
                t.turn_toward(local_angles(local));
                let muzzle_dir = tr.rotation * t.aim_dir();
                let to = (center - eye).normalize_or_zero();
                let cos = if dist < CLOSE_RANGE { CLOSE_FIRE_COS } else { FIRE_COS };
                if now > t.shot_time && muzzle_dir.dot(to) >= cos {
                    let start = muzzle(&t, &anim).map_or(eye, |m| m.translation);
                    fire(
                        &mut t,
                        e,
                        start,
                        (center - start).normalize_or(to),
                        Some((pe, center)),
                        &physics,
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

/// One bullet along `dir` from the next muzzle (at `start`): plays that
/// pair's firing sequence (whose events flash the muzzles), damages the
/// player when it reaches them and leaves a tracer.
fn fire(
    t: &mut Turret,
    e: Entity,
    start: Vec3,
    dir: Vec3,
    target: Option<(Entity, Vec3)>,
    physics: &Physics,
    fx: &mut Effects,
    commands: &mut Commands,
    damage: &mut MessageWriter<DamagePlayer>,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<EffectMaterial>,
    shared: &mut Option<(Handle<Mesh>, Handle<EffectMaterial>)>,
) {
    t.muzzle = (t.muzzle + 1) % 4;
    fx.sequences.push((if t.muzzle < 2 { "fire" } else { "fire2" }, "idlealert"));
    let ignore = [e];
    let tr = physics.trace(start, start + dir * 16384.0, Vec3::ZERO, Vec3::ZERO, mask::SOLID_ALL, &ignore);
    let mut end = tr.end;
    if let Some((pe, center)) = target
        && (tr.entity == Some(pe) || tr.fraction >= 1.0 || start.distance(tr.end) + 1.0 >= start.distance(center))
    {
        end = center;
        let push = dir.with_z(0.0).normalize_or_zero() * 4.0;
        damage.write(DamagePlayer { amount: BULLET_DAMAGE * PLAYER_DAMAGE_SCALE, push, source: Some(e) });
    }
    fx.sounds.push(SoundRequest::on("NPC_FloorTurret.ShotSounds", e));
    let (mesh, material) = shared
        .get_or_insert_with(|| {
            (meshes.add(super::tube_mesh(4)), materials.add(EffectMaterial::glow(Vec3::new(1.0, 0.82, 0.5) * 1.2, 6.0, 0.02)))
        })
        .clone();
    let distance = start.distance(end);
    let tracer = Tracer {
        start,
        dir: (end - start).normalize_or(dir),
        distance,
        length: portal2_core::rng::random_range(64.0, 128.0),
        width: portal2_core::rng::random_range(0.5, 1.5),
        spawned: fx.now,
    };
    let place = tracer.place(fx.now);
    commands.spawn((tracer, MapObject, Mesh3d(mesh), MeshMaterial3d(material), place.unwrap_or_default()));
}

impl Tracer {
    /// The streak's placement at `now`; None once it has arrived.
    fn place(&self, now: f64) -> Option<Transform> {
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

/// A turret's laser sight: its beam, the glow where it ends and, with the
/// visual overhaul, a soft red halo along the beam.
#[derive(Component)]
struct LaserSight {
    beam: Entity,
    halo: Entity,
    glow: Entity,
}

/// Where a turret's laser sight runs this frame (for its light).
#[derive(Component, Clone, Copy, Debug)]
pub struct LaserSightPath {
    pub start: Vec3,
    pub end: Vec3,
    pub dir: Vec3,
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

/// Draws each turret's laser sight from the eye along the head's aim to
/// whatever it meets.
#[allow(clippy::too_many_arguments)]
fn draw_lasers(
    physics: Physics,
    mut commands: Commands,
    turrets: Query<(Entity, &Turret, &GlobalTransform, Option<&LaserSight>)>,
    settings: Res<crate::visuals::fx::FxSettings>,
    mut placed: Query<(&mut Transform, &mut Visibility), Without<Turret>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut beams: ResMut<Assets<LaserBeamMaterial>>,
    mut sprites: ResMut<Assets<SpriteMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut textures: ResMut<TextureCache>,
    fs: Res<GameFs>,
    mut shared: Local<Option<SightAssets>>,
) {
    for (e, t, gt, sight) in &turrets {
        let on = t.enabled && !matches!(t.state, TurretState::Dead | TurretState::Closing);
        let tr = gt.compute_transform();
        let eye = tr.transform_point(t.eye);
        let dir = tr.rotation * t.aim_dir();
        let start = eye + dir * 2.0;
        let hit = physics.trace(start, eye + dir * 8192.0, Vec3::ZERO, Vec3::ZERO, mask::SOLID_ALL, &[e]).end;
        let beam_place = laser::beam_transform(start, hit);
        // Pulled back a little so the wall it lies on does not cut it.
        let halo_place = Transform::from_translation(hit - dir);
        let want = if on { Visibility::Visible } else { Visibility::Hidden };
        let glow_on = on && settings.has(crate::visuals::fx::features::EMISSIVE);
        commands.entity(e).insert(LaserSightPath { start, end: hit, dir });
        if let Some(s) = sight {
            let glow_want = if glow_on { want } else { Visibility::Hidden };
            for (entity, place, show) in [(s.beam, beam_place, want), (s.halo, halo_place, want), (s.glow, beam_place, glow_want)] {
                if let Ok((mut t, mut vis)) = placed.get_mut(entity) {
                    *t = place;
                    if *vis != show {
                        *vis = show;
                    }
                }
            }
            continue;
        }
        if !on {
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
        let beam = commands
            .spawn((
                MapObject,
                Mesh3d(assets.beam_mesh.clone()),
                MeshMaterial3d(assets.beam.clone()),
                beam_place,
                Visibility::Visible,
                NoFrustumCulling,
            ))
            .id();
        let halo = commands
            .spawn((
                MapObject,
                Mesh3d(assets.halo_mesh.clone()),
                MeshMaterial3d(assets.halo.clone()),
                halo_place,
                Visibility::Visible,
                NoFrustumCulling,
            ))
            .id();
        let glow = commands
            .spawn((
                MapObject,
                Mesh3d(assets.beam_mesh.clone()),
                MeshMaterial3d(assets.glow.clone()),
                beam_place,
                if glow_on { Visibility::Visible } else { Visibility::Hidden },
                NoFrustumCulling,
            ))
            .id();
        commands.entity(e).insert(LaserSight { beam, halo, glow });
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
        };
        t.turn_toward(Vec2::new(90.0, -40.0));
        assert_eq!(t.aim, Vec2::new(12.0, -12.0));
        for _ in 0..10 {
            t.turn_toward(Vec2::new(90.0, -40.0));
        }
        assert_eq!(t.aim, Vec2::new(60.0, -15.0));
    }
}
