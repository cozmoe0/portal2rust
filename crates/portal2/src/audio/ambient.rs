//! `ambient_generic`: a sound placed in the map. Looping ambients (the
//! default) start when the map does unless spawnflag 16 keeps them silent;
//! non-looping ones (spawnflag 32) play once per `PlaySound`. Spawnflag 1
//! plays everywhere (sound level 0); otherwise `radius` sets the level so the
//! sound reaches it: 40 dB plus 20·log10(radius / 36). A sound script name
//! brings its own volume, pitch and level; a bare wave uses the entity's
//! `health` (volume, 0–10), `pitch` and level.

use bevy::prelude::*;

use super::{Emit, Sounds};
use crate::entities::io::{InputCall, InputHandlers};
use crate::entities::{EntityRegistry, MapEntity, SpawnHandlers};

const SF_EVERYWHERE: u32 = 1;
const SF_START_SILENT: u32 = 16;
const SF_NOT_LOOPING: u32 = 32;

pub fn register(app: &mut App) {
    app.add_systems(Update, fade_ambients);
    let world = app.world_mut();
    world.resource_mut::<SpawnHandlers>().register("ambient_generic", spawn);
    world.resource_mut::<InputHandlers>().register("ambient_generic", input);
}

#[derive(Component, serde::Serialize, serde::Deserialize)]
pub struct Ambient {
    sound: String,
    volume: f32,
    pitch: f32,
    level: f32,
    looping: bool,
    source: Option<String>,
    origin: Vec3,
    active: bool,
    #[serde(skip)]
    channel: Option<u64>,
    /// Waiting for the map's sound reset before starting.
    pending: bool,
    /// Playing in a restored saved game; its sound starts again.
    #[serde(skip)]
    resume: bool,
    /// Volume factor and its ramp (per second, negative fades out).
    fade: f32,
    fade_rate: f32,
}

/// Sound level for a radius, as ambient_generic computes it.
pub fn radius_to_level(radius: f32, everywhere: bool) -> f32 {
    if radius > 0.0 && !everywhere { (40.0 + 20.0 * (radius / 36.0).log10()) as i32 as f32 } else { 0.0 }
}

fn spawn(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e).cloned() else { return };
    let Some(sound) = me.get("message").filter(|s| !s.is_empty()) else { return };
    let looping = !me.has_flag(SF_NOT_LOOPING);
    world.entity_mut(e).insert(Ambient {
        sound: sound.to_owned(),
        volume: me.get_f32("health").unwrap_or(10.0).clamp(0.0, 10.0) / 10.0,
        pitch: me.get_f32("pitch").filter(|p| *p > 0.0).unwrap_or(100.0),
        level: radius_to_level(me.get_f32("radius").unwrap_or(1250.0), me.has_flag(SF_EVERYWHERE)),
        looping,
        source: me.get("sourceentityname").filter(|s| !s.is_empty()).map(str::to_ascii_lowercase),
        origin: me.origin(),
        active: false,
        channel: None,
        pending: looping && !me.has_flag(SF_START_SILENT),
        resume: false,
        fade: 1.0,
        fade_rate: 0.0,
    });
}

/// Starts ambients that play from the start, once the map's sounds have
/// been reset.
pub fn start_pending(world: &mut World) {
    let resumed: Vec<Entity> = world.query::<(Entity, &Ambient)>().iter(world).filter(|(_, a)| a.resume).map(|(e, _)| e).collect();
    for e in resumed {
        let id = play(world, e);
        if let Some(mut a) = world.get_mut::<Ambient>(e) {
            a.resume = false;
            a.channel = id;
        }
    }
    let pending: Vec<Entity> = world.query::<(Entity, &Ambient)>().iter(world).filter(|(_, a)| a.pending).map(|(e, _)| e).collect();
    for e in pending {
        if let Some(mut a) = world.get_mut::<Ambient>(e) {
            a.pending = false;
        }
        toggle(world, e);
    }
}

