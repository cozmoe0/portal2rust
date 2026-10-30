//! `logic_choreographed_scene`: plays a choreographed scene (`SceneFile`),
//! also what `CreateSceneEntity` makes for scripts.
//!
//! Scenes come from the compiled `scenes/scenes.image` (the only place the
//! server reads them from), with the text `.vcd` as a fallback. Once
//! started, scene time runs with the game clock, scaled by `PitchShift`.
//! Each event fires when the clock passes its start: speak events play
//! their sound on the actor, fire-trigger events fire `OnTrigger<n>`.
//! The scene completes (`OnCompletion`) at its last event's end; `Cancel`
//! stops its speech and fires `OnCanceled`. Actors are found by name;
//! `!target1`..`!target8` use the entity's `target1`..`target8` keys (or
//! `SetTarget<n>`), and `!player`, `!activator` and `!self` resolve as
//! usual. Sequence events start the named sequence on the actor; gestures,
//! look-at, face, move-to and flex animation are not played yet.

use std::collections::HashMap;
use std::sync::Arc;

use bevy::prelude::*;
use source_formats::scenes_image::ScenesImage;
use source_formats::vcd::{EventKind, Scene};

use super::io::{InputCall, InputHandlers, fire_output};
use super::{EntityRegistry, MapEntity, SpawnHandlers};
use portal2_core::clock::GameClock;
use portal2_core::sets::ThinkSet;
use portal2_core::vfs::GameFs;

pub fn register(app: &mut App) {
    app.init_resource::<SceneLibrary>().add_systems(FixedUpdate, run_scenes.in_set(ThinkSet));
    let world = app.world_mut();
    world.resource_mut::<SpawnHandlers>().register("logic_choreographed_scene", spawn);
    world.resource_mut::<InputHandlers>().register("logic_choreographed_scene", input);
}

pub struct LoadedScene {
    /// Events in timeline order, with the actor each belongs to.
    events: Vec<(Option<String>, source_formats::vcd::Event)>,
    pub duration: f32,
}

#[derive(Resource, Default)]
pub struct SceneLibrary {
    image: Option<Option<ScenesImage>>,
    cache: HashMap<String, Option<Arc<LoadedScene>>>,
}

impl SceneLibrary {
    pub fn load(&mut self, fs: &GameFs, file: &str) -> Option<Arc<LoadedScene>> {
        let key = file.to_ascii_lowercase().replace('\\', "/");
        if let Some(s) = self.cache.get(&key) {
            return s.clone();
        }
        let image = self.image.get_or_insert_with(|| ScenesImage::from_fs(&fs.0).and_then(Result::ok));
        let scene: Option<Scene> = match image.as_ref().and_then(|i| i.load(&key)) {
            Some(Ok(s)) => Some(s),
            _ => {
                let path = source_formats::scenes_image::scene_key(&key).replace('\\', "/");
                fs.read_string(&path).and_then(|t| Scene::parse(&t).ok())
            }
        };
        if scene.is_none() {
            warn!("scene {file} not found");
        }
        let loaded = scene.map(|s| {
            let duration = s.end_time();
            let mut events: Vec<(Option<String>, source_formats::vcd::Event)> = s.events.iter().map(|e| (None, e.clone())).collect();
            for a in s.actors.iter().filter(|a| a.active) {
                for c in a.channels.iter().filter(|c| c.active) {
                    events.extend(c.events.iter().filter(|e| e.flags.active).map(|e| (Some(a.name.clone()), e.clone())));
                }
            }
            events.sort_by(|a, b| a.1.start.total_cmp(&b.1.start));
            Arc::new(LoadedScene { events, duration })
        });
        self.cache.insert(key, loaded.clone());
        loaded
    }
}

#[derive(Component, serde::Serialize, serde::Deserialize)]
pub struct ChoreoScene {
    file: String,
    targets: [Option<String>; 8],
    #[serde(skip)]
    loaded: Option<Arc<LoadedScene>>,
    pub playing: bool,
    pub paused: bool,
    time: f32,
    pitch: f32,
    next: usize,
    /// Game time the scene last started (orders an actor's scenes).
    #[serde(default)]
    started: f64,
    #[serde(with = "portal2_core::save::ids::opt_entity")]
    activator: Option<Entity>,
    /// Sound channels started by this scene's speak events.
    #[serde(skip)]
    speech: Vec<u64>,
    /// Actors this scene layers gestures on.
    #[serde(with = "portal2_core::save::ids::entities")]
    layered: Vec<Entity>,
}

