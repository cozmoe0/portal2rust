//! Faith plates (`trigger_catapult`): launch whatever steps onto them toward a
//! target entity, or along a fixed direction.

use avian3d::prelude::{LinearVelocity, RigidBody};
use bevy::prelude::*;

use crate::collision::{Collider as TraceCollider, Physics};
use crate::coords::angles_forward;
use crate::entities::io::{FireOutput, InputCall, InputHandlers};
use crate::entities::props::PhysicsProp;
use crate::entities::{EntityRegistry, MapEntity, SpawnHandlers};
use crate::player::movement::GRAVITY;
use crate::player::{Player, PlayerMoveSet};

pub struct CatapultPlugin;

impl Plugin for CatapultPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AirControl>().add_systems(FixedUpdate, run_catapults.after(PlayerMoveSet));
        let world = app.world_mut();
        world.resource_mut::<SpawnHandlers>().register("trigger_catapult", spawn_catapult);
        world.resource_mut::<InputHandlers>().register("trigger_catapult", catapult_input);
    }
}

/// While suppressed, airborne player input is ignored so launch arcs stay
/// true. Landing always restores control.
#[derive(Resource, Default)]
pub struct AirControl {
    pub suppressed_until: f64,
}

#[derive(Component)]
pub struct Catapult {
    pub enabled: bool,
    pub player_speed: f32,
    pub physics_speed: f32,
    pub target: Option<String>,
    pub direction: Vec3,
    /// Launch at exactly the set speed instead of adding lift to reach the
    /// target.
    pub use_exact: bool,
    pub air_suppress: f32,
    pub launch_players: bool,
    pub launch_props: bool,
    touching: Vec<Entity>,
}

/// Trigger spawnflags: clients, physics objects, everything.
const SF_CLIENTS: u32 = 1;
const SF_PHYSICS: u32 = 8;
const SF_EVERYTHING: u32 = 64;

fn spawn_catapult(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e).cloned() else { return };
    world.entity_mut(e).insert(Catapult {
        enabled: !me.get_bool("startdisabled"),
        player_speed: me.get_f32("playerspeed").unwrap_or(450.0),
        physics_speed: me.get_f32("physicsspeed").unwrap_or(450.0),
        target: me.get("launchtarget").filter(|s| !s.is_empty()).map(|s| s.to_ascii_lowercase()),
        direction: me.get_vec3("launchdirection").unwrap_or(Vec3::ZERO),
        use_exact: me.get_bool("useexactvelocity"),
        air_suppress: me.get_f32("airctrlsupressiontime").unwrap_or(-1.0),
        launch_players: me.spawnflags() & (SF_CLIENTS | SF_EVERYTHING) != 0,
        launch_props: me.spawnflags() & (SF_PHYSICS | SF_EVERYTHING) != 0,
        touching: Vec::new(),
    });
}

fn catapult_input(world: &mut World, call: &InputCall) -> bool {
    let Some(mut c) = world.get_mut::<Catapult>(call.target) else { return false };
    let v = source_formats::kv::parse_f32_lenient(&call.param);
    match call.input.as_str() {
        "enable" => c.enabled = true,
        "disable" => c.enabled = false,
        "toggle" => c.enabled = !c.enabled,
        "setplayerspeed" => c.player_speed = v,
        "setphysicsspeed" => c.physics_speed = v,
        "setlaunchtarget" => {
            let t = call.param.trim().to_ascii_lowercase();
            c.target = (!t.is_empty()).then_some(t);
        }
        _ => return false,
    }
    true
}

/// Velocity that carries an object from `from` to `target` with the given
/// horizontal speed under gravity.
pub fn launch_velocity(from: Vec3, target: Vec3, horizontal_speed: f32) -> Vec3 {
    let d = target - from;
    let flat = d.with_z(0.0);
    let dist = flat.length();
    let speed = horizontal_speed.max(1.0);
    if dist < 1.0 {
        // Straight up: enough to just reach the target height.
        return Vec3::Z * (2.0 * GRAVITY * d.z.max(16.0)).sqrt();
    }
    let t = dist / speed;
    let vz = (d.z + 0.5 * GRAVITY * t * t) / t;
    flat / dist * speed + Vec3::Z * vz
}

