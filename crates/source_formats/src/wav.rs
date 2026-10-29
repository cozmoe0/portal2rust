//! Sound files as Portal 2 ships them under `sound/`.
//!
//! Most are RIFF/WAVE: 8/16-bit PCM or Microsoft ADPCM (both decoded to
//! interleaved 16-bit samples), with loop points from `cue ` / `smpl`,
//! `LIST`/`adtl` labels and the `VDAT` sentence (lip-sync) chunk. Many `.wav`
//! files, including all character voice-over, are really bare MPEG audio
//! streams; those are recognised by content like the engine does, their frames
//! are validated and the stream is kept encoded.

use crate::fs::GameFileSystem;
use crate::reader::{Reader, str_from_nul};
use crate::{Error, Result, invalid};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Codec {
    /// `fmt ` tag 1, 8 or 16 bits.
    #[default]
    Pcm,
    /// Microsoft ADPCM (`fmt ` tag 2), decoded.
    Adpcm,
    /// MPEG audio frames with no RIFF header; kept in [`Wave::encoded`].
    Mp3,
    /// Any other `fmt ` tag; the `data` chunk is kept in [`Wave::encoded`].
    Other(u16),
}

#[derive(Clone, Debug, Default)]
pub struct Wave {
    pub codec: Codec,
    pub channels: u16,
    pub rate: u32,
    /// Bits per stored sample: 8/16 for PCM, 4 for ADPCM, 0 for MPEG audio.
    pub bits: u16,
    /// Decoded interleaved samples. Empty for MPEG audio, other codecs and
    /// [`Wave::parse_header`].
    pub samples: Vec<i16>,
    /// Length in sample frames (one sample per channel).
    pub frames: u32,
    /// Frame playback jumps back to when the sound loops. Like the engine this
    /// comes from the first `cue ` point or the first forward `smpl` loop,
    /// whichever chunk appears last in the file.
    pub loop_start: Option<u32>,
    /// Exclusive end of the first forward `smpl` loop. Informational: Portal 2
    /// loops from `loop_start` to the end of the data.
    pub loop_end: Option<u32>,
    pub cues: Vec<Cue>,
    pub labels: Vec<Label>,
    pub sentence: Option<SentenceData>,
    /// Undecoded stream: the MPEG frames for [`Codec::Mp3`], the `data` chunk
    /// for [`Codec::Other`]. Empty after [`Wave::parse_header`].
    pub encoded: Vec<u8>,
    pub mp3: Option<Mp3Info>,
    /// Ids of chunks that were skipped (editor metadata such as `bext`, `iXML`).
    pub other_chunks: Vec<[u8; 4]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cue {
    pub id: u32,
    pub position: u32,
    pub sample_offset: u32,
}

/// Entry of a `LIST`/`adtl` chunk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Label {
    /// Cue point id the entry annotates.
    pub cue: u32,
    /// `labl`, `note` or `ltxt`.
    pub kind: [u8; 4],
    pub text: String,
    /// Region length in sample frames (`ltxt` only).
    pub length: Option<u32>,
}

/// Details of a bare MPEG audio stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mp3Info {
    /// 1, 2, or 25 for MPEG 2.5.
    pub version: u8,
    pub layer: u8,
    pub frame_count: u32,
    pub samples_per_frame: u32,
    /// Lowest and highest frame bitrate in kbit/s (equal for constant bitrate).
    pub kbps: (u16, u16),
    /// Frames carry a CRC-16 after the header.
    pub crc: bool,
    /// Bytes after the last whole frame, not counting an ID3v1 tag.
    pub trailing: usize,
}

/// Contents of a `VDAT` chunk.
#[derive(Clone, Debug, PartialEq)]
pub enum SentenceData {
    Parsed(Sentence),
    /// Anything that is not the text sentence format.
    Raw(Vec<u8>),
}

