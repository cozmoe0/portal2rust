//! `func_tank`: brush turrets. The campaign only uses them as aiming rigs
//! (sphere-bot swivels, GLaDOS's pointer) that never fire: each turns its
//! local yaw and pitch toward a target entity, within its ranges around the
//! angles it spawned with, at most at its rates. As the Source tank does,
//! the turn speed is ten times the remaining angle (so it eases in),
//! capped at the rate. Targets are aimed at their eyes (the player) or
//! their centre, from the barrel point. A tank an input moved onto a model
//! attachment no longer turns (the original stops its movement there): the
//! first map's Wheatley rig leaves the aiming to the swivel below.

use bevy::prelude::*;

use crate::clock::{GameClock, ThinkSet};
use crate::coords::{angles_to_quat, quat_to_angles};
use crate::entities::io::{InputCall, InputHandlers, resolve_targets};
use crate::entities::{EntityRegistry, MapEntity, RidesAttachment, SpawnHandlers};
use crate::player::Player;

const ACTIVE: u32 = 1;

pub struct FuncTankPlugin;

impl Plugin for FuncTankPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(FixedUpdate, aim_tanks.in_set(ThinkSet));
        let world = app.world_mut();
        world.resource_mut::<SpawnHandlers>().register("func_tank", spawn_tank);
        world.resource_mut::<InputHandlers>().register("func_tank", tank_input);
    }
}

#[derive(Component, serde::Serialize, serde::Deserialize)]
pub struct FuncTank {
    active: bool,
    #[serde(with = "crate::save::ids::opt_entity")]
    target: Option<Entity>,
    /// A fixed point aimed at instead of an entity (`SetTargetPosition`).
    target_point: Option<Vec3>,
    yaw_rate: f32,
    yaw_range: f32,
    pitch_rate: f32,
    pitch_range: f32,
    /// Barrel offset (forward, left, up) the aim is taken from.
    barrel: Vec3,
    /// Local (pitch, yaw, roll) at spawn, the centre of the ranges.
    center: Option<Vec3>,
    /// Current local (pitch, yaw, roll).
    angles: Vec3,
}

impl crate::save::Restore for FuncTank {}

pub fn register_save(r: &mut crate::save::SaveRegistry) {
    r.component::<FuncTank>("func_tank");
}

fn spawn_tank(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e) else { return };
    let tank = FuncTank {
        active: me.spawnflags() & ACTIVE != 0,
        target: None,
        target_point: None,
        yaw_rate: me.get_f32("yawrate").unwrap_or(30.0),
        yaw_range: me.get_f32("yawrange").unwrap_or(180.0),
        pitch_rate: me.get_f32("pitchrate").unwrap_or(0.0),
        pitch_range: me.get_f32("pitchrange").unwrap_or(0.0),
        barrel: Vec3::new(me.get_f32("barrel").unwrap_or(0.0), me.get_f32("barrely").unwrap_or(0.0), me.get_f32("barrelz").unwrap_or(0.0)),
        center: None,
        angles: Vec3::ZERO,
    };
    world.entity_mut(e).insert(tank);
}

fn tank_input(world: &mut World, call: &InputCall) -> bool {
    let target = match call.input.as_str() {
        "settargetentity" | "settargetentityname" => {
            Some(resolve_targets(world, &call.param, call.activator, call.caller).into_iter().next())
        }
        _ => None,
    };
    let Some(mut tank) = world.get_mut::<FuncTank>(call.target) else { return false };
    match call.input.as_str() {
        "activate" => tank.active = true,
        "deactivate" => tank.active = false,
        "settargetentity" | "settargetentityname" => {
            tank.target = target.flatten();
            tank.target_point = None;
        }
        "cleartargetentity" => tank.target = None,
        "settargetposition" => {
            tank.target_point = source_formats::kv::parse_vec3(&call.param);
            tank.target = None;
        }
        // Firing, ammo and control inputs: these rigs never fire.
        "fireonce" | "startfire" | "stopfire" | "setfirerate" | "setdamage" | "setammo" | "setaimstate" => {}
        _ => return false,
    }
    true
}

/// Shortest signed difference `to - from` in degrees.
fn angle_distance(to: f32, from: f32) -> f32 {
    (to - from + 540.0).rem_euclid(360.0) - 180.0
}

#[allow(clippy::type_complexity)]
fn aim_tanks(
    clock: Res<GameClock>,
    registry: Res<EntityRegistry>,
    mut tanks: Query<(&mut FuncTank, &mut Transform, &GlobalTransform, Option<&ChildOf>), Without<RidesAttachment>>,
    targets: Query<&GlobalTransform, Without<FuncTank>>,
    players: Query<&Player>,
    parents: Query<&GlobalTransform>,
) {
    let dt = clock.frametime();
    for (mut tank, mut t, gt, parent) in &mut tanks {
        let center = match tank.center {
            Some(c) => c,
            None => {
                let c = quat_to_angles(t.rotation);
                tank.center = Some(c);
                tank.angles = c;
                c
            }
        };
        if !tank.active {
            continue;
        }
        let aim_at = match (tank.target, tank.target_point) {
            (Some(target), _) if registry.player == Some(target) => players
                .get(target)
                .ok()
                .map(|p| p.state.origin + Vec3::Z * p.state.eye_offset())
                .or_else(|| targets.get(target).ok().map(|g| g.translation())),
            (Some(target), _) => targets.get(target).ok().map(|g| g.translation()),
            (None, Some(p)) => Some(p),
            (None, None) => None,
        };
        let Some(aim_at) = aim_at else { continue };
        let barrel = gt.transform_point(tank.barrel);
        let dir = (aim_at - barrel).normalize_or_zero();
        if dir == Vec3::ZERO {
            continue;
        }
        // The wanted angles in the parent's space.
        let parent_rot = parent.and_then(|p| parents.get(p.parent()).ok()).map(|g| g.rotation()).unwrap_or(Quat::IDENTITY);
        let d = parent_rot.inverse() * dir;
        let mut want_yaw = d.y.atan2(d.x).to_degrees();
        let mut want_pitch = (-d.z).atan2(d.truncate().length()).to_degrees();
        if tank.yaw_range < 180.0 {
            want_yaw = center.y + angle_distance(want_yaw, center.y).clamp(-tank.yaw_range, tank.yaw_range);
        }
        if tank.pitch_range < 180.0 {
            want_pitch = center.x + angle_distance(want_pitch, center.x).clamp(-tank.pitch_range, tank.pitch_range);
        }
        let step = |current: f32, wanted: f32, rate: f32| {
            let speed = (angle_distance(wanted, current) * 10.0).clamp(-rate, rate);
            current + speed * dt
        };
        let yaw = step(tank.angles.y, want_yaw, tank.yaw_rate);
        let pitch = step(tank.angles.x, want_pitch, tank.pitch_rate);
        tank.angles = Vec3::new(pitch, yaw, center.z);
        t.rotation = angles_to_quat(tank.angles);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn angle_distance_wraps() {
        assert_eq!(angle_distance(10.0, 350.0), 20.0);
        assert_eq!(angle_distance(350.0, 10.0), -20.0);
        assert_eq!(angle_distance(180.0, 0.0), -180.0);
    }
}
