//! Security cameras (`npc_security_camera`): wall cameras that turn to watch
//! the player. An autostart camera (spawnflag 32) waits until it has a clear
//! line to the player's eyes, then tracks them; out of sight, or more than
//! 1500 units away, for half a second, it stops where it is and waits again.
//! Other cameras hold still until enabled. Disabled cameras fold up to pitch
//! 120.
//!
//! The head turns through the model's `aim_pitch` / `aim_yaw` pose
//! parameters (autoplay delta layers), at most 7 degrees per axis per 0.1 s
//! think, shown smoothed between thinks as the client interpolates pose
//! parameters. Pitch is the angle from the camera's down axis to the target,
//! yaw the angle around that axis from straight ahead (left positive).

use bevy::prelude::*;
use source_formats::bsp::contents;

use crate::entities::SpawnHandlers;
use crate::entities::io::{InputCall, InputHandlers};
use crate::entities::spawner::{ModelOptions, attach_studio_model};
use crate::player::Player;
use portal2_core::clock::GameClock;
use portal2_core::entity::MapEntity;
use portal2_core::sets::ThinkSet;
use portal2_render::models::StudioAnimator;
use portal2_world::collision::Physics;

const MODEL: &str = "models/props/security_camera.mdl";
const THINK_INTERVAL: f64 = 0.1;
/// Degrees per think in each axis.
const TURN_STEP: f32 = 7.0;
const RANGE: f32 = 1500.0;
/// Tracking continues this long after losing the player.
const LOST_AFTER: f64 = 0.5;
/// Aim offset refreshes only while the player is this close.
const JITTER_RANGE: f32 = 256.0;
/// Head pitch of a disabled camera.
const FOLDED_PITCH: f32 = 120.0;
/// Solid, line-of-sight blockers and movable objects block the view.
const SIGHT_MASK: u32 = contents::SOLID | contents::BLOCKLOS | contents::MOVEABLE;
const AUTOSTART: u32 = 32;
/// The camera sees from this far in front of its mount point.
const EYE_FORWARD: f32 = 10.0;

pub struct SecurityCameraPlugin;

impl Plugin for SecurityCameraPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(FixedUpdate, (think.in_set(ThinkSet), knock_off)).add_systems(Update, show_aim);
        let world = app.world_mut();
        world.resource_mut::<SpawnHandlers>().register("npc_security_camera", spawn_camera);
        world.resource_mut::<InputHandlers>().register("npc_security_camera", camera_input);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum Mode {
    /// Holding still until enabled.
    Idle,
    /// Waiting for a clear view of the player.
    Searching,
    Tracking,
    /// Folding up; stops thinking once there.
    Folding,
}

#[derive(Component, serde::Serialize, serde::Deserialize)]
pub struct SecurityCamera {
    mode: Mode,
    next_think: f64,
    /// (pitch, yaw) in degrees.
    goal: Vec2,
    aim: Vec2,
    /// Aim before the last think, and when that think ran (for smoothing).
    previous_aim: Vec2,
    last_think: f64,
    lost_at: f64,
    jitter: Vec3,
    jitter_thinks: i32,
    /// Still fixed to its wall (a portal opening there knocks it off).
    #[serde(default = "mounted_default")]
    mounted: bool,
}

fn mounted_default() -> bool {
    true
}

fn spawn_camera(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e).cloned() else { return };
    let path = me.studio_model().unwrap_or_else(|| MODEL.to_owned());
    let opts = ModelOptions { skin: me.get_i32("skin").unwrap_or(0).max(0) as usize, ..default() };
    attach_studio_model(world, e, &path, opts);
    if let Some(mut anim) = world.get_mut::<StudioAnimator>(e) {
        if let Some(idle) = anim.model.studio.find_sequence("idle") {
            anim.set_sequence_index(idle);
            anim.default_sequence = Some(idle);
        }
        anim.set_pose_parameter("aim_yaw", 0.0);
        anim.set_pose_parameter("aim_pitch", 0.0);
    }
    let autostart = me.spawnflags() & AUTOSTART != 0;
    let now = world.resource::<GameClock>().curtime;
    world.entity_mut(e).insert(SecurityCamera {
        mode: if autostart { Mode::Searching } else { Mode::Idle },
        // The first think comes 0.1 to 0.3 s after spawning.
        next_think: now + portal2_core::rng::random_range(0.1, 0.3) as f64,
        goal: Vec2::ZERO,
        aim: Vec2::ZERO,
        previous_aim: Vec2::ZERO,
        last_think: now,
        lost_at: 0.0,
        jitter: Vec3::ZERO,
        jitter_thinks: 0,
        mounted: true,
    });
}

