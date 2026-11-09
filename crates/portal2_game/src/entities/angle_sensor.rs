//! `point_anglesensor`: watches whether an entity (`target`) faces another
//! (`lookatname`), its forward axis within `tolerance` degrees of the line
//! between them, and fires `OnFacingLookat` once it has for `duration`
//! seconds (`OnNotFacingLookat` when it turns away again). `Test` reports
//! the current state. `sp_a2_core`'s fake vault door falls flat when its
//! button is pressed; the sensor on it starts GLaDOS's pit trap.

use bevy::prelude::*;

use super::io::{FireOutput, InputCall, InputHandlers};
use super::{EntityRegistry, MapEntity, SpawnHandlers};
use portal2_core::clock::GameClock;

pub fn register(app: &mut App) {
    app.add_systems(FixedUpdate, watch_sensors);
    let world = app.world_mut();
    world.resource_mut::<SpawnHandlers>().register("point_anglesensor", spawn_sensor);
    world.resource_mut::<InputHandlers>().register("point_anglesensor", sensor_input);
}

#[derive(Component, Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AngleSensor {
    pub enabled: bool,
    target: String,
    lookat: String,
    /// Cosine of the tolerance angle.
    min_dot: f32,
    duration: f64,
    /// Since when the target has faced the look-at entity, while it does.
    facing_since: Option<f64>,
    /// Whether `OnFacingLookat` has fired for the current spell of facing.
    fired: bool,
}

impl portal2_core::save::Restore for AngleSensor {}

pub fn register_save(r: &mut portal2_core::save::SaveRegistry) {
    r.component::<AngleSensor>("angle_sensor");
}

fn spawn_sensor(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e) else { return };
    let name = |k: &str| me.get(k).map(|s| s.trim().to_ascii_lowercase()).unwrap_or_default();
    let sensor = AngleSensor {
        enabled: !me.get_bool("startdisabled"),
        target: name("target"),
        lookat: name("lookatname"),
        min_dot: me.get_f32("tolerance").unwrap_or(15.0).to_radians().cos(),
        duration: f64::from(me.get_f32("duration").unwrap_or(0.0).max(0.0)),
        facing_since: None,
        fired: false,
    };
    world.entity_mut(e).insert(sensor);
}

fn sensor_input(world: &mut World, call: &InputCall) -> bool {
    let e = call.target;
    let facing = facing_now(world, e);
    let Some(mut s) = world.get_mut::<AngleSensor>(e) else { return false };
    match call.input.as_str() {
        "enable" => s.enabled = true,
        "disable" => {
            s.enabled = false;
            s.facing_since = None;
            s.fired = false;
        }
        "toggle" => s.enabled = !s.enabled,
        "settargetentity" => s.target = call.param.trim().to_ascii_lowercase(),
        "test" => {
            let output = if facing { "OnFacingLookat" } else { "OnNotFacingLookat" };
            world.write_message(FireOutput::new(e, output).with_activator(call.activator));
        }
        _ => return false,
    }
    true
}

/// Whether a sensor's target faces its look-at entity right now.
fn facing_now(world: &World, e: Entity) -> bool {
    let Some(s) = world.get::<AngleSensor>(e) else { return false };
    let registry = world.resource::<EntityRegistry>();
    let (Some(t), Some(l)) = (registry.first_named(&s.target), registry.first_named(&s.lookat)) else { return false };
    let (tt, lt) = (super::world_transform(world, t), super::world_transform(world, l));
    faces(&tt, lt.translation(), s.min_dot)
}

fn faces(target: &GlobalTransform, at: Vec3, min_dot: f32) -> bool {
    let forward = target.rotation() * Vec3::X;
    let to = (at - target.translation()).normalize_or_zero();
    to != Vec3::ZERO && forward.dot(to) >= min_dot
}

fn watch_sensors(
    clock: Res<GameClock>,
    registry: Res<EntityRegistry>,
    places: Query<&GlobalTransform>,
    mut sensors: Query<(Entity, &mut AngleSensor)>,
    mut out: MessageWriter<FireOutput>,
) {
    let now = clock.curtime;
    for (e, mut s) in &mut sensors {
        if !s.enabled {
            continue;
        }
        let target = registry.first_named(&s.target).and_then(|t| places.get(t).ok());
        let at = registry.first_named(&s.lookat).and_then(|l| places.get(l).ok()).map(GlobalTransform::translation);
        let (Some(target), Some(at)) = (target, at) else { continue };
        if faces(target, at, s.min_dot) {
            let since = *s.facing_since.get_or_insert(now);
            if !s.fired && now - since >= s.duration {
                s.fired = true;
                out.write(FireOutput::new(e, "OnFacingLookat"));
            }
        } else if s.facing_since.take().is_some() && s.fired {
            s.fired = false;
            out.write(FireOutput::new(e, "OnNotFacingLookat"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A door's up-pointing marker faces a point beside the door once the
    /// door has fallen towards it.
    #[test]
    fn faces_once_turned_towards_the_point() {
        let up = GlobalTransform::from(Transform::from_rotation(Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2)));
        let at = Vec3::new(-140.0, 0.0, -50.0);
        let min_dot = 30f32.to_radians().cos();
        assert!(!faces(&up, at, min_dot));
        let fallen = GlobalTransform::from(Transform::from_rotation(Quat::from_rotation_y(std::f32::consts::PI)));
        assert!(faces(&fallen, at, min_dot));
    }
}