/// Phoneme/word timing and options written by Valve's phoneme extractor.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sentence {
    pub version: String,
    pub text: String,
    pub words: Vec<Word>,
    /// `(time, value)` samples of the emphasis track.
    pub emphasis: Vec<(f32, f32)>,
    /// `OPTIONS` entries such as `voice_duck 1`.
    pub options: Vec<(String, String)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Word {
    pub text: String,
    pub start: f32,
    pub end: f32,
    pub phonemes: Vec<Phoneme>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Phoneme {
    pub code: i32,
    pub name: String,
    pub start: f32,
    pub end: f32,
    pub volume: f32,
}

impl Sentence {
    /// Parses the text form (`VERSION 1.0`, then `PLAINTEXT`, `WORDS`,
    /// `EMPHASIS`, `OPTIONS` blocks). Unknown blocks are skipped.
    pub fn parse(text: &str) -> Option<Self> {
        let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
        let version = lines.next()?.strip_prefix("VERSION")?.trim().to_owned();
        let mut s = Sentence { version, ..Default::default() };
        while let Some(section) = lines.next() {
            if lines.next()? != "{" {
                return None;
            }
            match section {
                "PLAINTEXT" => {
                    let mut parts = Vec::new();
                    loop {
                        match lines.next()? {
                            "}" => break,
                            l => parts.push(l),
                        }
                    }
                    s.text = parts.join("\n");
                }
                "WORDS" => loop {
                    let line = lines.next()?;
                    if line == "}" {
                        break;
                    }
                    let mut f = line.split_whitespace();
                    if f.next()? != "WORD" {
                        return None;
                    }
                    let mut word = Word {
                        text: f.next()?.to_owned(),
                        start: f.next()?.parse().ok()?,
                        end: f.next()?.parse().ok()?,
                        phonemes: Vec::new(),
                    };
                    if lines.next()? != "{" {
                        return None;
                    }
                    loop {
                        let line = lines.next()?;
                        if line == "}" {
                            break;
                        }
                        let mut f = line.split_whitespace();
                        word.phonemes.push(Phoneme {
                            code: f.next()?.parse().ok()?,
                            name: f.next()?.to_owned(),
                            start: f.next()?.parse().ok()?,
                            end: f.next()?.parse().ok()?,
                            volume: f.next().and_then(|v| v.parse().ok()).unwrap_or(1.0),
                        });
                    }
                    s.words.push(word);
                },
                "EMPHASIS" => loop {
                    let line = lines.next()?;
                    if line == "}" {
                        break;
                    }
                    let mut f = line.split_whitespace();
                    s.emphasis.push((f.next()?.parse().ok()?, f.next()?.parse().ok()?));
                },
                "OPTIONS" => loop {
                    let line = lines.next()?;
                    if line == "}" {
                        break;
                    }
                    let (k, v) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
                    s.options.push((k.to_owned(), v.trim().to_owned()));
                },
                _ => {
                    let mut depth = 1;
                    while depth > 0 {
                        match lines.next()? {
                            "{" => depth += 1,
                            "}" => depth -= 1,
                            _ => {}
                        }
                    }
                }
            }
        }
        Some(s)
    }

    pub fn option(&self, key: &str) -> Option<&str> {
        self.options.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.as_str())
    }

    /// `voice_duck 1`: the sound counts as voice and ducks other audio.
    pub fn voice_duck(&self) -> bool {
        self.option("voice_duck").is_some_and(|v| crate::kv::parse_i32_lenient(v) != 0)
    }
}

impl Wave {
    /// Parses and decodes a sound file (RIFF/WAVE or bare MPEG audio).
    pub fn parse(data: &[u8]) -> Result<Self> {
        Self::parse_inner(data, true)
    }

    /// Like [`Wave::parse`] but skips sample decoding and stream copies; the
    /// format, length, loop points and metadata are still filled in.
    pub fn parse_header(data: &[u8]) -> Result<Self> {
        Self::parse_inner(data, false)
    }

    /// Reads `path` (a full path such as `sound/vo/x.wav`) through the game filesystem.
    pub fn load(fs: &GameFileSystem, path: &str) -> Result<Self> {
        let data = fs.read(path).ok_or_else(|| Error::NotFound(path.to_owned()))?;
        Self::parse(&data)
    }

