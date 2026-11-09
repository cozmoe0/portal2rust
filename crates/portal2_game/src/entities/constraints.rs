//! `phys_constraint`: holds two objects in their relative placement, as a
//! weld. The game's templated turret carts (`sp_a2_bts4`'s dummy shoot and
//! scanner lines) weld each new turret to its cart's train this way, so the
//! turret rides the conveyor instead of tipping over where it spawned. Here
//! the second object rides the first as a child (a physics prop on a parent
//! goes kinematic and falls free again when let go); with only one object,
//! or the first missing, it is held still in the world.
//!
//! Spawnflags: 4 starts inactive, 16 waits for `TurnOn` before connecting
//! (both mean the weld waits for `TurnOn`). `TurnOff`, `Break` and `Kill`
//! let go.

use bevy::prelude::*;

use super::io::{EventQueue, EventTarget, InputCall, InputHandlers, fire_output};
use super::{EntityRegistry, MapEntity, SpawnHandlers};

const SF_START_INACTIVE: u32 = 4;
const SF_NO_CONNECT_UNTIL_ACTIVE: u32 = 16;

pub fn register(app: &mut App) {
    let world = app.world_mut();
    world.resource_mut::<SpawnHandlers>().register("phys_constraint", spawn_constraint);
    world.resource_mut::<InputHandlers>().register("phys_constraint", constraint_input);
}

/// Whether a weld holds its object, and how. The objects are found by
/// name each time, which a save keeps.
#[derive(Component, Clone, Copy, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Weld {
    pub active: bool,
    /// Whether the held object was given the first object as its parent
    /// (else it is only held still).
    pub parented: bool,
}

impl portal2_core::save::Restore for Weld {}

pub fn register_save(r: &mut portal2_core::save::SaveRegistry) {
    r.component::<Weld>("weld");
}

fn spawn_constraint(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e) else { return };
    let waits = me.has_flag(SF_START_INACTIVE) || me.has_flag(SF_NO_CONNECT_UNTIL_ACTIVE);
    world.entity_mut(e).insert(Weld::default());
    // The objects may spawn after the constraint (a template's members
    // come up one by one), so the weld connects on the next event pass.
    if !waits {
        world.resource_mut::<EventQueue>().push(0.0, EventTarget::Entity(e), "turnon", "", None, None);
    }
}

fn constraint_input(world: &mut World, call: &InputCall) -> bool {
    let e = call.target;
    match call.input.as_str() {
        "turnon" => connect(world, e),
        "turnoff" => release(world, e),
        "break" => {
            release(world, e);
            fire_output(world, e, "OnBreak", call.activator, None);
        }
        // Let go, then the generic handler removes the constraint.
        "kill" | "killhierarchy" => {
            release(world, e);
            return false;
        }
        _ => return false,
    }
    true
}

/// The object a weld holds and the name of the one it rides, if any.
fn objects(world: &World, e: Entity) -> Option<(Entity, Option<String>)> {
    let me = world.get::<MapEntity>(e)?;
    let named = |key: &str| me.get(key).filter(|n| !n.is_empty()).map(str::to_ascii_lowercase);
    let registry = world.resource::<EntityRegistry>();
    let first = named("attach1").and_then(|n| registry.first_named(&n).map(|x| (n, x)));
    let second = named("attach2").and_then(|n| registry.first_named(&n));
    match (first, second) {
        (Some((name, _)), Some(held)) => Some((held, Some(name))),
        (Some((_, held)), None) | (None, Some(held)) => Some((held, None)),
        (None, None) => None,
    }
}

fn connect(world: &mut World, e: Entity) {
    if world.get::<Weld>(e).is_none_or(|w| w.active) {
        return;
    }
    let Some((held, anchor)) = objects(world, e) else { return };
    let parented = match anchor {
        Some(name) => {
            super::set_parent(world, held, &name);
            true
        }
        None => {
            if world.get::<avian3d::prelude::RigidBody>(held).is_some() {
                world.entity_mut(held).insert((
                    avian3d::prelude::RigidBody::Kinematic,
                    avian3d::prelude::LinearVelocity::ZERO,
                    avian3d::prelude::AngularVelocity::ZERO,
                ));
            }
            false
        }
    };
    if let Some(mut w) = world.get_mut::<Weld>(e) {
        *w = Weld { active: true, parented };
    }
}

fn release(world: &mut World, e: Entity) {
    let Some(weld) = world.get_mut::<Weld>(e).map(|mut w| std::mem::take(&mut *w)) else { return };
    if !weld.active {
        return;
    }
    let Some((held, _)) = objects(world, e) else { return };
    if weld.parented && world.get::<ChildOf>(held).is_some() {
        super::clear_parent(world, held);
    } else {
        crate::physics::on_unparented(world, held);
    }
}
