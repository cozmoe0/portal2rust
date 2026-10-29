//! Soundscapes: the background sound of an area. Each `env_soundscape`
//! names a soundscape from `scripts/soundscapes_*.txt`; the player hears the
//! nearest enabled one within its radius (-1 = any distance) that has a clear
//! line to the player, and keeps the current one while still in range. On a
//! change the new soundscape's looping sounds fade in over its `fadetime`
//! while the old ones fade out, its random sounds start their timers, and its
//! `soundmixer` (or `Default_Mix`) becomes the active mixer.

use bevy::prelude::*;
use source_formats::soundscape::{Position, Soundscapes};
use source_formats::soundscript::{Interval, WaveName};

use super::{Emit, Sounds, ops};
use crate::collision::Physics;
use crate::entities::io::{InputCall, InputHandlers};
use crate::entities::{EntityRegistry, MapEntity, SpawnHandlers};
use crate::world::CurrentMap;

/// Seconds between soundscape choices.
const CHECK_PERIOD: f64 = 0.25;
const DEFAULT_FADE: f32 = 3.0;
/// How far from the listener a `random` position lands.
const RANDOM_DISTANCE: (f32, f32) = (180.0, 600.0);

pub fn register(app: &mut App) {
    app.init_resource::<SoundscapeState>();
    let world = app.world_mut();
    {
        let mut s = world.resource_mut::<SpawnHandlers>();
        for class in ["env_soundscape", "env_soundscape_triggerable", "env_soundscape_proxy"] {
            s.register(class, spawn);
        }
    }
    let mut h = world.resource_mut::<InputHandlers>();
    for class in ["env_soundscape", "env_soundscape_triggerable", "env_soundscape_proxy"] {
        h.register(class, input);
    }
}

#[derive(Component, Clone, serde::Serialize, serde::Deserialize)]
pub struct SoundscapeEnt {
    name: String,
    radius: f32,
    positions: Vec<Option<String>>,
    enabled: bool,
}

fn spawn(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e).cloned() else { return };
    let mut name = me.get("soundscape").unwrap_or("").to_owned();
    if me.classname == "env_soundscape_proxy"
        && let Some(main) = me.get("mainsoundscapename")
        && let Some(&m) = world.resource::<EntityRegistry>().by_name.get(&main.to_ascii_lowercase()).and_then(|l| l.first())
        && let Some(n) = world.get::<MapEntity>(m).and_then(|m| m.get("soundscape").map(str::to_owned))
    {
        name = n;
    }
    let positions = (0..8).map(|i| me.get(&format!("position{i}")).filter(|s| !s.is_empty()).map(str::to_ascii_lowercase)).collect();
    world.entity_mut(e).insert(SoundscapeEnt {
        name,
        radius: me.get_f32("radius").unwrap_or(128.0),
        positions,
        enabled: me.get_f32("startdisabled").unwrap_or(0.0) == 0.0,
    });
}

fn input(world: &mut World, call: &InputCall) -> bool {
    let Some(mut s) = world.get_mut::<SoundscapeEnt>(call.target) else { return false };
    match call.input.as_str() {
        "enable" => s.enabled = true,
        "disable" => s.enabled = false,
        "toggleenabled" => s.enabled = !s.enabled,
        _ => return false,
    }
    true
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Place {
    /// No spatialisation.
    Ambient,
    At(Vec3),
    Random,
}

struct Loop {
    wave: WaveName,
    volume: f32,
    pitch: f32,
    level: f32,
    place: Place,
}

struct RandomSound {
    waves: Vec<WaveName>,
    time: Interval,
    volume: Interval,
    pitch: Interval,
    level: Interval,
    place: Place,
    next: f64,
}

struct Playing {
    wave: String,
    place: Place,
    channel: u64,
    volume: f32,
    fade: f32,
    target: f32,
    rate: f32,
}

#[derive(Resource, Default)]
pub struct SoundscapeState {
    map: String,
    current: Option<Entity>,
    distance: f32,
    in_range: bool,
    loops: Vec<Playing>,
    randoms: Vec<RandomSound>,
    next_check: f64,
}

struct Ctx<'a> {
    scapes: &'a Soundscapes,
    positions: &'a [Option<Vec3>],
}

