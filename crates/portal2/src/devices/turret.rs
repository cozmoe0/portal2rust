//! Floor turrets (`npc_portal_turret_floor`, `docs/re/turret.md`): physics
//! props that think every 0.1 s. Closed, a turret watches for the player;
//! on sight it deploys and fires once its wind-up, longer the further the
//! player is, has passed, turning its head toward them and shooting when
//! it points within 10° of them. Losing sight it scans for a few seconds,
//! then folds up. It talks throughout. Tipping one over makes it spray
//! bullets and give up; carrying, lasers and disabling stop it.

use avian3d::prelude::RigidBody;
use bevy::prelude::*;
use source_formats::bsp::contents;

use super::EffectMaterial;
use super::laser::LaserHitTurret;
use crate::audio::SoundRequest;
use crate::clock::{GameClock, ThinkSet};
use crate::collision::{Physics, mask};
use crate::entities::io::{FireOutput, InputCall, InputHandlers};
use crate::entities::props::PhysicsProp;
use crate::entities::spawner::{ModelOptions, attach_studio_model};
use crate::entities::{MapEntity, SpawnHandlers};
use crate::models::StudioAnimator;
use crate::player::Player;
use crate::player::health::DamagePlayer;
use crate::world::MapObject;

const THINK: f64 = 0.1;
/// `PortalTurretBullet` damage, of which players take half
/// (`sk_dmg_take_scale1`).
const BULLET_DAMAGE: f32 = 3.0;
const PLAYER_DAMAGE_SCALE: f32 = 0.5;
/// Fires when pointing within 10° of the player (45° when closer than
/// `CLOSE_RANGE`).
const FIRE_COS: f32 = 0.984_807_7;
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
/// Seconds a tipped turret sprays bullets before giving up.
const TIPPED_TIME: f64 = 2.0;
/// Up axis further from vertical than 60° counts as knocked over.
const UPRIGHT_COS: f32 = 0.5;
/// Walls, glass, props and light bridges block a turret's view.
const SIGHT_MASK: u32 = mask::SOLID_ALL | contents::BLOCKLOS;

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
    /// Knocked over: spraying bullets until it gives up.
    Tipped,
    Dead,
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
}

fn spawn_turret(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e).cloned() else { return };
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
        },
    ));
}

fn turret_input(world: &mut World, call: &InputCall) -> bool {
    let Some(mut t) = world.get_mut::<Turret>(call.target) else { return false };
    match call.input.as_str() {
        "enable" => t.enabled = true,
        "disable" => t.enabled = false,
        "toggle" => t.enabled = !t.enabled,
        "enablegagging" => t.gagged = true,
        "disablegagging" => t.gagged = false,
        "selfdestruct" | "selfdestructimmediately" => t.state = TurretState::Dead,
        "firebullet" | "enablepickup" | "disablepickup" => {}
        _ => return false,
    }
    true
}

/// Short-lived tracer line.
#[derive(Component)]
struct Tracer {
    until: f64,
}