/// A mounted camera falls off once a linked portal's opening crosses the
/// line through its mount point along its facing (out to its radius both
/// ways), and becomes a prop that can be carried.
fn knock_off(
    portals: Res<crate::portals::Portals>,
    portal_query: Query<&crate::portals::Portal>,
    fs: Res<portal2_core::vfs::GameFs>,
    mut cameras: Query<(Entity, &mut SecurityCamera, &GlobalTransform, &portal2_world::collision::Collider, &MapEntity)>,
    mut sounds: MessageWriter<portal2_audio::SoundRequest>,
    mut commands: Commands,
) {
    let Some(frames) = crate::portals::linked_frames(&portals, &portal_query) else { return };
    for (e, mut c, gt, col, me) in &mut cameras {
        if !c.mounted {
            continue;
        }
        let t = gt.compute_transform();
        let forward = t.rotation * Vec3::X;
        let radius = ((col.shape.maxs - col.shape.mins) * 0.5).length();
        let (a, b) = (t.translation + forward * radius, t.translation - forward * radius);
        let hit = frames.iter().any(|f| {
            let (la, lb) = (f.to_local(a), f.to_local(b));
            if (la.x > 1.0 && lb.x > 1.0) || (la.x < -1.0 && lb.x < -1.0) {
                return false;
            }
            let s = if (la.x - lb.x).abs() > 1e-4 { (la.x / (la.x - lb.x)).clamp(0.0, 1.0) } else { 0.5 };
            f.in_opening(la.lerp(lb, s), 0.0)
        });
        if !hit {
            continue;
        }
        c.mounted = false;
        c.mode = Mode::Idle;
        sounds.write(portal2_audio::SoundRequest::at("SecurityCamera.Detachment", t.translation));
        if let Some(collider) = crate::physics::collider_for(&col.shape) {
            let mass = me.studio_model().and_then(|p| crate::physics::model_mass(&fs, &p)).unwrap_or(10.0);
            commands.entity(e).insert(crate::entities::props::PhysicsProp);
            crate::physics::make_dynamic(&mut commands, e, collider, mass);
        }
        info!("security camera {e} knocked off by a portal");
    }
}

fn camera_input(world: &mut World, call: &InputCall) -> bool {
    let now = world.resource::<GameClock>().curtime;
    let Some(mut c) = world.get_mut::<SecurityCamera>(call.target) else { return false };
    // A camera knocked off its wall stays dead.
    if !c.mounted {
        return matches!(call.input.as_str(), "enable" | "disable" | "toggle");
    }
    let start = |c: &mut SecurityCamera| {
        c.mode = Mode::Searching;
        c.next_think = now + THINK_INTERVAL;
    };
    let fold = |c: &mut SecurityCamera| {
        c.mode = Mode::Folding;
        c.next_think = now + THINK_INTERVAL;
    };
    match call.input.as_str() {
        "enable" => start(&mut c),
        "disable" | "ragdoll" => fold(&mut c),
        "toggle" => {
            if matches!(c.mode, Mode::Searching | Mode::Tracking) {
                fold(&mut c)
            } else {
                start(&mut c)
            }
        }
        "lookatblue" | "lookatorange" | "lookallteams" => start(&mut c),
        _ => return false,
    }
    true
}

fn think(
    clock: Res<GameClock>,
    physics: Physics,
    mut cameras: Query<(Entity, &mut SecurityCamera, &GlobalTransform)>,
    players: Query<(Entity, &Player)>,
) {
    let now = clock.curtime;
    // Noclip does not hide the player from cameras.
    let player = players.single().ok();
    for (e, mut c, gt) in &mut cameras {
        if c.mode == Mode::Idle || now < c.next_think {
            continue;
        }
        c.next_think = now + THINK_INTERVAL;
        c.previous_aim = c.aim;
        c.last_think = now;
        let tr = gt.compute_transform();
        let eye = tr.translation + tr.rotation * Vec3::X * EYE_FORWARD;
        let sees = |target: Vec3, pe: Entity| physics.trace(eye, target, Vec3::ZERO, Vec3::ZERO, SIGHT_MASK, &[e, pe]).fraction >= 1.0;
        match c.mode {
            Mode::Idle => {}
            Mode::Searching => {
                if let Some((pe, p)) = player
                    && sees(p.state.origin + Vec3::Z * p.state.eye_offset(), pe)
                {
                    c.mode = Mode::Tracking;
                    c.lost_at = now + LOST_AFTER;
                }
            }
            Mode::Tracking => {
                let target = player.map(|(pe, p)| (pe, p.state.origin, p.state.origin + Vec3::Z * p.state.eye_offset()));
                let distance = target.map_or(f32::MAX, |(_, origin, _)| origin.distance(eye));
                c.jitter_thinks -= 1;
                if c.jitter_thinks <= 0 && distance < JITTER_RANGE {
                    let r = portal2_core::rng::random_range;
                    c.jitter = Vec3::new(r(-8.0, 8.0), r(-8.0, 8.0), r(0.0, 32.0));
                    c.jitter_thinks = 5 + (r(0.0, 26.0) as i32).min(25);
                }
                let visible = target.is_some_and(|(pe, _, eyes)| sees(eyes, pe));
                if let Some((_, _, eyes)) = target
                    && visible
                    && distance <= RANGE
                {
                    let previous_yaw = c.goal.y;
                    c.goal = aim_at(tr.rotation, eyes + c.jitter - eye, previous_yaw);
                    c.lost_at = now + LOST_AFTER;
                } else if now > c.lost_at {
                    // Gives up and holds its current aim.
                    c.mode = Mode::Searching;
                    continue;
                }
                step_aim(&mut c);
            }
            Mode::Folding => {
                c.goal = Vec2::new(FOLDED_PITCH, 0.0);
                if !step_aim(&mut c) {
                    c.mode = Mode::Idle;
                }
            }
        }
    }
}