fn spawn(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e).cloned() else { return };
    let targets = std::array::from_fn(|i| me.get(&format!("target{}", i + 1)).filter(|s| !s.is_empty()).map(str::to_owned));
    world.entity_mut(e).insert(ChoreoScene {
        file: me.get("scenefile").unwrap_or("").to_owned(),
        targets,
        loaded: None,
        playing: false,
        paused: false,
        time: 0.0,
        pitch: 1.0,
        next: 0,
        started: 0.0,
        activator: None,
        speech: Vec::new(),
        layered: Vec::new(),
    });
}

/// The loaded scene of a scene entity (loading it on first use).
pub fn loaded(world: &mut World, e: Entity) -> Option<Arc<LoadedScene>> {
    let s = world.get::<ChoreoScene>(e)?;
    if let Some(l) = &s.loaded {
        return Some(l.clone());
    }
    let file = s.file.clone();
    let fs = world.get_resource::<GameFs>()?.clone();
    let l = world.resource_mut::<SceneLibrary>().load(&fs, &file)?;
    world.get_mut::<ChoreoScene>(e)?.loaded = Some(l.clone());
    Some(l)
}

fn input(world: &mut World, call: &InputCall) -> bool {
    let e = call.target;
    let Some(s) = world.get::<ChoreoScene>(e) else { return false };
    let playing = s.playing;
    match call.input.as_str() {
        "start" => {
            if playing {
                return true;
            }
            if loaded(world, e).is_none() {
                return true;
            }
            let now = world.resource::<GameClock>().curtime;
            if let Some(mut s) = world.get_mut::<ChoreoScene>(e) {
                s.started = now;
                s.playing = true;
                s.paused = false;
                s.time = 0.0;
                s.next = 0;
                s.activator = call.activator;
                s.speech.clear();
            }
            debug!("scene {} started", world.get::<ChoreoScene>(e).map_or("", |s| s.file.as_str()));
            fire_output(world, e, "OnStart", call.activator, None);
            // Events at time zero fire right away.
            advance(world, e, 0.0);
        }
        "pause" => {
            if let Some(mut s) = world.get_mut::<ChoreoScene>(e) {
                s.paused = true;
            }
        }
        "resume" => {
            if let Some(mut s) = world.get_mut::<ChoreoScene>(e) {
                s.paused = false;
            }
        }
        "cancel" => {
            if playing {
                stop_speech(world, e);
                update_gestures(world, e, None);
                if let Some(mut s) = world.get_mut::<ChoreoScene>(e) {
                    s.playing = false;
                }
                let activator = world.get::<ChoreoScene>(e).and_then(|s| s.activator);
                fire_output(world, e, "OnCanceled", activator, None);
            }
        }
        "pitchshift" => {
            let p = source_formats::kv::parse_f32_lenient(&call.param).clamp(0.25, 4.0);
            let speech = match world.get_mut::<ChoreoScene>(e) {
                Some(mut s) => {
                    s.pitch = p;
                    s.speech.clone()
                }
                None => Vec::new(),
            };
            for id in speech {
                portal2_audio::adjust(world, id, None, Some(p * 100.0));
            }
        }
        "reload" => {
            if let Some(mut s) = world.get_mut::<ChoreoScene>(e) {
                s.loaded = None;
            }
        }
        "trigger" => {
            let n = source_formats::kv::parse_i32_lenient(&call.param);
            if (1..=16).contains(&n) {
                fire_output(world, e, &format!("OnTrigger{n}"), call.activator, None);
            }
        }
        input if input.starts_with("settarget") => {
            let Some(i) = input["settarget".len()..].parse::<usize>().ok().filter(|i| (1..=8).contains(i)) else { return false };
            let name = call.param.trim().to_owned();
            if let Some(mut s) = world.get_mut::<ChoreoScene>(e) {
                s.targets[i - 1] = (!name.is_empty()).then_some(name);
            }
        }
        "cancelatnextinterrupt"
        | "interjectresponse"
        | "stopwaitingforactor"
        | "startwaitingforactor"
        | "scriptplayerdeath"
        | "setbreakpoint" => {}
        _ => return false,
    }
    true
}

