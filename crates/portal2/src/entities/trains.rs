//! Track trains: brush entities that travel along chains of `path_track`
//! nodes (elevators, the relaxation vault, moving platforms).
//!
//! A train snaps to its first node when the map starts and moves toward the
//! next node at its current speed each tick. Reaching a node fires the
//! node's `OnPass`, may change the train's speed, stop it (node flag
//! "disable train") or branch to the node's alternate path. Moving backward
//! follows the nodes that point at the current one.

use bevy::prelude::*;

use super::io::{FireOutput, InputCall, InputHandlers, fire_output};
use super::{EntityRegistry, MapEntity, SpawnHandlers, world_transform};
use crate::clock::{GameClock, ThinkSet};

pub fn register(app: &mut App) {
    app.add_systems(FixedUpdate, (activate_trains, move_trains).chain().in_set(ThinkSet));
    let world = app.world_mut();
    {
        let mut s = world.resource_mut::<SpawnHandlers>();
        s.register("func_tracktrain", spawn_train);
        s.register("path_track", spawn_path);
    }
    let mut h = world.resource_mut::<InputHandlers>();
    h.register("func_tracktrain", train_input);
    h.register("path_track", path_input);
}

/// path_track spawnflags.
mod path_flags {
    pub const DISABLED: u32 = 1;
    pub const FIRE_ONCE: u32 = 2;
    pub const BRANCH_REVERSE: u32 = 4;
    pub const DISABLE_TRAIN: u32 = 8;
    pub const TELEPORT: u32 = 16;
}

/// func_tracktrain spawnflags.
mod train_flags {
    pub const NO_PITCH: u32 = 1;
    pub const FIXED_ORIENTATION: u32 = 16;
}

#[derive(Component, Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PathTrack {
    pub next: Option<String>,
    pub alt: Option<String>,
    pub use_alt: bool,
    pub enabled: bool,
    pub speed: f32,
    pub fired: bool,
}

fn spawn_path(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e) else { return };
    let name = |k: &str| me.get(k).filter(|s| !s.is_empty()).map(str::to_ascii_lowercase);
    let p = PathTrack {
        next: name("target"),
        alt: name("altpath"),
        use_alt: false,
        enabled: !me.has_flag(path_flags::DISABLED),
        speed: me.get_f32("speed").unwrap_or(0.0),
        fired: false,
    };
    world.entity_mut(e).insert(p);
}

fn path_input(world: &mut World, call: &InputCall) -> bool {
    let Some(mut p) = world.get_mut::<PathTrack>(call.target) else { return false };
    match call.input.as_str() {
        "enablepath" => p.enabled = true,
        "disablepath" => p.enabled = false,
        "togglepath" => p.enabled = !p.enabled,
        "enablealternatepath" => p.use_alt = true,
        "disablealternatepath" => p.use_alt = false,
        "togglealternatepath" => p.use_alt = !p.use_alt,
        _ => return false,
    }
    true
}

#[derive(Component, Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Train {
    /// Node the train is heading for.
    #[serde(with = "crate::save::ids::opt_entity")]
    pub toward: Option<Entity>,
    /// Node it last passed or started at.
    #[serde(with = "crate::save::ids::opt_entity")]
    pub last: Option<Entity>,
    pub speed: f32,
    pub max_speed: f32,
    pub moving: bool,
    pub forward: bool,
    pub height: f32,
    /// Stop when this node is reached (`MoveToPathNode`).
    #[serde(with = "crate::save::ids::opt_entity")]
    pub destination: Option<Entity>,
    pub fixed_orientation: bool,
    pub no_pitch: bool,
    pub first: Option<String>,
    pub activated: bool,
}

/// Note the keyvalue names: `speed` is the speed the train starts with
/// (0 = stopped) and `startspeed` its maximum speed.
fn spawn_train(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e).cloned() else { return };
    let initial = me.get_f32("speed").unwrap_or(0.0);
    let t = Train {
        toward: None,
        last: None,
        speed: initial,
        max_speed: me.get_f32("startspeed").unwrap_or(100.0).abs(),
        moving: initial != 0.0,
        forward: initial >= 0.0,
        height: me.get_f32("height").unwrap_or(0.0),
        destination: None,
        fixed_orientation: me.has_flag(train_flags::FIXED_ORIENTATION) || me.get_i32("orientationtype") == Some(0),
        no_pitch: me.has_flag(train_flags::NO_PITCH),
        first: me.get("target").filter(|s| !s.is_empty()).map(str::to_ascii_lowercase),
        activated: false,
    };
    world.entity_mut(e).insert(t);
}

fn named(world: &World, name: &str) -> Option<Entity> {
    world.resource::<EntityRegistry>().first_named(name)
}

