//! Particle collections: one running instance of a definition with its
//! particles, control points, emitter state and child collections, and the
//! step that emits, initializes and moves particles the way the game's
//! particle library does (`docs/re/particles.md`).

use std::f32::consts::{PI, TAU};
use std::sync::OnceLock;

use bevy::math::{Quat, Vec3};

#[cfg(test)]
use super::defs::Op;
use super::defs::{Common, ConstraintKind, Def, DefRef, EmitterKind, ForceKind, InitKind, OpKind, field};

pub const MAX_CONTROL_POINTS: usize = 64;

/// Collision and lighting queries a collection may need.
pub trait Env {
    /// Fraction along `start..end` to the first solid hit and its normal.
    fn trace(&self, start: Vec3, end: Vec3) -> Option<(f32, Vec3)>;
    /// Light arriving at a point (linear RGB).
    fn light(&self, at: Vec3) -> Vec3;
}

/// An environment without world: nothing is hit, light is white.
#[cfg(test)]
pub struct NoEnv;

#[cfg(test)]
impl Env for NoEnv {
    fn trace(&self, _start: Vec3, _end: Vec3) -> Option<(f32, Vec3)> {
        None
    }
    fn light(&self, _at: Vec3) -> Vec3 {
        Vec3::ONE
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ControlPoint {
    pub position: Vec3,
    pub prev_position: Vec3,
    /// Local axes: x forward, y left, z up (Source's convention).
    pub forward: Vec3,
    pub left: Vec3,
    pub up: Vec3,
}

impl Default for ControlPoint {
    fn default() -> Self {
        Self { position: Vec3::ZERO, prev_position: Vec3::ZERO, forward: Vec3::X, left: Vec3::Y, up: Vec3::Z }
    }
}

impl ControlPoint {
    /// Local coordinates to a world direction.
    pub fn rotate(&self, v: Vec3) -> Vec3 {
        self.forward * v.x + self.left * v.y + self.up * v.z
    }

    /// Position at fraction `t` of the current step (0 = its start).
    pub fn at(&self, t: f32) -> Vec3 {
        self.prev_position.lerp(self.position, t)
    }

    pub fn set_rotation(&mut self, q: Quat) {
        self.forward = q * Vec3::X;
        self.left = q * Vec3::Y;
        self.up = q * Vec3::Z;
    }

    pub fn rotation(&self) -> Quat {
        Quat::from_mat3(&bevy::math::Mat3::from_cols(self.forward, self.left, self.up)).normalize()
    }
}

/// The 4096 uniform random numbers the library draws from.
fn table() -> &'static [f32; 4096] {
    static T: OnceLock<Box<[f32; 4096]>> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = Box::new([0.0f32; 4096]);
        let mut x: u32 = 0x9e37_79b9;
        for v in t.iter_mut() {
            // xorshift32
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            *v = (x >> 8) as f32 / (1u32 << 24) as f32;
        }
        t
    })
}

fn rand_at(i: u32) -> f32 {
    table()[(i & 0xfff) as usize]
}

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Schlick's bias curve: `bias` 0.5 is linear.
fn bias(t: f32, b: f32) -> f32 {
    if (b - 0.5).abs() < 1e-6 {
        return t;
    }
    let k = 1.0 / b.max(1e-4) - 2.0;
    t / ((1.0 - t) * k + 1.0)
}

fn remap_clamped(v: f32, a: (f32, f32), b: (f32, f32)) -> f32 {
    let d = a.1 - a.0;
    if d.abs() < 1e-12 {
        return if v >= a.1 { b.1 } else { b.0 };
    }
    b.0 + ((v - a.0) / d).clamp(0.0, 1.0) * (b.1 - b.0)
}

/// Smooth 3D gradient noise in [-1, 1].
pub fn noise(p: Vec3) -> f32 {
    fn hash(x: i32, y: i32, z: i32) -> u32 {
        let mut h = (x as u32).wrapping_mul(0x8da6_b343) ^ (y as u32).wrapping_mul(0xd816_3841) ^ (z as u32).wrapping_mul(0xcb1a_b31f);
        h ^= h >> 13;
        h = h.wrapping_mul(0x5bd1_e995);
        h ^ (h >> 15)
    }
    fn grad(h: u32, d: Vec3) -> f32 {
        const G: [Vec3; 12] = [
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(-1.0, 1.0, 0.0),
            Vec3::new(1.0, -1.0, 0.0),
            Vec3::new(-1.0, -1.0, 0.0),
            Vec3::new(1.0, 0.0, 1.0),
            Vec3::new(-1.0, 0.0, 1.0),
            Vec3::new(1.0, 0.0, -1.0),
            Vec3::new(-1.0, 0.0, -1.0),
            Vec3::new(0.0, 1.0, 1.0),
            Vec3::new(0.0, -1.0, 1.0),
            Vec3::new(0.0, 1.0, -1.0),
            Vec3::new(0.0, -1.0, -1.0),
        ];
        G[(h % 12) as usize].dot(d)
    }
    let i = p.floor();
    let f = p - i;
    let (x, y, z) = (i.x as i32, i.y as i32, i.z as i32);
    let u = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    let mut acc = [[[0.0f32; 2]; 2]; 2];
    for (dz, plane) in acc.iter_mut().enumerate() {
        for (dy, row) in plane.iter_mut().enumerate() {
            for (dx, v) in row.iter_mut().enumerate() {
                let o = Vec3::new(dx as f32, dy as f32, dz as f32);
                *v = grad(hash(x + dx as i32, y + dy as i32, z + dz as i32), f - o);
            }
        }
    }
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let x00 = lerp(acc[0][0][0], acc[0][0][1], u.x);
    let x10 = lerp(acc[0][1][0], acc[0][1][1], u.x);
    let x01 = lerp(acc[1][0][0], acc[1][0][1], u.x);
    let x11 = lerp(acc[1][1][0], acc[1][1][1], u.x);
    (lerp(lerp(x00, x10, u.y), lerp(x01, x11, u.y), u.z) * 1.3).clamp(-1.0, 1.0)
}

/// Particle attributes, one array per field.
#[derive(Clone, Debug, Default)]
pub struct Particles {
    pub count: usize,
    pub s: [Vec<f32>; field::COUNT],
    pub v: [Vec<Vec3>; field::COUNT],
    pub id: Vec<u32>,
    /// Values right after initialization, for operators that scale them.
    pub s0: [Vec<f32>; field::COUNT],
    pub v0: [Vec<Vec3>; field::COUNT],
}

impl Particles {
    fn push(&mut self, def: &Def, position: Vec3, created: f32, id: u32) {
        for f in 0..field::COUNT {
            if f == field::ID {
                continue;
            }
            if field::is_vector(f) {
                let v = match f {
                    field::POSITION | field::PREV_POSITION => position,
                    field::COLOR => Vec3::new(def.color[0], def.color[1], def.color[2]),
                    field::NORMAL => def.normal,
                    field::GLOW_RGB => Vec3::ONE,
                    _ => Vec3::ZERO,
                };
                self.v[f].push(v);
                self.v0[f].push(v);
            } else {
                let s = match f {
                    field::LIFE => 1.0,
                    field::RADIUS => def.radius,
                    field::ROLL => def.rotation,
                    field::ROLL_SPEED => def.rotation_speed,
                    field::ALPHA => def.color[3],
                    field::CREATED => created,
                    field::SEQUENCE => def.sequence as f32,
                    field::SEQUENCE1 => def.sequence1 as f32,
                    field::TRAIL_LENGTH => 0.1,
                    field::ALPHA2 | field::GLOW_ALPHA => 1.0,
                    _ => 0.0,
                };
                self.s[f].push(s);
                self.s0[f].push(s);
            }
        }
        self.id.push(id);
        self.count += 1;
    }

    fn swap_remove(&mut self, i: usize) {
        for f in 0..field::COUNT {
            if !self.s[f].is_empty() {
                self.s[f].swap_remove(i);
                self.s0[f].swap_remove(i);
            }
            if !self.v[f].is_empty() {
                self.v[f].swap_remove(i);
                self.v0[f].swap_remove(i);
            }
        }
        self.id.swap_remove(i);
        self.count -= 1;
    }

    pub fn get(&self, f: usize, i: usize) -> f32 {
        if f == field::ID {
            self.id[i] as f32
        } else if field::is_vector(f) {
            self.v[f][i].x
        } else {
            self.s[f][i]
        }
    }

    pub fn set(&mut self, f: usize, i: usize, value: f32) {
        if f == field::ID {
            return;
        }
        if field::is_vector(f) {
            self.v[f][i] = Vec3::splat(value);
        } else {
            self.s[f][i] = value;
        }
    }

    fn initial(&self, f: usize, i: usize) -> f32 {
        if f == field::ID {
            self.id[i] as f32
        } else if field::is_vector(f) {
            self.v0[f][i].x
        } else {
            self.s0[f][i]
        }
    }

    fn snapshot_initial(&mut self, from: usize) {
        for f in 0..field::COUNT {
            if f == field::ID {
                continue;
            }
            if field::is_vector(f) {
                let (src, dst) = (&self.v[f], &mut self.v0[f]);
                dst[from..].copy_from_slice(&src[from..]);
            } else {
                let (src, dst) = (&self.s[f], &mut self.s0[f]);
                dst[from..].copy_from_slice(&src[from..]);
            }
        }
    }

    pub fn age(&self, i: usize, time: f32) -> f32 {
        time - self.s[field::CREATED][i]
    }

    /// Age as a share of the lifetime (0 when the lifetime is 0).
    pub fn age_fraction(&self, i: usize, time: f32) -> f32 {
        let life = self.s[field::LIFE][i];
        if life > 0.0 { self.age(i, time) / life } else { 0.0 }
    }
}

#[derive(Clone, Debug, Default)]
struct EmitterState {
    /// Continuous: running total of particles owed.
    total: f32,
    emitted: i32,
    active: bool,
    /// When the emitter started (collection time).
    started: f32,
    /// Instant: count and remaining count, start time.
    count: i32,
    remaining: i32,
    fired: bool,
    start: f32,
}

#[derive(Clone, Debug, Default)]
struct OpState {
    timer: f32,
    value: f32,
}