fn stop_speech(world: &mut World, e: Entity) {
    let speech = world.get_mut::<ChoreoScene>(e).map(|mut s| std::mem::take(&mut s.speech)).unwrap_or_default();
    for id in speech {
        portal2_audio::stop_channel(world, id);
    }
}

/// Finds an actor named in a scene.
fn actor(world: &World, e: Entity, name: &str) -> Option<Entity> {
    let s = world.get::<ChoreoScene>(e)?;
    let lower = name.to_ascii_lowercase();
    let registry = world.resource::<EntityRegistry>();
    let by_name = |n: &str| registry.by_name.get(&n.to_ascii_lowercase()).and_then(|l| l.first()).copied();
    match lower.as_str() {
        "!player" => registry.player,
        "!self" | "!caller" => Some(e),
        "!activator" => s.activator,
        _ if lower.starts_with("!target") => {
            let i: usize = lower["!target".len()..].parse().ok()?;
            s.targets.get(i.checked_sub(1)?)?.as_deref().and_then(by_name)
        }
        _ => by_name(&lower),
    }
}

/// The playing scenes in which `who` has an event under way, oldest first
/// (what `GetCurrentScene` and `GetSceneByIndex` answer from).
pub fn active_scenes(world: &mut World, who: Entity) -> Vec<Entity> {
    let mut playing: Vec<(f64, Entity)> =
        world.query::<(Entity, &ChoreoScene)>().iter(world).filter(|(_, s)| s.playing).map(|(e, s)| (s.started, e)).collect();
    playing.sort_by(|a, b| a.0.total_cmp(&b.0));
    playing
        .into_iter()
        .filter(|&(_, e)| {
            let (Some(scene), Some(time)) = (loaded(world, e), world.get::<ChoreoScene>(e).map(|s| s.time)) else { return false };
            scene.events.iter().any(|(name, ev)| {
                ev.start <= time && time < ev.end.max(ev.start) && name.as_deref().and_then(|n| actor(world, e, n)) == Some(who)
            })
        })
        .map(|(_, e)| e)
        .collect()
}

/// Plays a speak event's line on `who`, `offset` seconds in, with its
/// caption for the rest of the line.
fn speak(world: &mut World, e: Entity, who: Entity, ev: &source_formats::vcd::Event, pitch: f32, offset: f32) {
    let level = ev.param2.trim().trim_end_matches(|c: char| c.is_ascii_alphabetic()).parse::<f32>().ok();
    let params = portal2_audio::Emit {
        entity: Some(who),
        pitch: Some(pitch * 100.0),
        soundlevel: level,
        no_caption: true,
        // A negative delay starts part-way into the sound.
        delay: -offset,
        ..default()
    };
    if let Some(id) = portal2_audio::emit(world, &ev.param, params)
        && let Some(mut s) = world.get_mut::<ChoreoScene>(e)
    {
        s.speech.push(id);
    }
    // The scene captions its own lines (a token may replace the sound's).
    if ev.cc.kind == source_formats::vcd::CcType::Master {
        let token = if ev.cc.token.is_empty() { ev.param.as_str() } else { ev.cc.token.as_str() };
        portal2_ui::captions::emit(world, token, (ev.duration() - offset).max(0.0) / pitch.max(0.01), false);
    }
}

/// After a saved game is restored: lines that were being spoken carry on
/// from where they were.
pub fn resume_speech(world: &mut World) {
    let playing: Vec<Entity> = world.query::<(Entity, &ChoreoScene)>().iter(world).filter(|(_, s)| s.playing).map(|(e, _)| e).collect();
    for e in playing {
        let Some(scene) = loaded(world, e) else { continue };
        let Some((time, pitch)) = world.get::<ChoreoScene>(e).map(|s| (s.time, s.pitch)) else { continue };
        for (actor_name, ev) in &scene.events {
            if ev.kind != EventKind::Speak || ev.start > time || time >= ev.end {
                continue;
            }
            if let Some(who) = actor_name.as_deref().and_then(|n| actor(world, e, n)) {
                speak(world, e, who, ev, pitch, time - ev.start);
            }
        }
    }
}