/// Flattens a soundscape and its nested `playsoundscape`s.
#[allow(clippy::too_many_arguments)]
fn flatten(
    ctx: &Ctx,
    name: &str,
    volume: f32,
    offset: i32,
    override_pos: Option<i32>,
    ambient: Option<Place>,
    depth: usize,
    loops: &mut Vec<Loop>,
    randoms: &mut Vec<RandomSound>,
) {
    let Some(s) = ctx.scapes.get(name) else { return };
    if depth >= source_formats::soundscape::MAX_NESTING {
        return;
    }
    let place_of = |pos: Option<Position>, origin: Option<Vec3>| -> Place {
        if let Some(o) = override_pos {
            return index_place(ctx, o);
        }
        match (pos, origin) {
            (Some(Position::Random), _) => Place::Random,
            (Some(Position::Index(i)), _) => index_place(ctx, i + offset),
            (None, Some(o)) => Place::At(o),
            (None, None) => ambient.unwrap_or(Place::Ambient),
        }
    };
    for l in &s.looping {
        let Some(wave) = l.wave.clone() else { continue };
        let Some(v) = l.volume else { continue };
        let v = ops::random_range(v.start, v.end) * volume;
        if v <= 0.0 {
            continue;
        }
        let pitch = l.pitch.map_or(100.0, |p| ops::random_range(p.start, p.end));
        let level = l.soundlevel.map_or(source_formats::soundscape::LOOPING_DEFAULT_SOUNDLEVEL, |p| ops::random_range(p.start, p.end));
        let place = place_of(l.position, l.origin);
        loops.push(Loop { wave, volume: v, pitch, level: if place == Place::Ambient { 0.0 } else { level }, place });
    }
    for r in &s.random {
        if r.waves.is_empty() {
            continue;
        }
        let scale = |i: Option<Interval>| i.map_or(Interval::fixed(0.0), |i| Interval { start: i.start * volume, end: i.end * volume });
        randoms.push(RandomSound {
            waves: r.waves.clone(),
            time: r.time.unwrap_or(Interval::fixed(10.0)),
            volume: scale(r.volume),
            pitch: r.pitch.unwrap_or(Interval::fixed(100.0)),
            level: r.soundlevel.unwrap_or(Interval::fixed(0.0)),
            place: place_of(r.position, r.origin),
            next: 0.0,
        });
    }
    for n in &s.nested {
        let v = n.volume.map_or(1.0, |v| ops::random_range(v.start, v.end)) * volume;
        let over = override_pos.or(n.position_override);
        let amb = match (n.ambient_position_override, n.ambient_origin_override) {
            (Some(i), _) => Some(index_place(ctx, i)),
            (None, Some(o)) => Some(Place::At(o)),
            _ => ambient,
        };
        flatten(ctx, &n.name, v, offset + n.position.unwrap_or(0), over, amb, depth + 1, loops, randoms);
    }
}

fn index_place(ctx: &Ctx, i: i32) -> Place {
    match usize::try_from(i).ok().and_then(|i| ctx.positions.get(i).copied().flatten()) {
        Some(p) => Place::At(p),
        None => Place::Ambient,
    }
}

fn wave_key(w: &WaveName) -> String {
    w.raw.to_ascii_lowercase()
}

fn start_loop(world: &mut World, l: &Loop) -> Option<u64> {
    let listener = world.resource::<Sounds>().listener.origin;
    let origin = match l.place {
        Place::At(p) => p,
        _ => listener,
    };
    super::emit(
        world,
        &l.wave.raw,
        Emit { origin: Some(origin), volume: Some(l.volume), pitch: Some(l.pitch), soundlevel: Some(l.level), slot: Some(6), ..default() },
    )
}

/// Picks the player's soundscape and keeps its sounds going.
pub fn update_soundscapes(world: &mut World) {
    let Some(map) = world.get_resource::<CurrentMap>().map(|m| m.name.clone()) else { return };
    let Some(now) = world.get_resource::<Sounds>().map(|s| s.now()) else { return };
    let dt = world.resource::<Time<Real>>().delta_secs();
    let mut state = std::mem::take(&mut *world.resource_mut::<SoundscapeState>());
    if state.map != map {
        state = SoundscapeState { map, ..default() };
    }
    if now >= state.next_check {
        state.next_check = now + CHECK_PERIOD;
        choose(world, &mut state, now);
    }
    // Fades.
    for p in state.loops.iter_mut() {
        if p.fade != p.target {
            p.fade = if p.rate > 0.0 { (p.fade + p.rate * dt).min(p.target) } else { (p.fade + p.rate * dt).max(p.target) };
            super::adjust(world, p.channel, Some(p.fade), None);
        }
    }
    let dead: Vec<u64> = state.loops.iter().filter(|p| p.target <= 0.0 && p.fade <= 0.0).map(|p| p.channel).collect();
    for id in &dead {
        super::stop_channel(world, *id);
    }
    state.loops.retain(|p| !dead.contains(&p.channel));
    // Random sounds.
    let listener = world.resource::<Sounds>().listener.origin;
    for r in state.randoms.iter_mut() {
        if now < r.next {
            continue;
        }
        r.next = now + ops::random_range(r.time.start, r.time.end).max(0.05) as f64;
        let wave = &r.waves[(ops::random_range(0.0, r.waves.len() as f32) as usize).min(r.waves.len() - 1)];
        let origin = match r.place {
            Place::At(p) => p,
            Place::Random => {
                let a = ops::random_range(0.0, std::f32::consts::TAU);
                listener + Vec3::new(a.cos(), a.sin(), 0.0) * ops::random_range(RANDOM_DISTANCE.0, RANDOM_DISTANCE.1)
            }
            Place::Ambient => listener,
        };
        let level = if r.place == Place::Ambient { 0.0 } else { ops::random_range(r.level.start, r.level.end) };
        super::emit(
            world,
            &wave.raw,
            Emit {
                origin: Some(origin),
                volume: Some(ops::random_range(r.volume.start, r.volume.end)),
                pitch: Some(ops::random_range(r.pitch.start, r.pitch.end)),
                soundlevel: Some(level),
                ..default()
            },
        );
    }
    *world.resource_mut::<SoundscapeState>() = state;
}