/// A child collection and whether it belongs to the end cap.
#[derive(Clone, Debug)]
pub struct ChildCollection {
    pub collection: Collection,
    pub end_cap: bool,
    pub group: i32,
}

#[derive(Clone, Debug)]
pub struct Collection {
    pub def: DefRef,
    pub time: f32,
    pub prev_time: f32,
    pub dt: f32,
    pub prev_dt: f32,
    pub particles: Particles,
    pub cps: [ControlPoint; MAX_CONTROL_POINTS],
    /// Highest control point the owner set, plus one.
    pub cps_used: usize,
    pub children: Vec<ChildCollection>,
    /// Cleared by `stop_emission`.
    pub emitting: bool,
    pub end_cap: bool,
    pub frozen: bool,
    /// Frames of the definition's sheet sequences, for `Lifetime From Sequence`.
    pub sequence_frames: Vec<(u32, f32)>,
    counter: u32,
    seed: u32,
    next_id: u32,
    emitters: Vec<EmitterState>,
    ops: Vec<OpState>,
    killed: Vec<usize>,
    deck: Vec<i32>,
    linear: i32,
    /// Accumulated force per particle during a step.
    accel: Vec<Vec3>,
    /// The parent's particle positions (current, previous) this frame.
    parent_cache: Vec<(Vec3, Vec3)>,
}

/// Creates collections from library indices (children need the library).
pub trait Library {
    fn def(&self, index: usize) -> Option<DefRef>;
}

impl Collection {
    pub fn new(def: DefRef, library: &dyn Library, seed: u32) -> Self {
        let children = def
            .children
            .iter()
            .filter(|c| !c.end_cap)
            .filter_map(|c| {
                let d = library.def(c.def)?;
                let group = d.group;
                let mut collection = Collection::new(d, library, seed.wrapping_mul(31).wrapping_add(c.def as u32 + 1));
                collection.time = -c.delay.max(0.0);
                Some(ChildCollection { collection, end_cap: false, group })
            })
            .collect();
        let mut c = Self {
            emitters: vec![EmitterState::default(); def.emitters.len()],
            ops: vec![OpState::default(); def.operators.len()],
            def,
            time: 0.0,
            prev_time: 0.0,
            dt: 0.0,
            prev_dt: 0.0,
            particles: Particles::default(),
            cps: [ControlPoint::default(); MAX_CONTROL_POINTS],
            cps_used: 1,
            children,
            emitting: true,
            end_cap: false,
            frozen: false,
            sequence_frames: Vec::new(),
            counter: 0,
            seed: seed & 0xfff,
            next_id: 0,
            killed: Vec::new(),
            deck: Vec::new(),
            linear: 0,
            accel: Vec::new(),
            parent_cache: Vec::new(),
        };
        c.start_emitters();
        c
    }

    fn rand(&mut self) -> f32 {
        let r = rand_at(self.counter.wrapping_add(self.seed));
        self.counter = self.counter.wrapping_add(1);
        r
    }

    fn rand_range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.rand()
    }

    fn rand_exp(&mut self, a: f32, b: f32, exponent: f32) -> f32 {
        let r = self.rand();
        let r = if exponent == 1.0 { r } else { r.powf(exponent) };
        a + (b - a) * r
    }

    /// Random number that stays the same for a particle and operator.
    fn particle_rand(&self, i: usize, salt: u32) -> f32 {
        rand_at(self.particles.id[i].wrapping_add(self.seed).wrapping_add(salt.wrapping_mul(97)))
    }

    fn start_emitters(&mut self) {
        let def = self.def.clone();
        for (i, e) in def.emitters.iter().enumerate() {
            let mut st = EmitterState { active: true, started: self.time, ..Default::default() };
            if let EmitterKind::Instant { start, start_max, count, count_min, .. } = e.kind {
                st.count = if count_min >= 0 { count_min + ((count - count_min + 1) as f32 * self.rand()) as i32 } else { count };
                st.remaining = st.count;
                st.start = if start_max > 0.0 { self.rand_range(start, start_max) } else { start };
            }
            self.emitters[i] = st;
        }
    }

    /// Sets a control point and passes it on to the children.
    pub fn set_cp(&mut self, i: usize, cp: ControlPoint) {
        if i >= MAX_CONTROL_POINTS {
            return;
        }
        self.cps[i] = cp;
        self.cps_used = self.cps_used.max(i + 1);
        for c in &mut self.children {
            c.collection.set_cp(i, cp);
        }
    }

    /// Stops emitting; existing particles live out their lives.
    pub fn stop_emission(&mut self) {
        self.emitting = false;
        for e in &mut self.emitters {
            e.active = false;
        }
        for c in &mut self.children {
            c.collection.stop_emission();
        }
    }

    /// Stops emission and starts the definition's end-cap children.
    pub fn start_end_cap(&mut self, library: &dyn Library) {
        if self.end_cap {
            return;
        }
        self.stop_emission();
        self.end_cap = true;
        let def = self.def.clone();
        for c in def.children.iter().filter(|c| c.end_cap) {
            if let Some(d) = library.def(c.def) {
                let group = d.group;
                let mut collection = Collection::new(d, library, self.seed.wrapping_add(c.def as u32 * 7));
                collection.time = -c.delay.max(0.0);
                for (i, cp) in self.cps.iter().enumerate().take(self.cps_used) {
                    collection.set_cp(i, *cp);
                }
                self.children.push(ChildCollection { collection, end_cap: true, group });
            }
        }
        for c in &mut self.children {
            if !c.end_cap {
                c.collection.start_end_cap(library);
            }
        }
    }

    /// Restarts emission from the current time.
    pub fn restart(&mut self) {
        self.emitting = true;
        self.end_cap = false;
        self.start_emitters();
        for c in &mut self.children {
            c.collection.restart();
        }
    }

    /// No particles left and nothing that could make more.
    pub fn finished(&self) -> bool {
        if self.particles.count > 0 || self.time < 0.0 {
            return false;
        }
        let may_emit = self.emitting
            && self.def.emitters.iter().zip(&self.emitters).any(|(e, st)| match e.kind {
                EmitterKind::Continuous { rate, duration, start, .. } => {
                    st.active && rate > 0.0 && (duration == 0.0 || self.time <= st.started + start + duration)
                }
                EmitterKind::Instant { .. } => st.active && st.remaining > 0,
                EmitterKind::Noise { duration, start, .. } => st.active && (duration == 0.0 || self.time <= st.started + start + duration),
                EmitterKind::Maintain { .. } => st.active,
            });
        !may_emit && self.children.iter().all(|c| c.collection.finished())
    }

    /// Operator strength at the current time, `None` when inactive.
    pub fn strength(&self, c: &Common) -> Option<f32> {
        if c.end_cap != -1 && self.end_cap != (c.end_cap == 1) {
            return None;
        }
        if c.constant {
            return Some(1.0);
        }
        let mut t = self.time;
        if c.time_offset.0 != 0 {
            let r = rand_at(c.time_offset.0 as u32 + self.seed);
            t = (t + c.time_offset.1 + (c.time_offset.2 - c.time_offset.1) * r).max(0.0);
        }
        if c.time_scale.0 != 0 && t > c.fade_in.0 {
            let r = rand_at(c.time_scale.0 as u32 + self.seed);
            let scale = c.time_scale.1 + (c.time_scale.2 - c.time_scale.1) * r;
            if scale >= 1e-4 {
                t = (t - c.fade_in.0) / scale + c.fade_in.0;
            }
        }
        if c.fade_oscillate > 0.0 {
            t = (self.time / c.fade_oscillate).fract();
        }
        let (start_in, mut end_in) = c.fade_in;
        let (mut start_out, mut end_out) = c.fade_out;
        let mut s = if t < start_in || (end_out > 0.0 && t > end_out) {
            0.0
        } else {
            end_in = end_in.max(start_in);
            start_out = start_out.max(end_in);
            end_out = end_out.max(start_out);
            let mut s: f32 = 1.0;
            if end_in > t && end_in > start_in {
                s = s.min((t - start_in) / (end_in - start_in));
            }
            if t > start_out && end_out > start_out {
                s = s.min((t - end_out) / (start_out - end_out));
            }
            s
        };
        if c.strength.0 != 0 {
            let r = rand_at(c.strength.0 as u32 + self.seed);
            let scale = c.strength.1 + (c.strength.2 - c.strength.1) * r;
            s *= (scale * s).max(0.0);
        }
        (s > 0.0).then_some(s)
    }

    pub fn simulate(&mut self, dt: f32, env: &dyn Env, library: &dyn Library) {
        if dt < 0.0 || self.frozen {
            return;
        }
        if self.time < 0.0 {
            // A child still waiting for its delay.
            self.time += dt;
            return;
        }
        if dt >= 1e-22 {
            let max = self.def.max_time_step;
            let mut remaining = if dt >= max * 10.0 { max } else { dt };
            while remaining > 0.0 {
                let mut step = remaining.min(max);
                if remaining <= max && self.def.min_time_step > remaining {
                    step = self.def.min_time_step;
                }
                remaining -= step;
                if self.dt != 0.0 {
                    self.prev_dt = self.dt;
                }
                self.prev_time = self.time;
                self.dt = step;
                if self.prev_dt == 0.0 {
                    self.prev_dt = step;
                }
                self.time += step;
                self.step(env, true);
                self.emit_and_init(env);
                if self.particles.count > 0 {
                    self.step(env, false);
                }
            }
        }
        // Children placing particles on the parent's see this frame's.
        let mut parent = Vec::new();
        for c in &mut self.children {
            if c.collection.def.initializers.iter().any(|i| matches!(i.kind, InitKind::FromParent { .. })) {
                if parent.is_empty() {
                    let p = &self.particles;
                    parent = (0..p.count).map(|i| (p.v[field::POSITION][i], p.v[field::PREV_POSITION][i])).collect();
                }
                c.collection.parent_cache.clone_from(&parent);
            }
            c.collection.simulate(dt, env, library);
        }
        self.children.retain(|c| !(c.end_cap && c.collection.finished()));
        for cp in self.cps.iter_mut() {
            cp.prev_position = cp.position;
        }
        if self.time > self.def.freeze_after {
            self.frozen = true;
        }
    }
}

