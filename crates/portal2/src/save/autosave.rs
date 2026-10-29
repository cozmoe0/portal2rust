//! Saving and reloading during play: `logic_autosave`, `trigger_autosave`,
//! `player_loadsaved`, dangerous autosaves (kept as the autosave only once
//! the player has survived their timer), reloading after death and
//! autosaving after level transitions.

use bevy::prelude::*;

use crate::entities::despawn_entity;
use crate::entities::io::{EventQueue, EventTarget, InputCall, InputHandlers};
use crate::player::health::Health;
use crate::screen::ScreenFade;
use portal2_core::clock::GameClock;
use portal2_core::console::Console;
use portal2_core::entity::{EntityRegistry, MapEntity};
use portal2_core::save::AutosaveRequest;
use portal2_core::sets::ThinkSet;

pub struct AutosavePlugin;

impl Plugin for AutosavePlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<AutosaveRequest>()
            .init_resource::<Dangerous>()
            .add_systems(Update, handle_requests)
            .add_systems(FixedUpdate, commit_when_safe.in_set(ThinkSet));
        let mut h = app.world_mut().resource_mut::<InputHandlers>();
        h.register("logic_autosave", logic_autosave_input);
        h.register("player_loadsaved", loadsaved_input);
        let mut c = app.world_mut().resource_mut::<Console>();
        c.register_cvar("sv_autosave", "1", 0);
        c.register_cvar("sk_auto_reload_time", "3", 0);
    }
}

/// The pending dangerous autosave.
#[derive(Resource, Default)]
struct Dangerous {
    deadline: Option<f64>,
    commit_health: f32,
}

fn player_health(world: &World) -> Option<f32> {
    let p = world.resource::<EntityRegistry>().player?;
    match world.get::<Health>(p) {
        Some(h) if h.dead => None,
        Some(h) => Some(h.current),
        None => Some(100.0),
    }
}

fn handle_requests(world: &mut World) {
    let requests: Vec<AutosaveRequest> = world.resource_mut::<Messages<AutosaveRequest>>().drain().collect();
    for r in requests {
        let now = world.resource::<GameClock>().curtime;
        // A dangerous save still waiting is good enough once another comes.
        if world.resource::<Dangerous>().deadline.is_some_and(|d| d >= now) && player_health(world).is_some() {
            world.resource_mut::<Dangerous>().deadline = None;
            portal2_core::console::queue(world, "autosavedangerousissafe");
        }
        if let Some(e) = r.remove {
            despawn_entity(world, e);
        }
        if r.dangerous <= 0.0 {
            portal2_core::console::queue(world, "autosave");
        } else if player_health(world).is_some_and(|h| h >= r.min_health) {
            portal2_core::console::queue(world, "autosavedangerous");
            *world.resource_mut::<Dangerous>() = Dangerous { deadline: Some(now + r.dangerous as f64), commit_health: r.commit_health };
        }
    }
}

fn commit_when_safe(world: &mut World) {
    let now = world.resource::<GameClock>().curtime;
    let Dangerous { deadline: Some(deadline), commit_health } = *world.resource::<Dangerous>() else { return };
    if now < deadline {
        return;
    }
    world.resource_mut::<Dangerous>().deadline = None;
    if player_health(world).is_some_and(|h| h >= commit_health) {
        portal2_core::console::queue(world, "autosavedangerousissafe");
    }
}

fn logic_autosave_input(world: &mut World, call: &InputCall) -> bool {
    let Some(me) = world.get::<MapEntity>(call.target).cloned() else { return false };
    match call.input.as_str() {
        "save" => portal2_core::console::queue(world, "autosave"),
        "savedangerous" => {
            world.write_message(AutosaveRequest {
                dangerous: source_formats::kv::parse_f32_lenient(&call.param),
                min_health: me.get_f32("minimumhitpoints").unwrap_or(0.0),
                commit_health: me.get_f32("minhitpointstocommit").unwrap_or(0.0),
                remove: None,
            });
        }
        "setminhitpointsthreshold" => {
            if let Some(mut m) = world.get_mut::<MapEntity>(call.target) {
                m.set("minimumhitpoints", &call.param);
            }
        }
        _ => return false,
    }
    true
}

/// `Reload`: fades the screen, then loads the most recent save after
/// `loadtime` seconds.
fn loadsaved_input(world: &mut World, call: &InputCall) -> bool {
    match call.input.as_str() {
        "reload" => {
            let Some(me) = world.get::<MapEntity>(call.target).cloned() else { return true };
            let rgb = me.get_vec3("rendercolor").unwrap_or(Vec3::ZERO) / 255.0;
            let now = world.resource::<GameClock>().curtime;
            *world.resource_mut::<ScreenFade>() = ScreenFade {
                color: rgb.extend(me.get_f32("renderamt").unwrap_or(255.0) / 255.0),
                start: now,
                duration: me.get_f32("duration").unwrap_or(2.0),
                hold: me.get_f32("holdtime").unwrap_or(0.0),
                fade_in: false,
                stay: true,
                modulate: false,
                active: true,
            };
            let delay = me.get_f32("loadtime").unwrap_or(0.0);
            world.resource_mut::<EventQueue>().push(delay, EventTarget::Entity(call.target), "__reload", "", None, None);
        }
        "__reload" => portal2_core::console::queue(world, "reload"),
        _ => return false,
    }
    true
}
