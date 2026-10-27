//! The output stream and the software mixer that runs on the audio thread.
//!
//! The main thread owns the sound channels and their operator stacks; each
//! frame it hands the mixer a target gain per speaker and a pitch for every
//! voice. The mixer resamples each voice to the device rate (linear
//! interpolation, like the original's default pitch quality), ramps gains
//! towards their targets to avoid clicks and sums everything into the
//! device's channels.

use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use super::cache::Pcm;

/// Seconds a gain change takes to reach its target.
const GAIN_RAMP: f32 = 0.02;

pub struct Voice {
    pub id: u64,
    pub pcm: Arc<Pcm>,
    /// Read position in source frames.
    pub pos: f64,
    /// Playback rate relative to the source (1 = original pitch).
    pub pitch: f32,
    /// Current and target gain for the left and right speakers.
    pub gain: [f32; 2],
    pub target: [f32; 2],
    pub looping: bool,
    /// Output frames still to wait before the voice starts.
    pub delay: u64,
    pub finished: bool,
    /// Stop as soon as the gain has ramped down.
    pub stopping: bool,
    /// Holds the voice without advancing (game paused).
    pub paused: bool,
}

impl Voice {
    pub fn new(id: u64, pcm: Arc<Pcm>, looping: bool) -> Self {
        Self {
            id,
            pcm,
            pos: 0.0,
            pitch: 1.0,
            gain: [0.0; 2],
            target: [0.0; 2],
            looping,
            delay: 0,
            finished: false,
            stopping: false,
            paused: false,
        }
    }
}

#[derive(Default)]
pub struct MixState {
    pub voices: Vec<Voice>,
    /// `volume` cvar.
    pub master: f32,
    /// Device frames mixed so far.
    pub frames_mixed: u64,
    /// Interleaved output kept for `--snd-record`, up to the frame limit.
    pub record: Option<(Vec<f32>, usize)>,
    pub channels: usize,
}

/// What the main thread keeps of the device.
#[derive(Clone)]
pub struct MixHandle {
    pub state: Arc<Mutex<MixState>>,
    pub rate: u32,
}

/// The stream must stay alive (and on some hosts on its thread).
pub struct OutputStream(#[allow(dead_code)] pub cpal::Stream);

/// Opens the default output device; `None` (and a warning) when there is none.
pub fn open() -> Option<(MixHandle, OutputStream)> {
    let host = cpal::default_host();
    let Some(device) = host.default_output_device() else {
        warn!("audio: no output device");
        return None;
    };
    let supported = match device.default_output_config() {
        Ok(c) => c,
        Err(e) => {
            warn!("audio: no output configuration: {e}");
            return None;
        }
    };
    let rate = supported.sample_rate();
    let channels = supported.channels() as usize;
    let config = supported.config();
    let state = Arc::new(Mutex::new(MixState { master: 1.0, ..default() }));
    let shared = state.clone();
    let err = |e| warn!("audio stream error: {e}");
    let mut scratch: Vec<f32> = Vec::new();
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => device.build_output_stream(
            &config,
            move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
                mix_into(&shared, rate, channels, out, &mut scratch);
            },
            err,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_output_stream(
            &config,
            move |out: &mut [i16], _: &cpal::OutputCallbackInfo| {
                scratch.resize(out.len(), 0.0);
                let mut tmp = std::mem::take(&mut scratch);
                mix_into(&shared, rate, channels, &mut tmp, &mut Vec::new());
                for (o, s) in out.iter_mut().zip(&tmp) {
                    *o = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
                }
                scratch = tmp;
            },
            err,
            None,
        ),
        other => {
            warn!("audio: unsupported sample format {other:?}");
            return None;
        }
    };
    let stream = match stream {
        Ok(s) => s,
        Err(e) => {
            warn!("audio: cannot open stream: {e}");
            return None;
        }
    };
    if let Err(e) = stream.play() {
        warn!("audio: cannot start stream: {e}");
        return None;
    }
    info!("audio: {} Hz, {channels} channels", rate);
    Some((MixHandle { state, rate }, OutputStream(stream)))
}

fn mix_into(shared: &Mutex<MixState>, rate: u32, channels: usize, out: &mut [f32], _scratch: &mut Vec<f32>) {
    out.fill(0.0);
    let Ok(mut state) = shared.lock() else { return };
    let frames = out.len() / channels.max(1);
    let master = state.master;
    let step_max = 1.0 / (GAIN_RAMP * rate as f32);
    for v in state.voices.iter_mut() {
        if v.finished || v.paused {
            continue;
        }
        mix_voice(v, rate, channels, frames, out, step_max, master);
    }
    state.frames_mixed += frames as u64;
    state.channels = channels;
    if let Some((buf, limit)) = &mut state.record
        && buf.len() < *limit * channels
    {
        buf.extend_from_slice(out);
    }
    if channels >= 2 {
        for s in out.iter_mut() {
            *s = s.clamp(-1.0, 1.0);
        }
    }
}

fn mix_voice(v: &mut Voice, rate: u32, channels: usize, frames: usize, out: &mut [f32], step_max: f32, master: f32) {
    let pcm = v.pcm.clone();
    let src_frames = pcm.frames as f64;
    if src_frames < 1.0 {
        v.finished = true;
        return;
    }
    let step = pcm.rate as f64 / rate as f64 * v.pitch.max(0.0) as f64;
    let loop_start = pcm.loop_start.map(|l| l as f64).filter(|l| *l < src_frames);
    let stereo = pcm.channels >= 2;
    for f in 0..frames {
        if v.delay > 0 {
            v.delay -= 1;
            continue;
        }
        for (g, t) in v.gain.iter_mut().zip(v.target) {
            let d = t - *g;
            *g += d.clamp(-step_max, step_max);
        }
        if v.stopping && v.gain[0] <= 1e-4 && v.gain[1] <= 1e-4 {
            v.finished = true;
            return;
        }
        let i = v.pos as usize;
        let frac = (v.pos - i as f64) as f32;
        let next = if i + 1 < pcm.frames as usize {
            i + 1
        } else if v.looping {
            loop_start.map_or(0, |l| l as usize)
        } else {
            i
        };
        let (l, r) = if stereo {
            let a = (pcm.sample(i, 0), pcm.sample(i, 1));
            let b = (pcm.sample(next, 0), pcm.sample(next, 1));
            (a.0 + (b.0 - a.0) * frac, a.1 + (b.1 - a.1) * frac)
        } else {
            let a = pcm.sample(i, 0);
            let b = pcm.sample(next, 0);
            let s = a + (b - a) * frac;
            (s, s)
        };
        let base = f * channels;
        if channels >= 2 {
            out[base] += l * v.gain[0] * master;
            out[base + 1] += r * v.gain[1] * master;
        } else {
            out[base] += (l * v.gain[0] + r * v.gain[1]) * 0.5 * master;
        }
        v.pos += step;
        if v.pos >= src_frames {
            match (v.looping, loop_start) {
                (true, Some(l)) => v.pos = l + (v.pos - src_frames) % (src_frames - l).max(1.0),
                (true, None) => v.pos %= src_frames,
                _ => {
                    v.finished = true;
                    return;
                }
            }
        }
    }
}