/// The library's sine stand-in: `4 f (1 - f)` on the fractional part of
/// `x`, changing sign every whole number (about `sin(π x)`).
fn sin_approx(x: f32) -> f32 {
    let a = x.abs();
    let f = a.fract();
    let whole = a.floor() as i64;
    let s = 4.0 * f * (1.0 - f);
    let s = if whole % 2 == 1 { -s } else { s };
    if x < 0.0 { -s } else { s }
}

fn axis(v: Vec3, i: usize) -> f32 {
    match i {
        0 => v.x,
        1 => v.y,
        _ => v.z,
    }
}

fn rand_vec(c: &mut Collection, min: Vec3, max: Vec3) -> Vec3 {
    Vec3::new(c.rand_range(min.x, max.x), c.rand_range(min.y, max.y), c.rand_range(min.z, max.z))
}

/// A random direction and a radius whose cube is uniform (a point evenly
/// spread through the unit ball).
fn random_in_sphere(c: &mut Collection) -> (Vec3, f32) {
    let cos_t = c.rand() * 2.0 - 1.0;
    let phi = (c.rand() * 0.9999 + 0.0001) * TAU;
    let r = c.rand().powf(1.0 / 3.0);
    let sin_t = (1.0 - cos_t * cos_t).max(0.0).sqrt();
    (Vec3::new(sin_t * phi.cos(), sin_t * phi.sin(), cos_t), r)
}

/// Noise mapped to 0..1, or folded with `absolute` (and flipped with `invert`).
fn noise01(p: Vec3, absolute: bool, invert: bool) -> f32 {
    let n = noise(p);
    if absolute {
        let a = n.abs();
        if invert { 1.0 - a } else { a }
    } else {
        n * 0.5 + 0.5
    }
}

impl Collection {
    fn cp(&self, i: i32) -> ControlPoint {
        self.cps[(i.max(0) as usize).min(MAX_CONTROL_POINTS - 1)]
    }

    /// Fraction of the current step at time `t`.
    fn step_fraction(&self, t: f32) -> f32 {
        if self.dt > 0.0 { ((t - self.prev_time) / self.dt).clamp(0.0, 1.0) } else { 1.0 }
    }

    fn kill(&mut self, i: usize) {
        self.killed.push(i);
    }

    fn apply_kills(&mut self) {
        if self.killed.is_empty() {
            return;
        }
        self.killed.sort_unstable();
        self.killed.dedup();
        let killed = std::mem::take(&mut self.killed);
        for &i in killed.iter().rev() {
            if i < self.particles.count {
                self.particles.swap_remove(i);
            }
        }
    }

    fn new_particle(&mut self, created: f32) {
        let pos = self.cps[0].at(self.step_fraction(created));
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let def = self.def.clone();
        self.particles.push(&def, pos, created, id);
    }

    fn emit_and_init(&mut self, env: &dyn Env) {
        let def = self.def.clone();
        if def.initial_particles > 0 && self.next_id == 0 {
            let first = self.particles.count;
            for _ in 0..def.initial_particles.min(def.max_particles) {
                self.new_particle(self.prev_time);
            }
            self.init_particles(first, env);
        }
        if !self.emitting {
            return;
        }
        for (k, e) in def.emitters.iter().enumerate() {
            let Some(strength) = self.strength(&e.common) else { continue };
            let first = self.particles.count;
            self.emit(k, &e.kind, strength);
            if self.particles.count > first {
                self.init_particles(first, env);
            }
        }
    }

    fn emit(&mut self, k: usize, kind: &EmitterKind, strength: f32) {
        let free = self.def.max_particles.saturating_sub(self.particles.count);
        let st = self.emitters[k].clone();
        if !st.active {
            return;
        }
        match *kind {
            EmitterKind::Continuous { start, rate, duration, scale_cp, scale_cp_field, per_used_cp } => {
                let begin = st.started + start;
                if begin > self.time || rate <= 0.0 {
                    return;
                }
                let mut r = rate * strength;
                if scale_cp >= 0 {
                    r *= axis(self.cp(scale_cp).position, scale_cp_field).max(0.0);
                }
                if per_used_cp != 0.0 {
                    r *= per_used_cp * self.cps_used as f32;
                }
                if r == 0.0 || (duration != 0.0 && self.prev_time > begin + duration) {
                    return;
                }
                let (mut from, mut to) = (self.prev_time, self.time);
                if duration != 0.0 {
                    from = from.max(begin);
                    to = to.min(begin + duration);
                }
                let window = (to - from).max(0.0);
                self.spread(k, r * window, from, to, free);
            }
            EmitterKind::Noise { start, duration, time_scale, time_offset, absolute, invert, min, max } => {
                let begin = st.started + start;
                if begin > self.time || (duration != 0.0 && self.prev_time > begin + duration) {
                    return;
                }
                let n = noise01(Vec3::new(self.time * time_scale + time_offset, 0.37, 0.71), absolute, invert);
                let r = (min + (max - min) * n) * strength;
                let (mut from, mut to) = (self.prev_time, self.time);
                if duration != 0.0 {
                    from = from.max(begin);
                    to = to.min(begin + duration);
                }
                let window = (to - from).max(0.0);
                self.spread(k, r.max(0.0) * window, from, to, free);
            }
            EmitterKind::Instant { per_step, scale_cp, scale_cp_field, .. } => {
                let start = st.started + st.start;
                if start > self.time {
                    return;
                }
                let scale = (scale_cp >= 0).then(|| axis(self.cp(scale_cp).position, scale_cp_field));
                let max_particles = self.def.max_particles.min(5000) as i32;
                let e = &mut self.emitters[k];
                if !e.fired {
                    e.fired = true;
                    let count = match scale {
                        Some(v) => (e.count as f32 * v) as i32,
                        None => e.count,
                    };
                    e.remaining = count.max(0);
                }
                if e.remaining <= 0 {
                    return;
                }
                let per = if per_step < 0 { max_particles } else { per_step };
                let n = e.remaining.min(per);
                e.remaining -= n;
                for _ in 0..(n.max(0) as usize).min(free) {
                    self.new_particle(start);
                }
            }
            EmitterKind::Maintain { count, start } => {
                if st.started + start > self.time {
                    return;
                }
                let want = (count.max(0) as usize).min(self.def.max_particles);
                for _ in self.particles.count..want {
                    self.new_particle(self.time);
                }
            }
        }
    }

    /// Adds `owed` to the emitter's running total and spawns the whole
    /// particles due, spread over `from..to`.
    fn spread(&mut self, k: usize, owed: f32, from: f32, to: f32, free: usize) {
        let e = &mut self.emitters[k];
        e.total += owed;
        let n = e.total.floor() as i32 - e.emitted;
        e.emitted += n;
        let n = (n.max(0) as usize).min(free);
        if n == 0 {
            return;
        }
        let step = (to - from) / n as f32;
        let mut t = from + step;
        for _ in 0..n {
            self.new_particle(t.min(to));
            t += step;
        }
    }

    fn init_particles(&mut self, first: usize, env: &dyn Env) {
        let def = self.def.clone();
        let mut unwritten: u32 = !(1 << field::CREATED);
        for init in def.initializers.iter().filter(|i| !i.kind.modifies()) {
            let w = init.kind.writes();
            if w != 0 && w & unwritten == 0 {
                continue;
            }
            let Some(strength) = self.strength(&init.common) else { continue };
            self.initialize(first, &init.kind, strength, env);
            unwritten &= !w;
        }
        for init in def.initializers.iter().filter(|i| i.kind.modifies()) {
            let Some(strength) = self.strength(&init.common) else { continue };
            self.initialize(first, &init.kind, strength, env);
        }
        self.apply_kills();
        let first = first.min(self.particles.count);
        self.particles.snapshot_initial(first);
    }

    fn set_velocity(&mut self, i: usize, v: Vec3) {
        let p = self.particles.v[field::POSITION][i];
        self.particles.v[field::PREV_POSITION][i] = p - v * self.prev_dt;
    }

    fn add_velocity(&mut self, i: usize, v: Vec3) {
        self.particles.v[field::PREV_POSITION][i] -= v * self.prev_dt;
    }

    /// Sets the velocity of a particle that has already moved this step.
    fn set_velocity_now(&mut self, i: usize, v: Vec3) {
        let p = self.particles.v[field::POSITION][i];
        self.particles.v[field::PREV_POSITION][i] = p - v * self.dt;
    }

    fn velocity(&self, i: usize) -> Vec3 {
        let p = &self.particles;
        if self.dt > 0.0 { (p.v[field::POSITION][i] - p.v[field::PREV_POSITION][i]) / self.dt } else { Vec3::ZERO }
    }

    fn scaled_output(&self, i: usize, output: usize, out: f32, scale_initial: bool, scale_current: bool) -> f32 {
        if scale_initial {
            out * self.particles.initial(output, i)
        } else if scale_current {
            out * self.particles.get(output, i)
        } else {
            out
        }
    }

