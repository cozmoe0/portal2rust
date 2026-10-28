//! Player movement in the style of Source/Quake: ground friction and
//! acceleration, air control capped to a small wish speed, gravity, sliding
//! along multiple surfaces, stair stepping and ground snapping, plus Portal
//! 2's air steering rules and its pull toward floor portals
//! (`docs/re/gameplay.md`).
//!
//! Tuning values are the game's console-variable defaults; the hull and eye
//! dimensions are the standard documented Source player sizes.

use bevy::prelude::*;

use crate::collision::{Physics, Trace, mask};
use crate::portals::placement::{HALF_HEIGHT, HALF_WIDTH, PortalFrame};

pub const GRAVITY: f32 = 600.0;
pub const FRICTION: f32 = 4.0;
pub const STOP_SPEED: f32 = 100.0;
pub const ACCELERATE: f32 = 10.0;
pub const AIR_ACCELERATE: f32 = 10.0;
pub const MAX_SPEED: f32 = 175.0;
pub const STEP_SIZE: f32 = 18.0;
pub const MAX_VELOCITY: f32 = 3500.0;
/// Peak height of a standing jump (tunable).
pub const JUMP_HEIGHT: f32 = 45.0;
/// Air acceleration only builds speed up to this along the wish direction.
pub const AIR_WISH_CAP: f32 = 60.0;
/// Above this horizontal speed the player cannot steer against an axis
/// they move along faster than `AIR_BRAKE_SPEED`.
const AIR_STEER_SPEED: f32 = 300.0;
const AIR_BRAKE_SPEED: f32 = 150.0;
pub const DUCK_SPEED_SCALE: f32 = 1.0 / 3.0;
/// Surfaces steeper than this are not standable.
pub const MIN_WALKABLE_NORMAL: f32 = 0.7;
/// Faster upward motion than this means the player has left the ground.
pub const NON_JUMP_VELOCITY: f32 = 140.0;
pub const DUCK_TIME: f32 = 0.2;
/// How quickly velocity settles onto a funnel's flow (per second).
const FUNNEL_RESPONSE: f32 = 5.0;

pub const HULL_MINS: Vec3 = Vec3::new(-16.0, -16.0, 0.0);
pub const HULL_MAXS: Vec3 = Vec3::new(16.0, 16.0, 72.0);
pub const DUCK_MAXS: Vec3 = Vec3::new(16.0, 16.0, 36.0);
pub const EYE_HEIGHT: f32 = 64.0;
pub const DUCK_EYE_HEIGHT: f32 = 28.0;

#[derive(Clone, Copy, Debug, Default)]
pub struct MoveInput {
    /// Forward / right / up intent in -1..1.
    pub forward: f32,
    pub right: f32,
    pub up: f32,
    pub jump: bool,
    pub duck: bool,
    /// View angles (pitch, yaw, roll) in degrees.
    pub angles: Vec3,
    /// Air steering is off (after a faith plate launch).
    pub steer_suppressed: bool,
}

/// Open portals the player is pulled toward while falling onto a floor
/// portal or rising into a ceiling one, with the `sv_player_funnel_*`
/// settings.
#[derive(Clone, Debug, Default)]
pub struct Funnel {
    pub portals: Vec<PortalFrame>,
    pub height_adjust: f32,
    pub snap_threshold: f32,
    pub speed_bonus: f32,
}

impl Funnel {
    /// The closest portal the player is lined up with, as the offset from
    /// the player's centre to the portal's.
    fn target(&self, center: Vec3, velocity: Vec3, eye_forward: Vec3, wish: Vec3) -> Option<Vec3> {
        let rising = velocity.z > 165.0;
        // Steering by hand, or falling without looking down, turns it off.
        if wish.x.abs() > 64.0 || wish.y.abs() > 64.0 {
            return None;
        }
        if !rising && (eye_forward.z >= -0.7 || velocity.z >= -165.0) {
            return None;
        }
        let mut best: Option<(f32, Vec3)> = None;
        for p in &self.portals {
            let delta = p.center - center;
            let reachable = if rising {
                p.forward.z < -0.7 && delta.z > 0.0 && delta.z <= 1024.0 && delta.z <= velocity.z * velocity.z / (2.0 * GRAVITY)
            } else {
                p.forward.z > 0.7 && delta.z < 0.0 && delta.z >= -1024.0
            };
            if !reachable {
                continue;
            }
            // The catch area grows from 1.5 to 3 times the opening between
            // 256 and 1024 units away.
            let scale = 1.5 + 1.5 * ((delta.z.abs() - 256.0) / 768.0).clamp(0.0, 1.0);
            let along = |axis: Vec3| delta.dot((axis - p.forward * axis.dot(p.forward)).normalize_or_zero()).abs();
            if along(p.left()) > HALF_WIDTH * scale || along(p.up) > HALF_HEIGHT * scale {
                continue;
            }
            let d2 = delta.length_squared();
            if best.is_none_or(|(b, _)| d2 < b) {
                best = Some((d2, delta));
            }
        }
        best.map(|(_, d)| d)
    }

