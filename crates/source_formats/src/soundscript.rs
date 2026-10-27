//! Sound scripts: `scripts/game_sounds_manifest.txt` and the files it lists.
//!
//! Every top-level block in a script is a named sound entry (`Category.Name`)
//! with channel, volume, pitch, sound level and the waves to play; scenes,
//! entities and captions refer to sounds by that name. Values are read the way
//! the engine's sound emitter reads
//! them: whole-value names (`VOL_NORM`, `PITCH_NORM`, `SNDLVL_*`, `ATTN_*`) or
//! comma ranges (`"95,105"`). Portal 2's version-2 entries add
//! `operator_stacks`, kept as raw KeyValues.
//!
//! When a name is defined twice the first definition wins, as in the engine;
//! only map override files (`maps/<map>_level_sounds.txt`) replace entries.

use std::collections::HashMap;

use crate::fs::GameFileSystem;
use crate::kv::{self, KeyValues, parse_f32_lenient, parse_i32_lenient};
use crate::text::normalize_path;
use crate::{Error, Result};

pub const MANIFEST: &str = "scripts/game_sounds_manifest.txt";

/// Sound level used when an entry sets none (`SNDLVL_NORM`).
pub const SNDLVL_NORM: u8 = 75;

/// Closed range a value is picked from (uniformly) each time a sound starts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Interval {
    pub start: f32,
    pub end: f32,
}

impl Interval {
    pub const fn fixed(v: f32) -> Self {
        Self { start: v, end: v }
    }

    /// `"a"` or `"a,b"`: comma-separated parts (empty ones skipped), each read
    /// like C `atof`. A single value gives a fixed interval.
    pub fn parse(s: &str) -> Self {
        let mut parts = s.split(',').filter(|p| !p.is_empty());
        let start = parts.next().map(parse_f32_lenient).unwrap_or(0.0);
        let end = parts.next().map(parse_f32_lenient).unwrap_or(start);
        Self { start, end }
    }

    pub fn is_fixed(&self) -> bool {
        self.start == self.end
    }

    /// Value at `t` in `0..=1`.
    pub fn lerp(&self, t: f32) -> f32 {
        self.start + (self.end - self.start) * t
    }
}

/// `volume`: `VOL_NORM` (1.0) or a range.
pub fn parse_volume(s: &str) -> Interval {
    if s.eq_ignore_ascii_case("VOL_NORM") { Interval::fixed(1.0) } else { Interval::parse(s) }
}

/// `pitch` in percent: `PITCH_NORM` (100), `PITCH_LOW` (95), `PITCH_HIGH` (120) or a range.
pub fn parse_pitch(s: &str) -> Interval {
    match s.to_ascii_uppercase().as_str() {
        "PITCH_NORM" => Interval::fixed(100.0),
        "PITCH_LOW" => Interval::fixed(95.0),
        "PITCH_HIGH" => Interval::fixed(120.0),
        _ => Interval::parse(s),
    }
}

const SOUNDLEVELS: [(&str, u8); 30] = [
    ("SNDLVL_NONE", 0),
    ("SNDLVL_20dB", 20),
    ("SNDLVL_25dB", 25),
    ("SNDLVL_30dB", 30),
    ("SNDLVL_35dB", 35),
    ("SNDLVL_40dB", 40),
    ("SNDLVL_45dB", 45),
    ("SNDLVL_50dB", 50),
    ("SNDLVL_55dB", 55),
    ("SNDLVL_IDLE", 60),
    ("SNDLVL_TALKING", 80),
    ("SNDLVL_60dB", 60),
    ("SNDLVL_65dB", 65),
    ("SNDLVL_STATIC", 66),
    ("SNDLVL_70dB", 70),
    ("SNDLVL_NORM", 75),
    ("SNDLVL_75dB", 75),
    ("SNDLVL_80dB", 80),
    ("SNDLVL_85dB", 85),
    ("SNDLVL_90dB", 90),
    ("SNDLVL_95dB", 95),
    ("SNDLVL_100dB", 100),
    ("SNDLVL_105dB", 105),
    ("SNDLVL_110dB", 110),
    ("SNDLVL_120dB", 120),
    ("SNDLVL_130dB", 130),
    ("SNDLVL_GUNFIRE", 140),
    ("SNDLVL_140dB", 140),
    ("SNDLVL_150dB", 150),
    ("SNDLVL_180dB", 180),
];