/// Node after `node` in the direction of travel.
fn next_node(world: &mut World, node: Entity, forward: bool) -> Option<Entity> {
    let p = world.get::<PathTrack>(node)?.clone();
    if forward {
        let name = if p.use_alt { p.alt.as_ref().or(p.next.as_ref()) } else { p.next.as_ref() };
        return name.and_then(|n| named(world, n)).filter(|n| world.get::<PathTrack>(*n).is_some_and(|p| p.enabled));
    }
    // Backward: a node that leads here.
    let me = world.get::<MapEntity>(node).and_then(|m| m.targetname.clone())?;
    let mut q = world.query::<(Entity, &PathTrack)>();
    q.iter(world)
        .find(|(_, p)| p.enabled && (p.next.as_deref() == Some(me.as_str()) || (p.use_alt && p.alt.as_deref() == Some(me.as_str()))))
        .map(|(e, _)| e)
}

fn node_position(world: &World, node: Entity, height: f32) -> Vec3 {
    world_transform(world, node).translation() + Vec3::Z * height
}

/// Moves the train's origin to a world position, keeping its parent space.
fn place(world: &mut World, e: Entity, pos: Vec3, facing: Option<Vec3>, no_pitch: bool) {
    let parent = world.get::<ChildOf>(e).map(|c| c.parent());
    let parent_affine = parent.map(|p| world_transform(world, p).affine()).unwrap_or(bevy::math::Affine3A::IDENTITY);
    let local = parent_affine.inverse().transform_point3(pos);
    let rotation = facing.filter(|d| d.length_squared() > 1e-6).map(|d| {
        let d = if no_pitch { Vec3::new(d.x, d.y, 0.0) } else { d };
        let yaw = d.y.atan2(d.x);
        let pitch = -(d.z / d.length().max(1e-6)).clamp(-1.0, 1.0).asin();
        let world_rot = Quat::from_rotation_z(yaw) * Quat::from_rotation_y(-pitch);
        let (_, parent_rot, _) = parent_affine.to_scale_rotation_translation();
        parent_rot.inverse() * world_rot
    });
    if let Some(mut t) = world.get_mut::<Transform>(e) {
        t.translation = local;
        if let Some(r) = rotation {
            t.rotation = r;
        }
    }
}

/// Snaps trains to their first node once the map's entities exist.
fn activate_trains(world: &mut World) {
    let mut q = world.query::<(Entity, &Train)>();
    let pending: Vec<(Entity, Option<String>, f32)> =
        q.iter(world).filter(|(_, t)| !t.activated).map(|(e, t)| (e, t.first.clone(), t.height)).collect();
    for (e, first, height) in pending {
        let start = first.as_deref().and_then(|n| named(world, n));
        if let Some(s) = start {
            let pos = node_position(world, s, height);
            place(world, e, pos, None, false);
        }
        let forward = world.get::<Train>(e).is_some_and(|t| t.forward);
        let toward = start.and_then(|s| next_node(world, s, forward));
        if let Some(mut t) = world.get_mut::<Train>(e) {
            t.activated = true;
            t.last = start;
            t.toward = toward;
        }
    }
}

fn move_trains(world: &mut World) {
    let dt = world.resource::<GameClock>().frametime();
    let mut q = world.query::<(Entity, &Train)>();
    let moving: Vec<Entity> = q.iter(world).filter(|(_, t)| t.moving && t.speed.abs() > 0.0).map(|(e, _)| e).collect();
    for e in moving {
        let Some(t) = world.get::<Train>(e).cloned() else { continue };
        let Some(target) = t.toward else {
            if let Some(mut t) = world.get_mut::<Train>(e) {
                t.moving = false;
            }
            continue;
        };
        let here = world_transform(world, e).translation();
        let goal = node_position(world, target, t.height);
        let to_goal = goal - here;
        let step = t.speed.abs() * dt;
        let dist = to_goal.length();
        let facing = (!t.fixed_orientation).then(|| if t.forward { to_goal } else { -to_goal });
        if dist > step {
            place(world, e, here + to_goal / dist * step, facing, t.no_pitch);
            continue;
        }
        place(world, e, goal, facing, t.no_pitch);
        arrive(world, e, target);
    }
}