fn play(world: &mut World, e: Entity) -> Option<u64> {
    let a = world.get::<Ambient>(e)?;
    let (sound, source, origin, fade) = (a.sound.clone(), a.source.clone(), a.origin, a.fade);
    let scripted = world.get_resource::<Sounds>().is_some_and(|s| s.entry(&sound).is_some());
    let (volume, pitch, level) = (a.volume, a.pitch, a.level);
    let source = source.and_then(|n| world.resource::<EntityRegistry>().by_name.get(&n).and_then(|l| l.first()).copied());
    let mut params = Emit { entity: source, origin: source.is_none().then_some(origin), ..default() };
    if !scripted {
        params.volume = Some(volume);
        params.pitch = Some(pitch);
        params.soundlevel = Some(level);
    }
    let id = super::emit(world, &sound, params)?;
    if !scripted || volume < 1.0 || fade < 1.0 {
        super::adjust(world, id, Some(if scripted { volume * fade } else { fade }), None);
    }
    Some(id)
}

fn stop(world: &mut World, e: Entity) {
    if let Some(id) = world.get_mut::<Ambient>(e).and_then(|mut a| a.channel.take()) {
        super::stop_channel(world, id);
    }
}

fn toggle(world: &mut World, e: Entity) {
    let Some(a) = world.get::<Ambient>(e) else { return };
    if a.active {
        stop(world, e);
        if let Some(mut a) = world.get_mut::<Ambient>(e) {
            a.active = false;
        }
        return;
    }
    let looping = a.looping;
    // A new non-looping play cuts off the previous one.
    stop(world, e);
    let id = play(world, e);
    if let Some(mut a) = world.get_mut::<Ambient>(e) {
        a.active = looping;
        a.channel = id;
    }
}

fn input(world: &mut World, call: &InputCall) -> bool {
    let e = call.target;
    let Some(a) = world.get::<Ambient>(e) else { return false };
    let (active, channel) = (a.active, a.channel);
    let value = source_formats::kv::parse_f32_lenient(&call.param);
    match call.input.as_str() {
        "playsound" => {
            if !active {
                toggle(world, e);
            }
        }
        "stopsound" => {
            stop(world, e);
            if let Some(mut a) = world.get_mut::<Ambient>(e) {
                a.active = false;
            }
        }
        "togglesound" => toggle(world, e),
        "volume" => {
            let v = value.clamp(0.0, 10.0) / 10.0;
            if let Some(mut a) = world.get_mut::<Ambient>(e) {
                a.volume = v;
            }
            if let Some(id) = channel {
                super::adjust(world, id, Some(v), None);
            }
        }
        "pitch" => {
            let p = value.clamp(1.0, 255.0);
            if let Some(mut a) = world.get_mut::<Ambient>(e) {
                a.pitch = p;
            }
            if let Some(id) = channel {
                super::adjust(world, id, None, Some(p));
            }
        }
        "fadein" => {
            if let Some(mut a) = world.get_mut::<Ambient>(e) {
                a.fade = 0.0;
                a.fade_rate = if value > 0.0 { 1.0 / value } else { 1000.0 };
            }
            if !active {
                toggle(world, e);
            }
        }
        "fadeout" => {
            if let Some(mut a) = world.get_mut::<Ambient>(e) {
                a.fade_rate = if value > 0.0 { -1.0 / value } else { -1000.0 };
            }
        }
        _ => return false,
    }
    true
}

fn fade_ambients(world: &mut World) {
    let dt = world.resource::<Time>().delta_secs();
    let mut changes = Vec::new();
    for (e, mut a) in world.query::<(Entity, &mut Ambient)>().iter_mut(world) {
        if a.fade_rate == 0.0 {
            continue;
        }
        a.fade = (a.fade + a.fade_rate * dt).clamp(0.0, 1.0);
        let done_out = a.fade_rate < 0.0 && a.fade <= 0.0;
        if a.fade >= 1.0 || done_out {
            a.fade_rate = 0.0;
        }
        changes.push((e, a.channel, a.fade * a.volume, done_out));
    }
    for (e, channel, scale, done_out) in changes {
        if let Some(id) = channel {
            super::adjust(world, id, Some(scale), None);
        }
        if done_out {
            stop(world, e);
            if let Some(mut a) = world.get_mut::<Ambient>(e) {
                a.active = false;
                a.fade = 1.0;
            }
        }
    }
}

impl crate::save::Restore for Ambient {
    fn keep(&mut self, live: Self) {
        self.channel = live.channel;
        // A looping sound that was playing starts again.
        self.resume = self.active && self.channel.is_none();
    }
}

pub fn register_save(r: &mut crate::save::SaveRegistry) {
    r.component::<Ambient>("ambient");
}