    /// Length in seconds.
    pub fn duration(&self) -> f32 {
        if self.rate == 0 { 0.0 } else { self.frames as f32 / self.rate as f32 }
    }

    fn parse_inner(data: &[u8], decode: bool) -> Result<Self> {
        if data.starts_with(b"RIFF") {
            parse_riff(data, decode)
        } else if data.starts_with(b"ID3") || is_mpeg_sync(data) {
            parse_mp3(data, decode)
        } else {
            Err(Error::Magic { expected: "RIFF or MPEG audio", found: data[..data.len().min(4)].to_vec() })
        }
    }
}

struct Format {
    tag: u16,
    channels: u16,
    rate: u32,
    block_align: u16,
    bits: u16,
    samples_per_block: u16,
    coefs: Vec<(i32, i32)>,
}

const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_ADPCM: u16 = 2;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

fn parse_format(body: &[u8]) -> Result<Format> {
    let mut r = Reader::new(body);
    let mut tag = r.u16()?;
    let channels = r.u16()?;
    let rate = r.u32()?;
    let _avg_bytes = r.u32()?;
    let block_align = r.u16()?;
    let bits = r.u16()?;
    let extra = if r.remaining() >= 2 { r.u16()? as usize } else { 0 };
    let mut samples_per_block = 0;
    let mut coefs = Vec::new();
    if tag == WAVE_FORMAT_EXTENSIBLE && extra >= 22 {
        r.skip(6)?; // valid bits, channel mask
        tag = r.u16()?; // first two bytes of the sub-format GUID
    } else if tag == WAVE_FORMAT_ADPCM && extra >= 4 {
        samples_per_block = r.u16()?;
        let count = r.u16()?;
        for _ in 0..count {
            coefs.push((r.i16()? as i32, r.i16()? as i32));
        }
    }
    if channels == 0 || rate == 0 {
        return invalid(format!("wave format with {channels} channels at {rate} Hz"));
    }
    Ok(Format { tag, channels, rate, block_align, bits, samples_per_block, coefs })
}

