//! `point_push`: pushes the player (spawnflag 8) and physics objects
//! (spawnflag 16) within `radius` away from itself, or along its facing
//! with spawnflag 2, by `magnitude`, fading out towards the edge unless
//! spawnflag 4 says not to (`CPointPush`: full strength inside a quarter of
//! the radius). The player is carried by a base velocity of that size;
//! physics objects take an impulse as if they weighed 100 kg each tick.
//! `sp_a2_core` keeps the player out of GLaDOS's maintenance pit and off her
//! head this way, and sweeps them into the exit elevator.

use avian3d::prelude::LinearVelocity;
use bevy::prelude::*;

use super::io::{InputCall, InputHandlers};
use super::{MapEntity, SpawnHandlers};
use crate::entities::props::PhysicsProp;
use crate::player::Player;
use portal2_core::sets::PlayerMoveSet;

const SF_DIRECTIONAL: u32 = 2;
const SF_NO_FALLOFF: u32 = 4;
const SF_PLAYER: u32 = 8;
const SF_PHYSICS: u32 = 16;

pub fn register(app: &mut App) {
    app.add_systems(FixedUpdate, push.before(PlayerMoveSet));
    let world = app.world_mut();
    world.resource_mut::<SpawnHandlers>().register("point_push", spawn_push);
    world.resource_mut::<InputHandlers>().register("point_push", push_input);
}

#[derive(Component, Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub struct PointPush {
    pub enabled: bool,
    radius: f32,
    magnitude: f32,
    flags: u32,
}

impl portal2_core::save::Restore for PointPush {}

pub fn register_save(r: &mut portal2_core::save::SaveRegistry) {
    r.component::<PointPush>("point_push");
}

fn spawn_push(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e) else { return };
    let push = PointPush {
        enabled: !me.get_bool("startdisabled") && me.get_i32("enabled").unwrap_or(1) != 0,
        radius: me.get_f32("radius").unwrap_or(128.0).max(1.0),
        magnitude: me.get_f32("magnitude").unwrap_or(100.0),
        flags: me.spawnflags(),
    };
    world.entity_mut(e).insert(push);
}

fn push_input(world: &mut World, call: &InputCall) -> bool {
    let Some(mut p) = world.get_mut::<PointPush>(call.target) else { return false };
    match call.input.as_str() {
        "enable" => p.enabled = true,
        "disable" => p.enabled = false,
        _ => return false,
    }
    true
}

/// The push on something at `at` from a push at `origin` facing `forward`.
fn push_at(p: &PointPush, origin: Vec3, forward: Vec3, at: Vec3) -> Option<Vec3> {
    let offset = at - origin;
    let dist = offset.length();
    if dist > p.radius {
        return None;
    }
    let dir = if p.flags & SF_DIRECTIONAL != 0 { forward } else { offset.normalize_or(Vec3::Z) };
    let falloff = if p.flags & SF_NO_FALLOFF != 0 { 1.0 } else { ((p.radius - dist) / (p.radius * 0.75)).clamp(0.0, 1.0) };
    Some(dir * p.magnitude * falloff)
}

fn push(
    time: Res<Time<Fixed>>,
    pushes: Query<(&PointPush, &GlobalTransform)>,
    mut players: Query<&mut Player>,
    mut props: Query<(&GlobalTransform, &mut LinearVelocity), With<PhysicsProp>>,
) {
    let dt = time.delta_secs();
    for (p, gt) in &pushes {
        if !p.enabled {
            continue;
        }
        let (origin, forward) = (gt.translation(), gt.rotation() * Vec3::X);
        if p.flags & SF_PLAYER != 0 {
            for mut player in &mut players {
                if player.noclip {
                    continue;
                }
                let (mins, maxs) = player.state.hull();
                let center = player.state.origin + (mins + maxs) * 0.5;
                let Some(v) = push_at(p, origin, forward, center) else { continue };
                if v.z > 0.0 && player.state.on_ground {
                    player.state.on_ground = false;
                    player.state.ground_entity = None;
                    player.state.origin.z += 1.0;
                }
                player.state.base_velocity = v;
                player.state.base_velocity_set = true;
            }
        }
        if p.flags & SF_PHYSICS != 0 {
            for (pt, mut velocity) in &mut props {
                if let Some(v) = push_at(p, origin, forward, pt.translation()) {
                    velocity.0 += v * 100.0 * dt;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_fades_to_the_edge_unless_constant() {
        let p = PointPush { enabled: true, radius: 100.0, magnitude: 150.0, flags: SF_PLAYER };
        let full = push_at(&p, Vec3::ZERO, Vec3::X, Vec3::new(20.0, 0.0, 0.0)).unwrap();
        assert!((full - Vec3::new(150.0, 0.0, 0.0)).length() < 1e-3);
        let half = push_at(&p, Vec3::ZERO, Vec3::X, Vec3::new(0.0, 62.5, 0.0)).unwrap();
        assert!((half.y - 75.0).abs() < 1e-3);
        assert!(push_at(&p, Vec3::ZERO, Vec3::X, Vec3::new(0.0, 0.0, 101.0)).is_none());
        let constant = PointPush { flags: SF_PLAYER | SF_NO_FALLOFF | SF_DIRECTIONAL, ..p };
        assert_eq!(push_at(&constant, Vec3::ZERO, Vec3::Y, Vec3::new(90.0, 0.0, 0.0)), Some(Vec3::new(0.0, 150.0, 0.0)));
    }
}