    /// Extra wish velocity toward the portal's centre. Where the player
    /// would pass the centre (or moves away from it) before reaching the
    /// portal, that axis of `velocity` is damped instead.
    pub fn correct(&self, velocity: &mut Vec3, center: Vec3, eye_forward: Vec3, wish: Vec3, dt: f32) -> Vec3 {
        let Some(delta) = self.target(center, *velocity, eye_forward, wish) else { return Vec3::ZERO };
        let push = 1.0 + (self.speed_bonus - 1.0) * (velocity.z / 1065.0).clamp(0.0, 1.0);
        // Time until the player's centre is `height_adjust` above the portal.
        let height = -delta.z - self.height_adjust;
        let Some(t) = arrival_time(-GRAVITY, 2.0 * velocity.z, 2.0 * height) else { return Vec3::ZERO };
        let half_life = 0.01 + 0.14 * ((delta.z.abs() - 128.0) / 896.0).clamp(0.0, 1.0);
        let damp = (-std::f32::consts::LN_2 / half_life * dt).exp();
        let mut out = Vec3::ZERO;
        for axis in 0..2 {
            let v = velocity[axis];
            if v == 0.0 || t <= delta[axis] / v {
                out[axis] = delta[axis] * push * 6.0 - v;
            } else if v.abs() <= self.snap_threshold {
                velocity[axis] = 0.0;
            } else {
                velocity[axis] *= damp;
            }
        }
        out
    }
}

/// A root of `a t² + b t + c = 0`: the positive one of the pair, or the
/// smaller when both are positive.
fn arrival_time(a: f32, b: f32, c: f32) -> Option<f32> {
    let (r1, r2) = if a == 0.0 {
        if b == 0.0 {
            return (c == 0.0).then_some(0.0);
        }
        (-c / b, -c / b)
    } else {
        let disc = b * b - 4.0 * a * c;
        if disc < 0.0 {
            return None;
        }
        let s = disc.sqrt();
        ((s - b) / (2.0 * a), (-b - s) / (2.0 * a))
    };
    let mut t = if r1 > 0.0 { r1 } else { r2 };
    if r1 > r2 && r2 >= 0.0 && r1 >= 0.0 {
        t = r2;
    }
    Some(t)
}

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub struct MoveState {
    pub origin: Vec3,
    pub velocity: Vec3,
    pub on_ground: bool,
    #[serde(with = "crate::save::ids::opt_entity")]
    pub ground_entity: Option<Entity>,
    pub ground_normal: Vec3,
    pub ducked: bool,
    /// 0 = standing view, 1 = fully ducked view.
    pub duck_fraction: f32,
    pub jump_held: bool,
}

impl Default for MoveState {
    fn default() -> Self {
        Self {
            origin: Vec3::ZERO,
            velocity: Vec3::ZERO,
            on_ground: false,
            ground_entity: None,
            ground_normal: Vec3::Z,
            ducked: false,
            duck_fraction: 0.0,
            jump_held: false,
        }
    }
}

impl MoveState {
    pub fn hull(&self) -> (Vec3, Vec3) {
        if self.ducked { (HULL_MINS, DUCK_MAXS) } else { (HULL_MINS, HULL_MAXS) }
    }

    pub fn eye_offset(&self) -> f32 {
        EYE_HEIGHT + (DUCK_EYE_HEIGHT - EYE_HEIGHT) * self.duck_fraction
    }
}

pub struct Mover<'a, 'w, 's> {
    pub physics: &'a Physics<'w, 's>,
    pub ignore: &'a [Entity],
    pub funnel: Option<&'a Funnel>,
}