/// Head angles (pitch from the camera's down axis, yaw around it) that look
/// along `to`; straight up or down keeps `previous_yaw`.
fn aim_at(rotation: Quat, to: Vec3, previous_yaw: f32) -> Vec2 {
    let d = to.normalize_or_zero();
    let (forward, left, up) = (rotation * Vec3::X, rotation * Vec3::Y, rotation * Vec3::Z);
    let pitch = (-up).dot(d).clamp(-1.0, 1.0).acos().to_degrees();
    let p = (d - up * d.dot(up)).normalize_or_zero();
    let yaw = if p.abs().max_element() < 0.01 {
        previous_yaw
    } else {
        let a = (-left).dot(p).clamp(-1.0, 1.0).acos().to_degrees();
        normalize_angle(if forward.dot(p) > 0.0 { a - 90.0 } else { -a - 90.0 })
    };
    Vec2::new(pitch, yaw)
}

/// Angle in (-180, 180].
fn normalize_angle(a: f32) -> f32 {
    let a = a % 360.0;
    if a > 180.0 {
        a - 360.0
    } else if a < -180.0 {
        a + 360.0
    } else {
        a
    }
}

/// Moves the aim up to one step towards the goal in each axis; false when
/// it was already there.
fn step_aim(c: &mut SecurityCamera) -> bool {
    let (goal, before) = (c.goal, c.aim);
    for axis in 0..2 {
        let (g, a) = (goal[axis], c.aim[axis]);
        c.aim[axis] = if g > a { (a + TURN_STEP).min(g) } else { (a - TURN_STEP).max(g) };
    }
    c.aim != before
}

/// Applies the aim to the model, interpolated between the last two thinks.
fn show_aim(clock: Res<GameClock>, fixed: Res<Time<Fixed>>, mut cameras: Query<(&SecurityCamera, &mut StudioAnimator)>) {
    let now = clock.curtime + fixed.overstep().as_secs_f64();
    for (c, mut anim) in &mut cameras {
        let t = (((now - c.last_think) / THINK_INTERVAL) as f32).clamp(0.0, 1.0);
        let aim = c.previous_aim.lerp(c.aim, t);
        anim.set_pose_parameter("aim_pitch", aim.x);
        anim.set_pose_parameter("aim_yaw", aim.y);
    }
}

impl portal2_core::save::Restore for SecurityCamera {}

pub fn register_save(r: &mut portal2_core::save::SaveRegistry) {
    r.component::<SecurityCamera>("security_camera");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aim_angles() {
        // Upright camera facing +X: level target ahead is pitch 90, yaw 0;
        // left is +90, right -90, below is pitch 0.
        let r = Quat::IDENTITY;
        let close = |a: Vec2, b: Vec2| (a - b).abs().max_element() < 0.01;
        assert!(close(aim_at(r, Vec3::X, 0.0), Vec2::new(90.0, 0.0)));
        assert!(close(aim_at(r, Vec3::Y, 0.0), Vec2::new(90.0, 90.0)));
        assert!(close(aim_at(r, -Vec3::Y, 0.0), Vec2::new(90.0, -90.0)));
        assert!(close(aim_at(r, Vec3::new(1.0, 0.0, -1.0), 0.0), Vec2::new(45.0, 0.0)));
        assert!(close(aim_at(r, -Vec3::Z, 33.0), Vec2::new(0.0, 33.0)));
        assert!((aim_at(r, -Vec3::X, 0.0).y.abs() - 180.0).abs() < 0.01);
    }

    #[test]
    fn steps_are_capped() {
        let mut c = SecurityCamera {
            mode: Mode::Tracking,
            next_think: 0.0,
            goal: Vec2::new(20.0, -3.0),
            aim: Vec2::ZERO,
            previous_aim: Vec2::ZERO,
            last_think: 0.0,
            lost_at: 0.0,
            jitter: Vec3::ZERO,
            jitter_thinks: 0,
            mounted: true,
        };
        assert!(step_aim(&mut c));
        assert_eq!(c.aim, Vec2::new(7.0, -3.0));
        step_aim(&mut c);
        step_aim(&mut c);
        assert_eq!(c.aim, Vec2::new(20.0, -3.0));
        assert!(!step_aim(&mut c));
    }
}
