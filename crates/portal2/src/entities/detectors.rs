//! Player and portal watchers: `func_portal_detector` (portals placed in a
//! volume), `logic_playerproxy` (player actions as outputs),
//! `player_speedmod` (movement speed scaling) and the player's own inputs.

use bevy::prelude::*;

use super::io::{FireOutput, InputCall, InputHandlers};
use super::{MapEntity, SpawnHandlers};
use crate::clock::ThinkSet;
use crate::collision::Physics;
use crate::player::Player;
use crate::portals::{Portal, Portals};

pub fn register(app: &mut App) {
    app.init_resource::<SpeedMod>().init_resource::<PlayerFog>().add_systems(FixedUpdate, (detect_portals, player_proxy_outputs).in_set(ThinkSet));
    let world = app.world_mut();
    {
        let mut s = world.resource_mut::<SpawnHandlers>();
        s.register("func_portal_detector", spawn_detector);
        s.register("logic_playerproxy", |_, _| {});
        s.register("player_speedmod", |_, _| {});
    }
    let mut h = world.resource_mut::<InputHandlers>();
    h.register("func_portal_detector", detector_input);
    h.register("logic_playerproxy", proxy_input);
    h.register("player_speedmod", speedmod_input);
    h.register("player", player_input);
}

// ------------------------------------------------------ func_portal_detector

#[derive(Component, Default, serde::Serialize, serde::Deserialize)]
pub struct PortalDetector {
    pub enabled: bool,
    /// Portal slot inside the volume, and whether it was linked then.
    inside: [bool; 2],
    linked: [bool; 2],
    both: bool,
}

fn spawn_detector(world: &mut World, e: Entity) {
    let enabled = !world.get::<MapEntity>(e).is_some_and(|m| m.get_bool("startdisabled"));
    world.entity_mut(e).insert(PortalDetector { enabled, ..default() });
}

fn detector_input(world: &mut World, call: &InputCall) -> bool {
    let Some(mut d) = world.get_mut::<PortalDetector>(call.target) else { return false };
    match call.input.as_str() {
        "enable" => d.enabled = true,
        "disable" => d.enabled = false,
        "toggle" => d.enabled = !d.enabled,
        _ => return false,
    }
    true
}

fn detect_portals(
    portals: Res<Portals>,
    portal_query: Query<&Portal>,
    physics: Physics,
    mut detectors: Query<(Entity, &mut PortalDetector)>,
    mut out: MessageWriter<FireOutput>,
) {
    let frames: Vec<Option<(Entity, Vec3)>> =
        portals.entities.iter().map(|e| e.and_then(|e| portal_query.get(e).ok().map(|p| (e, p.frame.center)))).collect();
    let linked = frames.iter().all(Option::is_some);
    for (de, mut d) in &mut detectors {
        if !d.enabled {
            continue;
        }
        for slot in 0..2 {
            let now_in = frames[slot].is_some_and(|(_, c)| physics.touches(de, c, Vec3::ZERO, Vec3::ZERO));
            let now_linked = now_in && linked;
            let activator = frames[slot].map(|(e, _)| e);
            let n = slot + 1;
            if now_in && !d.inside[slot] {
                out.write(FireOutput::new(de, "OnStartTouchPortal").with_activator(activator));
                out.write(FireOutput::new(de, &format!("OnStartTouchPortal{n}")).with_activator(activator));
            } else if !now_in && d.inside[slot] {
                out.write(FireOutput::new(de, "OnEndTouchPortal").with_activator(activator));
                out.write(FireOutput::new(de, &format!("OnEndTouchPortal{n}")).with_activator(activator));
            }
            if now_linked && !d.linked[slot] {
                out.write(FireOutput::new(de, "OnStartTouchLinkedPortal").with_activator(activator));
            } else if !now_linked && d.linked[slot] {
                out.write(FireOutput::new(de, "OnEndTouchLinkedPortal").with_activator(activator));
            }
            d.inside[slot] = now_in;
            d.linked[slot] = now_linked;
        }
        let both = d.linked[0] && d.linked[1];
        if both && !d.both {
            out.write(FireOutput::new(de, "OnStartTouchBothLinkedPortals"));
        } else if !both && d.both {
            out.write(FireOutput::new(de, "OnEndTouchBothLinkedPortals"));
        }
        d.both = both;
    }
}

// --------------------------------------------------------- logic_playerproxy

/// Player actions reported to every logic_playerproxy: jumping and
/// crouching.
fn player_proxy_outputs(
    players: Query<&Player>,
    proxies: Query<(Entity, &MapEntity)>,
    mut last: Local<Option<(bool, bool)>>,
    mut out: MessageWriter<FireOutput>,
) {
    let Ok(p) = players.single() else { return };
    let (jumping, ducked) = (p.state.jump_held && !p.state.on_ground, p.state.ducked);
    let before = last.replace((jumping, ducked));
    let Some((was_jumping, was_ducked)) = before else { return };
    let events: Vec<&str> = [
        (jumping && !was_jumping, "OnJump"),
        (ducked && !was_ducked, "OnDuck"),
        (!ducked && was_ducked, "OnUnDuck"),
    ]
    .into_iter()
    .filter_map(|(hit, name)| hit.then_some(name))
    .collect();
    if events.is_empty() {
        return;
    }
    for (e, me) in &proxies {
        if me.classname != "logic_playerproxy" {
            continue;
        }
        for ev in &events {
            out.write(FireOutput::new(e, ev));
        }
    }
}

/// Inputs that change how the player holds things or looks are accepted;
/// their effects come with the systems they belong to.
fn proxy_input(_world: &mut World, call: &InputCall) -> bool {
    matches!(
        call.input.as_str(),
        "setmotionbluramount"
            | "forcevmgrabcontroller"
            | "forcephysicsgrabcontroller"
            | "resetgrabcontroller"
            | "setdropenabled"
            | "addpotatostoportalgun"
            | "removepotatosfromportalgun"
            | "lowerweapon"
            | "paintplayerwithportalpaint"
            | "requestplayerhealth"
    )
}

// ---------------------------------------------------------- player_speedmod

/// Movement speed scale set by `player_speedmod` (1 = normal).
#[derive(Resource, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct SpeedMod(pub f32);

impl Default for SpeedMod {
    fn default() -> Self {
        Self(1.0)
    }
}

fn speedmod_input(world: &mut World, call: &InputCall) -> bool {
    if call.input != "modifyspeed" {
        return false;
    }
    let factor = source_formats::kv::parse_f32_lenient(&call.param).max(0.0);
    world.resource_mut::<SpeedMod>().0 = factor;
    true
}

// ------------------------------------------------------------- player inputs

/// The fog controller the player's view uses (`SetFogController`), by name.
#[derive(Resource, Default, Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PlayerFog {
    pub controller: Option<String>,
}

fn player_input(world: &mut World, call: &InputCall) -> bool {
    match call.input.as_str() {
        "setfogcontroller" => {
            let name = call.param.trim().to_ascii_lowercase();
            world.resource_mut::<PlayerFog>().controller = (!name.is_empty()).then_some(name);
            true
        }
        _ => false,
    }
}

impl crate::save::Restore for PortalDetector {}

impl crate::save::Restore for SpeedMod {}

impl crate::save::Restore for PlayerFog {}
