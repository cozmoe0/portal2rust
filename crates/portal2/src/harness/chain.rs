//! `--chain <map>`: runs the campaign's level-to-level flow without solving
//! the chambers. In each map the player is put into the exit elevator (its
//! door trigger first, then the trigger that starts the ride). Where the
//! ride is started by puzzle logic instead of a trigger, the player is put on
//! the elevator floor and the elevator script's `StartMoving()` is run; maps
//! without an exit elevator use the trigger that runs `TransitionFromMap()`.
//! The map's own entities and scripts must then change the level. `re/out/chain.txt`
//! lists per map how the exit was reached, the game time from entering it to
//! the level change, the next map, and how far from the arrival elevator
//! the player was once the map had started.
//!
//! `--chain-count` limits the number of maps, `--chain-speed` runs game time
//! faster than real time (8 by default).

use std::fmt::Write as _;

use bevy::ecs::system::SystemState;
use bevy::prelude::*;

use portal2_core::args::Args;
use portal2_core::clock::GameClock;
use portal2_core::entity::{EntityRegistry, MapEntity};
use portal2_core::sets::IoSet;
use portal2_core::sets::TransitionSet;
use portal2_game::entities::io::Outputs;
use portal2_game::entities::transition::PendingTransition;
use portal2_game::entities::triggers::Trigger;
use portal2_game::entities::world_transform;
use portal2_game::player::Player;
use portal2_game::player::movement::{HULL_MAXS, HULL_MINS};
use portal2_world::CurrentMap;
use portal2_world::collision::{Collider, Physics, mask};

/// Game seconds to let a map start before heading for the exit.
const SETTLE: f64 = 4.0;
/// Game seconds in the elevator's door trigger (the door opens and the
/// entrance clip goes away).
const APPROACH: f64 = 4.0;
/// Game seconds the level change may take once the exit is entered.
const RIDE_LIMIT: f64 = 120.0;

pub struct ChainPlugin;

impl Plugin for ChainPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup)
            .add_systems(PreUpdate, run.after(TransitionSet))
            .add_systems(FixedLast, watch_level_end.after(IoSet))
            .add_systems(Update, watch_deaths);
    }
}

#[derive(Clone, Copy, Debug)]
enum Stage {
    /// Waiting for the map to load.
    Loading,
    /// Letting the map start until the given time.
    Settling(f64),
    /// In the elevator's door trigger until the given time.
    Approaching(f64),
    /// In the exit trigger since `since`; the level must change by `deadline`.
    Riding { since: f64, deadline: f64, forced: bool },
}

#[derive(Default)]
struct Row {
    map: String,
    exit: &'static str,
    arrival: Option<f32>,
    ride: Option<f64>,
    next: Option<String>,
    notes: Vec<String>,
}

#[derive(Resource)]
struct Chain {
    map: String,
    left: u32,
    stage: Stage,
    row: Row,
    rows: Vec<Row>,
    exit: Option<Entity>,
    /// Game time of the last tick, and of the tick that asked for the level
    /// change.
    last_time: f64,
    ended_at: Option<f64>,
    /// The player died on the current map.
    died: bool,
}

fn setup(mut commands: Commands, args: Res<Args>, mut time: ResMut<Time<Virtual>>) {
    if args.chain.is_none() {
        return;
    }
    time.set_relative_speed(args.chain_speed.max(0.1));
    info!("chain run from {} ({} maps, {}x speed)", args.map, args.chain_count, args.chain_speed);
    commands.insert_resource(Chain {
        map: args.map.clone(),
        left: args.chain_count.max(1),
        stage: Stage::Loading,
        row: Row { map: args.map.clone(), ..default() },
        rows: Vec::new(),
        exit: None,
        last_time: 0.0,
        ended_at: None,
        died: false,
    });
}

/// Notes the tick in which a level change was requested.
fn watch_level_end(clock: Res<GameClock>, pending: Option<Res<PendingTransition>>, chain: Option<ResMut<Chain>>) {
    let Some(mut chain) = chain else { return };
    chain.last_time = clock.curtime;
    if pending.is_some() && chain.ended_at.is_none() && matches!(chain.stage, Stage::Riding { .. }) {
        chain.ended_at = Some(clock.curtime);
    }
}

fn watch_deaths(mut deaths: MessageReader<portal2_game::entities::triggers::PlayerDied>, chain: Option<ResMut<Chain>>) {
    if deaths.read().count() > 0
        && let Some(mut chain) = chain
    {
        chain.died = true;
    }
}

/// Ends the level the way the scripts do, to keep the run going.
fn force_level_end(world: &mut World) {
    if let Some(script) = world.resource::<EntityRegistry>().by_name.get("@transition_script").and_then(|l| l.first()).copied() {
        portal2_game::script::run_code(world, script, "RealTransitionFromMap()", "chain");
    }
}

