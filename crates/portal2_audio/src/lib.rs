//! Sound: the game's sound scripts, operator stacks, mixers and soundscapes
//! drive a software mixer that plays the installed WAV and MP3 files.
//!
//! Each started sound is a channel. Channels follow their entity, run their
//! entry's update stack every frame (or the default stack for entries that
//! have none) and hand the resulting speaker gains and pitch to the mixer on
//! the audio thread. See `docs/re/audio_runtime.md`.

pub mod cache;
pub mod device;
pub mod mixer;
pub mod ops;

use portal2_core::rng::random_range;
use std::collections::HashMap;
use std::sync::Arc;

use bevy::prelude::*;
use bevy::transform::TransformSystems;
use bevy::window::PrimaryWindow;
use source_formats::soundscript::{SoundChars, SoundEntry, SoundScripts, WaveName};

use cache::{Pcm, SoundCache};
use device::{MixHandle, OutputStream, Voice};
use ops::{Chan, Env, Instance, Library, Listener, Other, Stack, StackKind};
use portal2_core::LoadedMap;
use portal2_core::args::Args;
use portal2_core::console::{CommandArgs, Console};
use portal2_core::entity::{BrushBounds, EntityRegistry, MapEntity};
use portal2_core::events::CaptionRequest;
use portal2_core::vfs::GameFs;
use portal2_core::view::MainCamera;

/// `CHAN_AUTO`, `CHAN_STATIC`: channels that never replace each other.
const CHAN_AUTO: i32 = 0;
const CHAN_VOICE: i32 = 2;
const CHAN_STATIC: i32 = 6;
const CHAN_VOICE2: i32 = 7;

/// Library stack used for entries without stacks of their own.
const DEFAULT_UPDATE: &str = "update_default";

pub struct AudioPlugin;

/// Order of the sound work after transforms propagate (`PostUpdate`).
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AudioSet {
    /// A new map stops the old map's sounds and loads its sound overrides.
    MapChange,
    /// Sound entities start the sounds they were asked to play.
    Start,
    /// [`SoundRequest`]s, then every channel's update stack and the mix.
    Mix,
}

impl Plugin for AudioPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, init_sounds)
            .add_message::<SoundRequest>()
            .configure_sets(PostUpdate, (AudioSet::MapChange, AudioSet::Start, AudioSet::Mix).chain().after(TransformSystems::Propagate))
            .add_systems(
                PostUpdate,
                (map_changed.in_set(AudioSet::MapChange), (handle_requests, update_channels).chain().in_set(AudioSet::Mix)),
            )
            .add_systems(Last, pump_web_audio)
            .add_systems(Update, write_recording);
        let mut console = app.world_mut().resource_mut::<Console>();
        for (name, default) in [
            ("volume", "1.0"),
            ("snd_musicvolume", "1.0"),
            ("snd_gain", "1"),
            ("snd_gain_max", "1"),
            ("snd_gain_min", "0.01"),
            ("snd_refdist", "36"),
            ("snd_refdb", "60"),
            ("snd_foliage_db_loss", "4"),
            ("snd_op_test_convar", "1.0"),
            ("snd_mixer_master_level", "1.0"),
            ("snd_mixer_master_dsp", "1.0"),
            ("snd_disable_mixer_solo", "0"),
            ("snd_soundmixer", "Default_Mix"),
            ("snd_surround_speakers", "-1"),
            ("snd_mute_losefocus", "1"),
            ("dsp_volume", "0.8"),
        ] {
            console.register_cvar(name, default, portal2_core::console::cvars::flags::ARCHIVE);
        }
        console.register_command("snd_list", snd_list_command);
        console.register_command("play", play_command);
        console.register_command("playvol", play_command);
    }
}