/// Fires every event whose start the scene time has passed.
fn advance(world: &mut World, e: Entity, dt: f32) {
    // Loaded on first use, and again after a saved game is restored.
    if loaded(world, e).is_none() {
        return;
    }
    let Some(s) = world.get_mut::<ChoreoScene>(e) else { return };
    let Some(scene) = s.loaded.clone() else { return };
    let mut s = s;
    s.time += dt * s.pitch;
    let (time, mut next, pitch, activator) = (s.time, s.next, s.pitch, s.activator);
    while let Some((actor_name, ev)) = scene.events.get(next) {
        if ev.start > time {
            break;
        }
        next += 1;
        match ev.kind {
            EventKind::Speak => {
                let who = actor_name.as_deref().and_then(|n| actor(world, e, n));
                let Some(who) = who else {
                    debug!("scene actor {actor_name:?} not found for {}", ev.param);
                    continue;
                };
                speak(world, e, who, ev, pitch, 0.0);
            }
            EventKind::Sequence => {
                // The actor plays the named sequence (blending, gestures and
                // returning to idle afterwards are not done yet).
                if let Some(who) = actor_name.as_deref().and_then(|n| actor(world, e, n))
                    && let Some(mut anim) = world.get_mut::<portal2_render::models::StudioAnimator>(who)
                    && !anim.set_sequence(&ev.param)
                {
                    debug!("scene sequence {} missing on actor", ev.param);
                }
            }
            EventKind::Gesture => {
                // Layered every tick by update_gestures; noted here for tracing.
                let found = actor_name
                    .as_deref()
                    .and_then(|n| actor(world, e, n))
                    .and_then(|w| world.get::<portal2_render::models::StudioAnimator>(w))
                    .map(|a| a.model.studio.find_sequence(&ev.param).is_some());
                debug!("scene gesture {} on {actor_name:?}: sequence {found:?}", ev.param);
            }
            EventKind::FireTrigger => {
                let n = source_formats::kv::parse_i32_lenient(&ev.param);
                if (1..=16).contains(&n) {
                    fire_output(world, e, &format!("OnTrigger{n}"), activator, None);
                }
            }
            _ => {}
        }
    }
    let Some(mut s) = world.get_mut::<ChoreoScene>(e) else { return };
    s.next = next;
    let done = s.playing && next >= scene.events.len() && s.time >= scene.duration;
    update_gestures(world, e, (!done).then_some((&scene, time)));
    let Some(mut s) = world.get_mut::<ChoreoScene>(e) else { return };
    if done {
        s.playing = false;
        let file = s.file.clone();
        debug!("scene {file} completed");
        fire_output(world, e, "OnCompletion", activator, None);
    }
}

/// Intensity of an event `local` seconds after its start: its ramp when it
/// has one, otherwise the sequence's fade in and out at the edges.
fn event_weight(ev: &source_formats::vcd::Event, local: f32, fades: (f32, f32)) -> f32 {
    let s = &ev.ramp.samples;
    if let (Some(first), Some(last)) = (s.first(), s.last()) {
        if local <= first.time {
            return first.value;
        }
        if local >= last.time {
            return last.value;
        }
        let i = s.partition_point(|x| x.time <= local).max(1);
        let (a, b) = (s[i - 1], s[i]);
        let t = if b.time > a.time { (local - a.time) / (b.time - a.time) } else { 1.0 };
        return a.value + (b.value - a.value) * t;
    }
    let fade_in = if fades.0 > 0.0 { local / fades.0 } else { 1.0 };
    let fade_out = if fades.1 > 0.0 { (ev.duration() - local) / fades.1 } else { 1.0 };
    fade_in.min(fade_out).clamp(0.0, 1.0)
}