fn run(world: &mut World) {
    let Some(mut chain) = world.remove_resource::<Chain>() else { return };
    let current = world.get_resource::<CurrentMap>().map(|m| m.name.clone()).unwrap_or_default();
    let now = world.resource::<GameClock>().curtime;
    // A death would reload the map; note it and leave for the next one.
    if std::mem::take(&mut chain.died) && current == chain.map && !matches!(chain.stage, Stage::Loading) {
        let at = match chain.stage {
            Stage::Riding { since, .. } => format!(" {:.1} s into the exit", now - since),
            _ => " before the exit".to_owned(),
        };
        chain.row.notes.push(format!("player died{at}; forced RealTransitionFromMap()"));
        force_level_end(world);
        let since = if let Stage::Riding { since, .. } = chain.stage { since } else { now };
        chain.stage = Stage::Riding { since, deadline: now + 10.0, forced: true };
    }
    match chain.stage {
        Stage::Loading => {
            if current == chain.map && world.get_resource::<PendingTransition>().is_none() {
                chain.stage = Stage::Settling(now + SETTLE);
            }
        }
        Stage::Settling(until) if now >= until => {
            chain.row.arrival = arrival_distance(world);
            match find_exit(world) {
                Some(Exit { kind: ExitKind::Train(train), .. }) => {
                    chain.row.exit = "elevator (scripted start)";
                    ride_train(world, train, &mut chain.row);
                    chain.ended_at = None;
                    chain.stage = Stage::Riding { since: now, deadline: now + RIDE_LIMIT, forced: false };
                }
                Some(exit) => {
                    let ExitKind::Trigger(start) = exit.kind else { unreachable!() };
                    chain.exit = Some(start);
                    chain.row.exit = exit.label;
                    match exit.door {
                        Some(door) if place_in(world, door, &mut chain.row) => chain.stage = Stage::Approaching(now + APPROACH),
                        _ => enter_exit(world, &mut chain, now),
                    }
                }
                None => {
                    chain.row.exit = "none";
                    chain.row.notes.push("no exit trigger found".to_owned());
                    finish_row(&mut chain, None);
                    return finish(world, chain);
                }
            }
        }
        Stage::Approaching(until) if now >= until => enter_exit(world, &mut chain, now),
        Stage::Riding { since, deadline, forced } => {
            if current != chain.map {
                let ride = chain.ended_at.unwrap_or(chain.last_time) - since;
                chain.row.ride = Some(ride);
                info!("chain: {} -> {current} after {ride:.2} s", chain.map);
                finish_row(&mut chain, Some(current.clone()));
                chain.left -= 1;
                if chain.left == 0 {
                    return finish(world, chain);
                }
                chain.map = current.clone();
                chain.row = Row { map: current, ..default() };
                chain.stage = Stage::Loading;
            } else if now > deadline {
                if forced {
                    chain.row.notes.push("forced transition did not change the level".to_owned());
                    finish_row(&mut chain, None);
                    return finish(world, chain);
                }
                // Keep the run going: end the level the way the scripts do.
                chain.row.notes.push(format!("no level change within {RIDE_LIMIT} s; forced RealTransitionFromMap()"));
                force_level_end(world);
                chain.stage = Stage::Riding { since, deadline: now + 10.0, forced: true };
            }
        }
        _ => {}
    }
    world.insert_resource(chain);
}

fn finish_row(chain: &mut Chain, next: Option<String>) {
    let mut row = std::mem::take(&mut chain.row);
    row.next = next;
    chain.rows.push(row);
    chain.ended_at = None;
    chain.exit = None;
}

fn enter_exit(world: &mut World, chain: &mut Chain, now: f64) {
    let Some(exit) = chain.exit else { return };
    if let Some(mut t) = world.get_mut::<Trigger>(exit)
        && !t.enabled
    {
        t.enabled = true;
        chain.row.notes.push("exit trigger was disabled".to_owned());
    }
    place_in(world, exit, &mut chain.row);
    chain.ended_at = None;
    chain.stage = Stage::Riding { since: now, deadline: now + RIDE_LIMIT, forced: false };
}

enum ExitKind {
    /// A trigger to put the player in.
    Trigger(Entity),
    /// An exit elevator whose ride the map's logic starts.
    Train(Entity),
}

struct Exit {
    label: &'static str,
    kind: ExitKind,
    door: Option<Entity>,
}