fn choose(world: &mut World, state: &mut SoundscapeState, now: f64) {
    let Some(player) = world.resource::<EntityRegistry>().player else { return };
    let Some(ear) = world.get::<GlobalTransform>(player).map(|t| t.translation() + Vec3::Z * 64.0) else { return };
    let candidates: Vec<(Entity, Vec3, SoundscapeEnt)> =
        world.query::<(Entity, &GlobalTransform, &SoundscapeEnt)>().iter(world).map(|(e, t, s)| (e, t.translation(), s.clone())).collect();
    let clear = |world: &mut World, from: Vec3| -> bool {
        let mut sys: bevy::ecs::system::SystemState<Physics> = bevy::ecs::system::SystemState::new(world);
        let Ok(physics) = sys.get(world) else { return true };
        let t = physics.trace(from, ear, Vec3::ZERO, Vec3::ZERO, crate::collision::SOLID, &[player]);
        t.fraction >= 1.0 && !t.start_solid
    };
    let mut current = state.current;
    let mut distance = state.distance;
    let mut in_range = false;
    // The current soundscape first: it stays while in range and visible.
    if let Some(c) = current
        && let Some((_, origin, s)) = candidates.iter().find(|(e, _, _)| *e == c)
    {
        distance = origin.distance(ear);
        in_range = s.enabled && (s.radius == -1.0 || s.radius > distance) && clear(world, *origin);
        if !s.enabled {
            current = None;
        }
    }
    for (e, origin, s) in &candidates {
        if Some(*e) == current || !s.enabled {
            continue;
        }
        let range = origin.distance(ear);
        if (!in_range || range < distance) && (s.radius == -1.0 || s.radius > range) && clear(world, *origin) {
            current = Some(*e);
            distance = range;
            in_range = true;
        }
    }
    state.distance = distance;
    state.in_range = in_range;
    if current == state.current || current.is_none() {
        return;
    }
    state.current = current;
    let Some((_, _, ent)) = candidates.into_iter().find(|(e, _, _)| Some(*e) == current) else { return };
    let registry = world.resource::<EntityRegistry>();
    let targets: Vec<Option<Entity>> =
        ent.positions.iter().map(|p| p.as_ref().and_then(|n| registry.by_name.get(n).and_then(|l| l.first()).copied())).collect();
    let positions: Vec<Option<Vec3>> =
        targets.into_iter().map(|t| t.and_then(|t| world.get::<GlobalTransform>(t)).map(|g| g.translation())).collect();
    let (mut loops, mut randoms) = (Vec::new(), Vec::new());
    let (fade, mixer) = {
        let sounds = world.resource::<Sounds>();
        let scape = sounds.soundscapes.get(&ent.name);
        let fade = scape.and_then(|s| s.fadetime).unwrap_or(DEFAULT_FADE).max(0.01);
        let mixer = scape.and_then(|s| s.soundmixer.clone()).unwrap_or_else(|| "Default_Mix".to_owned());
        let ctx = Ctx { scapes: &sounds.soundscapes, positions: &positions };
        flatten(&ctx, &ent.name, 1.0, 0, None, None, 0, &mut loops, &mut randoms);
        (fade, mixer)
    };
    debug!("soundscape {} ({} loops, {} random)", ent.name, loops.len(), randoms.len());
    world.resource_mut::<Sounds>().mix.select(&mixer);
    let rate = 1.0 / fade;
    // Keep sounds the new soundscape also plays; fade the rest out.
    let mut keep = vec![false; state.loops.len()];
    for l in &loops {
        let key = wave_key(&l.wave);
        if let Some(i) = state.loops.iter().position(|p| p.wave == key && p.place == l.place && p.target > 0.0) {
            keep[i] = true;
            continue;
        }
        if let Some(id) = start_loop(world, l) {
            super::adjust(world, id, Some(0.0), None);
            state.loops.push(Playing { wave: key, place: l.place, channel: id, volume: l.volume, fade: 0.0, target: 1.0, rate });
            keep.push(true);
        }
    }
    for (p, k) in state.loops.iter_mut().zip(keep) {
        if !k {
            p.target = 0.0;
            p.rate = -rate;
        }
    }
    for r in randoms.iter_mut() {
        r.next = now + ops::random_range(r.time.start, r.time.end) as f64;
    }
    state.randoms = randoms;
}

impl crate::save::Restore for SoundscapeEnt {}
