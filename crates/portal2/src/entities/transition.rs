//! Level transitions: `point_changelevel` and the `changelevel` command.
//!
//! As in the original single-player game, the player keeps their place
//! relative to the old map's `info_landmark_exit`, which becomes the same
//! place relative to the new map's `info_landmark_entry` (translation only),
//! along with their velocity, view and crouch. The new map is entered with
//! the "transition" load type, so `logic_auto` fires `OnMapTransition`.
//!
//! The single-player scripts end a level through `portal_stats_controller`
//! (see [`StatsController`]).

use bevy::prelude::*;

use super::io::{InputCall, InputHandlers, fire_output};
use super::{EntityRegistry, MapEntity, SpawnHandlers, world_transform};
use crate::clock::{GameClock, ThinkSet};
use crate::player::{Player, PlayerInput, PlayerSpawnSet};
use crate::world::{CurrentMap, LoadMap, MapLoadType};

pub fn register(app: &mut App) {
    app.add_systems(PreUpdate, apply_transition.in_set(TransitionSet).after(PlayerSpawnSet))
        .add_systems(FixedUpdate, stats_think.in_set(ThinkSet));
    let world = app.world_mut();
    {
        let mut s = world.resource_mut::<SpawnHandlers>();
        for class in ["info_landmark_entry", "info_landmark_exit", "info_landmark", "trigger_transition"] {
            s.register(class, |_, _| {});
        }
        s.register("portal_stats_controller", spawn_stats_controller);
    }
    let mut h = world.resource_mut::<InputHandlers>();
    h.register("point_changelevel", changelevel_input);
    h.register("trigger_changelevel", changelevel_input);
    h.register("portal_stats_controller", stats_input);
}

/// Placement of the player relative to the new map's entry landmark (level
/// load, before the first tick).
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct TransitionSet;

/// Player state carried into the next map.
#[derive(Resource, Clone, Debug)]
pub struct PendingTransition {
    pub to: String,
    /// Player feet relative to the exit landmark, if the old map had one.
    pub offset: Option<Vec3>,
    pub velocity: Vec3,
    pub angles: Vec3,
    pub ducked: bool,
}

fn first_of_class(world: &World, class: &str) -> Option<Entity> {
    world.resource::<EntityRegistry>().by_class.get(class).and_then(|l| l.first().copied())
}

/// Starts a transition to `map` from the current one.
pub fn change_level(world: &mut World, map: &str) {
    let map = map.trim().trim_end_matches(".bsp").to_ascii_lowercase();
    if map.is_empty() || !world.resource::<crate::vfs::GameFs>().exists(&format!("maps/{map}.bsp")) {
        world.resource_mut::<crate::console::Console>().print(format!("changelevel failed: {map} not found"));
        return;
    }
    let player = world.resource::<EntityRegistry>().player;
    let state = player.and_then(|p| world.get::<Player>(p)).map(|p| p.state);
    let landmark = first_of_class(world, "info_landmark_exit").map(|l| world_transform(world, l).translation());
    let angles = world.resource::<PlayerInput>().input.angles;
    let pending = PendingTransition {
        to: map.clone(),
        offset: landmark.zip(state).map(|(l, s)| s.origin - l),
        velocity: state.map(|s| s.velocity).unwrap_or(Vec3::ZERO),
        angles,
        ducked: state.is_some_and(|s| s.ducked),
    };
    info!("changelevel to {map} (landmark offset {:?})", pending.offset);
    world.insert_resource(pending);
    world.write_message(LoadMap::new(map, MapLoadType::Transition));
}

fn changelevel_input(world: &mut World, call: &InputCall) -> bool {
    if call.input != "changelevel" {
        return false;
    }
    let target = match call.param.trim() {
        "" => world.get::<super::MapEntity>(call.target).and_then(|m| m.get("map").map(str::to_owned)).unwrap_or_default(),
        p => p.to_owned(),
    };
    fire_output(world, call.target, "OnChangeLevel", call.activator, None);
    change_level(world, &target);
    true
}