/// The trigger that starts the exit elevator (it runs `StartMoving()`) and
/// the elevator's door trigger; else the elevator another entity starts;
/// else the trigger that runs `TransitionFromMap()`.
fn find_exit(world: &mut World) -> Option<Exit> {
    let mut q = world.query::<(Entity, &MapEntity, &Outputs)>();
    let mut start: Option<(usize, Entity)> = None;
    let mut door: Option<(usize, Entity)> = None;
    let mut fallback: Option<(usize, Entity)> = None;
    let mut train_name: Option<(usize, String)> = None;
    for (e, me, outputs) in q.iter(world) {
        let first = |slot: &mut Option<(usize, Entity)>| {
            if slot.is_none_or(|(i, _)| me.index < i) {
                *slot = Some((me.index, e));
            }
        };
        let starts = outputs.connections.iter().find(|c| c.param.trim().eq_ignore_ascii_case("StartMoving()"));
        if !me.classname.starts_with("trigger_") {
            if let Some(c) = starts
                && train_name.as_ref().is_none_or(|(i, _)| me.index < *i)
            {
                train_name = Some((me.index, c.target.to_ascii_lowercase()));
            }
            continue;
        }
        if starts.is_some() {
            first(&mut start);
        }
        if outputs.connections.iter().any(|c| c.param.to_ascii_lowercase().contains("transitionfrommap()")) {
            first(&mut fallback);
        }
        if me.targetname.as_deref().is_some_and(|n| n.ends_with("elevator_door_open_trigger")) {
            first(&mut door);
        }
    }
    let train = train_name.and_then(|(_, n)| world.resource::<EntityRegistry>().by_name.get(&n).and_then(|l| l.first()).copied());
    match (start, train, fallback) {
        (Some((_, s)), _, _) => Some(Exit { label: "elevator", kind: ExitKind::Trigger(s), door: door.map(|(_, d)| d) }),
        (None, Some(t), _) => Some(Exit { label: "elevator (scripted start)", kind: ExitKind::Train(t), door: None }),
        (None, None, Some((_, f))) => Some(Exit { label: "transition trigger", kind: ExitKind::Trigger(f), door: None }),
        _ => None,
    }
}

/// Puts the player on the exit elevator's floor (its player teleport
/// destination when it has one) and starts the ride as the map would.
fn ride_train(world: &mut World, train: Entity, row: &mut Row) {
    let Some(player) = world.resource::<EntityRegistry>().player else { return };
    let name = world.get::<MapEntity>(train).and_then(|m| m.targetname.clone()).unwrap_or_default();
    let dest = world.resource::<EntityRegistry>().by_name.get(&format!("{name}_player_teleport_dest")).and_then(|l| l.first()).copied();
    let feet = match dest {
        Some(d) => world_transform(world, d).translation(),
        None => {
            row.notes.push("no player teleport destination; placed below the train origin".to_owned());
            world_transform(world, train).translation() - Vec3::Z * 59.0
        }
    };
    portal2_game::entities::teleport_entity(world, player, feet, None, Some(Vec3::ZERO));
    portal2_game::script::run_code(world, train, "StartMoving()", "chain");
}

/// Puts the player on the floor inside a trigger volume.
fn place_in(world: &mut World, trigger: Entity, row: &mut Row) -> bool {
    let Some(player) = world.resource::<EntityRegistry>().player else { return false };
    let Some((lo, hi)) = world
        .get::<Collider>(trigger)
        .zip(world.get::<GlobalTransform>(trigger))
        .map(|(c, gt)| portal2_game::physics::world_bounds(&c.shape, gt.translation(), gt.rotation()))
    else {
        row.notes.push("exit trigger has no volume".to_owned());
        return false;
    };
    let center = (lo + hi) * 0.5;
    let start = Vec3::new(center.x, center.y, center.z);
    let feet = {
        let mut state: SystemState<Physics> = SystemState::new(world);
        let Ok(physics) = state.get(world) else { return false };
        let tr = physics.trace(start, start - Vec3::Z * (hi.z - lo.z + 256.0), HULL_MINS, HULL_MAXS, mask::PLAYER_SOLID, &[player]);
        if tr.start_solid { start } else { tr.end + Vec3::Z * 0.1 }
    };
    if let Some(mut p) = world.get_mut::<Player>(player) {
        p.state.origin = feet;
        p.state.velocity = Vec3::ZERO;
        p.previous_origin = feet;
        p.ground_ref = None;
        p.noclip = false;
    }
    if let Some(mut t) = world.get_mut::<Transform>(player) {
        t.translation = feet;
    }
    true
}

/// Distance from the player to the arrival elevator, when the map has one.
fn arrival_distance(world: &World) -> Option<f32> {
    let registry = world.resource::<EntityRegistry>();
    let player = registry.player?;
    let elevator = *registry.by_name.get("arrival_elevator-elevator_1")?.first()?;
    let state = world.get::<Player>(player)?.state;
    let at = world_transform(world, elevator).translation();
    info!("arrival: player {:?} (ground {:?}, on ground {}), elevator {at:?}", state.origin, state.ground_entity, state.on_ground);
    Some(at.distance(state.origin))
}

fn finish(world: &mut World, chain: Chain) {
    let mut out = String::new();
    let _ = writeln!(out, "map | exit | ride (game s) | next | arrival distance | notes");
    for r in &chain.rows {
        let _ = writeln!(
            out,
            "{} | {} | {} | {} | {} | {}",
            r.map,
            r.exit,
            r.ride.map(|t| format!("{t:.2}")).unwrap_or_else(|| "-".into()),
            r.next.as_deref().unwrap_or("-"),
            r.arrival.map(|d| format!("{d:.0}")).unwrap_or_else(|| "-".into()),
            r.notes.join("; "),
        );
    }
    let changed = chain.rows.iter().filter(|r| r.next.is_some()).count();
    let _ = writeln!(out, "\n{} maps, {changed} level changes", chain.rows.len());
    let _ = std::fs::create_dir_all("re/out");
    let _ = std::fs::write("re/out/chain.txt", &out);
    info!("chain report written to re/out/chain.txt ({changed} level changes)");
    world.write_message(AppExit::Success);
}
