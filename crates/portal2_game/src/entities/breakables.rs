//! `func_breakable`: a brush that breaks when its health runs out or on
//! `Break`, firing `OnBreak` and leaving the map (`CBreakable::Die` removes
//! it). Spawnflag 1 makes it break on `Break` alone. Lasers wear it down:
//! `sp_a2_bts5`'s neurotoxin pipes are cut by steering the laser onto the
//! one-health breakables along them, each counting towards the tank's
//! destruction; `sp_a2_core`'s catapult floor panels break as the plates
//! fling.

use bevy::prelude::*;

use super::io::{InputCall, InputHandlers, fire_output};
use super::{MapEntity, SpawnHandlers, despawn_entity};

/// spawnflags: only break on trigger (no damage).
const SF_ONLY_TRIGGER: u32 = 1;

pub fn register(app: &mut App) {
    app.add_message::<BreakableDamage>().add_systems(FixedUpdate, apply_damage);
    let world = app.world_mut();
    world.resource_mut::<SpawnHandlers>().register("func_breakable", spawn_breakable);
    world.resource_mut::<InputHandlers>().register("func_breakable", breakable_input);
}

#[derive(Component, Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub struct Breakable {
    pub health: f32,
    /// Damage does nothing; only `Break` breaks it.
    pub trigger_only: bool,
}

impl portal2_core::save::Restore for Breakable {}

pub fn register_save(r: &mut portal2_core::save::SaveRegistry) {
    r.component::<Breakable>("breakable");
}

/// Damage dealt to a breakable this tick (by a laser, for now).
#[derive(Message, Clone, Copy, Debug)]
pub struct BreakableDamage {
    pub entity: Entity,
    pub amount: f32,
    pub attacker: Option<Entity>,
}

fn spawn_breakable(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e) else { return };
    let breakable = Breakable { health: me.get_f32("health").unwrap_or(1.0), trigger_only: me.has_flag(SF_ONLY_TRIGGER) };
    world.entity_mut(e).insert(breakable);
}

fn breakable_input(world: &mut World, call: &InputCall) -> bool {
    let e = call.target;
    let Some(mut b) = world.get_mut::<Breakable>(e) else { return false };
    let value = source_formats::kv::parse_f32_lenient(&call.param);
    match call.input.as_str() {
        "break" => {
            break_now(world, e, call.activator);
            return true;
        }
        "sethealth" => b.health = value,
        "addhealth" => b.health += value,
        "removehealth" => b.health -= value,
        _ => return false,
    }
    if b.health <= 0.0 && call.input != "sethealth" {
        break_now(world, e, call.activator);
    }
    true
}

fn break_now(world: &mut World, e: Entity, activator: Option<Entity>) {
    if world.get::<Breakable>(e).is_none() {
        return;
    }
    world.entity_mut(e).remove::<Breakable>();
    fire_output(world, e, "OnBreak", activator, None);
    despawn_entity(world, e);
}

fn apply_damage(world: &mut World) {
    let hits: Vec<BreakableDamage> = world.resource_mut::<Messages<BreakableDamage>>().drain().collect();
    for hit in hits {
        let Some(mut b) = world.get_mut::<Breakable>(hit.entity) else { continue };
        if b.trigger_only {
            continue;
        }
        b.health -= hit.amount;
        if b.health <= 0.0 {
            break_now(world, hit.entity, hit.attacker);
        }
    }
}