/// Places the player in the new map once it has loaded.
fn apply_transition(world: &mut World) {
    let Some(pending) = world.get_resource::<PendingTransition>().cloned() else { return };
    let Some(map) = world.get_resource::<CurrentMap>() else { return };
    if map.name != pending.to {
        return;
    }
    let Some(player) = world.resource::<EntityRegistry>().player else { return };
    let entry = first_of_class(world, "info_landmark_entry").map(|l| world_transform(world, l).translation());
    if let (Some(entry), Some(offset)) = (entry, pending.offset)
        && let Some(mut p) = world.get_mut::<Player>(player)
    {
        let origin = entry + offset;
        p.state.origin = origin;
        p.previous_origin = origin;
        p.state.velocity = pending.velocity;
        p.state.ducked = pending.ducked;
        p.ground_ref = None;
        if let Some(mut t) = world.get_mut::<Transform>(player) {
            t.translation = origin;
        }
    }
    world.resource_mut::<PlayerInput>().input.angles = pending.angles;
    world.remove_resource::<PendingTransition>();
    // `sv_autosave`: entering a level by transition autosaves.
    if world.resource::<crate::console::Console>().bool("sv_autosave") {
        crate::console::queue(world, "autosave");
    }
}

// --------------------------------------------------- portal_stats_controller

/// The level-end driver of the single-player scripts. The game makes one
/// per map when the map has none. `TransitionFromMap()` fires its
/// `OnLevelEnd`; the first think (every 0.1 s) after the end time runs
/// `RealTransitionFromMap()` in the transition script's scope and removes
/// the controller. Challenge-mode parts (leaderboards, cheat reports, the
/// timed `OnLevelEnd` delay) do not apply to the campaign and are left out.
#[derive(Component, serde::Serialize, serde::Deserialize)]
pub struct StatsController {
    /// When the level ended, once `OnLevelEnd` arrived.
    end_time: Option<f64>,
    next_think: f64,
}

/// Creates the map's controller unless the map placed one.
pub fn ensure_stats_controller(world: &mut World) {
    if first_of_class(world, "portal_stats_controller").is_none() {
        super::spawn_runtime_entity(world, "portal_stats_controller", &[]);
    }
}

fn spawn_stats_controller(world: &mut World, e: Entity) {
    world.entity_mut(e).insert(StatsController { end_time: None, next_think: f64::MAX });
}

fn stats_input(world: &mut World, call: &InputCall) -> bool {
    let now = world.resource::<GameClock>().curtime;
    let Some(mut c) = world.get_mut::<StatsController>(call.target) else { return false };
    match call.input.as_str() {
        // Outside challenge mode the level ends at once.
        "onlevelend" => c.end_time = Some(now),
        "onlevelstart" => c.end_time = None,
        _ => return false,
    }
    c.next_think = now + 0.1;
    debug!("portal_stats_controller: {} at {now:.2}", call.input);
    true
}

/// The entity whose scope holds `RealTransitionFromMap`: `@transition_script`,
/// else `script_check_finish_game`, else the first logic_script with
/// "transition_script" in its name.
fn transition_script(world: &World) -> Option<Entity> {
    let registry = world.resource::<EntityRegistry>();
    for name in ["@transition_script", "script_check_finish_game"] {
        if let Some(&e) = registry.by_name.get(name).and_then(|l| l.first()) {
            return Some(e);
        }
    }
    let scripts = registry.by_class.get("logic_script")?;
    scripts
        .iter()
        .copied()
        .find(|&e| world.get::<MapEntity>(e).and_then(|m| m.targetname.as_deref()).is_some_and(|n| n.contains("transition_script")))
}

fn stats_think(world: &mut World) {
    let clock = *world.resource::<GameClock>();
    let due: Vec<(Entity, Option<f64>)> = world
        .query::<(Entity, &StatsController)>()
        .iter(world)
        .filter(|(_, c)| clock.due(c.next_think))
        .map(|(e, c)| (e, c.end_time))
        .collect();
    for (e, end) in due {
        if let Some(end) = end
            && clock.curtime > end
            && let Some(script) = transition_script(world)
        {
            info!("level end: RealTransitionFromMap() at {:.2}", clock.curtime);
            crate::script::run_code(world, script, "RealTransitionFromMap()", "CBaseEntity::RunScript");
            super::despawn_entity(world, e);
            continue;
        }
        if let Some(mut c) = world.get_mut::<StatsController>(e) {
            c.next_think = clock.curtime + 0.1;
        }
    }
}

impl crate::save::Restore for StatsController {}

pub fn register_save(r: &mut crate::save::SaveRegistry) {
    r.component::<StatsController>("stats_controller");
}