    fn initialize(&mut self, first: usize, kind: &InitKind, strength: f32, env: &dyn Env) {
        use field::*;
        let n = self.particles.count;
        match *kind {
            InitKind::RandomScalar { field, min, max, exponent } => {
                let (min, max) = (min * strength, max * strength);
                for i in first..n {
                    let v = self.rand_exp(min, max, exponent);
                    self.particles.set(field, i, v);
                }
            }
            InitKind::RandomRotation { field, initial, min, max, exponent, flip } => {
                for i in first..n {
                    let mut v = initial + self.rand_exp(min, max, exponent);
                    if flip && self.rand() < 0.5 {
                        v = -v;
                    }
                    self.particles.set(field, i, v);
                }
            }
            InitKind::RandomColor { color1, color2, tint, tint_cp, clamp_min, clamp_max, blend, amplify, field } => {
                let c1 = Vec3::new(color1[0], color1[1], color1[2]);
                let c2 = Vec3::new(color2[0], color2[1], color2[2]);
                let tint_color = (tint > 0.0).then(|| {
                    // Light as the engine reports it (gamma), clamped and amplified.
                    let l = env.light(self.cp(tint_cp).position);
                    let g = Vec3::new(to_gamma(l.x), to_gamma(l.y), to_gamma(l.z));
                    let lo = Vec3::new(clamp_min[0], clamp_min[1], clamp_min[2]);
                    let hi = Vec3::new(clamp_max[0], clamp_max[1], clamp_max[2]).max(lo);
                    g.clamp(lo, hi) * amplify
                });
                if !is_vector(field) {
                    return;
                }
                for i in first..n {
                    let t = self.rand();
                    let mut c = c1.lerp(c2, t);
                    if let Some(l) = tint_color {
                        let mixed = match blend {
                            0 => l,
                            1 => Vec3::new(overlay(c.x, l.x), overlay(c.y, l.y), overlay(c.z, l.z)),
                            2 => c.min(l),
                            3 => c.max(l),
                            _ => c * l,
                        };
                        c = c.lerp(mixed, tint).min(Vec3::ONE);
                    }
                    self.particles.v[field][i] = c;
                }
            }
            InitKind::RandomSequence { field, min, max, shuffle, linear } => {
                for i in first..n {
                    let v = if shuffle || linear {
                        let len = (max - min + 1).max(1);
                        if self.deck.len() != len as usize || self.linear >= len {
                            self.deck = (min..=max.max(min)).collect();
                            if shuffle {
                                for j in (1..self.deck.len()).rev() {
                                    let k = (self.rand() * (j + 1) as f32) as usize;
                                    self.deck.swap(j, k.min(j));
                                }
                            }
                            self.linear = 0;
                        }
                        let v = self.deck[self.linear as usize];
                        self.linear += 1;
                        v
                    } else {
                        min + ((max - min + 1) as f32 * self.rand()) as i32
                    };
                    self.particles.set(field, i, v as f32);
                }
            }
            InitKind::YawFlip { chance } => {
                for i in first..n {
                    if self.rand() < chance {
                        self.particles.s[YAW][i] += PI;
                    }
                }
            }
            InitKind::Sphere { distance, bias, bias_abs, local_bias, cp, speed, local_speed, distribute_cps, growth_time, scale_cp } => {
                let scale = if scale_cp >= 0 { self.cp(scale_cp).position } else { Vec3::ONE };
                for i in first..n {
                    let created = self.particles.s[CREATED][i].max(self.prev_time);
                    let mut cpi = cp;
                    if distribute_cps {
                        let grow = if growth_time != 0.0 { self.time.min(growth_time) / growth_time } else { 1.0 };
                        let top = (self.cps_used as f32 * grow).floor() as i32;
                        cpi = cp + ((top - cp + 1).max(0) as f32 * self.rand()) as i32;
                    }
                    let (mut dir, r) = random_in_sphere(self);
                    if bias_abs.x != 0.0 {
                        dir.x = dir.x.abs();
                    }
                    if bias_abs.y != 0.0 {
                        dir.y = dir.y.abs();
                    }
                    if bias_abs.z != 0.0 {
                        dir.z = dir.z.abs();
                    }
                    let dir = (dir * bias).normalize_or_zero();
                    let dist = distance.0 + (distance.1 - distance.0) * r;
                    let c = self.cp(cpi);
                    let offset = dir * dist * scale.x;
                    let offset = if local_bias { c.rotate(offset) } else { offset };
                    self.particles.v[POSITION][i] = c.at(self.step_fraction(created)) + offset;
                    let mut vel = Vec3::ZERO;
                    if speed.1 > 0.0 {
                        vel += dir * self.rand_exp(speed.0, speed.1, speed.2) * scale.y;
                    }
                    if local_speed.0 != Vec3::ZERO || local_speed.1 != Vec3::ZERO {
                        // The game takes local y along the control point's right
                        // here, not its left.
                        let l = rand_vec(self, local_speed.0, local_speed.1);
                        vel += (c.forward * l.x - c.left * l.y + c.up * l.z) * scale.z;
                    }
                    self.set_velocity(i, vel);
                }
            }
            InitKind::Box { min, max, cp, local } => {
                for i in first..n {
                    let o = rand_vec(self, min, max);
                    let c = self.cp(cp);
                    let created = self.particles.s[CREATED][i];
                    let pos = c.at(self.step_fraction(created)) + if local { c.rotate(o) } else { o };
                    self.particles.v[POSITION][i] = pos;
                    self.particles.v[PREV_POSITION][i] = pos;
                }
            }
            InitKind::Offset { min, max, cp, local, proportional } => {
                for i in first..n {
                    let scale = if proportional { self.particles.s[RADIUS][i] } else { 1.0 };
                    let mut o = rand_vec(self, min * scale, max * scale);
                    if local {
                        o = self.cp(cp).rotate(o);
                    }
                    self.particles.v[POSITION][i] += o;
                    self.particles.v[PREV_POSITION][i] += o;
                }
            }
            InitKind::Warp { min, max, cp, transition, start, reverse } => {
                let c = self.cp(cp).position;
                for i in first..n {
                    let w = if transition > 0.0 {
                        let mut t = ((self.time - start) / transition).clamp(0.0, 1.0);
                        if reverse {
                            t = 1.0 - t;
                        }
                        min.lerp(max, t)
                    } else {
                        rand_vec(self, min, max)
                    };
                    for f in [POSITION, PREV_POSITION] {
                        let p = self.particles.v[f][i];
                        self.particles.v[f][i] = c + (p - c) * w;
                    }
                }
            }
            InitKind::VelocityNoise { cp, time_scale, spatial_scale, time_offset, spatial_offset, absolute, invert, min, max, local } => {
                for i in first..n {
                    let p = self.particles.v[POSITION][i] * spatial_scale + spatial_offset;
                    let t = self.time * time_scale + time_offset;
                    let mut v = Vec3::ZERO;
                    for k in 0..3 {
                        let s =
                            noise01(Vec3::new(p.x + t, p.y + k as f32 * 31.7, p.z - t), axis(absolute, k) != 0.0, axis(invert, k) != 0.0);
                        let value = axis(min, k) + (axis(max, k) - axis(min, k)) * s;
                        match k {
                            0 => v.x = value,
                            1 => v.y = value,
                            _ => v.z = value,
                        }
                    }
                    if local {
                        v = self.cp(cp).rotate(v);
                    }
                    self.add_velocity(i, v * strength);
                }
            }
            InitKind::VelocityRandom { cp, speed, local } => {
                for i in first..n {
                    let (dir, _) = random_in_sphere(self);
                    let s = self.rand_range(speed.0, speed.1);
                    let l = rand_vec(self, local.0, local.1);
                    let v = dir.normalize_or_zero() * s + self.cp(cp).rotate(l);
                    self.add_velocity(i, v);
                }
            }
            InitKind::VelocityInherit { cp, scale } => {
                let c = self.cp(cp);
                let v = if self.dt > 0.0 { (c.position - c.prev_position) / self.dt } else { Vec3::ZERO };
                for i in first..n {
                    self.add_velocity(i, v * scale);
                }
            }
            InitKind::VelocityFromCp { cp, scale } => {
                let v = self.cp(cp).position * scale;
                for i in first..n {
                    self.set_velocity(i, v);
                }
            }
            InitKind::Ring { cp, radius, thickness, speed, angles, even, even_count, xy_velocity } => {
                let c = self.cp(cp);
                let rot = Quat::from_euler(bevy::math::EulerRot::ZYX, angles.y.to_radians(), angles.x.to_radians(), angles.z.to_radians());
                for i in first..n {
                    let a = if even {
                        let count = if even_count > 0.0 { even_count } else { (n - first).max(1) as f32 };
                        (self.particles.id[i] as f32 % count) / count * TAU
                    } else {
                        self.rand() * TAU
                    };
                    let r = radius + self.rand_range(-thickness, thickness);
                    let dir = rot * Vec3::new(a.cos(), a.sin(), 0.0);
                    let lift = if xy_velocity { Vec3::ZERO } else { rot * Vec3::Z * self.rand_range(-1.0, 1.0) };
                    self.particles.v[POSITION][i] = c.position + c.rotate(dir * r);
                    let s = self.rand_range(speed.0, speed.1);
                    let v = c.rotate((dir + lift).normalize_or_zero() * s);
                    self.set_velocity(i, v);
                }
            }
            InitKind::AlongPath { sequential, max_distance, bulge, start_cp, end_cp, mid, steps, looping } => {
                let (a, b) = (self.cp(start_cp).position, self.cp(end_cp).position);
                for i in first..n {
                    let t = if sequential {
                        let steps = steps.max(1.0);
                        let k = self.particles.id[i] as f32 % (steps * if looping { 1.0 } else { 2.0 });
                        let t = k / steps;
                        if t > 1.0 { 2.0 - t } else { t }
                    } else {
                        self.rand()
                    };
                    let pos = path_point(self, a, b, t, mid, bulge, max_distance);
                    self.particles.v[POSITION][i] = pos;
                    self.particles.v[PREV_POSITION][i] = pos;
                }
            }
            InitKind::PlaceOnGround { offset, max_length, kill } => {
                for i in first..n {
                    let p = self.particles.v[POSITION][i];
                    let start = p + Vec3::Z * max_length;
                    let end = p - Vec3::Z * max_length;
                    match env.trace(start, end) {
                        Some((f, _)) => {
                            let z = start.lerp(end, f).z + offset;
                            self.particles.v[POSITION][i].z = z;
                            self.particles.v[PREV_POSITION][i].z += z - p.z;
                        }
                        None if kill => self.kill(i),
                        None => {}
                    }
                }
            }
            InitKind::LifetimeFromSequence { fps } => {
                for i in first..n {
                    let seq = self.particles.s[SEQUENCE][i] as u32;
                    let frames = self.sequence_frames.iter().find(|(s, _)| *s == seq).map(|f| f.1).unwrap_or(1.0);
                    self.particles.s[LIFE][i] = frames / fps.max(0.001);
                }
            }
            InitKind::RemapScalar { input, in_range, output, out_range, scale_initial, clamp_to_range, emitter_window } => {
                if emitter_window.0 >= 0.0 && (self.time < emitter_window.0 || self.time > emitter_window.1) {
                    return;
                }
                for i in first..n {
                    let v = if input == ID { self.particles.id[i] as f32 } else { self.particles.get(input, i) };
                    if clamp_to_range && (v < in_range.0.min(in_range.1) || v > in_range.0.max(in_range.1)) {
                        continue;
                    }
                    let mut out = remap_clamped(v, in_range, out_range);
                    if scale_initial {
                        out *= self.particles.get(output, i);
                    }
                    self.particles.set(output, i, out);
                }
            }
            InitKind::RemapNoise {
                field,
                time_scale,
                spatial_scale,
                time_offset,
                spatial_offset,
                absolute,
                invert,
                out_range,
                world_time_scale,
            } => {
                for i in first..n {
                    let p = self.particles.v[POSITION][i] * spatial_scale + spatial_offset;
                    let t = self.particles.s[CREATED][i] * time_scale + time_offset + self.time * world_time_scale;
                    let s = noise01(Vec3::new(p.x + t, p.y, p.z), absolute, invert);
                    self.particles.set(field, i, out_range.0 + (out_range.1 - out_range.0) * s);
                }
            }
            InitKind::RemapDistanceToCp { cp, distance, output, out_range, scale_initial, within_only } => {
                let c = self.cp(cp).position;
                for i in first..n {
                    let d = self.particles.v[POSITION][i].distance(c);
                    if within_only && (d < distance.0 || d > distance.1) {
                        continue;
                    }
                    let mut out = remap_clamped(d, distance, out_range);
                    if scale_initial {
                        out *= self.particles.get(output, i);
                    }
                    self.particles.set(output, i, out);
                }
            }
            InitKind::RemapCount { in_range, output, out_range, scale_initial, within_only } => {
                for i in first..n {
                    let v = self.particles.id[i] as f32;
                    if within_only && (v < in_range.0 as f32 || v > in_range.1 as f32) {
                        continue;
                    }
                    let mut out = remap_clamped(v, (in_range.0 as f32, in_range.1 as f32), out_range);
                    if scale_initial {
                        out *= self.particles.get(output, i);
                    }
                    self.particles.set(output, i, out);
                }
            }
            InitKind::RemapCpToScalar { cp, component, in_range, output, out_range, scale_initial } => {
                let out = remap_clamped(axis(self.cp(cp).position, component), in_range, out_range);
                for i in first..n {
                    let o = if scale_initial { out * self.particles.get(output, i) } else { out };
                    self.particles.set(output, i, o);
                }
            }
            InitKind::RemapCpToVector { cp, in_range, output, out_range, scale_initial, offset } => {
                if !is_vector(output) {
                    return;
                }
                let p = self.cp(cp).position;
                let v = Vec3::new(
                    remap_clamped(p.x, (in_range.0.x, in_range.1.x), (out_range.0.x, out_range.1.x)),
                    remap_clamped(p.y, (in_range.0.y, in_range.1.y), (out_range.0.y, out_range.1.y)),
                    remap_clamped(p.z, (in_range.0.z, in_range.1.z), (out_range.0.z, out_range.1.z)),
                );
                for i in first..n {
                    if offset && output == POSITION {
                        self.particles.v[POSITION][i] += v;
                        self.particles.v[PREV_POSITION][i] += v;
                    } else if scale_initial {
                        self.particles.v[output][i] *= v;
                    } else {
                        self.particles.v[output][i] = v;
                    }
                }
            }
            InitKind::PreAgeNoise { time_scale, spatial_scale, time_offset, spatial_offset, absolute, invert, age } => {
                for i in first..n {
                    let p = self.particles.v[POSITION][i] * spatial_scale + spatial_offset;
                    let s = noise01(Vec3::new(p.x + self.time * time_scale + time_offset, p.y, p.z), absolute, invert);
                    let pre = age.0 + (age.1 - age.0) * s;
                    let life = self.particles.s[LIFE][i];
                    self.particles.s[CREATED][i] -= pre * life;
                }
            }
            InitKind::NormalAlign { cp } => {
                let f = self.cp(cp).forward;
                for i in first..n {
                    self.particles.v[NORMAL][i] = f;
                }
            }
            InitKind::NormalOffset { cp, min, max, local, normalize } => {
                for i in first..n {
                    let mut o = rand_vec(self, min, max);
                    if local {
                        o = self.cp(cp).rotate(o);
                    }
                    let mut v = self.particles.v[NORMAL][i] + o;
                    if normalize {
                        v = v.normalize_or(Vec3::Z);
                    }
                    self.particles.v[NORMAL][i] = v;
                }
            }
            InitKind::BetweenPoints { speed, end_spread, start_offset, end_offset, end_cp } => {
                let a = self.cps[0].position;
                let b = self.cp(end_cp).position;
                for i in first..n {
                    let (d, r) = random_in_sphere(self);
                    let target = b + d * r * end_spread;
                    let dir = (target - a).normalize_or(Vec3::X);
                    let start = a + dir * start_offset;
                    let dist = (target - start).length() - end_offset;
                    let s = self.rand_range(speed.0, speed.1).max(0.001);
                    self.particles.v[POSITION][i] = start;
                    self.set_velocity(i, dir * s);
                    self.particles.s[LIFE][i] = (dist / s).max(0.0);
                }
            }
            InitKind::FromParent { velocity_scale, random } => {
                if self.parent_cache.is_empty() {
                    for i in first..n {
                        self.kill(i);
                    }
                    return;
                }
                for i in first..n {
                    let k = if random { (self.rand() * self.parent_cache.len() as f32) as usize } else { self.particles.id[i] as usize }
                        % self.parent_cache.len();
                    let (p, prev) = self.parent_cache[k];
                    self.particles.v[POSITION][i] = p;
                    self.particles.v[PREV_POSITION][i] = p - (p - prev) * velocity_scale;
                }
            }
            InitKind::TimeToImpact { max_length, offset, bias } => {
                for i in first..n {
                    let v = self.particles.v[POSITION][i] - self.particles.v[PREV_POSITION][i];
                    let speed = if self.prev_dt > 0.0 { v.length() / self.prev_dt } else { 0.0 };
                    if speed <= 0.0 {
                        continue;
                    }
                    let dir = (v * bias).normalize_or_zero();
                    let start = self.particles.v[POSITION][i];
                    let end = start + dir * max_length;
                    let dist = match env.trace(start, end) {
                        Some((f, _)) => (max_length * f - offset).max(0.0),
                        None => max_length,
                    };
                    self.particles.s[LIFE][i] = dist / speed;
                }
            }
            InitKind::RemapScalarToVector { input, in_range, output, out_range, scale_initial, local, cp, emitter_window } => {
                if !is_vector(output) || (emitter_window.0 >= 0.0 && (self.time < emitter_window.0 || self.time > emitter_window.1)) {
                    return;
                }
                let c = self.cp(cp);
                for i in first..n {
                    let t = remap_clamped(self.particles.get(input, i), in_range, (0.0, 1.0));
                    let mut v = out_range.0.lerp(out_range.1, t);
                    if local && output != COLOR {
                        v = c.rotate(v);
                    }
                    if output == POSITION && local {
                        v += c.position;
                    }
                    if scale_initial {
                        self.particles.v[output][i] *= v;
                    } else {
                        self.particles.v[output][i] = v;
                    }
                }
            }
            InitKind::LitColor { color1, color2, clamp_min, clamp_max, blend, amplify } => {
                let c1 = Vec3::new(color1[0], color1[1], color1[2]);
                let c2 = Vec3::new(color2[0], color2[1], color2[2]);
                let lo = Vec3::new(clamp_min[0], clamp_min[1], clamp_min[2]);
                let hi = Vec3::new(clamp_max[0], clamp_max[1], clamp_max[2]).max(lo);
                for i in first..n {
                    let l = env.light(self.particles.v[POSITION][i]);
                    let l = Vec3::new(to_gamma(l.x), to_gamma(l.y), to_gamma(l.z)).clamp(lo, hi) * amplify;
                    let c = c1.lerp(c2, self.rand());
                    let lit = match blend {
                        0 => l,
                        1 => Vec3::new(overlay(c.x, l.x), overlay(c.y, l.y), overlay(c.z, l.z)),
                        2 => c.min(l),
                        3 => c.max(l),
                        _ => c * l,
                    };
                    self.particles.v[COLOR][i] = lit.min(Vec3::ONE);
                }
            }
            InitKind::Unsupported(_) => {}
        }
    }

