//! Entities that feed the game instructor: `env_instructor_hint` (shows a
//! map's own hint through the "Serverside Hint" lesson), and
//! `info_game_event_proxy` (fires a named game event). The visibility
//! monitor fires `entity_visible` the first time the player sees a cube or
//! a pedestal button nearby (the pick up and push button lessons), and
//! fires the event of proxies flagged to go off when seen. Facts in
//! docs/re/instructor.md.

use bevy::prelude::*;
use source_formats::bsp::contents;

use super::io::{InputCall, InputHandlers};
use super::{EntityRegistry, MapEntity, SpawnHandlers};
use crate::collision::Physics;
use crate::player::{Player, PlayerInput};
use portal2_core::clock::GameClock;
use portal2_core::console::Console;
use portal2_core::coords::angles_forward;
use portal2_core::events::{EventValue, GameEvent};
use portal2_core::sets::ThinkSet;

pub fn register(app: &mut App) {
    app.add_message::<GameEvent>().add_systems(FixedUpdate, (watch_new, poll_visibility).chain().in_set(ThinkSet));
    let world = app.world_mut();
    {
        let mut s = world.resource_mut::<SpawnHandlers>();
        s.register("env_instructor_hint", |_, _| {});
        s.register("info_target_instructor_hint", |_, _| {});
        s.register("info_game_event_proxy", spawn_proxy);
    }
    {
        let mut h = world.resource_mut::<InputHandlers>();
        h.register("env_instructor_hint", hint_input);
        h.register("info_game_event_proxy", proxy_input);
    }
    let mut console = world.resource_mut::<Console>();
    console.register_cvar("sv_portal2_pickup_hint_range", "350.0", 0);
    console.register_cvar("sv_portal2_button_hint_range", "350.0", 0);
    console.register_cvar("vismon_poll_frequency", ".5", portal2_core::console::cvars::flags::CHEAT);
    console.register_cvar("vismon_trace_limit", "12", portal2_core::console::cvars::flags::CHEAT);
}

/// `env_instructor_hint` spawnflags/keys folded into the locator flags the
/// game sends: pulse 1/2/4, alpha 8/16/32, shake 64/128, static 0x100.
fn hint_flags(me: &MapEntity) -> i32 {
    let option = |key: &str, unit: i32| match me.get_i32(key).unwrap_or(0) {
        n if n > 0 => unit << (n - 1),
        _ => 0,
    };
    option("hint_pulseoption", 1)
        | option("hint_alphaoption", 8)
        | option("hint_shakeoption", 64)
        | if me.get_bool("hint_static") { 0x100 } else { 0 }
}

/// Whether the game would send the entity to the client: it has a model
/// (instructor hint targets are sent anyway).
fn networked(me: &MapEntity) -> bool {
    me.classname == "info_target_instructor_hint" || me.get("model").is_some_and(|m| !m.is_empty())
}

fn hint_input(world: &mut World, call: &InputCall) -> bool {
    let e = call.target;
    let Some(me) = world.get::<MapEntity>(e).cloned() else { return false };
    let name = me.targetname.clone().unwrap_or_default();
    let event = match call.input.as_str() {
        "showhint" => {
            let registry = world.resource::<EntityRegistry>();
            let player = registry.player;
            // The named target, else the activator, else the player. Only
            // entities the client knows of count: the game does not send
            // ones without a model, so a hint "on" a scene or a logic
            // entity stays on the player.
            let target = me
                .get("hint_target")
                .filter(|s| !s.is_empty())
                .and_then(|n| registry.by_name.get(&n.to_ascii_lowercase()))
                .and_then(|l| l.first().copied())
                .or(call.activator)
                .or(player)
                .filter(|&t| Some(t) == player || world.get::<MapEntity>(t).is_some_and(networked));
            let activator_player = call.activator.filter(|a| Some(*a) == player);
            let caption = me.get("hint_caption").unwrap_or("").to_owned();
            let activator_caption = me.get("hint_activator_caption").filter(|s| !s.is_empty()).unwrap_or(&caption).to_owned();
            let color = me.get_vec3("hint_color").unwrap_or(Vec3::splat(255.0));
            let s = |k: &str| EventValue::Str(me.get(k).unwrap_or("").to_owned());
            let i = |k: &str| EventValue::Int(me.get_i32(k).unwrap_or(0));
            GameEvent::new("instructor_server_hint_create")
                .with("hint_name", EventValue::Str(name))
                .with("hint_replace_key", s("hint_replace_key"))
                .with("hint_target", EventValue::Entity(target))
                .with("hint_activator_userid", EventValue::Entity(activator_player))
                .with("hint_timeout", i("hint_timeout"))
                .with("hint_icon_onscreen", s("hint_icon_onscreen"))
                .with("hint_icon_offscreen", s("hint_icon_offscreen"))
                .with("hint_caption", EventValue::Str(caption))
                .with("hint_activator_caption", EventValue::Str(activator_caption))
                .with("hint_color", EventValue::Str(format!("{:03},{:03},{:03}", color.x as i32, color.y as i32, color.z as i32)))
                .with("hint_icon_offset", EventValue::Float(me.get_f32("hint_icon_offset").unwrap_or(0.0)))
                .with("hint_range", EventValue::Float(me.get_f32("hint_range").unwrap_or(0.0)))
                .with("hint_flags", EventValue::Int(hint_flags(&me)))
                .with("hint_binding", s("hint_binding"))
                .with("hint_gamepad_binding", s("hint_gamepad_binding"))
                .with("hint_allow_nodraw_target", i("hint_allow_nodraw_target"))
                .with("hint_nooffscreen", i("hint_nooffscreen"))
                .with("hint_forcecaption", i("hint_forcecaption"))
                .with("hint_local_player_only", i("hint_local_player_only"))
        }
        "endhint" => GameEvent::new("instructor_server_hint_stop").with("hint_name", EventValue::Str(name)),
        _ => return false,
    };
    world.write_message(event);
    true
}