fn parse_riff(data: &[u8], decode: bool) -> Result<Wave> {
    if data.len() < 12 || &data[8..12] != b"WAVE" {
        return Err(Error::Magic { expected: "WAVE", found: data[8.min(data.len())..12.min(data.len())].to_vec() });
    }
    let mut wave = Wave::default();
    let mut format = None;
    let mut samples: &[u8] = &[];
    let mut pos = 12;
    while pos + 8 <= data.len() {
        let id: [u8; 4] = data[pos..pos + 4].try_into().unwrap();
        let size = u32::from_le_bytes(data[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let start = pos + 8;
        // Lenient about truncated files: a chunk never reaches past the data.
        let body = &data[start..start.saturating_add(size).min(data.len())];
        match &id {
            b"fmt " => format = Some(parse_format(body)?),
            b"data" => samples = body,
            b"cue " if body.len() >= 4 => {
                let mut r = Reader::new(body);
                let count = r.u32()? as usize;
                let first = wave.cues.len();
                for _ in 0..count.min(r.remaining() / 24) {
                    let id = r.u32()?;
                    let position = r.u32()?;
                    r.skip(12)?; // chunk id, chunk start, block start
                    wave.cues.push(Cue { id, position, sample_offset: r.u32()? });
                }
                if let Some(cue) = wave.cues.get(first) {
                    wave.loop_start = Some(cue.sample_offset);
                }
            }
            b"smpl" if body.len() >= 36 => {
                let mut r = Reader::at(body, 28);
                let loops = r.u32()?;
                r.skip(4)?; // sampler data size
                if loops > 0 && r.remaining() >= 24 {
                    r.skip(4)?; // cue point id
                    let kind = r.u32()?;
                    let start = r.u32()?;
                    let end = r.u32()?;
                    if kind == 0 {
                        wave.loop_start = Some(start);
                        wave.loop_end = Some(end.saturating_add(1));
                    }
                }
            }
            b"LIST" if body.starts_with(b"adtl") => parse_adtl(&body[4..], &mut wave.labels),
            b"LIST" | b"fact" => {}
            b"VDAT" => {
                let text = std::str::from_utf8(body).ok().map(|t| t.trim_end_matches('\0'));
                wave.sentence = Some(match text.and_then(Sentence::parse) {
                    Some(s) => SentenceData::Parsed(s),
                    None => SentenceData::Raw(body.to_vec()),
                });
            }
            _ => wave.other_chunks.push(id),
        }
        pos = start.saturating_add(size).saturating_add(size & 1);
    }
    let Some(f) = format else { return invalid("wave without fmt chunk") };
    wave.channels = f.channels;
    wave.rate = f.rate;
    wave.bits = f.bits;
    match f.tag {
        WAVE_FORMAT_PCM => {
            if f.bits != 8 && f.bits != 16 {
                return invalid(format!("unsupported {}-bit PCM", f.bits));
            }
            let frame_bytes = f.channels as usize * (f.bits as usize / 8);
            wave.frames = (samples.len() / frame_bytes) as u32;
            if decode {
                let samples = &samples[..wave.frames as usize * frame_bytes];
                wave.samples = if f.bits == 8 {
                    samples.iter().map(|&b| ((b as i16) - 128) << 8).collect()
                } else {
                    samples.as_chunks::<2>().0.iter().map(|&c| i16::from_le_bytes(c)).collect()
                };
            }
        }
        WAVE_FORMAT_ADPCM => {
            wave.codec = Codec::Adpcm;
            let adpcm = AdpcmLayout::new(&f)?;
            wave.frames = adpcm.frames(samples.len());
            if decode {
                wave.samples = adpcm.decode(samples)?;
            }
        }
        tag => {
            wave.codec = Codec::Other(tag);
            if f.block_align > 0 && f.bits > 0 {
                wave.frames = (samples.len() / f.block_align as usize) as u32;
            }
            if decode {
                wave.encoded = samples.to_vec();
            }
        }
    }
    Ok(wave)
}

fn parse_adtl(mut data: &[u8], out: &mut Vec<Label>) {
    while data.len() >= 8 {
        let kind: [u8; 4] = data[..4].try_into().unwrap();
        let size = u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize;
        let body = &data[8..8usize.saturating_add(size).min(data.len())];
        let word = |at: usize| body.get(at..at + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
        let label = match (&kind, word(0)) {
            (b"labl" | b"note", Some(cue)) => Some(Label { cue, kind, text: str_from_nul(&body[4..]), length: None }),
            (b"ltxt", Some(cue)) => {
                let text = body.get(20..).map(str_from_nul).unwrap_or_default();
                Some(Label { cue, kind, text, length: word(4) })
            }
            _ => None,
        };
        out.extend(label);
        data = &data[8usize.saturating_add(size).saturating_add(size & 1).min(data.len())..];
    }
}

// Microsoft ADPCM: per-channel block headers (predictor, delta, two seed
// samples) followed by 4-bit codes, high nibble first.
const ADPCM_ADAPT: [i32; 16] = [230, 230, 230, 230, 307, 409, 512, 614, 768, 614, 512, 409, 307, 230, 230, 230];
const ADPCM_COEFS: [(i32, i32); 7] = [(256, 0), (512, -256), (0, 0), (192, 64), (240, 0), (460, -208), (392, -232)];

struct AdpcmLayout {
    channels: usize,
    block_align: usize,
    samples_per_block: usize,
    coefs: Vec<(i32, i32)>,
}

impl AdpcmLayout {
    fn new(f: &Format) -> Result<Self> {
        let channels = f.channels as usize;
        let block_align = f.block_align as usize;
        if block_align < 7 * channels {
            return invalid(format!("ADPCM block of {block_align} bytes for {channels} channels"));
        }
        let samples_per_block = match f.samples_per_block as usize {
            0 => (block_align - 7 * channels) * 2 / channels + 2,
            n => n,
        };
        let coefs = if f.coefs.is_empty() { ADPCM_COEFS.to_vec() } else { f.coefs.clone() };
        Ok(Self { channels, block_align, samples_per_block, coefs })
    }

    /// Sample frames held by a block of `bytes` (full or the short last one).
    fn block_frames(&self, bytes: usize) -> usize {
        if bytes < 7 * self.channels {
            return 0;
        }
        (2 + (bytes - 7 * self.channels) * 2 / self.channels).min(self.samples_per_block)
    }

    fn frames(&self, len: usize) -> u32 {
        let full = len / self.block_align;
        (full * self.block_frames(self.block_align) + self.block_frames(len % self.block_align)) as u32
    }

    fn decode(&self, data: &[u8]) -> Result<Vec<i16>> {
        let ch = self.channels;
        let mut out = Vec::with_capacity(self.frames(data.len()) as usize * ch);
        for block in data.chunks(self.block_align) {
            let frames = self.block_frames(block.len());
            if frames == 0 {
                break;
            }
            let mut r = Reader::new(block);
            let mut state = Vec::with_capacity(ch);
            for _ in 0..ch {
                let predictor = r.u8()? as usize;
                let Some(&coef) = self.coefs.get(predictor) else {
                    return invalid(format!("ADPCM predictor {predictor} out of range"));
                };
                state.push(AdpcmChannel { coef, delta: 0, s1: 0, s2: 0 });
            }
            for c in state.iter_mut() {
                c.delta = r.i16()? as i32;
            }
            for c in state.iter_mut() {
                c.s1 = r.i16()? as i32;
            }
            for c in state.iter_mut() {
                c.s2 = r.i16()? as i32;
            }
            out.extend(state.iter().map(|c| c.s2 as i16));
            out.extend(state.iter().map(|c| c.s1 as i16));
            let codes = &block[7 * ch..];
            for i in 0..(frames - 2) * ch {
                let byte = codes[i / 2];
                let nibble = if i % 2 == 0 { byte >> 4 } else { byte & 0x0F };
                out.push(state[i % ch].step(nibble));
            }
        }
        Ok(out)
    }
}

struct AdpcmChannel {
    coef: (i32, i32),
    delta: i32,
    s1: i32,
    s2: i32,
}

impl AdpcmChannel {
    fn step(&mut self, nibble: u8) -> i16 {
        let signed = ((nibble as i32) << 28) >> 28;
        let predicted = (self.s1 * self.coef.0 + self.s2 * self.coef.1) >> 8;
        let sample = (predicted + signed * self.delta).clamp(i16::MIN as i32, i16::MAX as i32);
        self.s2 = self.s1;
        self.s1 = sample;
        self.delta = ((ADPCM_ADAPT[nibble as usize] * self.delta) >> 8).max(16);
        sample as i16
    }
}

fn is_mpeg_sync(b: &[u8]) -> bool {
    b.len() >= 2 && b[0] == 0xFF && b[1] & 0xE0 == 0xE0
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct FrameHeader {
    version: u8,
    layer: u8,
    crc: bool,
    kbps: u16,
    rate: u32,
    channels: u16,
    size: usize,
    samples: u32,
}

const KBPS_V1: [[u16; 15]; 3] = [
    [0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448],
    [0, 32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384],
    [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320],
];
const KBPS_V2: [[u16; 15]; 2] =
    [[0, 32, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256], [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160]];

fn frame_header(h: &[u8]) -> Option<FrameHeader> {
    if h.len() < 4 || !is_mpeg_sync(h) {
        return None;
    }
    let version = match (h[1] >> 3) & 3 {
        0 => 25,
        2 => 2,
        3 => 1,
        _ => return None,
    };
    let layer = match (h[1] >> 1) & 3 {
        1 => 3,
        2 => 2,
        3 => 1,
        _ => return None,
    };
    let index = (h[2] >> 4) as usize;
    if index == 0 || index == 15 {
        return None; // free format or invalid
    }
    let kbps = if version == 1 { KBPS_V1[layer as usize - 1][index] } else { KBPS_V2[(layer > 1) as usize][index] };
    let base = match (h[2] >> 2) & 3 {
        0 => 44100,
        1 => 48000,
        2 => 32000,
        _ => return None,
    };
    let rate = match version {
        1 => base,
        2 => base / 2,
        _ => base / 4,
    };
    let padding = ((h[2] >> 1) & 1) as usize;
    let bitrate = kbps as usize * 1000;
    let rate_us = rate as usize;
    let (size, samples) = match layer {
        1 => ((12 * bitrate / rate_us + padding) * 4, 384),
        2 => (144 * bitrate / rate_us + padding, 1152),
        _ if version == 1 => (144 * bitrate / rate_us + padding, 1152),
        _ => (72 * bitrate / rate_us + padding, 576),
    };
    let channels = if h[3] >> 6 == 3 { 1 } else { 2 };
    Some(FrameHeader { version, layer, crc: h[1] & 1 == 0, kbps, rate, channels, size, samples })
}

fn parse_mp3(data: &[u8], decode: bool) -> Result<Wave> {
    let mut pos = 0;
    if data.starts_with(b"ID3") && data.len() >= 10 {
        let size = data[6..10].iter().fold(0usize, |acc, &b| acc << 7 | (b & 0x7F) as usize);
        pos = 10 + size + if data[5] & 0x10 != 0 { 10 } else { 0 };
        if pos > data.len() {
            return Err(Error::Eof);
        }
    }
    let mut end = data.len();
    if end >= pos + 128 && &data[end - 128..end - 125] == b"TAG" {
        end -= 128;
    }
    let begin = pos;
    let mut first: Option<FrameHeader> = None;
    let mut frames = 0u32;
    let mut kbps = (u16::MAX, 0);
    while let Some(h) = frame_header(&data[pos..end]) {
        if pos + h.size > end {
            break;
        }
        if let Some(f) = first {
            if (f.version, f.layer, f.rate, f.channels) != (h.version, h.layer, h.rate, h.channels) {
                return invalid(format!("MPEG stream changes format at byte {pos}"));
            }
        } else {
            first = Some(h);
        }
        frames += 1;
        kbps = (kbps.0.min(h.kbps), kbps.1.max(h.kbps));
        pos += h.size;
    }
    let Some(f) = first else { return invalid("no MPEG audio frames") };
    Ok(Wave {
        codec: Codec::Mp3,
        channels: f.channels,
        rate: f.rate,
        bits: 0,
        frames: frames * f.samples,
        encoded: if decode { data[begin..pos].to_vec() } else { Vec::new() },
        mp3: Some(Mp3Info {
            version: f.version,
            layer: f.layer,
            frame_count: frames,
            samples_per_frame: f.samples,
            kbps,
            crc: f.crc,
            trailing: end - pos,
        }),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn riff(chunks: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
        let mut body = b"WAVE".to_vec();
        for (id, data) in chunks {
            body.extend_from_slice(*id);
            body.extend_from_slice(&(data.len() as u32).to_le_bytes());
            body.extend_from_slice(data);
            if data.len() % 2 == 1 {
                body.push(0);
            }
        }
        let mut out = b"RIFF".to_vec();
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend(body);
        out
    }

    fn fmt(tag: u16, channels: u16, rate: u32, align: u16, bits: u16, extra: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&tag.to_le_bytes());
        v.extend_from_slice(&channels.to_le_bytes());
        v.extend_from_slice(&rate.to_le_bytes());
        v.extend_from_slice(&(rate * align as u32).to_le_bytes());
        v.extend_from_slice(&align.to_le_bytes());
        v.extend_from_slice(&bits.to_le_bytes());
        if !extra.is_empty() {
            v.extend_from_slice(&(extra.len() as u16).to_le_bytes());
            v.extend_from_slice(extra);
        }
        v
    }

    fn u32s(values: &[u32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    #[test]
    fn pcm_and_loops() {
        let data: Vec<u8> = [0i16, 1000, -1000, 32767].iter().flat_map(|s| s.to_le_bytes()).collect();
        let cue = u32s(&[1, 1, 7, 0, 0, 0, 1]);
        let smpl = u32s(&[0, 0, 0, 60, 0, 0, 0, 1, 0, 1, 0, 1, 2, 0, 0]);
        let wav = riff(&[(b"fmt ", fmt(1, 2, 22050, 4, 16, &[])), (b"cue ", cue), (b"data", data), (b"smpl", smpl)]);
        let w = Wave::parse(&wav).unwrap();
        assert_eq!((w.codec, w.channels, w.rate, w.bits, w.frames), (Codec::Pcm, 2, 22050, 16, 2));
        assert_eq!(w.samples, vec![0, 1000, -1000, 32767]);
        assert_eq!(w.cues, vec![Cue { id: 1, position: 7, sample_offset: 1 }]);
        // `smpl` comes after `cue ` and wins, as in the engine.
        assert_eq!((w.loop_start, w.loop_end), (Some(1), Some(3)));

        let eight = riff(&[(b"fmt ", fmt(1, 1, 11025, 1, 8, &[])), (b"data", vec![0, 128, 255])]);
        assert_eq!(Wave::parse(&eight).unwrap().samples, vec![-32768, 0, 32512]);
    }

    #[test]
    fn labels_and_sentence() {
        let mut adtl = b"adtl".to_vec();
        adtl.extend_from_slice(b"labl");
        adtl.extend_from_slice(&9u32.to_le_bytes());
        adtl.extend_from_slice(&1u32.to_le_bytes());
        adtl.extend_from_slice(b"loop\0\0");
        adtl.extend_from_slice(b"ltxt");
        adtl.extend_from_slice(&20u32.to_le_bytes());
        adtl.extend_from_slice(&u32s(&[1, 500]));
        adtl.extend_from_slice(b"rgn \0\0\0\0\0\0\0\0");
        let vdat = b"VERSION 1.0\nPLAINTEXT\n{\nhello there\n}\nWORDS\n{\nWORD hello 0.1 0.4\n{\n104 h 0.1 0.2 1\n201 e 0.2 0.4 0.8\n}\n}\nEMPHASIS\n{\n0.0 0.5\n}\nCLOSECAPTION\n{\nenglish\n{\nx\n}\n}\nOPTIONS\n{\nvoice_duck 1\n}\n".to_vec();
        let wav = riff(&[(b"fmt ", fmt(1, 1, 44100, 2, 16, &[])), (b"data", vec![0; 4]), (b"LIST", adtl), (b"VDAT", vdat)]);
        let w = Wave::parse(&wav).unwrap();
        assert_eq!(w.labels.len(), 2);
        assert_eq!((w.labels[0].kind, w.labels[0].text.as_str()), (*b"labl", "loop"));
        assert_eq!(w.labels[1].length, Some(500));
        let Some(SentenceData::Parsed(s)) = &w.sentence else { panic!("sentence not parsed") };
        assert_eq!(s.text, "hello there");
        assert_eq!(s.words[0].phonemes[1].name, "e");
        assert_eq!(s.emphasis, vec![(0.0, 0.5)]);
        assert!(s.voice_duck());
        assert_eq!(w.loop_start, None);
    }

    #[test]
    fn adpcm_block() {
        // Mono block: predictor 0 (256, 0), delta 16, seeds 100 / 50, then
        // codes 1 and -1. Expected values follow the MS ADPCM update rule.
        let mut extra = Vec::new();
        extra.extend_from_slice(&4u16.to_le_bytes()); // samples per block
        extra.extend_from_slice(&7u16.to_le_bytes());
        for (a, b) in ADPCM_COEFS {
            extra.extend_from_slice(&(a as i16).to_le_bytes());
            extra.extend_from_slice(&(b as i16).to_le_bytes());
        }
        let mut block = vec![0u8];
        block.extend_from_slice(&16i16.to_le_bytes());
        block.extend_from_slice(&100i16.to_le_bytes());
        block.extend_from_slice(&50i16.to_le_bytes());
        block.push(0x1F);
        let wav = riff(&[(b"fmt ", fmt(2, 1, 22050, 8, 4, &extra)), (b"data", block)]);
        let w = Wave::parse(&wav).unwrap();
        assert_eq!(w.codec, Codec::Adpcm);
        assert_eq!(w.frames, 4);
        // 100 + 1*16 = 116, delta stays 16; then 116 - 16 = 100.
        assert_eq!(w.samples, vec![50, 100, 116, 100]);
    }

    #[test]
    fn mpeg_frames() {
        // Two MPEG-1 Layer III mono frames at 64 kbit/s, 44.1 kHz: 208 bytes each.
        let mut frame = vec![0u8; 208];
        frame[..4].copy_from_slice(&[0xFF, 0xFB, 0x50, 0xC0]);
        let mut data = frame.clone();
        data.extend_from_slice(&frame);
        let w = Wave::parse(&data).unwrap();
        assert_eq!((w.codec, w.channels, w.rate, w.frames), (Codec::Mp3, 1, 44100, 2304));
        let info = w.mp3.unwrap();
        assert_eq!((info.frame_count, info.kbps, info.crc, info.trailing), (2, (64, 64), false, 0));
        assert_eq!(w.encoded.len(), 416);
        assert!(Wave::parse(b"junk").is_err());
    }

    /// Decodes every `.wav` in the installation (skipped without one) and
    /// reports how many there are of each format.
    #[test]
    fn install_every_wave_decodes() {
        let Some(fs) = GameFileSystem::find_install().and_then(|root| GameFileSystem::portal2(root).ok()) else {
            eprintln!("Portal 2 installation not found; skipping");
            return;
        };
        let paths = fs.list("sound", Some("wav"), true);
        assert!(!paths.is_empty(), "no sound/*.wav files found");
        let check = |path: &String| -> std::result::Result<(String, Wave), String> {
            let data = fs.read(path).ok_or("unreadable")?;
            let mut w = Wave::parse(&data).map_err(|e| e.to_string())?;
            let decoded = matches!(w.codec, Codec::Pcm | Codec::Adpcm);
            if decoded && w.samples.len() != w.frames as usize * w.channels as usize {
                return Err(format!("{} samples for {} frames", w.samples.len(), w.frames));
            }
            if w.frames == 0 {
                return Err("no audio".into());
            }
            let key = match w.codec {
                Codec::Mp3 => {
                    let m = w.mp3.unwrap();
                    format!("MPEG-{} layer {} {} kbit/s", m.version, m.layer, m.kbps.1)
                }
                Codec::Pcm => format!("PCM {}-bit", w.bits),
                c => format!("{c:?}"),
            };
            let key = format!("{key}, {} ch, {} Hz", w.channels, w.rate);
            w.samples = Vec::new();
            w.encoded = Vec::new();
            Ok((key, w))
        };
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
        let results: Vec<_> = std::thread::scope(|s| {
            let jobs: Vec<_> = paths
                .chunks(paths.len().div_ceil(threads))
                .map(|chunk| s.spawn(move || chunk.iter().map(|p| (p, check(p))).collect::<Vec<_>>()))
                .collect();
            jobs.into_iter().flat_map(|j| j.join().unwrap()).collect()
        });

        let mut formats = std::collections::BTreeMap::<String, usize>::new();
        let (mut looped, mut sentences, mut labelled, mut trailing, mut seconds) = (0, 0, 0, 0, 0.0f64);
        let mut failures = Vec::new();
        for (path, r) in &results {
            match r {
                Ok((key, w)) => {
                    *formats.entry(key.clone()).or_default() += 1;
                    looped += w.loop_start.is_some() as usize;
                    sentences += w.sentence.is_some() as usize;
                    labelled += !w.labels.is_empty() as usize;
                    trailing += w.mp3.is_some_and(|m| m.trailing > 0) as usize;
                    seconds += w.duration() as f64;
                }
                Err(e) => failures.push(format!("{path}: {e}")),
            }
        }
        println!("{} wave files, {:.1} hours of audio", results.len(), seconds / 3600.0);
        for (k, n) in &formats {
            println!("  {n:5}  {k}");
        }
        println!("  looped {looped}, with sentence data {sentences}, with labels {labelled}, MPEG with trailing bytes {trailing}");
        for f in failures.iter().take(20) {
            println!("  FAILED {f}");
        }
        assert!(failures.is_empty(), "{} of {} waves failed to decode", failures.len(), results.len());
    }
}