    /// Runs the operators that go before emission (`before`) or the rest.
    fn step(&mut self, env: &dyn Env, before: bool) {
        let def = self.def.clone();
        for (k, op) in def.operators.iter().enumerate() {
            if op.kind.before_emission() != before {
                continue;
            }
            let Some(strength) = self.strength(&op.common) else { continue };
            self.operate(k, &op.kind, strength, env);
            self.apply_kills();
            if !before && self.particles.count == 0 {
                break;
            }
        }
    }

    fn operate(&mut self, k: usize, kind: &OpKind, strength: f32, env: &dyn Env) {
        use field::*;
        let n = self.particles.count;
        let time = self.time;
        let dt = self.dt;
        match *kind {
            OpKind::Movement { gravity, drag, passes } => {
                let adj = (1.0 - drag.max(0.0)).max(0.0).powf(30.0 * dt);
                let scale = if self.prev_dt > 0.0 { adj * dt / self.prev_dt } else { adj };
                self.accel.clear();
                self.accel.resize(n, gravity);
                let def = self.def.clone();
                for f in &def.forces {
                    if let Some(s) = self.strength(&f.common) {
                        self.add_forces(&f.kind, s);
                    }
                }
                let p = &mut self.particles;
                for i in 0..n {
                    let pos = p.v[POSITION][i];
                    let prev = p.v[PREV_POSITION][i];
                    p.v[PREV_POSITION][i] = pos;
                    p.v[POSITION][i] = pos + (pos - prev) * scale + self.accel[i] * dt * dt;
                }
                if !def.constraints.is_empty() {
                    for _ in 0..passes.max(1) {
                        for c in &def.constraints {
                            if let Some(s) = self.strength(&c.common) {
                                self.constrain(&c.kind, s, env);
                            }
                        }
                    }
                }
            }
            OpKind::Decay => {
                for i in 0..n {
                    let life = self.particles.s[LIFE][i];
                    if life <= self.particles.age(i, time) || life <= 0.0 {
                        self.kill(i);
                    }
                }
            }
            OpKind::FadeAndKill { alpha, fade_in, fade_out } => {
                for i in 0..n {
                    let life = self.particles.s[LIFE][i];
                    let age = self.particles.age(i, time);
                    if life <= age {
                        self.kill(i);
                        continue;
                    }
                    let t = age / life;
                    let a0 = self.particles.s0[ALPHA][i];
                    if t >= fade_in.0 && t < fade_in.1 {
                        let s = smoothstep((t - fade_in.0) / (fade_in.1 - fade_in.0).max(1e-6));
                        self.particles.s[ALPHA][i] = a0 * alpha.0 + s * (a0 - a0 * alpha.0);
                    }
                    if t >= fade_out.0 && t < fade_out.1 {
                        let s = smoothstep((t - fade_out.0) / (fade_out.1 - fade_out.0).max(1e-6));
                        self.particles.s[ALPHA][i] = a0 + s * (a0 * alpha.1 - a0);
                    }
                }
            }
            OpKind::FadeIn { time: range, exponent, proportional } => {
                for i in 0..n {
                    let fade = if range.0 == range.1 {
                        range.0
                    } else {
                        range.0 + (range.1 - range.0) * self.particle_rand(i, k as u32).powf(exponent)
                    };
                    let t = if proportional { self.particles.age_fraction(i, time).clamp(0.0, 1.0) } else { self.particles.age(i, time) };
                    if t < fade && fade > 0.0 {
                        self.particles.s[ALPHA][i] = self.particles.s0[ALPHA][i] * smoothstep(t / fade);
                    }
                }
            }
            OpKind::FadeOut { time: range, exponent, proportional, ease, bias: b } => {
                let mut range = range;
                if range.0 != range.1 && range.0 == 0.0 {
                    range = (1.192_092_9e-7, 1.192_092_9e-7);
                }
                for i in 0..n {
                    let fade = if range.0 == range.1 {
                        range.0
                    } else {
                        range.0 + (range.1 - range.0) * self.particle_rand(i, k as u32).powf(exponent)
                    };
                    let life = self.particles.s[LIFE][i];
                    let age = self.particles.age(i, time);
                    let f = if proportional {
                        let start = 1.0 - fade;
                        let t = if life > 0.0 { age / life } else { 0.0 };
                        if t <= start {
                            continue;
                        }
                        ((t - start) / (1.0 - start).max(1e-6)).clamp(0.0, 1.0)
                    } else {
                        let start = life - fade;
                        if age <= start {
                            continue;
                        }
                        ((age - start) / fade.max(1e-6)).clamp(0.0, 1.0)
                    };
                    let a0 = self.particles.s0[ALPHA][i];
                    self.particles.s[ALPHA][i] = if ease { (a0 * (1.0 - smoothstep(f))).max(0.0) } else { (1.0 - bias(f, b)) * a0 };
                }
            }
            OpKind::FadeInSimple { time: fade } => {
                for i in 0..n {
                    let t = self.particles.age_fraction(i, time).clamp(0.0, 1.0);
                    if t < fade {
                        self.particles.s[ALPHA][i] = self.particles.s0[ALPHA][i] * smoothstep(t / fade);
                    }
                }
            }
            OpKind::FadeOutSimple { time: fade } => {
                let start = 1.0 - fade;
                for i in 0..n {
                    let t = self.particles.age_fraction(i, time).clamp(0.0, 1.0);
                    if t > start {
                        let f = ((t - start) / fade.max(1e-6)).clamp(0.0, 1.0);
                        self.particles.s[ALPHA][i] = self.particles.s0[ALPHA][i] * (1.0 - smoothstep(f));
                    }
                }
            }
            OpKind::RadiusScale { time: (start, end), scale, ease, bias: b } => {
                if start >= end {
                    return;
                }
                for i in 0..n {
                    let life = self.particles.s[LIFE][i];
                    if life <= 0.0 {
                        continue;
                    }
                    let t = self.particles.age(i, time) / life;
                    if t < start || t >= end {
                        continue;
                    }
                    let f = (t - start) / (end - start);
                    let f = if ease { smoothstep(f) } else { bias(f, b) };
                    let r0 = self.particles.s0[RADIUS][i];
                    self.particles.s[RADIUS][i] = r0 * (scale.0 + f * (scale.1 - scale.0));
                }
            }
            OpKind::ColorFade { color, time: (start, end), ease, field } => {
                if start == end || !is_vector(field) {
                    return;
                }
                let target = Vec3::new(color[0], color[1], color[2]);
                for i in 0..n {
                    let life = self.particles.s[LIFE][i];
                    if life <= 0.0 {
                        continue;
                    }
                    let t = self.particles.age(i, time) / life;
                    let mut f = ((t - start) / (end - start)).clamp(0.0, 1.0);
                    if ease {
                        f = smoothstep(f);
                    }
                    let c0 = self.particles.v0[field][i];
                    self.particles.v[field][i] = c0 + (target - c0) * f;
                }
            }
            OpKind::Spin { field, rate, min_rate, stop_time } => {
                let rate = rate * strength;
                if rate == 0.0 {
                    return;
                }
                let mut step = (rate * dt).abs();
                if stop_time == 0.0 {
                    step %= TAU;
                }
                if rate < 0.0 {
                    step = -step;
                }
                let min_step = (min_rate * dt).abs();
                for i in 0..n {
                    let decay = if stop_time != 0.0 {
                        let life = self.particles.s[LIFE][i];
                        (1.0 - self.particles.age(i, time) / (life * stop_time).max(1e-6)).max(0.0)
                    } else {
                        1.0
                    };
                    // The game keeps the larger of the decayed step and the minimum.
                    let mut v = self.particles.s[field][i] + (decay * step).max(min_step);
                    if v >= TAU {
                        v -= TAU;
                    } else if v <= -TAU {
                        v += TAU;
                    }
                    self.particles.s[field][i] = v;
                }
            }
            OpKind::SpinUpdate => {
                for i in 0..n {
                    self.particles.s[ROLL][i] += self.particles.s[ROLL_SPEED][i] * dt;
                }
            }
            OpKind::Oscillate { field, rate, frequency, proportional, start, end, start_end_proportional, multiplier, phase, vector } => {
                let base = multiplier * time + phase;
                for i in 0..n {
                    let life = self.particles.s[LIFE][i];
                    if life <= 0.0 {
                        continue;
                    }
                    let age = self.particles.age(i, time);
                    let t = if start_end_proportional { age / life } else { age };
                    let s = start.0 + (start.1 - start.0) * self.particle_rand(i, 11);
                    let e = end.0 + (end.1 - end.0) * self.particle_rand(i, 12);
                    if t < s || t >= e {
                        continue;
                    }
                    let rf = self.particle_rand(i, 0);
                    let rr = self.particle_rand(i, 1);
                    for c in 0..if vector { 3 } else { 1 } {
                        let f = axis(frequency.0, c) + (axis(frequency.1, c) - axis(frequency.0, c)) * rf;
                        let r = axis(rate.0, c) + (axis(rate.1, c) - axis(rate.0, c)) * rr;
                        let x = if proportional { age / life * f * multiplier + phase } else { f * base };
                        let delta = sin_approx(x) * r * dt * strength;
                        if vector && is_vector(field) {
                            let v = &mut self.particles.v[field][i];
                            match c {
                                0 => v.x += delta,
                                1 => v.y += delta,
                                _ => v.z += delta,
                            }
                        } else if !is_vector(field) {
                            let v = &mut self.particles.s[field][i];
                            *v += delta;
                            if field == ALPHA || field == ALPHA2 {
                                *v = v.clamp(0.0, 1.0);
                            }
                        }
                    }
                }
            }
            OpKind::OscillateSimple { field, rate, frequency, multiplier, phase } => {
                if is_vector(field) {
                    return;
                }
                let delta = sin_approx(frequency * (multiplier * time + phase)) * rate * dt * strength;
                for i in 0..n {
                    let v = &mut self.particles.s[field][i];
                    *v += delta;
                    if field == ALPHA || field == ALPHA2 {
                        *v = v.clamp(0.0, 1.0);
                    }
                }
            }
            OpKind::Lock { cp, fade_start, fade_end, distance_fade } => {
                let c = self.cp(cp);
                let delta = c.position - c.prev_position;
                if delta == Vec3::ZERO {
                    return;
                }
                for i in 0..n {
                    let t = self.particles.age_fraction(i, time);
                    let s = fade_start.0 + (fade_start.1 - fade_start.0) * self.particle_rand(i, 21).powf(fade_start.2);
                    let e = fade_end.0 + (fade_end.1 - fade_end.0) * self.particle_rand(i, 22).powf(fade_end.2);
                    let mut amount = 1.0;
                    if t > s {
                        amount = if e > s { (1.0 - (t - s) / (e - s)).clamp(0.0, 1.0) } else { 0.0 };
                    }
                    if distance_fade > 0.0 {
                        let d = self.particles.v[POSITION][i].distance(c.position);
                        amount *= (1.0 - d / distance_fade).clamp(0.0, 1.0);
                    }
                    let m = delta * amount * strength;
                    self.particles.v[POSITION][i] += m;
                    self.particles.v[PREV_POSITION][i] += m;
                }
            }
            OpKind::Ramp { field, rate, start, end, proportional, spline } => {
                if is_vector(field) {
                    return;
                }
                for i in 0..n {
                    let life = self.particles.s[LIFE][i];
                    let age = self.particles.age(i, time);
                    let t = if proportional { if life > 0.0 { age / life } else { 0.0 } } else { age };
                    let s = start.0 + (start.1 - start.0) * self.particle_rand(i, 31);
                    let e = end.0 + (end.1 - end.0) * self.particle_rand(i, 32);
                    if t < s || t >= e {
                        continue;
                    }
                    let r = rate.0 + (rate.1 - rate.0) * self.particle_rand(i, 33);
                    let mut scale = 1.0;
                    if let Some((b, ease_out)) = spline {
                        let f = ((t - s) / (e - s).max(1e-6)).clamp(0.0, 1.0);
                        scale = bias(if ease_out { 1.0 - f } else { f }, b);
                    }
                    self.particles.s[field][i] += r * scale * dt;
                }
            }
            OpKind::Clamp { field, min, max } => {
                for i in 0..n {
                    if is_vector(field) {
                        let v = &mut self.particles.v[field][i];
                        *v = v.clamp(Vec3::splat(min), Vec3::splat(max.max(min)));
                    } else {
                        let v = &mut self.particles.s[field][i];
                        *v = v.clamp(min, max.max(min));
                    }
                }
            }
            OpKind::MinimumDecay { field, minimum } => {
                for i in 0..n {
                    let v = if field == PREV_POSITION { self.velocity(i).length() } else { self.particles.get(field, i) };
                    if v < minimum {
                        self.kill(i);
                    }
                }
            }
            OpKind::RemapDistanceToCp { cp, distance, output, out_range, scale_initial, scale_current, within_only } => {
                let c = self.cp(cp).position;
                for i in 0..n {
                    let d = self.particles.v[POSITION][i].distance(c);
                    if within_only && (d < distance.0 || d > distance.1) {
                        continue;
                    }
                    let out = remap_clamped(d, distance, out_range);
                    let out = self.scaled_output(i, output, out, scale_initial, scale_current);
                    self.particles.set(output, i, out);
                }
            }
            OpKind::RemapScalar { input, in_range, output, out_range } => {
                for i in 0..n {
                    let v = self.particles.get(input, i);
                    self.particles.set(output, i, remap_clamped(v, in_range, out_range));
                }
            }
            OpKind::RemapSpeed { in_range, output, out_range, scale_initial, scale_current } => {
                for i in 0..n {
                    let out = remap_clamped(self.velocity(i).length(), in_range, out_range);
                    let out = self.scaled_output(i, output, out, scale_initial, scale_current);
                    self.particles.set(output, i, out);
                }
            }
            OpKind::RemapCpToScalar { cp, component, in_range, output, out_range, scale_initial, scale_current } => {
                let out = remap_clamped(axis(self.cp(cp).position, component), in_range, out_range);
                for i in 0..n {
                    let o = self.scaled_output(i, output, out, scale_initial, scale_current);
                    self.particles.set(output, i, o);
                }
            }
            OpKind::RemapDirectionToCp { cp, output, scale, normalize } => {
                if !is_vector(output) {
                    return;
                }
                let c = self.cp(cp).position;
                for i in 0..n {
                    let mut d = c - self.particles.v[POSITION][i];
                    if normalize {
                        d = d.normalize_or_zero();
                    }
                    self.particles.v[output][i] = d * scale;
                }
            }
            OpKind::RemapVelocityToVector { output, scale, normalize } => {
                if !is_vector(output) {
                    return;
                }
                for i in 0..n {
                    let mut v = self.velocity(i);
                    if normalize {
                        v = v.normalize_or_zero();
                    }
                    self.particles.v[output][i] = v * scale;
                }
            }
            OpKind::PercentageBetweenCps {
                range,
                output,
                out_range,
                start_cp,
                end_cp,
                scale_initial,
                scale_current,
                within_only,
                radius,
            } => {
                let (a, b) = (self.cp(start_cp).position, self.cp(end_cp).position);
                let len = a.distance(b).max(1e-4);
                for i in 0..n {
                    let p = self.particles.v[POSITION][i];
                    let pct = if radius { p.distance(a) / len } else { (p - a).dot(b - a) / (len * len) };
                    if within_only && (pct < range.0 || pct > range.1) {
                        continue;
                    }
                    let out = remap_clamped(pct, range, out_range);
                    let out = self.scaled_output(i, output, out, scale_initial, scale_current);
                    self.particles.set(output, i, out);
                }
            }
            OpKind::Dampen { cp, range, scale } => {
                let c = self.cp(cp).position;
                if range <= 0.0 {
                    return;
                }
                for i in 0..n {
                    let p = self.particles.v[POSITION][i];
                    let d = p.distance(c);
                    if d >= range {
                        continue;
                    }
                    let keep = 1.0 - (1.0 - d / range) * scale.clamp(0.0, 1.0) * strength;
                    let prev = self.particles.v[PREV_POSITION][i];
                    self.particles.v[PREV_POSITION][i] = p - (p - prev) * keep;
                }
            }
            OpKind::MaxVelocity { max } => {
                for i in 0..n {
                    let v = self.velocity(i);
                    let s = v.length();
                    if s > max && s > 0.0 {
                        self.set_velocity_now(i, v * (max / s));
                    }
                }
            }
            OpKind::SetCpPositions { world, offset_cp, points } => {
                let base = self.cp(offset_cp);
                for (cp, parent, loc) in points {
                    if !(1..MAX_CONTROL_POINTS as i32).contains(&cp) {
                        continue;
                    }
                    let mut c = self.cp(parent);
                    c.prev_position = self.cps[cp as usize].position;
                    c.position = if world { loc } else { base.position + base.rotate(loc) };
                    self.set_cp(cp as usize, c);
                }
            }
            OpKind::SetCpRotation { axis: ax, rate, cp, local_cp } => {
                if !(0..MAX_CONTROL_POINTS as i32).contains(&cp) {
                    return;
                }
                let mut c = self.cp(cp);
                let a = if local_cp >= 0 { self.cp(local_cp).rotate(ax) } else { ax };
                c.set_rotation(Quat::from_axis_angle(a.normalize_or(Vec3::Z), rate * dt * strength) * c.rotation());
                self.set_cp(cp as usize, c);
            }
            OpKind::MaintainPath { max_distance, bulge, start_cp, end_cp, mid, steps, looping, cohesion, use_count } => {
                let (a, b) = (self.cp(start_cp).position, self.cp(end_cp).position);
                let steps = if use_count { n as f32 } else { steps }.max(1.0);
                for i in 0..n {
                    let k = self.particles.id[i] as f32 % (steps * if looping { 1.0 } else { 2.0 });
                    let mut t = k / steps;
                    if t > 1.0 {
                        t = 2.0 - t;
                    }
                    let target = path_point_stable(self, i, a, b, t, mid, bulge, max_distance);
                    let m = (target - self.particles.v[POSITION][i]) * cohesion.clamp(0.0, 1.0);
                    self.particles.v[POSITION][i] += m;
                    self.particles.v[PREV_POSITION][i] += m;
                }
            }
            OpKind::CullPlane { cp, offset, normal } => {
                let c = self.cp(cp).position;
                let nrm = normal.normalize_or(Vec3::Z);
                for i in 0..n {
                    if (self.particles.v[POSITION][i] - c).dot(nrm) < offset {
                        self.kill(i);
                    }
                }
            }
            OpKind::CullSphere { cp, distance, offset, inside } => {
                let c = self.cp(cp);
                let centre = c.position + c.rotate(offset);
                for i in 0..n {
                    let d = self.particles.v[POSITION][i].distance(centre);
                    if (inside && d < distance) || (!inside && d > distance) {
                        self.kill(i);
                    }
                }
            }
            OpKind::CullRandom { time: (s, e), exponent, percentage } => {
                for i in 0..n {
                    let t = self.particles.age_fraction(i, time);
                    if t < s || t > e {
                        continue;
                    }
                    let cut = s + (e - s) * self.particle_rand(i, 41).powf(exponent);
                    if t >= cut && self.particle_rand(i, 42) < percentage {
                        self.kill(i);
                    }
                }
            }
            OpKind::LerpInitial { time: (s, e), field, value } => {
                if is_vector(field) {
                    return;
                }
                for i in 0..n {
                    let t = self.particles.age_fraction(i, time);
                    if t < s || t > e {
                        continue;
                    }
                    let f = ((t - s) / (e - s).max(1e-6)).clamp(0.0, 1.0);
                    let v0 = self.particles.s0[field][i];
                    self.particles.s[field][i] = v0 + (value - v0) * f;
                }
            }
            OpKind::SetChildCps { group, first_cp, count, first_particle, orient } => {
                let pts: Vec<(Vec3, Vec3)> = (0..count.max(0) as usize)
                    .filter_map(|j| {
                        let i = first_particle.max(0) as usize + j;
                        (i < n).then(|| (self.particles.v[POSITION][i], self.velocity(i)))
                    })
                    .collect();
                for c in self.children.iter_mut().filter(|c| c.group == group) {
                    for (j, (p, v)) in pts.iter().enumerate() {
                        let idx = first_cp.max(0) as usize + j;
                        if idx >= MAX_CONTROL_POINTS {
                            break;
                        }
                        let mut cp = c.collection.cps[idx];
                        cp.prev_position = cp.position;
                        cp.position = *p;
                        if orient && *v != Vec3::ZERO {
                            cp.set_rotation(Quat::from_rotation_arc(Vec3::X, v.normalize()));
                        }
                        c.collection.set_cp(idx, cp);
                    }
                }
            }
            OpKind::SetCpsFromParticles { first_cp, count, first_particle } => {
                for j in 0..count.max(0) as usize {
                    let i = first_particle.max(0) as usize + j;
                    let idx = first_cp.max(0) as usize + j;
                    if i >= n || idx >= MAX_CONTROL_POINTS || idx == 0 {
                        continue;
                    }
                    let mut cp = self.cps[idx];
                    cp.prev_position = cp.position;
                    cp.position = self.particles.v[POSITION][i];
                    self.set_cp(idx, cp);
                }
            }
            OpKind::ImpactPoint { set_cp, from_cp, direction, rate, length, offset } => {
                let st = &mut self.ops[k];
                st.timer -= dt;
                if st.timer > 0.0 && st.value != 0.0 {
                    return;
                }
                st.timer = rate;
                st.value = 1.0;
                if !(0..MAX_CONTROL_POINTS as i32).contains(&set_cp) {
                    return;
                }
                let from = self.cp(from_cp);
                let dir = if direction != Vec3::ZERO { from.rotate(direction).normalize_or(from.forward) } else { from.forward };
                let end = from.position + dir * length;
                let hit = match env.trace(from.position, end) {
                    Some((f, _)) => from.position.lerp(end, f) - dir * offset,
                    None => end,
                };
                let mut c = self.cps[set_cp as usize];
                c.prev_position = c.position;
                c.position = hit;
                self.set_cp(set_cp as usize, c);
            }
            OpKind::RemapCpSpeedToCp { input, in_range, output, component, out_range } => {
                if !(1..MAX_CONTROL_POINTS as i32).contains(&output) {
                    return;
                }
                let c = self.cp(input);
                let speed = if dt > 0.0 { (c.position - c.prev_position).length() / dt } else { 0.0 };
                let v = remap_clamped(speed, in_range, out_range);
                let mut o = self.cps[output as usize];
                match component {
                    0 => o.position.x = v,
                    1 => o.position.y = v,
                    _ => o.position.z = v,
                }
                self.set_cp(output as usize, o);
            }
            OpKind::RestartAfter { time: (lo, hi), children_only } => {
                let r = self.rand_range(lo, hi);
                let st = &mut self.ops[k];
                if st.value == 0.0 {
                    st.value = 1.0;
                    st.timer = r;
                }
                st.timer -= dt;
                if st.timer <= 0.0 {
                    st.timer = r;
                    if children_only {
                        for c in &mut self.children {
                            c.collection.restart();
                        }
                    } else {
                        self.start_emitters();
                    }
                }
            }
            OpKind::NormalLock { cp } => {
                let f = self.cp(cp).forward;
                for i in 0..n {
                    self.particles.v[NORMAL][i] = f;
                }
            }
            OpKind::Unsupported(_) => {}
        }
    }

