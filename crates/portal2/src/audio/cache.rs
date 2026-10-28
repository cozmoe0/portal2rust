//! Decoded sounds, loaded on first use and kept for the session.
//!
//! WAV files come decoded from `source_formats::wav`; the voice-over is bare
//! MPEG-1 Layer III, decoded here with Symphonia.

use std::collections::HashMap;
use std::sync::Arc;

use bevy::prelude::*;
use source_formats::fs::GameFileSystem;
use source_formats::wav::{Codec, Wave};

/// 16-bit samples, interleaved.
pub struct Pcm {
    pub rate: u32,
    pub channels: u16,
    pub frames: u32,
    pub samples: Vec<i16>,
    pub loop_start: Option<u32>,
}

impl Pcm {
    #[inline]
    pub fn sample(&self, frame: usize, channel: usize) -> f32 {
        let c = self.channels.max(1) as usize;
        self.samples.get(frame * c + channel.min(c - 1)).map_or(0.0, |&s| s as f32 / 32768.0)
    }

    pub fn duration(&self) -> f32 {
        if self.rate == 0 { 0.0 } else { self.frames as f32 / self.rate as f32 }
    }
}

#[derive(Default)]
pub struct SoundCache {
    loaded: HashMap<String, Option<Arc<Pcm>>>,
}

impl SoundCache {
    /// Makes `pcm` the sound for `path` (sound decoded elsewhere, such as a
    /// video's audio track).
    pub fn insert(&mut self, path: &str, pcm: Pcm) {
        self.loaded.insert(path.to_ascii_lowercase().replace('\\', "/"), Some(Arc::new(pcm)));
    }

    /// `path` is the file path (`sound/...`) without sound characters.
    pub fn get(&mut self, fs: &GameFileSystem, path: &str) -> Option<Arc<Pcm>> {
        let key = path.to_ascii_lowercase().replace('\\', "/");
        if let Some(p) = self.loaded.get(&key) {
            return p.clone();
        }
        let pcm = load(fs, &key).map(Arc::new);
        if pcm.is_none() {
            debug!("audio: cannot load {key}");
        }
        self.loaded.insert(key, pcm.clone());
        pcm
    }
}

fn load(fs: &GameFileSystem, path: &str) -> Option<Pcm> {
    let data = fs.read(path)?;
    let wave = Wave::parse(&data).ok()?;
    match wave.codec {
        Codec::Pcm | Codec::Adpcm => Some(Pcm {
            rate: wave.rate,
            channels: wave.channels.max(1),
            frames: wave.frames,
            samples: wave.samples,
            loop_start: wave.loop_start,
        }),
        Codec::Mp3 => decode_mp3(&wave.encoded).map(|mut p| {
            p.loop_start = wave.loop_start;
            p
        }),
        Codec::Other(_) => None,
    }
}

fn decode_mp3(bytes: &[u8]) -> Option<Pcm> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::{CODEC_TYPE_NULL, DecoderOptions};
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let source = std::io::Cursor::new(bytes.to_vec());
    let stream = MediaSourceStream::new(Box::new(source), Default::default());
    let mut hint = Hint::new();
    hint.with_extension("mp3");
    let probed = symphonia::default::get_probe()
        .format(&hint, stream, &FormatOptions::default(), &MetadataOptions::default())
        .ok()?;
    let mut format = probed.format;
    let track = format.tracks().iter().find(|t| t.codec_params.codec != CODEC_TYPE_NULL)?.clone();
    let mut decoder = symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default()).ok()?;
    let mut samples: Vec<i16> = Vec::new();
    let mut rate = track.codec_params.sample_rate.unwrap_or(44100);
    let mut channels = track.codec_params.channels.map(|c| c.count() as u16).unwrap_or(1);
    let mut buf: Option<SampleBuffer<i16>> = None;
    while let Ok(packet) = format.next_packet() {
        if packet.track_id() != track.id {
            continue;
        }
        let Ok(decoded) = decoder.decode(&packet) else { continue };
        let spec = *decoded.spec();
        rate = spec.rate;
        channels = spec.channels.count() as u16;
        let b = buf.get_or_insert_with(|| SampleBuffer::new(decoded.capacity() as u64, spec));
        if b.capacity() < decoded.capacity() * spec.channels.count() {
            *b = SampleBuffer::new(decoded.capacity() as u64, spec);
        }
        b.copy_interleaved_ref(decoded);
        samples.extend_from_slice(b.samples());
    }
    let channels = channels.max(1);
    let frames = (samples.len() / channels as usize) as u32;
    (frames > 0).then_some(Pcm { rate, channels, frames, samples, loop_start: None })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decodes a few installed voice lines (skipped without an install).
    #[test]
    fn voice_over_decodes_at_full_scale() {
        let Some(root) = GameFileSystem::find_install() else { return };
        let Ok(fs) = GameFileSystem::portal2(root) else { return };
        for path in ["sound/vo/announcer/prehub46.wav", "sound/vo/glados/prehub14.wav"] {
            let Some(pcm) = load(&fs, path) else { panic!("{path} did not decode") };
            let peak = pcm.samples.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
            println!("{path}: {} Hz, {} ch, {:.2} s, peak {:.3}", pcm.rate, pcm.channels, pcm.duration(), peak as f32 / 32768.0);
            assert!(pcm.duration() > 0.5);
        }
    }
}
