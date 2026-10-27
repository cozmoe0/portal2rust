//! Player movement in the style of Source/Quake: ground friction and
//! acceleration, air control capped to a small wish speed, gravity, sliding
//! along multiple surfaces, stair stepping and ground snapping.
//!
//! Tuning values are the game's console-variable defaults; the hull and eye
//! dimensions are the standard documented Source player sizes.

use bevy::prelude::*;

use crate::collision::{Physics, Trace, mask};

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
pub const AIR_WISH_CAP: f32 = 30.0;
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
}

#[derive(Clone, Copy, Debug)]
pub struct MoveState {
    pub origin: Vec3,
    pub velocity: Vec3,
    pub on_ground: bool,
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
            air_accelerate(&mut s.velocity, wish_dir, wish_speed, AIR_ACCELERATE, dt);
            clamp_velocity(&mut s.velocity);
            self.slide_move(s, dt);
            s.velocity.z -= GRAVITY * dt * 0.5;
        }
        self.categorize_position(s);
        clamp_velocity(&mut s.velocity);
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

    #[test]
    fn jump_height_matches_setting() {
        let v0 = (2.0 * GRAVITY * JUMP_HEIGHT).sqrt();
        assert!((v0 * v0 / (2.0 * GRAVITY) - JUMP_HEIGHT).abs() < 1e-3);
    }
}