fn strip_prefix_ignore_case<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix).then(|| &s[prefix.len()..])
}

/// A `SNDLVL_*` name in dB: the engine's table (case-insensitive), otherwise
/// the number after `SNDLVL_` when it is in 1..=180 (`SNDLVL_125dB`). The
/// engine falls back to [`SNDLVL_NORM`] for anything else.
pub fn soundlevel_by_name(s: &str) -> Option<u8> {
    if let Some(&(_, level)) = SOUNDLEVELS.iter().find(|(name, _)| name.eq_ignore_ascii_case(s)) {
        return Some(level);
    }
    let n = parse_i32_lenient(strip_prefix_ignore_case(s, "SNDLVL_")?);
    (1..=180).contains(&n).then_some(n as u8)
}

/// `soundlevel` in dB: a `SNDLVL_` name, or a range of plain numbers.
pub fn parse_soundlevel(s: &str) -> Interval {
    if strip_prefix_ignore_case(s, "SNDLVL_").is_some() {
        Interval::fixed(soundlevel_by_name(s).unwrap_or(SNDLVL_NORM) as f32)
    } else {
        let i = Interval::parse(s);
        Interval { start: i.start.trunc(), end: i.start.trunc() + (i.end - i.start).trunc() }
    }
}

/// `ATTN_*` names. The engine maps unknown names to `ATTN_NORM` (0.8).
pub fn attenuation_by_name(s: &str) -> Option<f32> {
    Some(match s.to_ascii_uppercase().as_str() {
        "ATTN_NONE" => 0.0,
        "ATTN_NORM" => 0.8,
        "ATTN_IDLE" => 2.0,
        "ATTN_STATIC" => 1.25,
        "ATTN_RICOCHET" => 1.5,
        "ATTN_GUNFIRE" => 0.27,
        _ => return None,
    })
}

/// Old-style distance attenuation as a sound level: `50 + 20 / a` dB
/// (truncated), 0 for no attenuation.
pub fn attenuation_to_soundlevel(a: f32) -> f32 {
    if a == 0.0 { 0.0 } else { (50.0 + 20.0 / a).trunc() }
}

/// `attenuation` converted to a sound level range in dB.
pub fn parse_attenuation(s: &str) -> Interval {
    if strip_prefix_ignore_case(s, "ATTN_").is_some() {
        Interval::fixed(attenuation_to_soundlevel(attenuation_by_name(s).unwrap_or(0.8)))
    } else {
        let i = Interval::parse(s);
        Interval { start: attenuation_to_soundlevel(i.start), end: attenuation_to_soundlevel(i.end) }
    }
}

/// Entity sound channel. Sounds on the same entity and channel replace each
/// other, except on `Auto` (a free slot is picked) and `Static`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Channel {
    #[default]
    Auto,
    Weapon,
    Voice,
    Item,
    Body,
    Stream,
    Static,
    /// Plain channel number from the script.
    Other(i32),
}

