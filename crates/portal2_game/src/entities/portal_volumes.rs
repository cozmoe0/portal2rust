//! `func_noportal_volume` (no portal may be placed inside it) and
//! `func_portal_bumper` (portals fit around it): both start active unless
//! spawnflag 1 ("Start inactive") is set and switch with `Activate`,
//! `Deactivate` and `Toggle`. `sp_a2_bts3`'s lower walls by the gap are
//! covered by an inactive blocker that a trigger further on turns on, so
//! the player cannot portal back once past it.

use bevy::prelude::*;

use super::io::{InputCall, InputHandlers};
use super::{MapEntity, SpawnHandlers};

const CLASSES: [&str; 2] = ["func_noportal_volume", "func_portal_bumper"];
/// spawnflags: start inactive.
const SF_START_INACTIVE: u32 = 1;

pub fn register(app: &mut App) {
    let world = app.world_mut();
    for class in CLASSES {
        world.resource_mut::<SpawnHandlers>().register(class, spawn_volume);
        world.resource_mut::<InputHandlers>().register(class, volume_input);
    }
}

/// Whether a no-portal volume or portal bumper is doing its job.
#[derive(Component, Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub struct PortalVolume {
    pub active: bool,
}

impl portal2_core::save::Restore for PortalVolume {}

pub fn register_save(r: &mut portal2_core::save::SaveRegistry) {
    r.component::<PortalVolume>("portal_volume");
}

fn spawn_volume(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e) else { return };
    let active = !me.has_flag(SF_START_INACTIVE);
    world.entity_mut(e).insert(PortalVolume { active });
}

fn volume_input(world: &mut World, call: &InputCall) -> bool {
    let Some(mut v) = world.get_mut::<PortalVolume>(call.target) else { return false };
    v.active = match call.input.as_str() {
        "activate" => true,
        "deactivate" => false,
        "toggle" => !v.active,
        _ => return false,
    };
    true
}