impl Mover<'_, '_, '_> {
    fn trace(&self, start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3) -> Trace {
        self.physics.trace(start, end, mins, maxs, mask::PLAYER_SOLID, self.ignore)
    }

    /// Advances the player by one tick.
    pub fn step(&self, s: &mut MoveState, input: &MoveInput, dt: f32) {
        self.unstick(s);
        self.update_duck(s, input, dt);

        let yaw = input.angles.y.to_radians();
        let forward = Vec3::new(yaw.cos(), yaw.sin(), 0.0);
        let right = Vec3::new(yaw.sin(), -yaw.cos(), 0.0);
        let mut wish = forward * input.forward + right * input.right;
        wish.z = 0.0;
        let mut wish_speed = wish.length().min(1.0) * MAX_SPEED;
        let wish_dir = wish.normalize_or_zero();
        if s.ducked && s.on_ground {
            wish_speed *= DUCK_SPEED_SCALE;
        }

        // Jumping uses the button edge so holding it does not bunny hop.
        if input.jump && !s.jump_held && s.on_ground {
            s.velocity.z = (2.0 * GRAVITY * JUMP_HEIGHT).sqrt();
            s.on_ground = false;
            s.ground_entity = None;
        }
        s.jump_held = input.jump;

        if s.on_ground {
            apply_friction(s, dt);
            accelerate(&mut s.velocity, wish_dir, wish_speed, ACCELERATE, dt);
            s.velocity.z = 0.0;
            clamp_velocity(&mut s.velocity);
            self.walk_move(s, dt);
        } else {
            // Gravity is split around the move for a symmetric arc.
            s.velocity.z -= GRAVITY * dt * 0.5;
            let wish = self.air_wish(s, input, dt);
            air_accelerate(&mut s.velocity, wish.normalize_or_zero(), wish.length().min(MAX_SPEED), AIR_ACCELERATE, dt);
            clamp_velocity(&mut s.velocity);
            self.slide_move(s, dt);
            s.velocity.z -= GRAVITY * dt * 0.5;
        }
        self.categorize_position(s);
        clamp_velocity(&mut s.velocity);
    }

    /// The wish velocity in the air. The view's forward axis is flattened
    /// but only rescaled to full length when looking less than 30° up or
    /// down, so steering weakens when looking steeply.
    fn air_wish(&self, s: &mut MoveState, input: &MoveInput, dt: f32) -> Vec3 {
        let q = crate::coords::angles_to_quat(input.angles);
        let eye_forward = q * Vec3::X;
        let mut forward = Vec3::new(eye_forward.x, eye_forward.y, 0.0);
        if eye_forward.z.abs() < 0.5 {
            forward = forward.normalize_or_zero();
        }
        let yaw = input.angles.y.to_radians();
        let right = Vec3::new(yaw.sin(), -yaw.cos(), 0.0);
        let mut moves = Vec2::new(input.forward, input.right) * MAX_SPEED;
        if moves.length() > MAX_SPEED {
            moves = moves.normalize() * MAX_SPEED;
        }
        let mut wish = forward * moves.x + right * moves.y;

        let mut correction = Vec3::ZERO;
        if s.velocity.truncate().length_squared() <= AIR_STEER_SPEED * AIR_STEER_SPEED {
            if let Some(funnel) = self.funnel {
                let (mins, maxs) = s.hull();
                let center = s.origin + (mins + maxs) * 0.5;
                correction = funnel.correct(&mut s.velocity, center, eye_forward, wish, dt);
            }
        } else {
            for axis in 0..2 {
                let (v, w) = (s.velocity[axis], wish[axis]);
                if (v > AIR_BRAKE_SPEED && w < 0.0) || (v < -AIR_BRAKE_SPEED && w > 0.0) {
                    wish[axis] = 0.0;
                }
            }
        }
        if input.steer_suppressed {
            wish = Vec3::ZERO;
        }
        wish + correction
    }

    fn update_duck(&self, s: &mut MoveState, input: &MoveInput, dt: f32) {
        if input.duck && !s.ducked {
            if s.on_ground {
                s.ducked = true;
            } else {
                // In the air the legs tuck up: keep the eyes where they are.
                let lift = HULL_MAXS.z - DUCK_MAXS.z;
                let tr = self.trace(s.origin, s.origin + Vec3::Z * lift, HULL_MINS, DUCK_MAXS);
                s.origin = tr.end;
                s.ducked = true;
                s.duck_fraction = 1.0;
            }
        } else if !input.duck && s.ducked {
            let target = if s.on_ground { s.origin } else { s.origin - Vec3::Z * (HULL_MAXS.z - DUCK_MAXS.z) };
            let fits = !self.trace(target, target, HULL_MINS, HULL_MAXS).start_solid;
            if fits {
                if !s.on_ground {
                    s.origin = target;
                    s.duck_fraction = 0.0;
                }
                s.ducked = false;
            }
        }
        let goal = if s.ducked { 1.0 } else { 0.0 };
        let step = dt / DUCK_TIME;
        s.duck_fraction += (goal - s.duck_fraction).clamp(-step, step);
    }

    /// Nudges the player out of geometry it starts inside of.
    fn unstick(&self, s: &mut MoveState) {
        let (mins, maxs) = s.hull();
        if !self.trace(s.origin, s.origin, mins, maxs).start_solid {
            return;
        }
        for dz in [1.0, 2.0, 4.0, 8.0, 12.0, 18.0, 24.0, 32.0] {
            for dir in [Vec3::Z, Vec3::X, -Vec3::X, Vec3::Y, -Vec3::Y, -Vec3::Z] {
                let p = s.origin + dir * dz;
                if !self.trace(p, p, mins, maxs).start_solid {
                    s.origin = p;
                    return;
                }
            }
        }
    }

    fn categorize_position(&self, s: &mut MoveState) {
        let (mins, maxs) = s.hull();
        if s.velocity.z > NON_JUMP_VELOCITY {
            s.on_ground = false;
            s.ground_entity = None;
            return;
        }
        let tr = self.trace(s.origin, s.origin - Vec3::Z * 2.0, mins, maxs);
        if tr.fraction < 1.0 && tr.normal.z >= MIN_WALKABLE_NORMAL && !tr.all_solid {
            s.on_ground = true;
            s.ground_entity = tr.entity;
            s.ground_normal = tr.normal;
            if tr.fraction > 0.0 {
                s.origin = tr.end;
            }
            if s.velocity.z < 0.0 {
                s.velocity.z = 0.0;
            }
        } else {
            s.on_ground = false;
            s.ground_entity = None;
        }
    }

    fn walk_move(&self, s: &mut MoveState, dt: f32) {
        let (mins, maxs) = s.hull();
        let dest = s.origin + s.velocity * dt;
        let tr = self.trace(s.origin, dest, mins, maxs);
        if tr.fraction >= 1.0 {
            s.origin = tr.end;
            self.stay_on_ground(s);
            return;
        }

        // Compare a plain slide with one that steps up first.
        let start = *s;
        let mut low = start;
        self.slide_move(&mut low, dt);

        let mut high = start;
        let up = self.trace(high.origin, high.origin + Vec3::Z * STEP_SIZE, mins, maxs);
        if !up.start_solid && !up.all_solid {
            high.origin = up.end;
        }
        self.slide_move(&mut high, dt);
        let down = self.trace(high.origin, high.origin - Vec3::Z * STEP_SIZE, mins, maxs);
        let stepped = if down.normal.z < MIN_WALKABLE_NORMAL || down.start_solid {
            false
        } else {
            high.origin = down.end;
            true
        };
        let horiz = |a: Vec3, b: Vec3| (a.truncate() - b.truncate()).length_squared();
        if stepped && horiz(high.origin, start.origin) > horiz(low.origin, start.origin) {
            s.origin = high.origin;
            s.velocity = Vec3::new(high.velocity.x, high.velocity.y, low.velocity.z);
        } else {
            s.origin = low.origin;
            s.velocity = low.velocity;
        }
        self.stay_on_ground(s);
    }

    /// Keeps the player glued to slopes and stairs when walking down them.
    fn stay_on_ground(&self, s: &mut MoveState) {
        let (mins, maxs) = s.hull();
        let start = self.trace(s.origin, s.origin + Vec3::Z * 2.0, mins, maxs).end;
        let tr = self.trace(start, start - Vec3::Z * STEP_SIZE, mins, maxs);
        if tr.fraction > 0.0 && tr.fraction < 1.0 && !tr.start_solid && tr.normal.z >= MIN_WALKABLE_NORMAL {
            let dz = (tr.end.z - s.origin.z).abs();
            if dz > 0.5 * DIST_STEP_EPS {
                s.origin = tr.end;
            }
        }
    }

    /// Moves along the velocity, sliding off up to four surfaces per tick.
    fn slide_move(&self, s: &mut MoveState, dt: f32) {
        let (mins, maxs) = s.hull();
        let primal = s.velocity;
        let mut original = s.velocity;
        let mut planes: Vec<Vec3> = Vec::with_capacity(5);
        let mut time_left = dt;
        for _ in 0..4 {
            if s.velocity.length_squared() < 1e-6 {
                break;
            }
            let end = s.origin + s.velocity * time_left;
            let tr = self.trace(s.origin, end, mins, maxs);
            if tr.all_solid {
                s.velocity = Vec3::ZERO;
                return;
            }
            if tr.fraction > 0.0 {
                s.origin = tr.end;
                original = s.velocity;
                planes.clear();
            }
            if tr.fraction >= 1.0 {
                break;
            }
            time_left -= time_left * tr.fraction;
            if planes.len() >= 5 {
                s.velocity = Vec3::ZERO;
                break;
            }
            planes.push(tr.normal);

            let mut found = false;
            for i in 0..planes.len() {
                let v = clip_velocity(original, planes[i], 1.0);
                if planes.iter().enumerate().all(|(j, p)| j == i || v.dot(*p) >= 0.0) {
                    s.velocity = v;
                    found = true;
                    break;
                }
            }
            if !found {
                if planes.len() != 2 {
                    s.velocity = Vec3::ZERO;
                    break;
                }
                // Slide along the crease between the two planes.
                let dir = planes[0].cross(planes[1]).normalize_or_zero();
                s.velocity = dir * dir.dot(s.velocity);
            }
            if s.velocity.dot(primal) <= 0.0 {
                s.velocity = Vec3::ZERO;
                break;
            }
        }
    }

    /// Carried by an excursion funnel: no gravity or friction, velocity
    /// eases toward `flow`, collisions still slide.
    pub fn funnel_move(&self, s: &mut MoveState, input: &MoveInput, flow: Vec3, dt: f32) {
        self.unstick(s);
        self.update_duck(s, input, dt);
        s.jump_held = input.jump;
        s.velocity = s.velocity.lerp(flow, (dt * FUNNEL_RESPONSE).min(1.0));
        clamp_velocity(&mut s.velocity);
        s.on_ground = false;
        s.ground_entity = None;
        self.slide_move(s, dt);
    }

    /// Free flight without collision (noclip).
    pub fn noclip(&self, s: &mut MoveState, input: &MoveInput, dt: f32, fast: bool) {
        let q = crate::coords::angles_to_quat(input.angles);
        let wish = q * Vec3::X * input.forward - q * Vec3::Y * input.right + Vec3::Z * input.up;
        let speed = if fast { 1200.0 } else { 400.0 };
        let target = wish.normalize_or_zero() * speed;
        s.velocity = s.velocity.lerp(target, (dt * 10.0).min(1.0));
        s.origin += s.velocity * dt;
        s.on_ground = false;
        s.ground_entity = None;
    }
}