/// What a turret does in one think, applied after the per-turret logic.
struct Effects<'a> {
    now: f64,
    sounds: Vec<SoundRequest>,
    outputs: Vec<FireOutput>,
    sequences: Vec<(&'a str, &'a str)>,
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

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn run_turrets(
    clock: Res<GameClock>,
    physics: Physics,
    mut turrets: Query<(Entity, &mut Turret, &GlobalTransform, &MapEntity, Option<&RigidBody>, Option<&mut StudioAnimator>)>,
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
            out.write(FireOutput::new(e, "OnDeath"));
        }
    }
    let player = players.single().ok().filter(|(_, p)| !p.noclip);
    for (e, mut t, gt, me, _, anim) in &mut turrets {
        if t.state == TurretState::Dead || now < t.next_think {
            continue;
        }
        t.next_think = now + THINK;
        let mut fx = Effects { now, sounds: Vec::new(), outputs: Vec::new(), sequences: Vec::new() };
        let tr = gt.compute_transform();
        let eye = tr.transform_point(t.eye);
        let up = tr.rotation * Vec3::Z;

        // Carried: no shooting, complains once.
        let carried = held == Some(e);
        if carried != t.held {
            t.held = carried;
            if carried {
                t.say(e, Talk::Pickup, &mut fx);
            }
        }

        // Knocked over: sprays bullets for a while, then gives up.
        if up.z < UPRIGHT_COS && !carried {
            let since = *t.tipped_since.get_or_insert(now);
            if t.state != TurretState::Tipped {
                t.state = TurretState::Tipped;
                t.say(e, Talk::Tipped, &mut fx);
            }
            if now - since > TIPPED_TIME {
                t.state = TurretState::Dead;
                fx.outputs.push(FireOutput::new(e, "OnTipped"));
                fx.sounds.push(SoundRequest::on("NPC_FloorTurret.Die", e));
            } else {
                let dir = tr.rotation * t.aim_dir();
                fire(&mut t, e, eye, dir, None, &physics, &mut fx, &mut commands, &mut damage, &mut meshes, &mut materials, &mut shared);
            }
            flush(fx, &mut sound_out, &mut out, anim);
            continue;
        }
        t.tipped_since = None;
        if t.state == TurretState::Tipped {
            t.state = TurretState::Idle;
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
                    fire(
                        &mut t,
                        e,
                        eye,
                        to,
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
                let line = if t.enabled { Talk::Search } else { Talk::Disabled };
                t.say(e, line, &mut fx);
                fx.outputs.push(FireOutput::new(e, "OnLostTarget"));
            }
            (TurretState::Scanning, None) => {
                if now > t.search_until {
                    // Folds back up.
                    t.state = TurretState::Idle;
                    t.aim = Vec2::ZERO;
                    fx.sequences.push(("retract", "idle"));
                    fx.sounds.push(SoundRequest::on("NPC_FloorTurret.Retire", e));
                    t.say(e, Talk::Retire, &mut fx);
                } else {
                    // Sweeps its head around while searching.
                    let s = now as f32;
                    t.turn_toward(Vec2::new((s * 2.0).sin() * 20.0, (s * 1.3).sin() * 20.0));
                }
            }
            _ => {}
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

/// One bullet along `dir` from the next muzzle: damages the player when it
/// reaches them, leaves a tracer, and pushes a tipped turret back.
#[allow(clippy::too_many_arguments)]
fn fire(
    t: &mut Turret,
    e: Entity,
    eye: Vec3,
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
    let side = if t.muzzle < 2 { 1.0 } else { -1.0 };
    let start = eye + dir * 8.0 + dir.cross(Vec3::Z).normalize_or_zero() * (6.0 * side);
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
            (meshes.add(super::tube_mesh(4)), materials.add(EffectMaterial::glow(Vec3::new(1.0, 0.8, 0.4) * 3.0, 6.0, 0.02)))
        })
        .clone();
    commands.spawn((
        Tracer { until: fx.now + 0.05 },
        MapObject,
        Mesh3d(mesh),
        MeshMaterial3d(material),
        super::segment_transform(start, end, 0.4, 0.4),
    ));
}

fn fade_tracers(clock: Res<GameClock>, tracers: Query<(Entity, &Tracer)>, mut commands: Commands) {
    for (e, t) in &tracers {
        if clock.curtime >= t.until {
            commands.entity(e).despawn();
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

/// The red laser sight (`effects/redlaser1`, colour 255 32 32) from the
/// eye along the head's aim to whatever it meets.
#[derive(Component)]
struct LaserSight(Entity);

#[allow(clippy::type_complexity)]
fn draw_lasers(
    physics: Physics,
    mut commands: Commands,
    turrets: Query<(Entity, &Turret, &GlobalTransform, Option<&LaserSight>)>,
    mut beams: Query<(&mut Transform, &mut Visibility), Without<Turret>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<EffectMaterial>>,
    mut shared: Local<Option<(Handle<Mesh>, Handle<EffectMaterial>)>>,
) {
    for (e, t, gt, sight) in &turrets {
        let on = t.enabled && !matches!(t.state, TurretState::Dead);
        let tr = gt.compute_transform();
        let eye = tr.transform_point(t.eye);
        let dir = tr.rotation * t.aim_dir();
        let hit = physics.trace(eye + dir * 2.0, eye + dir * 8192.0, Vec3::ZERO, Vec3::ZERO, mask::SOLID_ALL, &[e]).end;
        let place = super::segment_transform(eye + dir * 2.0, hit, 0.35, 0.35);
        match sight.and_then(|s| beams.get_mut(s.0).ok()) {
            Some((mut bt, mut vis)) => {
                *bt = place;
                let want = if on { Visibility::Visible } else { Visibility::Hidden };
                if *vis != want {
                    *vis = want;
                }
            }
            None if on => {
                let (mesh, material) = shared
                    .get_or_insert_with(|| {
                        (meshes.add(super::tube_mesh(4)), materials.add(EffectMaterial::glow(Vec3::new(1.0, 0.125, 0.125) * 2.0, 0.0, 0.0)))
                    })
                    .clone();
                let beam = commands.spawn((MapObject, Mesh3d(mesh), MeshMaterial3d(material), place, Visibility::Visible)).id();
                commands.entity(e).insert(LaserSight(beam));
            }
            None => {}
        }
    }
}

impl crate::save::Restore for Turret {}

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
        };
        t.turn_toward(Vec2::new(90.0, -40.0));
        assert_eq!(t.aim, Vec2::new(12.0, -12.0));
        for _ in 0..10 {
            t.turn_toward(Vec2::new(90.0, -40.0));
        }
        assert_eq!(t.aim, Vec2::new(60.0, -15.0));
    }
}
