//! Brush movers: sliding and rotating doors, linear movers, toggleable
//! brushes and continuously rotating brushes.

use bevy::prelude::*;

use super::io::{FireOutput, InputCall, InputHandlers, fire_output};
use super::{MapEntity, SpawnHandlers};
use crate::clock::{GameClock, ThinkSet};
use crate::coords::angles_forward;
use crate::world::CurrentMap;

pub fn register(app: &mut App) {
    app.add_systems(FixedUpdate, (move_doors, rotate_brushes).in_set(ThinkSet));
    let world = app.world_mut();
    {
        let mut s = world.resource_mut::<SpawnHandlers>();
        s.register("func_door", spawn_door);
        s.register("func_door_rotating", spawn_door);
        s.register("func_movelinear", spawn_movelinear);
        s.register("func_brush", spawn_brush);
        s.register("func_rotating", spawn_rotating);
    }
    let mut h = world.resource_mut::<InputHandlers>();
    h.register("func_door", door_input);
    h.register("func_door_rotating", door_input);
    h.register("func_movelinear", door_input);
    h.register("func_brush", brush_input);
    h.register("func_rotating", rotating_input);
    h.register("func_clip_vphysics", clip_input);
}

/// func_clip_vphysics: Enable / Disable its blocking volume.
fn clip_input(world: &mut World, call: &InputCall) -> bool {
    let enabled = match call.input.as_str() {
        "enable" => true,
        "disable" => false,
        _ => return false,
    };
    if let Some(mut c) = world.get_mut::<crate::collision::Collider>(call.target) {
        c.enabled = enabled;
    }
    true
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MoverState {
    Closed,
    Opening,
    Open,
    Closing,
}

/// Movement between a closed and an open placement. `progress` runs 0..1.
#[derive(Component, Clone, serde::Serialize, serde::Deserialize)]
pub struct Mover {
    #[serde(skip)]
    pub closed: Transform,
    #[serde(skip)]
    pub open: Transform,
    /// Distance (units or degrees) covered by a full move.
    pub travel: f32,
    pub speed: f32,
    pub progress: f32,
    pub target: f32,
    pub state: MoverState,
    /// Seconds before an opened door closes again (-1 = stays open).
    pub wait: f32,
    pub close_at: Option<f64>,
    pub locked: bool,
    /// func_movelinear reports "fully open/closed" but not open/close starts.
    pub linear: bool,
}

impl Mover {
    fn start(&mut self, target: f32) -> Option<&'static str> {
        if (self.target - target).abs() < 1e-6 && self.state != MoverState::Closed && self.state != MoverState::Open {
            return None;
        }
        self.target = target.clamp(0.0, 1.0);
        self.close_at = None;
        if (self.progress - self.target).abs() < 1e-6 {
            return None;
        }
        if self.target > self.progress {
            self.state = MoverState::Opening;
            Some("OnOpen")
        } else {
            self.state = MoverState::Closing;
            Some("OnClose")
        }
    }
}

fn model_size(world: &World, me: &MapEntity) -> Vec3 {
    let Some(map) = world.get_resource::<CurrentMap>() else { return Vec3::splat(64.0) };
    me.brush_model()
        .and_then(|m| map.bsp.models.get(m))
        .map(|m| (m.maxs - m.mins).abs())
        .unwrap_or(Vec3::splat(64.0))
}

fn spawn_door(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e).cloned() else { return };
    let closed = world.get::<Transform>(e).copied().unwrap_or_default();
    let speed = me.get_f32("speed").filter(|s| *s > 0.0).unwrap_or(100.0);
    let wait = me.get_f32("wait").unwrap_or(4.0);
    let rotating = me.classname == "func_door_rotating";
    let (open, travel) = if rotating {
        let distance = me.get_f32("distance").unwrap_or(90.0);
        // Spawnflags pick the hinge axis: 64 = roll (X), 128 = pitch (Y), default yaw (Z).
        let axis = if me.has_flag(64) {
            Vec3::X
        } else if me.has_flag(128) {
            Vec3::Y
        } else {
            Vec3::Z
        };
        let sign = if me.has_flag(2) { -1.0 } else { 1.0 };
        let rot = Quat::from_axis_angle(axis, (distance * sign).to_radians());
        (Transform { rotation: closed.rotation * rot, ..closed }, distance.abs())
    } else {
        let movedir = angles_forward(me.get_vec3("movedir").unwrap_or(Vec3::ZERO));
        let size = model_size(world, &me);
        let lip = me.get_f32("lip").unwrap_or(0.0);
        let distance = (movedir.abs().dot(size) - lip).max(0.0);
        (Transform { translation: closed.translation + movedir * distance, ..closed }, distance)
    };
    let start_open = me.get_i32("spawnpos") == Some(1) || me.has_flag(1);
    let progress = if start_open { 1.0 } else { 0.0 };
    let mover = Mover {
        closed,
        open,
        travel,
        speed,
        progress,
        target: progress,
        state: if start_open { MoverState::Open } else { MoverState::Closed },
        wait,
        close_at: None,
        locked: me.has_flag(2048),
        linear: false,
    };
    world.entity_mut(e).insert(mover);
}