/// Velocity of exactly `speed` that reaches `target`: the flatter of the two
/// ballistic arcs, or 45 degrees when the target is out of reach.
pub fn exact_launch_velocity(from: Vec3, target: Vec3, speed: f32) -> Vec3 {
    let d = target - from;
    let flat = d.with_z(0.0);
    let x = flat.length();
    if x < 1.0 {
        return Vec3::Z * speed;
    }
    let s2 = speed * speed;
    let disc = s2 * s2 - GRAVITY * (GRAVITY * x * x + 2.0 * d.z * s2);
    let angle = if disc >= 0.0 { ((s2 - disc.sqrt()) / (GRAVITY * x)).atan() } else { std::f32::consts::FRAC_PI_4 };
    flat / x * speed * angle.cos() + Vec3::Z * speed * angle.sin()
}

impl Catapult {
    fn velocity(&self, from: Vec3, target: Option<Vec3>, speed: f32) -> Vec3 {
        match target {
            Some(t) if self.use_exact => exact_launch_velocity(from, t, speed),
            Some(t) => launch_velocity(from, t, speed),
            None => angles_forward(self.direction) * speed,
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_catapults(
    time: Res<Time>,
    physics: Physics,
    registry: Res<EntityRegistry>,
    transforms: Query<&GlobalTransform>,
    mut catapults: Query<(Entity, &mut Catapult)>,
    mut players: Query<(Entity, &mut Player)>,
    mut props: Query<(Entity, &TraceCollider, &GlobalTransform, &RigidBody, &mut LinearVelocity), With<PhysicsProp>>,
    mut air: ResMut<AirControl>,
    mut out: MessageWriter<FireOutput>,
) {
    let now = time.elapsed_secs_f64();
    let player = players.single_mut().ok();
    let (pe, mut p) = match player {
        Some((pe, p)) => (Some(pe), Some(p)),
        None => (None, None),
    };
    for (ce, mut c) in &mut catapults {
        if !c.enabled {
            c.touching.clear();
            continue;
        }
        let target_pos = c
            .target
            .as_ref()
            .and_then(|t| registry.first_named(t))
            .and_then(|t| transforms.get(t).ok())
            .map(|t| t.translation());
        let mut now_touching = Vec::new();

        if let (Some(pe), Some(p)) = (pe, p.as_deref_mut())
            && c.launch_players
            && !p.noclip
        {
            let (mins, maxs) = p.state.hull();
            if physics.touches(ce, p.state.origin, mins, maxs) {
                now_touching.push(pe);
                if !c.touching.contains(&pe) {
                    // The player's origin (feet) is aimed at the target.
                    let v = c.velocity(p.state.origin, target_pos, c.player_speed);
                    p.state.velocity = v;
                    p.state.on_ground = false;
                    p.state.ground_entity = None;
                    p.ground_ref = None;
                    air.suppressed_until =
                        if c.air_suppress > 0.0 { now + c.air_suppress as f64 } else { f64::INFINITY };
                    out.write(FireOutput::new(ce, "OnCatapulted").with_activator(Some(pe)));
                    info!("catapult launched player at {:.0?}", v);
                }
            }
        }

        for (prop, col, gt, body, mut lin) in &mut props {
            if !c.launch_props || *body != RigidBody::Dynamic {
                continue;
            }
            let t = gt.compute_transform();
            let (mins, maxs) = crate::physics::world_bounds(&col.shape, t.translation, t.rotation);
            let center = (mins + maxs) * 0.5;
            if !physics.touches(ce, center, mins - center, maxs - center) {
                continue;
            }
            now_touching.push(prop);
            if !c.touching.contains(&prop) {
                lin.0 = c.velocity(center, target_pos, c.physics_speed);
                out.write(FireOutput::new(ce, "OnCatapulted").with_activator(Some(prop)));
            }
        }
        c.touching = now_touching;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_lands_on_target() {
        let from = Vec3::ZERO;
        let target = Vec3::new(600.0, 200.0, 128.0);
        let v = launch_velocity(from, target, 450.0);
        let t = target.with_z(0.0).length() / 450.0;
        let landed = from + v * t + Vec3::NEG_Z * 0.5 * GRAVITY * t * t;
        assert!(landed.distance(target) < 0.5, "{landed:?}");
    }

    #[test]
    fn exact_launch_has_set_speed_and_lands() {
        let target = Vec3::new(400.0, 0.0, 64.0);
        let v = exact_launch_velocity(Vec3::ZERO, target, 700.0);
        assert!((v.length() - 700.0).abs() < 0.1);
        let t = 400.0 / v.x;
        let z = v.z * t - 0.5 * GRAVITY * t * t;
        assert!((z - 64.0).abs() < 0.5, "{z}");
    }
}