const DIST_STEP_EPS: f32 = 0.031_25;

pub fn clip_velocity(v: Vec3, normal: Vec3, overbounce: f32) -> Vec3 {
    let backoff = v.dot(normal) * overbounce;
    let mut out = v - normal * backoff;
    let adjust = out.dot(normal);
    if adjust < 0.0 {
        out -= normal * adjust;
    }
    out
}

fn apply_friction(s: &mut MoveState, dt: f32) {
    let speed = s.velocity.length();
    if speed < 0.1 {
        return;
    }
    let control = speed.max(STOP_SPEED);
    let drop = control * FRICTION * dt;
    let new_speed = (speed - drop).max(0.0);
    s.velocity *= new_speed / speed;
}

fn accelerate(vel: &mut Vec3, wish_dir: Vec3, wish_speed: f32, accel: f32, dt: f32) {
    let current = vel.dot(wish_dir);
    let add = wish_speed - current;
    if add <= 0.0 {
        return;
    }
    let accel_speed = (accel * dt * wish_speed).min(add);
    *vel += wish_dir * accel_speed;
}

fn air_accelerate(vel: &mut Vec3, wish_dir: Vec3, wish_speed: f32, accel: f32, dt: f32) {
    let capped = wish_speed.min(AIR_WISH_CAP);
    let current = vel.dot(wish_dir);
    let add = capped - current;
    if add <= 0.0 {
        return;
    }
    let accel_speed = (accel * wish_speed * dt).min(add);
    *vel += wish_dir * accel_speed;
}