    fn add_forces(&mut self, kind: &ForceKind, strength: f32) {
        use field::*;
        let n = self.particles.count;
        match *kind {
            ForceKind::Random { min, max } => {
                for i in 0..n {
                    let f = rand_vec(self, min, max);
                    self.accel[i] += f * strength;
                }
            }
            ForceKind::Attract { cp, amount, falloff } => {
                let c = self.cp(cp).position;
                for i in 0..n {
                    let d = c - self.particles.v[POSITION][i];
                    let len = d.length().max(1e-3);
                    self.accel[i] += d / len * (amount * strength / len.powf(falloff).max(1e-6));
                }
            }
            ForceKind::Twist { amount, axis: ax, local } => {
                let c = self.cps[0];
                let a = if local { c.rotate(ax) } else { ax }.normalize_or(Vec3::Z);
                for i in 0..n {
                    let r = self.particles.v[POSITION][i] - c.position;
                    self.accel[i] += a.cross(r).normalize_or_zero() * amount * strength;
                }
            }
            ForceKind::PlaneDistance { cp, min, max, force_min, force_max, normal, exponent } => {
                let c = self.cp(cp).position;
                let nrm = normal.normalize_or(Vec3::Z);
                for i in 0..n {
                    let d = (self.particles.v[POSITION][i] - c).dot(nrm);
                    let t = ((d - min) / (max - min).max(1e-6)).clamp(0.0, 1.0).powf(exponent);
                    self.accel[i] += force_min.lerp(force_max, t) * strength;
                }
            }
            ForceKind::Turbulence { scales, amounts } => {
                for i in 0..n {
                    let p = self.particles.v[POSITION][i];
                    let mut f = Vec3::ZERO;
                    for (s, a) in scales.iter().zip(amounts) {
                        if *s == 0.0 {
                            continue;
                        }
                        let q = p * *s;
                        f += Vec3::new(noise(q), noise(q + Vec3::splat(17.0)), noise(q + Vec3::splat(41.0))) * a;
                    }
                    self.accel[i] += f * strength;
                }
            }
            ForceKind::Unsupported(_) => {}
        }
    }

