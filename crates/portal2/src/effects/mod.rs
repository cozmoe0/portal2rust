//! Particle effects the game starts: `info_particle_system` entities,
//! `env_ar2explosion`, and the effects the code itself plays (portal shots
//! and openings, laser glows, powered catchers, fizzler scan lines). The
//! systems run on the particle engine in `portal2_particles`.

pub mod ar2_explosion;
pub mod hooks;

use bevy::prelude::*;
use portal2_core::entity::MapEntity;
use portal2_particles::ParticleEffect;
use portal2_particles::sim::MAX_CONTROL_POINTS;

use crate::entities::SpawnHandlers;
use crate::entities::io::{InputCall, InputHandlers};

pub struct EffectsPlugin;

impl Plugin for EffectsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(hooks::HooksPlugin);
        let world = app.world_mut();
        world.resource_mut::<SpawnHandlers>().register("info_particle_system", spawn_info);
        world.resource_mut::<InputHandlers>().register("info_particle_system", effect_input);
        ar2_explosion::register(app);
    }
}

fn spawn_info(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e).cloned() else { return };
    let mut effect = ParticleEffect::new(me.get("effect_name").unwrap_or(""), me.get_bool("start_active"));
    for i in 1..MAX_CONTROL_POINTS {
        if let Some(name) = me.get(&format!("cpoint{i}")).filter(|n| !n.trim().is_empty()) {
            effect.cp_names.push((i, name.trim().to_ascii_lowercase()));
        }
    }
    world.entity_mut(e).insert(effect);
}

fn effect_input(world: &mut World, call: &InputCall) -> bool {
    world.get_mut::<ParticleEffect>(call.target).is_some_and(|mut fx| fx.input(&call.input))
}