fn spawn_movelinear(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e).cloned() else { return };
    let closed = world.get::<Transform>(e).copied().unwrap_or_default();
    let movedir = angles_forward(me.get_vec3("movedir").unwrap_or(Vec3::ZERO));
    let distance = me.get_f32("movedistance").unwrap_or(100.0);
    let start = me.get_f32("startposition").unwrap_or(0.0).clamp(0.0, 1.0);
    let mover = Mover {
        closed,
        open: Transform { translation: closed.translation + movedir * distance, ..closed },
        travel: distance.abs(),
        speed: me.get_f32("speed").filter(|s| *s > 0.0).unwrap_or(100.0),
        progress: start,
        target: start,
        state: if start >= 1.0 { MoverState::Open } else { MoverState::Closed },
        wait: -1.0,
        close_at: None,
        locked: false,
        linear: true,
    };
    world.entity_mut(e).insert(mover);
}

/// Puts movers that start open (or part-way) at their starting position.
/// Runs after parenting, so children placed in the map follow along.
pub fn apply_start_positions(world: &mut World, entities: impl IntoIterator<Item = Entity>) {
    for e in entities {
        if let Some(m) = world.get::<Mover>(e).cloned() {
            apply_progress(world, e, &m);
        }
    }
}

/// Re-expresses a mover's end positions when its parent changes: from the
/// old parent's space (`from`) into the new one (`to`), both given as world
/// placements (identity for no parent).
pub fn rebase(world: &mut World, e: Entity, from: bevy::math::Affine3A, to: bevy::math::Affine3A) {
    let Some(mut m) = world.get_mut::<Mover>(e) else { return };
    let change = to.inverse() * from;
    m.closed = Transform::from_matrix(Mat4::from(change * m.closed.compute_affine()));
    m.open = Transform::from_matrix(Mat4::from(change * m.open.compute_affine()));
}

fn apply_progress(world: &mut World, e: Entity, m: &Mover) {
    if let Some(mut t) = world.get_mut::<Transform>(e) {
        t.translation = m.closed.translation.lerp(m.open.translation, m.progress);
        t.rotation = m.closed.rotation.slerp(m.open.rotation, m.progress);
    }
}

fn door_input(world: &mut World, call: &InputCall) -> bool {
    let e = call.target;
    let Some(mut m) = world.get_mut::<Mover>(e) else { return false };
    let started = match call.input.as_str() {
        "open" if !m.locked => m.start(1.0),
        "close" if !m.locked => m.start(0.0),
        "toggle" if !m.locked => {
            let to = if matches!(m.state, MoverState::Open | MoverState::Opening) { 0.0 } else { 1.0 };
            m.start(to)
        }
        "setposition" => {
            let p = source_formats::kv::parse_f32_lenient(&call.param);
            m.start(p)
        }
        "lock" => {
            m.locked = true;
            None
        }
        "unlock" => {
            m.locked = false;
            None
        }
        "setspeed" => {
            m.speed = source_formats::kv::parse_f32_lenient(&call.param).max(0.0);
            None
        }
        "open" | "close" | "toggle" => None,
        _ => return false,
    };
    let linear = m.linear;
    if let Some(output) = started
        && !linear
    {
        fire_output(world, e, output, call.activator, None);
    }
    true
}

fn move_doors(
    clock: Res<GameClock>,
    mut movers: Query<(Entity, &mut Mover, &mut Transform)>,
    mut out: MessageWriter<FireOutput>,
) {
    let dt = clock.frametime();
    let now = clock.curtime;
    for (e, mut m, mut t) in &mut movers {
        if m.state == MoverState::Open
            && let Some(at) = m.close_at
            && clock.due(at)
        {
            m.close_at = None;
            if m.start(0.0).is_some() && !m.linear {
                out.write(FireOutput::new(e, "OnClose"));
            }
        }
        if !matches!(m.state, MoverState::Opening | MoverState::Closing) {
            continue;
        }
        let step = if m.travel > 0.0 { m.speed * dt / m.travel } else { 1.0 };
        if m.target > m.progress {
            m.progress = (m.progress + step).min(m.target);
        } else {
            m.progress = (m.progress - step).max(m.target);
        }
        t.translation = m.closed.translation.lerp(m.open.translation, m.progress);
        t.rotation = m.closed.rotation.slerp(m.open.rotation, m.progress);
        if (m.progress - m.target).abs() < 1e-6 {
            if m.target >= 1.0 {
                m.state = MoverState::Open;
                out.write(FireOutput::new(e, "OnFullyOpen"));
                if m.wait >= 0.0 && !m.linear {
                    m.close_at = Some(now + m.wait as f64);
                }
            } else if m.target <= 0.0 {
                m.state = MoverState::Closed;
                out.write(FireOutput::new(e, "OnFullyClosed"));
            } else {
                m.state = MoverState::Open;
            }
        }
    }
}