fn clamp_velocity(v: &mut Vec3) {
    *v = v.clamp(Vec3::splat(-MAX_VELOCITY), Vec3::splat(MAX_VELOCITY));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_removes_into_plane_component() {
        let v = clip_velocity(Vec3::new(100.0, 0.0, -50.0), Vec3::Z, 1.0);
        assert!(v.z.abs() < 1e-4 && (v.x - 100.0).abs() < 1e-4);
    }

    #[test]
    fn ground_accel_reaches_max_speed() {
        let mut s = MoveState { on_ground: true, ..Default::default() };
        for _ in 0..120 {
            apply_friction(&mut s, 1.0 / 60.0);
            accelerate(&mut s.velocity, Vec3::X, MAX_SPEED, ACCELERATE, 1.0 / 60.0);
        }
        // Friction and acceleration settle a little under the cap.
        assert!(s.velocity.x > 0.8 * MAX_SPEED && s.velocity.x <= MAX_SPEED + 1e-3, "{}", s.velocity.x);
    }

    fn floor_funnel() -> Funnel {
        let floor = PortalFrame { center: Vec3::ZERO, forward: Vec3::Z, up: Vec3::X };
        Funnel { portals: vec![floor], height_adjust: 128.0, snap_threshold: 10.0, speed_bonus: 2.0 }
    }

    #[test]
    fn funnel_pulls_falling_player_toward_floor_portal() {
        let f = floor_funnel();
        let down = Vec3::new(0.0, 0.0, -1.0);
        // Falling, looking down, 40 units off centre: pulled back toward it.
        let mut v = Vec3::new(0.0, 0.0, -400.0);
        let push = f.correct(&mut v, Vec3::new(40.0, 0.0, 400.0), down, Vec3::ZERO, 1.0 / 60.0);
        assert!(push.x < 0.0 && push.y == 0.0, "{push}");
        // Drifting past the centre too fast: that axis is damped instead.
        let mut v = Vec3::new(-300.0, 0.0, -400.0);
        let push = f.correct(&mut v, Vec3::new(40.0, 0.0, 400.0), down, Vec3::ZERO, 1.0 / 60.0);
        assert_eq!(push.x, 0.0);
        assert!(v.x > -300.0 && v.x < 0.0, "{v}");
        // Not looking down, steering by hand, or outside the catch area: no pull.
        let mut v = Vec3::new(0.0, 0.0, -400.0);
        assert_eq!(f.correct(&mut v, Vec3::new(40.0, 0.0, 400.0), Vec3::X, Vec3::ZERO, 1.0 / 60.0), Vec3::ZERO);
        assert_eq!(f.correct(&mut v, Vec3::new(40.0, 0.0, 400.0), down, Vec3::new(100.0, 0.0, 0.0), 1.0 / 60.0), Vec3::ZERO);
        assert_eq!(f.correct(&mut v, Vec3::new(400.0, 0.0, 400.0), down, Vec3::ZERO, 1.0 / 60.0), Vec3::ZERO);
    }

    #[test]
    fn arrival_time_prefers_the_future_root() {
        // 2h + 2vz t - g t² = 0 with h = 300, vz = 0: t = sqrt(2h/g) = 1 s.
        let t = arrival_time(-GRAVITY, 0.0, 600.0).unwrap();
        assert!((t - 1.0).abs() < 1e-4, "{t}");
        assert!(arrival_time(-GRAVITY, 0.0, -600.0).is_none());
    }

    #[test]
    fn jump_height_matches_setting() {
        let v0 = (2.0 * GRAVITY * JUMP_HEIGHT).sqrt();
        assert!((v0 * v0 / (2.0 * GRAVITY) - JUMP_HEIGHT).abs() < 1e-3);
    }
}