    fn constrain(&mut self, kind: &ConstraintKind, _strength: f32, env: &dyn Env) {
        use field::*;
        let n = self.particles.count;
        match *kind {
            ConstraintKind::Collide { bounce, slide, radius_scale, kill, min_kill_speed } => {
                for i in 0..n {
                    let prev = self.particles.v[PREV_POSITION][i];
                    let pos = self.particles.v[POSITION][i];
                    if prev == pos {
                        continue;
                    }
                    let Some((f, normal)) = env.trace(prev, pos) else { continue };
                    let speed = (pos - prev).length() / self.dt.max(1e-4);
                    if kill && (min_kill_speed < 0.0 || speed >= min_kill_speed) {
                        self.kill(i);
                        continue;
                    }
                    let r = (self.particles.s[RADIUS][i] * radius_scale * 0.1).min(2.0);
                    let hit = prev.lerp(pos, f) + normal * r;
                    let vel = pos - prev;
                    let into = vel.dot(normal);
                    let out = (vel - normal * into) * slide - normal * into * bounce;
                    self.particles.v[POSITION][i] = hit;
                    self.particles.v[PREV_POSITION][i] = hit - out;
                }
            }
            ConstraintKind::Distance { cp, min, max, offset, global } => {
                let c = self.cp(cp);
                let centre = if global { offset } else { c.position + c.rotate(offset) };
                for i in 0..n {
                    let d = self.particles.v[POSITION][i] - centre;
                    let len = d.length();
                    if len > 0.0 && (len < min || len > max) {
                        self.particles.v[POSITION][i] = centre + d / len * len.clamp(min, max.max(min));
                    }
                }
            }
            ConstraintKind::Path { min, max, max_mid, max_end, travel_time, start_cp, end_cp, mid } => {
                let (a, b) = (self.cp(start_cp).position, self.cp(end_cp).position);
                let time = self.time;
                for i in 0..n {
                    let t = (self.particles.age(i, time) / travel_time.max(1e-3)).clamp(0.0, 1.0);
                    let centre = a.lerp(b, t);
                    let m = if max_mid >= 0.0 { max_mid } else { max };
                    let e = if max_end >= 0.0 { max_end } else { max };
                    let limit =
                        if t < mid { max + (m - max) * (t / mid.max(1e-3)) } else { m + (e - m) * ((t - mid) / (1.0 - mid).max(1e-3)) };
                    let d = self.particles.v[POSITION][i] - centre;
                    let len = d.length();
                    if len > 0.0 && (len < min || len > limit) {
                        self.particles.v[POSITION][i] = centre + d / len * len.clamp(min, limit.max(min));
                    }
                }
            }
            ConstraintKind::Plane { cp, point, normal, global_origin, global_normal } => {
                let c = self.cp(cp);
                let origin = if global_origin { point } else { c.position + c.rotate(point) };
                let nrm = if global_normal { normal } else { c.rotate(normal) }.normalize_or(Vec3::Z);
                for i in 0..n {
                    let p = self.particles.v[POSITION][i];
                    let d = (p - origin).dot(nrm);
                    if d < 0.0 {
                        self.particles.v[POSITION][i] = p - nrm * d;
                    }
                }
            }
            ConstraintKind::Unsupported(_) => {}
        }
    }
}