/// Development aid: lists the solid static volumes and entities the player's
/// hull overlaps.
/// Lists the playing sound channels.
/// In the browser the mix is made here, each frame, for the page's audio
/// worklet (see `device::web`); elsewhere the device's own thread mixes.
fn pump_web_audio(sounds: Option<Res<Sounds>>, time: Res<Time<Real>>) {
    #[cfg(target_arch = "wasm32")]
    if let Some(out) = sounds.as_ref().and_then(|s| s.output.as_ref()) {
        device::web::pump(out, time.elapsed_secs_f64());
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (sounds, time);
}

fn snd_list_command(world: &mut World, _args: &CommandArgs) {
    let lines = list(world);
    portal2_core::console::commands::print(world, format!("{} channels", lines.len()));
    for l in lines {
        portal2_core::console::commands::print(world, l);
    }
}

/// `play <sound>` / `playvol <sound> <volume>`: plays a sound without
/// spatialisation.
fn play_command(world: &mut World, args: &CommandArgs) {
    let name = args.arg(1).to_owned();
    if name.is_empty() {
        return;
    }
    let volume = Some(args.arg(2)).filter(|v| !v.is_empty()).map(source_formats::kv::parse_f32_lenient);
    emit(world, &name, Emit { volume, soundlevel: Some(0.0), ..Default::default() });
}

/// A sound to start (or stop) from an ordinary system; handled before the
/// channels update.
#[derive(Message, Clone)]
pub struct SoundRequest {
    pub name: String,
    pub params: Emit,
    pub stop: bool,
}

impl SoundRequest {
    pub fn at(name: &str, origin: Vec3) -> Self {
        Self { name: name.to_owned(), params: Emit { origin: Some(origin), ..default() }, stop: false }
    }

    pub fn on(name: &str, entity: Entity) -> Self {
        Self { name: name.to_owned(), params: Emit { entity: Some(entity), ..default() }, stop: false }
    }

    pub fn stop(name: &str, entity: Entity) -> Self {
        Self { name: name.to_owned(), params: Emit { entity: Some(entity), ..default() }, stop: true }
    }
}

fn handle_requests(world: &mut World) {
    let requests: Vec<SoundRequest> = world.resource_mut::<Messages<SoundRequest>>().drain().collect();
    for r in requests {
        if r.stop {
            stop(world, r.params.entity, &r.name);
        } else {
            emit(world, &r.name, r.params);
        }
    }
}

/// Parameters for starting a sound; anything unset comes from the entry.
#[derive(Clone, Default)]
pub struct Emit {
    pub entity: Option<Entity>,
    pub origin: Option<Vec3>,
    pub volume: Option<f32>,
    /// Multiplies the volume (physics impacts scale theirs with speed).
    pub volume_scale: Option<f32>,
    /// Percent, 100 = normal.
    pub pitch: Option<f32>,
    pub soundlevel: Option<f32>,
    pub slot: Option<i32>,
    /// Seconds.
    pub delay: f32,
    /// The caller shows the caption itself (scene speech).
    pub no_caption: bool,
}

/// Compiled stacks of one entry.
#[derive(Clone, Default)]
struct EntryStacks {
    start: Option<Arc<Stack>>,
    update: Option<Arc<Stack>>,
    stop: Option<Arc<Stack>>,
}

struct Channel {
    id: u64,
    entry: String,
    entity: Option<Entity>,
    origin: Vec3,
    /// Plays without spatialisation (sound level 0 on entries without stacks).
    flat: bool,
    looping: bool,
    started: f64,
    sound_start: f64,
    /// Seconds into the wave it starts from (a line resumed part-way).
    skip: f32,
    pcm: Arc<Pcm>,
    stop_at: Option<f64>,
    update: Option<(Arc<Stack>, Instance)>,
    stop: Option<(Arc<Stack>, Instance)>,
    chan: Chan,
    gains: [f32; 2],
    pitch: f32,
    /// Volume factor set by inputs (ambient_generic `Volume`, fades).
    scale: f32,
    finished: bool,
}

struct Block {
    entry: Option<String>,
    substring: bool,
    entity: Option<i32>,
    channel: Option<i32>,
    until: f64,
}

#[derive(Resource)]
pub struct Sounds {
    base_scripts: Arc<SoundScripts>,
    scripts: Arc<SoundScripts>,
    library: Library,
    pub mix: mixer::MixData,
    pub soundscapes: source_formats::soundscape::Soundscapes,
    cache: SoundCache,
    compiled: HashMap<String, EntryStacks>,
    channels: Vec<Channel>,
    next_id: u64,
    blocks: Vec<Block>,
    opvars: HashMap<String, f32>,
    output: Option<MixHandle>,
    map: String,
    /// Where the player hears from, updated every frame.
    pub listener: Listener,
    /// Host time, seconds.
    now: f64,
}

impl Sounds {
    pub fn entry(&self, name: &str) -> Option<&SoundEntry> {
        self.scripts.get(name)
    }

    pub fn now(&self) -> f64 {
        self.now
    }

    /// How far open a speaking entity's mouth is, measured as the engine
    /// measures it for lip sync and lit eyes (`SND_MoveMouth16`): the mean
    /// magnitude of ten 8-bit samples taken about 80-111 frames apart from
    /// where each of its voice channels is playing. 0 (silent) to 128.
    pub fn mouth_open(&self, entity: Entity) -> f32 {
        let mut open = 0.0f32;
        for c in &self.channels {
            if c.entity != Some(entity)
                || c.finished
                || c.stop_at.is_some_and(|t| t <= self.now)
                || (c.chan.slot != CHAN_VOICE && c.chan.slot != CHAN_VOICE2)
            {
                continue;
            }
            let t = (self.now - c.sound_start) as f32 * c.pitch + c.skip;
            let frames = c.pcm.frames as usize;
            if t < 0.0 || frames == 0 {
                continue;
            }
            let mut frame = (t * c.pcm.rate as f32) as usize;
            if c.looping {
                frame %= frames;
            } else if frame >= frames {
                continue;
            }
            let stride = c.pcm.channels.max(1) as usize;
            let mut sum = 0i32;
            for _ in 0..10 {
                let data = c.pcm.samples.get((frame % frames) * stride).map_or(0, |&s| (s >> 8) as i32);
                sum += data.abs();
                frame += 80 + (data as u8 & 0x1F) as usize;
            }
            open = open.max(sum as f32 / 10.0);
        }
        open
    }
}

fn init_sounds(world: &mut World) {
    let Some(fs) = world.get_resource::<GameFs>().map(|f| f.0.clone()) else { return };
    let args = world.resource::<Args>();
    let quiet = args.quiet();
    let scripts = SoundScripts::load(&fs).unwrap_or_else(|e| {
        warn!("audio: sound scripts: {e}");
        SoundScripts::from_manifest("", &mut |_| None)
    });
    let output = if quiet {
        None
    } else {
        device::open().map(|(handle, stream)| {
            world.insert_non_send(stream);
            handle
        })
    };
    let scripts = Arc::new(scripts);
    info!("audio: {} sound entries", scripts.len());
    if let (Some(out), Some(_)) = (&output, &world.resource::<Args>().snd_record) {
        let seconds = world.resource::<Args>().snd_seconds;
        if let Ok(mut s) = out.state.lock() {
            s.record = Some((Vec::new(), (seconds * out.rate as f32) as usize));
        }
    }
    world.insert_resource(Sounds {
        base_scripts: scripts.clone(),
        scripts,
        library: Library::load(&fs),
        mix: mixer::MixData::load(&fs),
        soundscapes: source_formats::soundscape::Soundscapes::load(&fs, None)
            .unwrap_or_else(|_| source_formats::soundscape::Soundscapes::from_manifest("", None, &mut |_| None)),
        cache: SoundCache::default(),
        compiled: HashMap::new(),
        channels: Vec::new(),
        next_id: 1,
        blocks: Vec::new(),
        opvars: HashMap::new(),
        output,
        map: String::new(),
        listener: Listener { origin: Vec3::ZERO, forward: Vec3::X, right: Vec3::NEG_Y, velocity: Vec3::ZERO },
        now: 0.0,
    });
    // Keep the type in use when there is no device.
    let _ = std::mem::size_of::<OutputStream>();
}

/// On a new map: stop everything, apply the map's sound overrides and load
/// its soundscapes.
fn map_changed(world: &mut World) {
    let Some(name) = world.get_resource::<LoadedMap>().map(|m| m.name.clone()).filter(|n| !n.is_empty()) else { return };
    let Some(fs) = world.get_resource::<GameFs>().map(|f| f.0.clone()) else { return };
    let Some(mut sounds) = world.get_resource_mut::<Sounds>() else { return };
    if sounds.map == name {
        return;
    }
    sounds.map = name.clone();
    stop_all_inner(&mut sounds);
    let mut scripts = (*sounds.base_scripts).clone();
    if scripts.load_map_overrides(&fs, &name) {
        debug!("audio: {name} sound overrides");
    }
    sounds.scripts = Arc::new(scripts);
    sounds.compiled.clear();
    sounds.blocks.clear();
    if let Ok(s) = source_formats::soundscape::Soundscapes::load(&fs, Some(&name)) {
        sounds.soundscapes = s;
    }
    sounds.mix.select("Default_Mix");
}

fn stop_all_inner(sounds: &mut Sounds) {
    sounds.channels.clear();
    if let Some(out) = &sounds.output
        && let Ok(mut s) = out.state.lock()
    {
        s.voices.clear();
    }
}

/// Where a sound attached to `e` comes from: the centre of a brush model,
/// otherwise the entity origin.
pub fn sound_origin(world: &World, e: Entity) -> Option<Vec3> {
    let gt = world.get::<GlobalTransform>(e)?;
    if let Some(b) = world.get::<BrushBounds>(e) {
        return Some(gt.transform_point((b.mins + b.maxs) * 0.5));
    }
    if world.resource::<EntityRegistry>().player == Some(e) {
        return Some(gt.translation() + Vec3::Z * 36.0);
    }
    Some(gt.translation())
}

/// Starts a sound entry (or a raw wave path) and returns its channel id.
pub fn emit(world: &mut World, name: &str, params: Emit) -> Option<u64> {
    let fs = world.get_resource::<GameFs>()?.0.clone();
    let origin = params.origin.or_else(|| params.entity.and_then(|e| sound_origin(world, e))).unwrap_or(Vec3::ZERO);
    let class = params.entity.and_then(|e| world.get::<MapEntity>(e)).map(|m| m.classname.clone());
    let player = world.resource::<EntityRegistry>().player;
    let local_player = params.entity.is_some() && params.entity == player;
    let class = class.or_else(|| local_player.then(|| "player".to_owned()));
    let no_caption = params.no_caption;
    let mut sounds = world.get_resource_mut::<Sounds>()?;
    let entry = sounds.scripts.get(name).map(|e| e.name.clone());
    let id = sounds.start(&fs, name, params, origin, class.as_deref(), local_player)?;
    // Sound entries with a caption show it while they play.
    if let Some(entry) = entry.filter(|_| !no_caption) {
        let length = sounds.channels.iter().find(|c| c.id == id).map_or(0.0, |c| c.chan.duration / c.pitch.max(0.01));
        let time = world.resource::<portal2_core::clock::GameClock>().curtime;
        world.write_message(CaptionRequest { token: entry, duration: length, from_player: local_player, time });
    }
    Some(id)
}

/// Registers decoded samples under a wave path (`sound/...`), so that
/// `emit` can play them by that name.
pub fn register_pcm(world: &mut World, path: &str, rate: u32, channels: u16, samples: Vec<i16>) {
    let Some(mut sounds) = world.get_resource_mut::<Sounds>() else { return };
    let frames = (samples.len() / channels.max(1) as usize) as u32;
    sounds.cache.insert(path, Pcm { rate, channels, frames, samples, loop_start: None });
}

/// Stops sounds of `name` (entry or wave) on `entity` (or unattached ones).
pub fn stop(world: &mut World, entity: Option<Entity>, name: &str) {
    let Some(mut sounds) = world.get_resource_mut::<Sounds>() else { return };
    let now = sounds.now;
    for c in sounds.channels.iter_mut().filter(|c| c.entity == entity && c.entry.eq_ignore_ascii_case(name)) {
        c.stop_at.get_or_insert(now);
    }
}

pub fn stop_channel(world: &mut World, id: u64) {
    let Some(mut sounds) = world.get_resource_mut::<Sounds>() else { return };
    let now = sounds.now;
    if let Some(c) = sounds.channels.iter_mut().find(|c| c.id == id) {
        c.stop_at.get_or_insert(now);
    }
}

pub fn is_playing(world: &World, id: u64) -> bool {
    world.get_resource::<Sounds>().is_some_and(|s| s.channels.iter().any(|c| c.id == id && !c.finished))
}

/// Volume factor and pitch (percent) of a running channel.
pub fn adjust(world: &mut World, id: u64, scale: Option<f32>, pitch: Option<f32>) {
    let Some(mut sounds) = world.get_resource_mut::<Sounds>() else { return };
    if let Some(c) = sounds.channels.iter_mut().find(|c| c.id == id) {
        if let Some(s) = scale {
            c.scale = s;
        }
        if let Some(p) = pitch {
            c.chan.pitch = p / 100.0;
        }
    }
}

/// Length of an entry's first wave in seconds (0 when unknown).
pub fn duration(world: &mut World, name: &str) -> f32 {
    let Some(fs) = world.get_resource::<GameFs>().map(|f| f.0.clone()) else { return 0.0 };
    let Some(mut sounds) = world.get_resource_mut::<Sounds>() else { return 0.0 };
    let path = match sounds.scripts.get(name) {
        Some(e) => e.waves.first().and_then(WaveName::file_path),
        None => WaveName::parse(name).file_path(),
    };
    path.and_then(|p| sounds.cache.get(&fs, &p)).map_or(0.0, |p| p.duration())
}

impl Sounds {
    fn stacks_for(&mut self, entry: Option<&SoundEntry>) -> EntryStacks {
        let Some(entry) = entry else {
            return EntryStacks { update: self.library.compile(StackKind::Update, None, Some(DEFAULT_UPDATE)).map(Arc::new), ..default() };
        };
        let key = entry.name.to_ascii_lowercase();
        if let Some(s) = self.compiled.get(&key) {
            return s.clone();
        }
        let own = entry.operator_stacks.as_ref();
        let s = EntryStacks {
            start: self.library.compile(StackKind::Start, own, None).map(Arc::new),
            update: self.library.compile(StackKind::Update, own, Some(DEFAULT_UPDATE)).map(Arc::new),
            stop: self.library.compile(StackKind::Stop, own, None).map(Arc::new),
        };
        self.compiled.insert(key, s.clone());
        s
    }

    fn start(
        &mut self,
        fs: &source_formats::fs::GameFileSystem,
        name: &str,
        params: Emit,
        origin: Vec3,
        class: Option<&str>,
        local_player: bool,
    ) -> Option<u64> {
        let scripts = self.scripts.clone();
        let entry = scripts.get(name);
        let (wave, volume, pitch, level, slot, delay_ms) = match entry {
            Some(e) => {
                let wave = match e.waves.len() {
                    0 => return None,
                    1 => e.waves[0].clone(),
                    n => e.waves[(random_range(0.0, n as f32) as usize).min(n - 1)].clone(),
                };
                let v = random_range(e.volume.start, e.volume.end);
                let p = random_range(e.pitch.start, e.pitch.end);
                let l = random_range(e.soundlevel.start, e.soundlevel.end);
                (wave, v, p, l, e.channel.index(), e.delay_msec)
            }
            None if name.contains('/') || name.contains('.') => (WaveName::parse(name), 1.0, 100.0, 75.0, CHAN_AUTO, 0),
            None => {
                debug!("audio: unknown sound {name}");
                return None;
            }
        };
        if wave.chars.contains(SoundChars::SENTENCE) || wave.chars.contains(SoundChars::USER_VOX) {
            return None;
        }
        let path = wave.file_path()?;
        if path.eq_ignore_ascii_case("sound/common/null.wav") && entry.is_some_and(|e| e.operator_stacks.is_none()) {
            return None;
        }
        let volume = params.volume.unwrap_or(volume) * params.volume_scale.unwrap_or(1.0);
        let pitch = params.pitch.unwrap_or(pitch);
        let level = params.soundlevel.unwrap_or(level);
        let slot = params.slot.unwrap_or(slot);
        let entity_index = params.entity.map_or(-1, |e| e.index_u32() as i32);
        let entry_name = entry.map_or_else(|| name.to_owned(), |e| e.name.clone());
        let now = self.now;
        self.blocks.retain(|b| b.until > now);
        let lower = entry_name.to_ascii_lowercase();
        if self.blocks.iter().any(|b| {
            let name_ok = b.entry.as_ref().is_none_or(|e| {
                let e = e.to_ascii_lowercase();
                if b.substring { lower.contains(&e) } else { lower == e }
            });
            name_ok && b.entity.is_none_or(|x| x == entity_index) && b.channel.is_none_or(|x| x == slot)
        }) {
            return None;
        }
        let pcm = self.cache.get(fs, &path)?;
        debug!("sound {name} -> {path} vol {volume:.2} lvl {level} chan {slot} pitch {pitch}");
        let stacks = self.stacks_for(entry);
        let mut chan = Chan {
            entry: entry_name.clone(),
            entity_index,
            has_entity: params.entity.is_some(),
            slot,
            origin,
            volume,
            level,
            pitch: pitch / 100.0,
            duration: pcm.duration(),
            groups: self.mix.groups_for(path.strip_prefix("sound/").unwrap_or(&path), class, local_player, slot, level),
            occlusion: 1.0,
            ..default()
        };
        // Same entity and channel replace each other, except auto and static.
        if params.entity.is_some() && slot != CHAN_AUTO && slot != CHAN_STATIC {
            for c in self.channels.iter_mut().filter(|c| c.entity == params.entity && c.chan.slot == slot) {
                c.stop_at.get_or_insert(now);
                c.stop = None;
            }
        }
        let mut delay = params.delay + delay_ms.max(0) as f32 / 1000.0;
        if let Some(start) = &stacks.start {
            let others = self.others(None);
            let mut env = self.env(&others);
            let mut inst = start.instance();
            start.run(&mut inst, &mut chan, &mut env);
            let effects = std::mem::take(&mut env.effects);
            self.apply_effects(fs, effects, params.entity, origin);
            if chan.block_start {
                return None;
            }
            if let Some(d) = chan.delay.take() {
                delay += d;
            }
        }
        let id = self.next_id;
        self.next_id += 1;
        let looping = pcm.loop_start.is_some();
        let flat = entry.is_none_or(|e| e.operator_stacks.is_none()) && level == 0.0;
        if let Some(out) = &self.output
            && let Ok(mut s) = out.state.lock()
        {
            let mut v = Voice::new(id, pcm.clone(), looping);
            v.pitch = chan.pitch;
            // A negative delay starts part-way into the sound.
            if delay >= 0.0 {
                v.delay = (delay * out.rate as f32) as u64;
            } else {
                v.pos = ((-delay) * pcm.rate as f32) as f64 % pcm.frames.max(1) as f64;
            }
            s.voices.push(v);
        }
        self.channels.push(Channel {
            id,
            entry: entry_name,
            entity: params.entity,
            origin,
            flat,
            looping,
            started: now,
            sound_start: now + delay.max(0.0) as f64,
            skip: (-delay).max(0.0),
            pcm: pcm.clone(),
            stop_at: None,
            update: stacks.update.map(|s| {
                let i = s.instance();
                (s, i)
            }),
            stop: stacks.stop.map(|s| {
                let i = s.instance();
                (s, i)
            }),
            chan,
            gains: [0.0; 2],
            pitch: pitch / 100.0,
            scale: 1.0,
            finished: false,
        });
        Some(id)
    }

    fn others(&self, except: Option<u64>) -> Vec<Other> {
        self.channels
            .iter()
            .filter(|c| Some(c.id) != except && !c.finished && c.stop_at.is_none())
            .map(|c| Other { id: c.id, entry: c.entry.clone(), entity_index: c.chan.entity_index, slot: c.chan.slot, started: c.started })
            .collect()
    }

    fn env<'a>(&'a self, others: &'a [Other]) -> Env<'a> {
        Env {
            listener: &self.listener,
            mix: &self.mix,
            cvar: &CVAR_SNAPSHOT_FN,
            opvars: &self.opvars,
            others,
            now: self.now,
            map: &self.map,
            speaker_mode: SPEAKER_MODE.with(|m| m.get()),
            effects: Vec::new(),
        }
    }

    fn apply_effects(&mut self, fs: &source_formats::fs::GameFileSystem, effects: Vec<ops::Effect>, entity: Option<Entity>, origin: Vec3) {
        for e in effects {
            match e {
                ops::Effect::Stop(ids) => {
                    let now = self.now;
                    for c in self.channels.iter_mut().filter(|c| ids.contains(&c.id)) {
                        c.stop_at.get_or_insert(now);
                    }
                }
                ops::Effect::Start { entry, delay } => {
                    let params = Emit { entity, origin: Some(origin), delay, ..default() };
                    self.start(fs, &entry, params, origin, None, false);
                }
                ops::Effect::Block { entry, substring, entity, channel, until } => {
                    self.blocks.push(Block { entry, substring, entity, channel, until });
                }
                ops::Effect::MixLayer { layer, group, field, value } => {
                    if let (Some(l), Some(g)) = (self.mix.layer(&layer), self.mix.group(&group)) {
                        self.mix.set_layer_field(l, g, &field, value);
                    }
                }
                ops::Effect::Opvar(name, value) => {
                    self.opvars.insert(name, value);
                }
            }
        }
    }
}

// The operators read console variables through a plain function; the
// values are copied from the console once per frame.
thread_local! {
    static CVARS: std::cell::RefCell<HashMap<String, f32>> = std::cell::RefCell::new(HashMap::new());
    static SPEAKER_MODE: std::cell::Cell<i32> = const { std::cell::Cell::new(2) };
}

static CVAR_SNAPSHOT_FN: fn(&str) -> f32 = cvar_snapshot;

fn cvar_snapshot(name: &str) -> f32 {
    CVARS.with(|c| c.borrow().get(&name.to_ascii_lowercase()).copied().unwrap_or(0.0))
}

const SNAPSHOT_CVARS: [&str; 14] = [
    "volume",
    "snd_musicvolume",
    "snd_gain",
    "snd_gain_max",
    "snd_gain_min",
    "snd_refdist",
    "snd_foliage_db_loss",
    "snd_op_test_convar",
    "snd_mixer_master_level",
    "snd_mixer_master_dsp",
    "snd_disable_mixer_solo",
    "snd_surround_speakers",
    "snd_mute_losefocus",
    "dsp_volume",
];

fn update_channels(world: &mut World) {
    let dt = world.resource::<Time<Real>>().delta_secs_f64();
    {
        let console = world.resource::<Console>();
        CVARS.with(|c| {
            let mut c = c.borrow_mut();
            for name in SNAPSHOT_CVARS {
                c.insert(name.to_owned(), console.f32(name));
            }
        });
        let speakers = console.i32("snd_surround_speakers");
        SPEAKER_MODE.with(|m| m.set(if speakers == 0 { 0 } else { 2 }));
    }
    let focused = world.query_filtered::<&Window, With<PrimaryWindow>>().iter(world).next().is_none_or(|w| w.focused);
    let listener = {
        let mut q = world.query_filtered::<&GlobalTransform, With<MainCamera>>();
        q.iter(world).next().map(|t| {
            let (_, rot, pos) = t.to_scale_rotation_translation();
            Listener { origin: pos, forward: rot * Vec3::NEG_Z, right: rot * Vec3::X, velocity: Vec3::ZERO }
        })
    };
    // Follow entities.
    let attached: Vec<(usize, Entity)> = match world.get_resource::<Sounds>() {
        Some(s) => s.channels.iter().enumerate().filter_map(|(i, c)| c.entity.map(|e| (i, e))).collect(),
        None => return,
    };
    let positions: Vec<(usize, Option<Vec3>)> = attached.into_iter().map(|(i, e)| (i, sound_origin(world, e))).collect();
    let Some(fs) = world.get_resource::<GameFs>().map(|f| f.0.clone()) else { return };
    let mut sounds = world.resource_mut::<Sounds>();
    sounds.now += dt;
    if let Some(l) = listener {
        sounds.listener = l;
    }
    for (i, pos) in positions {
        let c = &mut sounds.channels[i];
        match pos {
            Some(p) => c.origin = p,
            // The entity is gone: looping sounds stop with it.
            None if c.looping => {
                let now = sounds.now;
                sounds.channels[i].stop_at.get_or_insert(now);
            }
            None => {}
        }
    }
    let now = sounds.now;
    let mut channels = std::mem::take(&mut sounds.channels);
    let others_all: Vec<Other> = channels
        .iter()
        .filter(|c| !c.finished && c.stop_at.is_none())
        .map(|c| Other { id: c.id, entry: c.entry.clone(), entity_index: c.chan.entity_index, slot: c.chan.slot, started: c.started })
        .collect();
    let mut all_effects = Vec::new();
    for c in channels.iter_mut() {
        c.chan.origin = c.origin;
        c.chan.entry_elapsed = (now - c.started) as f32;
        c.chan.sound_elapsed = (now - c.sound_start).max(0.0) as f32;
        c.chan.stop_elapsed = c.stop_at.map_or(0.0, |t| (now - t).max(1e-4) as f32);
        c.chan.speakers = None;
        c.chan.stop_hold = None;
        let others: Vec<Other> = others_all.iter().filter(|o| o.id != c.id).map(|o| Other { entry: o.entry.clone(), ..*o }).collect();
        let mut env = sounds.env(&others);
        if c.stop_at.is_some()
            && let Some((stack, mut inst)) = c.stop.take()
        {
            stack.run(&mut inst, &mut c.chan, &mut env);
        }
        match &mut c.update {
            Some((stack, inst)) if !c.flat => stack.run(inst, &mut c.chan, &mut env),
            _ => {
                // Unspatialised: full volume on both speakers through the
                // mixer's "All" group.
                let (v, _, _) = sounds.mix.group("All").map_or((1.0, 1.0, 1.0), |g| sounds.mix.values(g, 1.0, 1.0));
                let g = c.chan.volume * v * cvar_snapshot("snd_gain");
                c.chan.speakers = Some([g, g, 0.0, 0.0, 0.0, 0.0]);
                c.chan.out_pitch = Some(c.chan.pitch);
                c.chan.mixlayer_trigger = 1.0;
            }
        }
        all_effects.extend(std::mem::take(&mut env.effects).into_iter().map(|e| (e, c.entity, c.origin)));
        let sp = c.chan.speakers.unwrap_or([0.0; 6]);
        c.gains = [sp[0] * c.scale, sp[1] * c.scale];
        c.pitch = c.chan.out_pitch.unwrap_or(c.chan.pitch);
        if c.stop_at.is_some() && !c.chan.stop_hold.unwrap_or(false) {
            c.finished = true;
        }
    }
    // Hand the results to the mixer and drop finished voices.
    if let Some(out) = sounds.output.clone()
        && let Ok(mut s) = out.state.lock()
    {
        let mute = !focused && cvar_snapshot("snd_mute_losefocus") != 0.0;
        s.master = if mute { 0.0 } else { cvar_snapshot("volume").clamp(0.0, 1.0) };
        let mut by_id: HashMap<u64, usize> = HashMap::new();
        for (i, c) in channels.iter().enumerate() {
            by_id.insert(c.id, i);
        }
        for v in s.voices.iter_mut() {
            match by_id.get(&v.id).map(|&i| &channels[i]) {
                Some(c) if !c.finished => {
                    v.target = c.gains;
                    v.pitch = c.pitch;
                    if v.gain == [0.0, 0.0] && v.pos == 0.0 {
                        // First update: start at the computed gain.
                        v.gain = c.gains;
                    }
                }
                _ => {
                    v.target = [0.0; 2];
                    v.stopping = true;
                }
            }
        }
        let finished: Vec<u64> = s.voices.iter().filter(|v| v.finished).map(|v| v.id).collect();
        s.voices.retain(|v| !v.finished);
        for id in finished {
            if let Some(&i) = by_id.get(&id) {
                channels[i].finished = true;
            }
        }
    } else {
        // No device: one-shots end after their length.
        for c in channels.iter_mut() {
            if !c.looping && (now - c.sound_start) as f32 > c.chan.duration / c.pitch.max(0.01) {
                c.finished = true;
            }
        }
    }
    channels.retain(|c| !c.finished);
    sounds.channels = channels;
    let sounds = &mut *sounds;
    sounds.mix.update_triggers(now, sounds.channels.iter().map(|c| (c.chan.groups.as_slice(), c.chan.mixlayer_trigger)));
    sounds.mix.update_layers(cvar_snapshot("snd_disable_mixer_solo") != 0.0);
    for (e, entity, origin) in all_effects {
        sounds.apply_effects(&fs, vec![e], entity, origin);
    }
}

/// `--snd-record`: writes the recorded mix once it is complete.
fn write_recording(world: &mut World, mut done: Local<bool>) {
    if *done {
        return;
    }
    let Some(path) = world.resource::<Args>().snd_record.clone() else {
        *done = true;
        return;
    };
    let Some(out) = world.get_resource::<Sounds>().and_then(|s| s.output.clone()) else { return };
    let Ok(mut s) = out.state.lock() else { return };
    let channels = s.channels.max(1);
    let Some((buf, limit)) = &s.record else { return };
    if buf.len() < limit * channels {
        return;
    }
    let mut data = Vec::with_capacity(44 + buf.len() * 2);
    let bytes = (buf.len() * 2) as u32;
    data.extend_from_slice(b"RIFF");
    data.extend_from_slice(&(36 + bytes).to_le_bytes());
    data.extend_from_slice(b"WAVEfmt ");
    data.extend_from_slice(&16u32.to_le_bytes());
    data.extend_from_slice(&1u16.to_le_bytes());
    data.extend_from_slice(&(channels as u16).to_le_bytes());
    data.extend_from_slice(&out.rate.to_le_bytes());
    data.extend_from_slice(&(out.rate * channels as u32 * 2).to_le_bytes());
    data.extend_from_slice(&(channels as u16 * 2).to_le_bytes());
    data.extend_from_slice(&16u16.to_le_bytes());
    data.extend_from_slice(b"data");
    data.extend_from_slice(&bytes.to_le_bytes());
    for x in buf {
        data.extend_from_slice(&((x.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    s.record = None;
    drop(s);
    match std::fs::write(&path, data) {
        Ok(()) => info!("audio: recorded mix to {path}"),
        Err(e) => warn!("audio: cannot write {path}: {e}"),
    }
    *done = true;
}

/// Prints the playing channels (`snd_list`).
pub fn list(world: &World) -> Vec<String> {
    let Some(s) = world.get_resource::<Sounds>() else { return Vec::new() };
    s.channels
        .iter()
        .map(|c| {
            format!(
                "{:>4} {:<40} gains {:.3}/{:.3} pitch {:.2} lvl {} groups {:?}{}",
                c.id,
                c.entry,
                c.gains[0],
                c.gains[1],
                c.pitch,
                c.chan.level,
                c.chan.groups,
                if c.stop_at.is_some() { " stopping" } else { "" }
            )
        })
        .collect()
}

/// Pcm handle for code that wants a sound's length.
#[allow(dead_code)]
pub fn pcm(world: &mut World, path: &str) -> Option<Arc<Pcm>> {
    let fs = world.get_resource::<GameFs>()?.0.clone();
    world.get_resource_mut::<Sounds>()?.cache.get(&fs, path)
}