impl Channel {
    const NAMES: [(&'static str, Channel); 7] = [
        ("CHAN_AUTO", Channel::Auto),
        ("CHAN_WEAPON", Channel::Weapon),
        ("CHAN_VOICE", Channel::Voice),
        ("CHAN_ITEM", Channel::Item),
        ("CHAN_BODY", Channel::Body),
        ("CHAN_STREAM", Channel::Stream),
        ("CHAN_STATIC", Channel::Static),
    ];

    pub fn from_index(i: i32) -> Self {
        match i {
            0 => Channel::Auto,
            1 => Channel::Weapon,
            2 => Channel::Voice,
            3 => Channel::Item,
            4 => Channel::Body,
            5 => Channel::Stream,
            6 => Channel::Static,
            n => Channel::Other(n),
        }
    }

    pub fn index(self) -> i32 {
        match self {
            Channel::Auto => 0,
            Channel::Weapon => 1,
            Channel::Voice => 2,
            Channel::Item => 3,
            Channel::Body => 4,
            Channel::Stream => 5,
            Channel::Static => 6,
            Channel::Other(n) => n,
        }
    }

    /// A `CHAN_*` name (case-insensitive) or, like the engine, any value not
    /// starting with `chan_` read as a number. Unknown `CHAN_` names give `None`
    /// (the engine uses `Auto`).
    pub fn parse(s: &str) -> Option<Self> {
        if strip_prefix_ignore_case(s, "chan_").is_none() {
            return Some(Channel::from_index(parse_i32_lenient(s)));
        }
        Self::NAMES.iter().find(|(name, _)| name.eq_ignore_ascii_case(s)).map(|&(_, c)| c)
    }
}

/// Prefix characters on a wave name that change how it is played. The engine
/// skips any number of them before the path.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SoundChars(u16);

impl SoundChars {
    /// `*` stream from disk instead of loading the whole file.
    pub const STREAM: Self = Self(1);
    /// `?` realtime voice chat data, not a file.
    pub const USER_VOX: Self = Self(1 << 1);
    /// `!` sentence name from `scripts/sentences.txt`, not a file.
    pub const SENTENCE: Self = Self(1 << 2);
    /// `#` bypasses DSP room effects.
    pub const DRY_MIX: Self = Self(1 << 3);
    /// `@` omnidirectional: no spatialisation.
    pub const OMNI: Self = Self(1 << 4);
    /// `(` directional stereo.
    pub const DIR_STEREO: Self = Self(1 << 5);
    /// `>` doppler stereo: left channel approaching, right receding.
    pub const DOPPLER: Self = Self(1 << 6);
    /// `<` directional: left channel facing the listener, right facing away.
    pub const DIRECTIONAL: Self = Self(1 << 7);
    /// `^` distance variant: left channel near, right far.
    pub const DIST_VARIANT: Self = Self(1 << 8);
    /// `)` spatialised stereo.
    pub const SPATIAL_STEREO: Self = Self(1 << 9);
    /// `}` cheap, non-interpolated pitch shifting.
    pub const FAST_PITCH: Self = Self(1 << 10);
    /// `$` the client emits the close caption even when it otherwise would not.
    pub const FORCE_CAPTION: Self = Self(1 << 11);

    const ALL: [(char, SoundChars); 12] = [
        ('*', Self::STREAM),
        ('?', Self::USER_VOX),
        ('!', Self::SENTENCE),
        ('#', Self::DRY_MIX),
        ('@', Self::OMNI),
        ('(', Self::DIR_STEREO),
        ('>', Self::DOPPLER),
        ('<', Self::DIRECTIONAL),
        ('^', Self::DIST_VARIANT),
        (')', Self::SPATIAL_STEREO),
        ('}', Self::FAST_PITCH),
        ('$', Self::FORCE_CAPTION),
    ];

    pub fn from_char(c: char) -> Option<Self> {
        Self::ALL.iter().find(|(ch, _)| *ch == c).map(|&(_, f)| f)
    }

    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn bits(self) -> u16 {
        self.0
    }

    /// The spatial mode the engine picks when several are given: `(` beats
    /// `)`, which beats `@`, `^`, `<` and `>` in that order.
    pub fn spatial_mode(self) -> Option<Self> {
        [Self::DIR_STEREO, Self::SPATIAL_STEREO, Self::OMNI, Self::DIST_VARIANT, Self::DIRECTIONAL, Self::DOPPLER]
            .into_iter()
            .find(|&m| self.contains(m))
    }
}

impl std::ops::BitOr for SoundChars {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for SoundChars {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

/// Splits leading sound characters off a wave name.
pub fn split_sound_chars(name: &str) -> (SoundChars, &str) {
    let mut chars = SoundChars::default();
    for (i, c) in name.char_indices() {
        match SoundChars::from_char(c) {
            Some(f) => chars |= f,
            None => return (chars, &name[i..]),
        }
    }
    (chars, "")
}

/// A wave reference from a script.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WaveName {
    /// As written, including sound characters.
    pub raw: String,
    pub chars: SoundChars,
    /// Path relative to `sound/` without the sound characters.
    pub path: String,
}

impl WaveName {
    pub fn parse(raw: &str) -> Self {
        let (chars, path) = split_sound_chars(raw);
        Self { raw: raw.to_owned(), chars, path: path.to_owned() }
    }