/// Handles reaching a node: outputs, speed changes, branching and stops.
fn arrive(world: &mut World, e: Entity, node: Entity) {
    let flags = world.get::<MapEntity>(node).map(|m| m.spawnflags()).unwrap_or(0);
    let Some(p) = world.get::<PathTrack>(node).cloned() else { return };
    if !(flags & path_flags::FIRE_ONCE != 0 && p.fired) {
        fire_output(world, node, "OnPass", Some(e), None);
        if let Some(mut pt) = world.get_mut::<PathTrack>(node) {
            pt.fired = true;
        }
    }
    let Some(mut t) = world.get::<Train>(e).cloned() else { return };
    t.last = Some(node);
    if p.speed > 0.0 {
        t.speed = p.speed.copysign(t.speed);
    }
    let reached_destination = t.destination == Some(node);
    if flags & path_flags::DISABLE_TRAIN != 0 || reached_destination {
        t.moving = false;
        t.destination = None;
    }
    let forward = t.forward;
    t.toward = next_node(world, node, forward);
    if t.toward.is_none() {
        t.moving = false;
    }
    if let Some(next) = t.toward
        && world.get::<MapEntity>(next).is_some_and(|m| m.has_flag(path_flags::TELEPORT))
    {
        let pos = node_position(world, next, t.height);
        place(world, e, pos, None, false);
    }
    let stopped = !t.moving;
    if let Some(mut tt) = world.get_mut::<Train>(e) {
        *tt = t;
    }
    if reached_destination {
        world.write_message(FireOutput::new(e, "OnArrivedAtDestinationNode"));
    }
    let _ = stopped;
    let _ = path_flags::BRANCH_REVERSE;
}

fn train_input(world: &mut World, call: &InputCall) -> bool {
    let e = call.target;
    let Some(mut t) = world.get::<Train>(e).cloned() else { return false };
    let p = source_formats::kv::parse_f32_lenient(&call.param);
    let mut started = false;
    match call.input.as_str() {
        "startforward" | "startbackward" => {
            let forward = call.input == "startforward";
            if forward != t.forward {
                t.forward = forward;
                t.toward = t.last.and_then(|l| next_node(world, l, forward));
            }
            t.speed = t.max_speed;
            t.moving = true;
            started = true;
        }
        "stop" => t.moving = false,
        "resume" => {
            t.moving = true;
            started = true;
        }
        "toggle" => {
            t.moving = !t.moving;
            started = t.moving;
        }
        "reverse" => {
            t.forward = !t.forward;
            t.toward = t.last.and_then(|l| next_node(world, l, t.forward));
        }
        "setspeed" => {
            t.speed = t.max_speed * p.clamp(0.0, 1.0);
            t.moving = t.speed > 0.0;
            started = t.moving;
        }
        "setspeedreal" => {
            t.speed = p.abs();
            t.moving = t.speed > 0.0;
            started = t.moving;
        }
        "setspeeddir" | "setspeeddiraccel" => {
            let forward = p >= 0.0;
            if forward != t.forward {
                t.forward = forward;
                t.toward = t.last.and_then(|l| next_node(world, l, forward));
            }
            t.speed = t.max_speed * p.abs().clamp(0.0, 1.0);
            t.moving = t.speed > 0.0;
            started = t.moving;
        }
        "setmaxspeed" => t.max_speed = p.abs(),
        "teleporttopathtrack" | "teleporttopathnode" => {
            let Some(node) = named(world, &call.param.trim().to_ascii_lowercase()) else { return true };
            let pos = node_position(world, node, t.height);
            place(world, e, pos, None, false);
            t.last = Some(node);
            t.toward = next_node(world, node, t.forward);
        }
        "movetopathnode" => {
            let Some(node) = named(world, &call.param.trim().to_ascii_lowercase()) else { return true };
            t.destination = Some(node);
            // Head along the path toward it; pick the direction that reaches it.
            let reachable = |world: &mut World, forward: bool| -> bool {
                let mut cur = t.last;
                for _ in 0..256 {
                    let Some(c) = cur else { return false };
                    if c == node {
                        return true;
                    }
                    cur = next_node(world, c, forward);
                }
                false
            };
            let forward = if reachable(world, true) { true } else { !reachable(world, false) };
            if forward != t.forward || t.toward.is_none() {
                t.forward = forward;
                t.toward = t.last.and_then(|l| next_node(world, l, forward));
            }
            if t.last == Some(node) {
                t.destination = None;
                world.write_message(FireOutput::new(e, "OnArrivedAtDestinationNode"));
            } else {
                if t.speed == 0.0 {
                    t.speed = t.max_speed;
                }
                t.moving = true;
                started = true;
            }
        }
        "lockorientation" => t.fixed_orientation = true,
        "unlockorientation" => t.fixed_orientation = false,
        _ => return false,
    }
    if let Some(mut tt) = world.get_mut::<Train>(e) {
        *tt = t;
    }
    if started {
        fire_output(world, e, "OnStart", call.activator, None);
    }
    true
}

impl crate::save::Restore for Train {}

impl crate::save::Restore for PathTrack {}