/// Layers every gesture event spanning the scene time onto its actor; the
/// gesture's sequence plays over the event's length. `None` clears them.
fn update_gestures(world: &mut World, e: Entity, at: Option<(&LoadedScene, f32)>) {
    use portal2_render::models::animation::Layer;
    let mut layers: Vec<(Entity, Layer)> = Vec::new();
    if let Some((scene, time)) = at {
        for (actor_name, ev) in &scene.events {
            if ev.kind != EventKind::Gesture || ev.start > time || time >= ev.end {
                continue;
            }
            let Some(who) = actor_name.as_deref().and_then(|n| actor(world, e, n)) else { continue };
            let Some(anim) = world.get::<portal2_render::models::StudioAnimator>(who) else { continue };
            let Some(sequence) = anim.model.studio.find_sequence(&ev.param) else { continue };
            let local = time - ev.start;
            let cycle = (local / ev.duration().max(0.01)).clamp(0.0, 1.0);
            let weight = event_weight(ev, local, anim.sequence_fades(sequence));
            layers.push((who, Layer { sequence, cycle, weight }));
        }
    }
    let previous = world.get_mut::<ChoreoScene>(e).map(|mut s| std::mem::take(&mut s.layered)).unwrap_or_default();
    let mut touched: Vec<Entity> = layers.iter().map(|(w, _)| *w).collect();
    touched.dedup();
    for who in previous.iter().chain(touched.iter()) {
        let mine: Vec<Layer> = layers.iter().filter(|(w, _)| w == who).map(|(_, l)| *l).collect();
        if let Some(mut anim) = world.get_mut::<portal2_render::models::StudioAnimator>(*who) {
            anim.set_layers(mine);
        }
    }
    if let Some(mut s) = world.get_mut::<ChoreoScene>(e) {
        s.layered = touched;
    }
}

fn run_scenes(world: &mut World) {
    let dt = world.resource::<GameClock>().frametime();
    let playing: Vec<Entity> =
        world.query::<(Entity, &ChoreoScene)>().iter(world).filter(|(_, s)| s.playing && !s.paused).map(|(e, _)| e).collect();
    for e in playing {
        advance(world, e, dt);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use source_formats::fs::GameFileSystem;

    /// Loads installed scenes through the image (skipped without an install).
    #[test]
    fn loads_scenes_from_the_image() {
        let Some(root) = GameFileSystem::find_install() else { return };
        let Ok(fs) = GameFileSystem::portal2(root) else { return };
        let fs = GameFs(Arc::new(fs));
        let mut lib = SceneLibrary::default();
        let s = lib.load(&fs, "scenes/npc/announcer/prehub46.vcd").expect("scene");
        let speak: Vec<&str> = s.events.iter().filter(|(_, e)| e.kind == EventKind::Speak).map(|(_, e)| e.param.as_str()).collect();
        println!("prehub46: {:.2} s, speak {speak:?}", s.duration);
        assert!(speak.iter().any(|p| p.eq_ignore_ascii_case("announcer.prehub46")));
        assert!((s.duration - 9.5).abs() < 0.5, "{}", s.duration);
        // Keys are case- and slash-insensitive, the extension optional.
        assert!(lib.load(&fs, r"Scenes\NPC\announcer\prehub46").is_some());
    }
}

#[cfg(test)]
mod census {
    use super::*;
    use source_formats::fs::GameFileSystem;

    /// Which actors carry look-at, face and gesture events (run with
    /// `--ignored --nocapture`).
    #[test]
    #[ignore]
    fn animation_event_actors() {
        let Some(root) = GameFileSystem::find_install() else { return };
        let Ok(fs) = GameFileSystem::portal2(root) else { return };
        let Some(Ok(image)) = ScenesImage::from_fs(&fs) else { return };
        let mut counts: HashMap<(String, String), usize> = HashMap::new();
        for i in 0..image.len() {
            let Ok(scene) = image.scene(i) else { continue };
            for a in &scene.actors {
                for c in &a.channels {
                    for e in &c.events {
                        if matches!(e.kind, EventKind::LookAt | EventKind::Face | EventKind::Gesture | EventKind::Sequence) {
                            *counts.entry((a.name.to_ascii_lowercase(), e.kind.name().to_owned())).or_default() += 1;
                        }
                    }
                }
            }
        }
        let mut v: Vec<_> = counts.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        for ((actor, kind), n) in v.iter().take(30) {
            println!("{n:>6} {kind:<10} {actor}");
        }
    }
}

impl portal2_core::save::Restore for ChoreoScene {
    fn keep(&mut self, live: Self) {
        self.loaded = live.loaded;
    }
}

pub fn register_save(r: &mut portal2_core::save::SaveRegistry) {
    r.component::<ChoreoScene>("choreo_scene");
}

/// Scene file, whether it plays and its time (for `--save-info`).
pub fn describe(s: &ChoreoScene) -> (String, bool, f32) {
    (s.file.clone(), s.playing, s.time)
}