fn to_gamma(c: f32) -> f32 {
    let c = c.max(0.0);
    if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

fn overlay(base: f32, top: f32) -> f32 {
    if top >= 0.5 { 1.0 - 2.0 * (1.0 - base) * (1.0 - top) } else { 2.0 * base * top }
}

/// A point at `t` along a bulging path from `a` to `b`, scattered within
/// `max_distance`.
fn path_point(c: &mut Collection, a: Vec3, b: Vec3, t: f32, mid: f32, bulge: f32, max_distance: f32) -> Vec3 {
    let (dir, _) = random_in_sphere(c);
    let (jitter, r) = random_in_sphere(c);
    let control = a.lerp(b, mid) + dir * bulge * a.distance(b);
    quadratic(a, control, b, t) + jitter * r * max_distance
}

/// Like `path_point`, with the scatter fixed per particle.
#[allow(clippy::too_many_arguments)]
fn path_point_stable(c: &Collection, i: usize, a: Vec3, b: Vec3, t: f32, mid: f32, bulge: f32, max_distance: f32) -> Vec3 {
    let r = |s: u32| c.particle_rand(i, s) * 2.0 - 1.0;
    let dir = Vec3::new(r(51), r(52), r(53)).normalize_or_zero();
    let jitter = Vec3::new(r(54), r(55), r(56));
    let control = a.lerp(b, mid) + dir * bulge * a.distance(b);
    quadratic(a, control, b, t) + jitter * max_distance
}

fn quadratic(a: Vec3, c: Vec3, b: Vec3, t: f32) -> Vec3 {
    let u = 1.0 - t;
    a * (u * u) + c * (2.0 * u * t) + b * (t * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Lib;
    impl Library for Lib {
        fn def(&self, _index: usize) -> Option<DefRef> {
            None
        }
    }

    fn def_from(emitters: Vec<Op<EmitterKind>>, operators: Vec<Op<OpKind>>) -> DefRef {
        let pcf = source_formats::pcf::ParticleSystemDefinition::default();
        let mut d = Def::compile(&pcf, |_| None);
        d.emitters = emitters;
        d.operators = operators;
        std::sync::Arc::new(d)
    }

    fn op<K>(kind: K) -> Op<K> {
        Op { common: Common { end_cap: -1, constant: true, ..Default::default() }, kind }
    }

    #[test]
    fn continuous_rate_carries_fractions() {
        let def = def_from(
            vec![op(EmitterKind::Continuous { start: 0.0, rate: 10.0, duration: 0.0, scale_cp: -1, scale_cp_field: 0, per_used_cp: 0.0 })],
            vec![],
        );
        let mut c = Collection::new(def, &Lib, 1);
        for _ in 0..60 {
            c.simulate(1.0 / 60.0, &NoEnv, &Lib);
        }
        // Ten a second, fractions carried over between frames.
        assert!((9..=10).contains(&c.particles.count), "{}", c.particles.count);
    }

    #[test]
    fn decay_kills_at_lifetime() {
        let def = def_from(
            vec![op(EmitterKind::Instant {
                start: 0.0,
                start_max: -1.0,
                count: 5,
                count_min: -1,
                per_step: -1,
                scale_cp: -1,
                scale_cp_field: 0,
            })],
            vec![op(OpKind::Decay)],
        );
        let mut c = Collection::new(def, &Lib, 1);
        c.simulate(0.05, &NoEnv, &Lib);
        assert_eq!(c.particles.count, 5);
        // The default lifetime is one second.
        for _ in 0..25 {
            c.simulate(0.05, &NoEnv, &Lib);
        }
        assert_eq!(c.particles.count, 0);
        assert!(c.finished());
    }

    #[test]
    fn sine_stand_in_tracks_sine() {
        for k in 0..40 {
            let x = k as f32 * 0.13 - 2.0;
            assert!((sin_approx(x) - (PI * x).sin()).abs() < 0.06, "{x}");
        }
    }
}