    /// Normalised game path (`sound/...`), or `None` for sentence and voice
    /// chat names, which are not files.
    pub fn file_path(&self) -> Option<String> {
        if self.chars.contains(SoundChars::SENTENCE) || self.chars.contains(SoundChars::USER_VOX) {
            return None;
        }
        Some(normalize_path(&format!("sound/{}", self.path)))
    }
}

/// Version-2 entries keep at most this many `rndwave` waves.
pub const MAX_RNDWAVES: usize = 63;

#[derive(Clone, Debug)]
pub struct SoundEntry {
    /// Name as written (lookups are case-insensitive).
    pub name: String,
    /// Script file it came from.
    pub file: String,
    pub channel: Channel,
    pub volume: Interval,
    /// Percent; 100 plays at the recorded rate.
    pub pitch: Interval,
    /// dB, from `soundlevel` or the older `attenuation`.
    pub soundlevel: Interval,
    /// Set by `CompatibilitySoundlevel` / `CompatibilityAttenuation`, which
    /// ask for the old attenuation curve.
    pub compatibility: bool,
    pub waves: Vec<WaveName>,
    pub delay_msec: i32,
    pub play_to_owner_only: bool,
    /// `soundentry_version`: 1 for classic entries, 2 for operator-stack entries.
    pub version: i32,
    pub operator_stacks: Option<KeyValues>,
    /// Listed with `preload_file` in the manifest.
    pub preload: bool,
    /// Keys the engine does not read.
    pub extra: Vec<KeyValues>,
}

impl SoundEntry {
    fn new(name: &str, file: &str) -> Self {
        Self {
            name: name.to_owned(),
            file: file.to_owned(),
            channel: Channel::Auto,
            volume: Interval::fixed(1.0),
            pitch: Interval::fixed(100.0),
            soundlevel: Interval::fixed(SNDLVL_NORM as f32),
            compatibility: false,
            waves: Vec::new(),
            delay_msec: 0,
            play_to_owner_only: false,
            version: 1,
            operator_stacks: None,
            preload: false,
            extra: Vec::new(),
        }
    }

