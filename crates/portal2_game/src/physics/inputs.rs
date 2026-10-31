//! Physics inputs on map entities: `EnableMotion` / `DisableMotion` (and
//! `Wake` / `Sleep`) on physics props and `func_physbox`, which in the
//! campaign start frozen (spawnflag 32768) until a script lets them fall,
//! and `env_physexplosion`'s `Explode`, which pushes the physics objects
//! around it like the Source physics explosion: the damage falls off by
//! 1 per 2.5 units, and each point of it pushes as hard as 4 in/s on a
//! 75 kg body, at most 400 in/s worth, varied ±15% per object.

use avian3d::prelude::*;
use bevy::prelude::*;

use crate::entities::io::{InputCall, InputHandlers, fire_output};
use crate::entities::props::PhysicsProp;
use portal2_core::entity::MapEntity;
use portal2_world::collision::Collider as TraceCollider;

/// The explosion's radius when its keyvalue is 0: this times its magnitude.
const RADIUS_PER_MAGNITUDE: f32 = 2.5;
/// Push per point of damage (75 kg pushed 4 in/s) and its cap (400 in/s).
const IMPULSE_PER_DAMAGE: f32 = 75.0 * 4.0;
const MAX_IMPULSE: f32 = 75.0 * 400.0;
const SF_PUSH_PLAYER: u32 = 2;

pub fn register(app: &mut App) {
    let mut h = app.world_mut().resource_mut::<InputHandlers>();
    for class in [
        "func_physbox",
        "func_physbox_multiplayer",
        "prop_weighted_cube",
        "prop_physics",
        "prop_physics_override",
        "prop_physics_multiplayer",
        "npc_personality_core",
    ] {
        h.register(class, motion_input);
    }
    h.register("env_physexplosion", explosion_input);
}

fn mass_of(world: &World, e: Entity) -> f32 {
    world.get::<Mass>(e).map(|m| m.0).filter(|m| *m > 0.0).unwrap_or(40.0)
}

fn motion_input(world: &mut World, call: &InputCall) -> bool {
    let e = call.target;
    match call.input.as_str() {
        "enablemotion" | "wake" => {
            if let Some(mut me) = world.get_mut::<MapEntity>(e) {
                me.set_flag(super::SF_MOTION_DISABLED, false);
            }
            let dynamic = world.get::<RigidBody>(e).is_some_and(|b| b.is_dynamic());
            // A prop on a parent keeps riding it, free to fall once
            // `ClearParent` lets it go: maps send both when they drop one
            // (Wheatley out of a socket), and `sp_a1_wakeup` enables motion on
            // Wheatley as it plugs him into his socket.
            let parented = crate::entities::map_parent(world, e).is_some();
            if !dynamic
                && !parented
                && let Some(shape) = world.get::<TraceCollider>(e).map(|c| c.shape.clone())
            {
                let Some(collider) = super::prop_collider(&shape) else { return true };
                let mass = world.get::<MapEntity>(e).map_or(40.0, |me| super::prop_mass(world.resource::<portal2_core::vfs::GameFs>(), me));
                let mut commands = world.commands();
                super::make_dynamic(&mut commands, e, collider, mass);
                world.flush();
                world.entity_mut(e).insert(PhysicsProp);
                fire_output(world, e, "OnMotionEnabled", call.activator, None);
            }
            true
        }
        "disablemotion" | "sleep" => {
            if call.input == "disablemotion"
                && let Some(mut me) = world.get_mut::<MapEntity>(e)
            {
                me.set_flag(super::SF_MOTION_DISABLED, true);
            }
            if world.get::<RigidBody>(e).is_some() {
                world.entity_mut(e).insert((RigidBody::Kinematic, LinearVelocity::ZERO, AngularVelocity::ZERO));
            }
            true
        }
        _ => false,
    }
}

fn explosion_input(world: &mut World, call: &InputCall) -> bool {
    if call.input != "explode" {
        return false;
    }
    let Some(me) = world.get::<MapEntity>(call.target).cloned() else { return true };
    let center = world.get::<GlobalTransform>(call.target).map(|g| g.translation()).unwrap_or_else(|| me.origin());
    let magnitude = me.get_f32("magnitude").unwrap_or(100.0);
    let radius = me.get_f32("radius").filter(|r| *r > 0.0).unwrap_or(magnitude * RADIUS_PER_MAGNITUDE);
    let mut bodies = world.query::<(Entity, &RigidBody, &GlobalTransform)>();
    let targets: Vec<(Entity, Vec3)> = bodies
        .iter(world)
        .filter(|(_, body, _)| body.is_dynamic())
        .map(|(e, _, gt)| (e, gt.translation()))
        .filter(|(_, p)| p.distance(center) <= radius)
        .collect();
    for (e, pos) in targets {
        let dist = pos.distance(center);
        let damage = (magnitude - dist / RADIUS_PER_MAGNITUDE).max(1.0);
        let force = (damage * IMPULSE_PER_DAMAGE).min(MAX_IMPULSE) * portal2_core::rng::random_range(0.85, 1.15);
        let dir = (pos - center).normalize_or(Vec3::Z);
        let dv = dir * force / mass_of(world, e);
        if let Some(mut v) = world.get_mut::<LinearVelocity>(e) {
            v.0 += dv;
        }
    }
    if me.spawnflags() & SF_PUSH_PLAYER != 0
        && let Some(player) = world.resource::<portal2_core::entity::EntityRegistry>().player
        && let Some(mut p) = world.get_mut::<crate::player::Player>(player)
    {
        let at = p.state.origin + Vec3::Z * 36.0;
        let dist = at.distance(center);
        if dist <= radius {
            let damage = (magnitude - dist / RADIUS_PER_MAGNITUDE).max(1.0);
            let force = (damage * IMPULSE_PER_DAMAGE).min(MAX_IMPULSE);
            p.state.velocity += (at - center).normalize_or(Vec3::Z) * force / 75.0;
            p.state.on_ground = false;
        }
    }
    true
}