// ------------------------------------------------------------------ func_brush

#[derive(Component, serde::Serialize, serde::Deserialize)]
pub struct Toggleable {
    pub enabled: bool,
}

fn spawn_brush(world: &mut World, e: Entity) {
    let disabled = world.get::<MapEntity>(e).is_some_and(|m| m.get_bool("startdisabled"));
    world.entity_mut(e).insert(Toggleable { enabled: !disabled });
    if disabled && let Some(mut v) = world.get_mut::<Visibility>(e) {
        *v = Visibility::Hidden;
    }
}

fn brush_input(world: &mut World, call: &InputCall) -> bool {
    let e = call.target;
    let Some(current) = world.get::<Toggleable>(e).map(|t| t.enabled) else { return false };
    let enabled = match call.input.as_str() {
        "enable" => true,
        "disable" => false,
        "toggle" => !current,
        _ => return false,
    };
    if let Some(mut t) = world.get_mut::<Toggleable>(e) {
        t.enabled = enabled;
    }
    if let Some(mut v) = world.get_mut::<Visibility>(e) {
        *v = if enabled { Visibility::Inherited } else { Visibility::Hidden };
    }
    // Collision follows unless the brush is always solid.
    let always_solid = world.get::<MapEntity>(e).is_some_and(|m| m.get_i32("solidity") == Some(2));
    if !always_solid && let Some(mut c) = world.get_mut::<crate::collision::Collider>(e) {
        c.enabled = enabled;
    }
    true
}

// --------------------------------------------------------------- func_rotating

#[derive(Component, serde::Serialize, serde::Deserialize)]
pub struct Rotating {
    pub axis: Vec3,
    pub max_speed: f32,
    pub speed: f32,
    pub on: bool,
    pub reversed: bool,
}

fn spawn_rotating(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e) else { return };
    let axis = if me.has_flag(4) {
        Vec3::X
    } else if me.has_flag(8) {
        Vec3::Y
    } else {
        Vec3::Z
    };
    let r = Rotating {
        axis,
        max_speed: me.get_f32("maxspeed").unwrap_or(100.0),
        speed: 0.0,
        on: me.has_flag(1),
        reversed: me.has_flag(2),
    };
    world.entity_mut(e).insert(r);
}

fn rotating_input(world: &mut World, call: &InputCall) -> bool {
    let Some(mut r) = world.get_mut::<Rotating>(call.target) else { return false };
    match call.input.as_str() {
        "start" => r.on = true,
        "stop" => r.on = false,
        "toggle" => r.on = !r.on,
        "reverse" => r.reversed = !r.reversed,
        "startforward" => {
            r.on = true;
            r.reversed = false;
        }
        "startbackward" => {
            r.on = true;
            r.reversed = true;
        }
        "setspeed" => r.max_speed = r.max_speed.max(1.0) * source_formats::kv::parse_f32_lenient(&call.param).clamp(0.0, 1.0),
        _ => return false,
    }
    true
}

fn rotate_brushes(clock: Res<GameClock>, mut q: Query<(&mut Rotating, &mut Transform)>) {
    let dt = clock.frametime();
    for (mut r, mut t) in &mut q {
        // Spin up and down over about a second rather than snapping.
        let target = if r.on { r.max_speed } else { 0.0 };
        let accel = r.max_speed.max(1.0);
        r.speed += (target - r.speed).clamp(-accel * dt, accel * dt);
        if r.speed.abs() > 1e-3 {
            let sign = if r.reversed { -1.0 } else { 1.0 };
            let axis = r.axis;
            t.rotate_local(Quat::from_axis_angle(axis, (r.speed * sign * dt).to_radians()));
        }
    }
}

impl crate::save::Restore for Mover {
    fn keep(&mut self, live: Self) {
        self.closed = live.closed;
        self.open = live.open;
    }
}

impl crate::save::Restore for Rotating {}

impl crate::save::Restore for Toggleable {}