    /// Parses one entry block; problems are appended to `warnings`.
    pub fn parse(kv: &KeyValues, file: &str, warnings: &mut Vec<String>) -> Self {
        let mut e = Self::new(&kv.key, file);
        if let Some(v) = kv.get_str("soundentry_version").filter(|v| !v.is_empty()) {
            e.version = parse_i32_lenient(v);
        }
        let mut warn = |msg: String| warnings.push(format!("{file}: {}: {msg}", kv.key));
        for c in kv.children() {
            let key = c.key.to_ascii_lowercase();
            match (key.as_str(), c.as_str()) {
                ("channel", Some(v)) => {
                    e.channel = Channel::parse(v).unwrap_or_else(|| {
                        warn(format!("unknown channel {v}"));
                        Channel::Auto
                    })
                }
                ("volume", Some(v)) => e.volume = parse_volume(v),
                ("pitch", Some(v)) => e.pitch = parse_pitch(v),
                ("wave", Some(v)) => e.waves.push(WaveName::parse(v)),
                ("rndwave", None) => {
                    for w in c.children().iter().filter_map(|w| w.as_str()) {
                        if e.version > 1 && e.waves.len() >= MAX_RNDWAVES {
                            warn(format!("more than {MAX_RNDWAVES} rndwave entries"));
                            break;
                        }
                        e.waves.push(WaveName::parse(w));
                    }
                }
                ("attenuation" | "compatibilityattenuation", Some(v)) => {
                    if strip_prefix_ignore_case(v, "ATTN_").is_some() && attenuation_by_name(v).is_none() {
                        warn(format!("unknown attenuation {v}"));
                    }
                    e.soundlevel = parse_attenuation(v);
                    e.compatibility = key.starts_with("compat");
                }
                ("soundlevel" | "compatibilitysoundlevel", Some(v)) => {
                    let named = strip_prefix_ignore_case(v, "SNDLVL_").is_some();
                    let numeric = v.trim_start().starts_with(|c: char| c.is_ascii_digit() || "+-.".contains(c));
                    if (named && soundlevel_by_name(v).is_none()) || (!named && !numeric) {
                        warn(format!("unknown sound level {v}"));
                    }
                    e.soundlevel = parse_soundlevel(v);
                    e.compatibility = key.starts_with("compat");
                }
                ("play_to_owner_only", Some(v)) => e.play_to_owner_only = parse_i32_lenient(v) != 0,
                ("delay_msec", Some(v)) => e.delay_msec = parse_i32_lenient(v).max(0),
                ("soundentry_version", _) => {}
                ("operator_stacks", None) => e.operator_stacks = Some(c.clone()),
                _ => e.extra.push(c.clone()),
            }
        }
        if e.waves.is_empty() {
            warn("no waves".to_owned());
        }
        e
    }
}

/// All sound entries from the manifest, keyed by lowercase name.
#[derive(Default)]
pub struct SoundScripts {
    entries: Vec<SoundEntry>,
    index: HashMap<String, usize>,
    /// Script files read, in order.
    pub files: Vec<String>,
    /// Files listed in the manifest that could not be read.
    pub missing_files: Vec<String>,
    /// `(name, file)` of definitions ignored because the name already existed.
    pub duplicates: Vec<(String, String)>,
    /// Values the engine would reject or replace with a default.
    pub warnings: Vec<String>,
}

impl SoundScripts {
    /// Reads [`MANIFEST`] and every script it lists.
    pub fn load(fs: &GameFileSystem) -> Result<Self> {
        let manifest = fs.read_string(MANIFEST).ok_or_else(|| Error::NotFound(MANIFEST.to_owned()))?;
        Ok(Self::from_manifest(&manifest, &mut |path| fs.read_string(path)))
    }

    /// Builds the set from manifest text; `read` returns a script's text.
    pub fn from_manifest(manifest: &str, read: &mut dyn FnMut(&str) -> Option<String>) -> Self {
        let mut scripts = Self::default();
        let Some(root) = kv::parse(manifest) else { return scripts };
        for c in root.children() {
            let Some(path) = c.as_str() else { continue };
            let preload = match c.key.to_ascii_lowercase().as_str() {
                "precache_file" | "autocache_file" => false,
                "preload_file" => true,
                other => {
                    scripts.warnings.push(format!("{MANIFEST}: unexpected key {other}"));
                    continue;
                }
            };
            let path = normalize_path(path);
            if scripts.files.contains(&path) {
                continue;
            }
            match read(&path) {
                Some(text) => scripts.add_file(&path, &text, preload),
                None => scripts.missing_files.push(path),
            }
        }
        scripts
    }

    /// Adds a script's entries. Names that already exist keep their first definition.
    pub fn add_file(&mut self, file: &str, text: &str, preload: bool) {
        self.add(file, text, preload, false);
    }

    /// Adds a map override script, replacing entries with the same name.
    pub fn add_overrides(&mut self, file: &str, text: &str) {
        self.add(file, text, false, true);
    }

    /// Applies `maps/<map>_level_sounds.txt` when the map has one.
    pub fn load_map_overrides(&mut self, fs: &GameFileSystem, map: &str) -> bool {
        let path = format!("maps/{map}_level_sounds.txt");
        let Some(text) = fs.read_string(&path) else { return false };
        self.add_overrides(&path, &text);
        true
    }