// ---------------------------------------------------------------- proxy

/// spawnflags: fire the event when the player first sees the proxy.
const SF_AUTO_VISIBLE: u32 = 1;

fn spawn_proxy(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e) else { return };
    if me.has_flag(SF_AUTO_VISIBLE) {
        let range = me.get_f32("range").unwrap_or(0.0);
        world.entity_mut(e).insert(Watched { range, seen: false });
    }
}

fn proxy_event(me: &MapEntity, e: Entity, player: Option<Entity>) -> Option<GameEvent> {
    let name = me.get("event_name").filter(|s| !s.is_empty())?;
    Some(GameEvent::new(name).with("userid", EventValue::Entity(player)).with("subject", EventValue::Entity(Some(e))))
}

fn proxy_input(world: &mut World, call: &InputCall) -> bool {
    if call.input != "generategameevent" {
        return false;
    }
    let player = world.resource::<EntityRegistry>().player;
    let activator = call.activator.filter(|a| Some(*a) == player);
    if let Some(event) = world.get::<MapEntity>(call.target).and_then(|me| proxy_event(me, call.target, activator)) {
        world.write_message(event);
    }
    true
}

// ------------------------------------------------------ visibility monitor

/// Something the player is told about the first time they see it, within
/// `range` (0: any distance).
#[derive(Component)]
pub struct Watched {
    range: f32,
    seen: bool,
}

/// Cubes and pedestal buttons join the monitor as they appear.
fn watch_new(console: Res<Console>, new: Query<(Entity, &MapEntity), (Added<MapEntity>, Without<Watched>)>, mut commands: Commands) {
    for (e, me) in &new {
        let cvar = match me.classname.as_str() {
            "prop_weighted_cube" => "sv_portal2_pickup_hint_range",
            "prop_button" => "sv_portal2_button_hint_range",
            _ => continue,
        };
        commands.entity(e).insert(Watched { range: console.f32(cvar), seen: false });
    }
}

/// The player's view cone for "sees" (cosine of half its angle, flat).
const VIEW_CONE: f32 = 0.766;

#[allow(clippy::too_many_arguments)]
fn poll_visibility(
    clock: Res<GameClock>,
    console: Res<Console>,
    physics: Physics,
    input: Res<PlayerInput>,
    players: Query<(Entity, &Player)>,
    mut watched: Query<(Entity, &mut Watched, &MapEntity, &GlobalTransform)>,
    mut next_poll: Local<f64>,
    mut events: MessageWriter<GameEvent>,
) {
    let now = clock.curtime;
    if now < *next_poll && *next_poll - now < 10.0 {
        return;
    }
    *next_poll = now + console.f32("vismon_poll_frequency").max(0.05) as f64;
    let Ok((pe, p)) = players.single() else { return };
    let eye = p.state.origin + Vec3::Z * p.state.eye_offset();
    let facing = angles_forward(Vec3::new(0.0, input.input.angles.y, 0.0)).truncate();
    let mut traces = console.i32("vismon_trace_limit").max(1);
    for (e, mut w, me, gt) in &mut watched {
        if w.seen || traces <= 0 {
            continue;
        }
        let pos = gt.translation();
        if w.range > 0.0 && pos.distance(eye) > w.range {
            continue;
        }
        if facing.dot((pos - eye).truncate().normalize_or_zero()) <= VIEW_CONE {
            continue;
        }
        traces -= 1;
        let mask = contents::SOLID | contents::MOVEABLE | contents::BLOCKLOS;
        if physics.trace(eye, pos, Vec3::ZERO, Vec3::ZERO, mask, &[pe, e]).fraction < 1.0 {
            continue;
        }
        w.seen = true;
        let event = if me.classname == "info_game_event_proxy" {
            proxy_event(me, e, Some(pe))
        } else {
            Some(
                GameEvent::new("entity_visible")
                    .with("userid", EventValue::Entity(Some(pe)))
                    .with("subject", EventValue::Entity(Some(e)))
                    .with("classname", EventValue::Str(me.classname.clone()))
                    .with("entityname", EventValue::Str(me.targetname.clone().unwrap_or_default())),
            )
        };
        if let Some(event) = event {
            events.write(event);
        }
    }
}