    fn add(&mut self, file: &str, text: &str, preload: bool, replace: bool) {
        self.files.push(file.to_owned());
        for root in kv::parse_document(text).roots {
            if root.children().is_empty() {
                continue;
            }
            if root.key.is_empty() {
                self.warnings.push(format!("{file}: unnamed entry block"));
                continue;
            }
            let mut entry = SoundEntry::parse(&root, file, &mut self.warnings);
            entry.preload = preload;
            let key = root.key.to_ascii_lowercase();
            match self.index.get(&key) {
                Some(&i) if replace => self.entries[i] = entry,
                Some(_) => self.duplicates.push((root.key.clone(), file.to_owned())),
                None => {
                    self.index.insert(key, self.entries.len());
                    self.entries.push(entry);
                }
            }
        }
    }

    pub fn get(&self, name: &str) -> Option<&SoundEntry> {
        self.index.get(&name.to_ascii_lowercase()).map(|&i| &self.entries[i])
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Entries in definition order.
    pub fn iter(&self) -> impl Iterator<Item = &SoundEntry> {
        self.entries.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values() {
        assert_eq!(Interval::parse("95, 105"), Interval { start: 95.0, end: 105.0 });
        assert_eq!(Interval::parse(".5"), Interval::fixed(0.5));
        assert_eq!(parse_volume("vol_norm"), Interval::fixed(1.0));
        assert_eq!(parse_pitch("PITCH_HIGH"), Interval::fixed(120.0));
        assert_eq!(parse_soundlevel("SNDLVL_90db"), Interval::fixed(90.0));
        assert_eq!(parse_soundlevel("SNDLVL_125dB"), Interval::fixed(125.0));
        assert_eq!(parse_soundlevel("SNDLVL_TALKING"), Interval::fixed(80.0));
        assert_eq!(parse_soundlevel("SNDLVL75dB"), Interval { start: 0.0, end: 0.0 });
        assert_eq!(parse_soundlevel("SNDLVL_bogus"), Interval::fixed(75.0));
        assert_eq!(parse_soundlevel("70,80"), Interval { start: 70.0, end: 80.0 });
        assert_eq!(parse_attenuation("0"), Interval::fixed(0.0));
        assert_eq!(parse_attenuation("ATTN_STATIC"), Interval::fixed(66.0));
        assert_eq!(Channel::parse("CHAN_VOICE"), Some(Channel::Voice));
        assert_eq!(Channel::parse("chan_static"), Some(Channel::Static));
        assert_eq!(Channel::parse("12"), Some(Channel::Other(12)));
        assert_eq!(Channel::parse("CHAN_VOICE2"), None);
    }

    #[test]
    fn sound_chars() {
        let w = WaveName::parse("*#)vo\\Glados\\Line.wav");
        assert!(w.chars.contains(SoundChars::STREAM | SoundChars::DRY_MIX));
        assert_eq!(w.chars.spatial_mode(), Some(SoundChars::SPATIAL_STEREO));
        assert_eq!(w.file_path().as_deref(), Some("sound/vo/glados/line.wav"));
        assert_eq!(WaveName::parse("!HG_ALERT").file_path(), None);
        assert_eq!(split_sound_chars("(@x").0.spatial_mode(), Some(SoundChars::DIR_STEREO));
    }

    #[test]
    fn manifest_order() {
        let manifest = r#"game_sounds_manifest
        {
            "precache_file" "scripts/a.txt"
            "preload_file" "scripts/b.txt"
            "precache_file" "scripts/missing.txt"
            "precache_file" "scripts/console.txt" [$GAMECONSOLE]
        }"#;
        let a = r#"
            "Test.One" { "channel" "CHAN_ITEM" "volume" "0.5,0.7" "soundlevel" "SNDLVL_75dB" "wave" "a/one.wav" }
            "Test.Two" { "rndwave" { "wave" "@a/x.wav" "wave" "a/y.wav" } "attenuation" "0.8" }
        "#;
        let b = r#"
            "test.one" { "wave" "b/one.wav" }
            "Test.Three" { "soundentry_version" "2" "wave" "b/three.wav" "operator_stacks" { "start_stack" { "import_stack" "x" } } }
        "#;
        let s = SoundScripts::from_manifest(manifest, &mut |p| match p {
            "scripts/a.txt" => Some(a.to_owned()),
            "scripts/b.txt" => Some(b.to_owned()),
            _ => None,
        });
        assert_eq!(s.len(), 3);
        assert_eq!(s.missing_files, vec!["scripts/missing.txt".to_owned()]);
        let one = s.get("TEST.ONE").unwrap();
        assert_eq!(
            (one.channel, one.volume, one.waves[0].path.as_str()),
            (Channel::Item, Interval { start: 0.5, end: 0.7 }, "a/one.wav")
        );
        assert_eq!(s.duplicates, vec![("test.one".to_owned(), "scripts/b.txt".to_owned())]);
        let two = s.get("test.two").unwrap();
        assert_eq!(two.waves.len(), 2);
        assert_eq!(two.soundlevel, Interval::fixed(75.0));
        let three = s.get("Test.Three").unwrap();
        assert!(three.preload && three.version == 2 && three.operator_stacks.is_some());

        let mut s = s;
        s.add_overrides("maps/x_level_sounds.txt", r#""Test.One" { "wave" "c/one.wav" }"#);
        assert_eq!(s.get("test.one").unwrap().waves[0].path, "c/one.wav");
    }

    /// Loads the installation's sound scripts (skipped without one) and checks
    /// the waves they name. The shipped scripts still reference several hundred
    /// Half-Life 2, Portal 1 and cut files that no VPK or loose directory of
    /// the game contains; those are reported, and the test only fails if the
    /// miss rate grows (which would point at a path handling bug).
    #[test]
    fn install_sound_scripts_resolve() {
        let Some(fs) = GameFileSystem::find_install().and_then(|root| GameFileSystem::portal2(root).ok()) else {
            eprintln!("Portal 2 installation not found; skipping");
            return;
        };
        let scripts = SoundScripts::load(&fs).unwrap();
        let v2 = scripts.iter().filter(|e| e.version > 1).count();
        let stacks = scripts.iter().filter(|e| e.operator_stacks.is_some()).count();
        println!(
            "{} entries from {} files ({v2} version 2, {stacks} with operator stacks), duplicate definitions ignored: {:?}",
            scripts.len(),
            scripts.files.len(),
            scripts.duplicates
        );
        let mut channels = std::collections::BTreeMap::<String, usize>::new();
        let mut chars = std::collections::BTreeMap::<char, usize>::new();
        let mut extra = std::collections::BTreeMap::<String, usize>::new();
        for e in scripts.iter() {
            *channels.entry(format!("{:?}", e.channel)).or_default() += 1;
            for k in &e.extra {
                *extra.entry(k.key.to_ascii_lowercase()).or_default() += 1;
            }
            for w in &e.waves {
                for (c, f) in SoundChars::ALL {
                    if w.chars.contains(f) {
                        *chars.entry(c).or_default() += 1;
                    }
                }
            }
        }
        println!("  channels {channels:?}");
        println!("  sound chars {chars:?}");
        println!("  keys the engine ignores {extra:?}");
        println!("  {} warnings", scripts.warnings.len());
        for w in scripts.warnings.iter().take(10) {
            println!("    {w}");
        }
        assert!(scripts.missing_files.is_empty(), "missing scripts {:?}", scripts.missing_files);
        assert!(scripts.len() > 1000);

        let mut checked = 0;
        let mut misses = std::collections::BTreeMap::<String, Vec<String>>::new();
        for e in scripts.iter() {
            for path in e.waves.iter().filter_map(WaveName::file_path) {
                checked += 1;
                if !fs.exists(&path) {
                    let dir = path.splitn(4, '/').take(3).collect::<Vec<_>>().join("/");
                    misses.entry(dir).or_default().push(e.name.clone());
                }
            }
        }
        let missing: usize = misses.values().map(Vec::len).sum();
        println!("  {checked} wave references, {missing} missing:");
        for (dir, names) in &misses {
            println!("    {:4}  {dir}  (e.g. {})", names.len(), names[0]);
        }
        assert!(checked > 5000);
        assert!(missing * 10 < checked, "{missing} of {checked} wave references do not resolve");
    }
}
