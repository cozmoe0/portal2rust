//! Soundscapes: `scripts/soundscapes_manifest.txt`, the files it lists and a
//! map's own `scripts/soundscapes_<map>.txt`.
//!
//! A soundscape (chosen by `env_soundscape` entities) sets the room DSP preset
//! and plays looping, random and nested sounds. Keys follow the client's
//! parser; a missing value is `None` and the defaults the client uses are noted
//! on the fields. A name defined more than once resolves to the last
//! definition, as in the client.

use std::collections::HashMap;

use glam::Vec3;

use crate::fs::GameFileSystem;
use crate::kv::{self, KeyValues, parse_f32_lenient, parse_i32_lenient};
use crate::soundscript::{Interval, WaveName, parse_attenuation, parse_soundlevel};
use crate::text::normalize_path;
use crate::{Error, Result};

pub const MANIFEST: &str = "scripts/soundscapes_manifest.txt";

/// Sound level of a `playlooping` sound without `soundlevel` / `attenuation`
/// (`ATTN_NORM` as the client converts it).
pub const LOOPING_DEFAULT_SOUNDLEVEL: f32 = 74.0;

/// Nesting depth after which the client stops following `playsoundscape`.
pub const MAX_NESTING: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Position {
    /// Index into the `env_soundscape`'s `position0`..`position7` targets,
    /// offset by any enclosing `playsoundscape` `position`.
    Index(i32),
    /// Somewhere random around the listener each time the sound plays.
    Random,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LoopingSound {
    pub wave: Option<WaveName>,
    /// Scaled by the enclosing volume. Missing means 0, and the client does
    /// not start a sound with volume 0.
    pub volume: Option<Interval>,
    /// Missing means 100.
    pub pitch: Option<Interval>,
    /// dB from `soundlevel` or `attenuation`; missing means
    /// [`LOOPING_DEFAULT_SOUNDLEVEL`].
    pub soundlevel: Option<Interval>,
    /// Without a position or origin the sound plays without spatialisation.
    pub position: Option<Position>,
    pub origin: Option<Vec3>,
    pub radius: Option<f32>,
    pub suppress_on_restore: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RandomSound {
    /// From the `rndwave` block; one is picked per play.
    pub waves: Vec<WaveName>,
    /// Seconds between plays.
    pub time: Option<Interval>,
    pub volume: Option<Interval>,
    pub pitch: Option<Interval>,
    pub soundlevel: Option<Interval>,
    pub position: Option<Position>,
    pub origin: Option<Vec3>,
    pub suppress_on_restore: bool,
}

/// `playsoundscape`: runs another soundscape inside this one.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SubSoundscape {
    pub name: String,
    /// Multiplies the nested sounds' volumes (missing means 1).
    pub volume: Option<Interval>,
    /// Added to the nested soundscape's position indices.
    pub position: Option<i32>,
    /// Plays every positioned and ambient sound of the nested soundscape at
    /// this position (unless an outer override is already active).
    pub position_override: Option<i32>,
    /// Plays the nested soundscape's unpositioned sounds at this position.
    pub ambient_position_override: Option<i32>,
    /// Plays the nested soundscape's unpositioned sounds at this point.
    pub ambient_origin_override: Option<Vec3>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Soundscape {
    pub name: String,
    /// Script file it came from.
    pub file: String,
    /// DSP preset index (`scripts/dsp_presets.txt`).
    pub dsp: Option<i32>,
    pub dsp_player: Option<i32>,
    pub dsp_volume: Option<f32>,
    /// Seconds to crossfade into this soundscape.
    pub fadetime: Option<f32>,
    pub soundmixer: Option<String>,
    pub looping: Vec<LoopingSound>,
    pub random: Vec<RandomSound>,
    pub nested: Vec<SubSoundscape>,
    /// Keys the client does not read.
    pub extra: Vec<KeyValues>,
}

/// `origin`-style vectors: comma-separated parts, each read like C `atof`
/// (so `"1, 2, 3;"` and `"1,2,3"` both work).
pub fn parse_origin(s: &str) -> Vec3 {
    let mut v = [0.0f32; 3];
    for (slot, part) in v.iter_mut().zip(s.split(',').filter(|p| !p.is_empty())) {
        *slot = parse_f32_lenient(part);
    }
    Vec3::from_array(v)
}

fn parse_position(s: &str) -> Position {
    if s.eq_ignore_ascii_case("random") { Position::Random } else { Position::Index(parse_i32_lenient(s)) }
}

impl Soundscape {
    /// Parses one soundscape block; unknown keys are appended to `warnings`.
    pub fn parse(kv: &KeyValues, file: &str, warnings: &mut Vec<String>) -> Self {
        let mut s = Soundscape { name: kv.key.clone(), file: file.to_owned(), ..Default::default() };
        let mut warn = |what: &str, key: &str| warnings.push(format!("{file}: {}: unknown {what} key {key}", kv.key));
        for c in kv.children() {
            match (c.key.to_ascii_lowercase().as_str(), c.as_str()) {
                ("dsp", Some(v)) => s.dsp = Some(parse_i32_lenient(v)),
                ("dsp_player", Some(v)) => s.dsp_player = Some(parse_i32_lenient(v)),
                ("dsp_volume", Some(v)) => s.dsp_volume = Some(parse_f32_lenient(v)),
                ("fadetime", Some(v)) => s.fadetime = Some(parse_f32_lenient(v)),
                ("soundmixer", Some(v)) => s.soundmixer = Some(v.to_owned()),
                ("playlooping", None) => {
                    let mut l = LoopingSound::default();
                    for k in c.children() {
                        let Some(v) = k.as_str() else {
                            warn("playlooping", &k.key);
                            continue;
                        };
                        match k.key.to_ascii_lowercase().as_str() {
                            "wave" => l.wave = Some(WaveName::parse(v)),
                            "volume" => l.volume = Some(Interval::parse(v)),
                            "pitch" => l.pitch = Some(Interval::parse(v)),
                            "soundlevel" => l.soundlevel = Some(parse_soundlevel(v)),
                            "attenuation" => l.soundlevel = Some(parse_attenuation(v)),
                            "position" => l.position = Some(parse_position(v)),
                            "origin" => l.origin = Some(parse_origin(v)),
                            "radius" => l.radius = Some(parse_f32_lenient(v)),
                            "suppress_on_restore" => l.suppress_on_restore = parse_i32_lenient(v) != 0,
                            _ => warn("playlooping", &k.key),
                        }
                    }
                    s.looping.push(l);
                }
                ("playrandom", None) => {
                    let mut r = RandomSound::default();
                    for k in c.children() {
                        let key = k.key.to_ascii_lowercase();
                        if key == "rndwave" {
                            r.waves.extend(k.children().iter().filter_map(|w| w.as_str()).map(WaveName::parse));
                            continue;
                        }
                        let Some(v) = k.as_str() else {
                            warn("playrandom", &k.key);
                            continue;
                        };
                        match key.as_str() {
                            "time" => r.time = Some(Interval::parse(v)),
                            "volume" => r.volume = Some(Interval::parse(v)),
                            "pitch" => r.pitch = Some(Interval::parse(v)),
                            "soundlevel" => r.soundlevel = Some(parse_soundlevel(v)),
                            "attenuation" => r.soundlevel = Some(parse_attenuation(v)),
                            "position" => r.position = Some(parse_position(v)),
                            "origin" => r.origin = Some(parse_origin(v)),
                            "suppress_on_restore" => r.suppress_on_restore = parse_i32_lenient(v) != 0,
                            _ => warn("playrandom", &k.key),
                        }
                    }
                    s.random.push(r);
                }
                ("playsoundscape", None) => {
                    let mut n = SubSoundscape::default();
                    for k in c.children() {
                        let Some(v) = k.as_str() else {
                            warn("playsoundscape", &k.key);
                            continue;
                        };
                        match k.key.to_ascii_lowercase().as_str() {
                            "name" => n.name = v.to_owned(),
                            "volume" => n.volume = Some(Interval::parse(v)),
                            "position" => n.position = Some(parse_i32_lenient(v)),
                            "positionoverride" => n.position_override = Some(parse_i32_lenient(v)),
                            "ambientpositionoverride" => n.ambient_position_override = Some(parse_i32_lenient(v)),
                            "ambientoriginoverride" => n.ambient_origin_override = Some(parse_origin(v)),
                            _ => warn("playsoundscape", &k.key),
                        }
                    }
                    s.nested.push(n);
                }
                _ => {
                    warn("soundscape", &c.key);
                    s.extra.push(c.clone());
                }
            }
        }
        s
    }
}

/// Every soundscape from the manifest (and optionally a map file).
#[derive(Default)]
pub struct Soundscapes {
    list: Vec<Soundscape>,
    /// Lowercase name to the index of its last definition.
    index: HashMap<String, usize>,
    /// Script files read, in order.
    pub files: Vec<String>,
    /// Files listed in the manifest that could not be read.
    pub missing_files: Vec<String>,
    /// Keys the client would report as unknown.
    pub warnings: Vec<String>,
}

impl Soundscapes {
    /// Reads [`MANIFEST`], its files and, for `map`, `scripts/soundscapes_<map>.txt`.
    pub fn load(fs: &GameFileSystem, map: Option<&str>) -> Result<Self> {
        let manifest = fs.read_string(MANIFEST).ok_or_else(|| Error::NotFound(MANIFEST.to_owned()))?;
        Ok(Self::from_manifest(&manifest, map, &mut |path| fs.read_string(path)))
    }

    /// Builds the set from manifest text; `read` returns a script's text. The
    /// map file is read last unless the manifest already lists it.
    pub fn from_manifest(manifest: &str, map: Option<&str>, read: &mut dyn FnMut(&str) -> Option<String>) -> Self {
        let mut s = Self::default();
        if let Some(root) = kv::parse(manifest) {
            for c in root.children() {
                match (c.key.eq_ignore_ascii_case("file"), c.as_str()) {
                    (true, Some(path)) => s.read_file(&normalize_path(path), read),
                    _ => s.warnings.push(format!("{MANIFEST}: unexpected key {}", c.key)),
                }
            }
        }
        if let Some(map) = map {
            let path = normalize_path(&format!("scripts/soundscapes_{map}.txt"));
            if !s.files.contains(&path)
                && let Some(text) = read(&path)
            {
                s.add_file(&path, &text);
            }
        }
        s
    }

    fn read_file(&mut self, path: &str, read: &mut dyn FnMut(&str) -> Option<String>) {
        match read(path) {
            Some(text) => self.add_file(path, &text),
            None => self.missing_files.push(path.to_owned()),
        }
    }

    /// Adds every non-empty top-level block of a soundscape file.
    pub fn add_file(&mut self, file: &str, text: &str) {
        self.files.push(file.to_owned());
        for root in kv::parse_document(text).roots {
            if root.children().is_empty() {
                continue;
            }
            self.index.insert(root.key.to_ascii_lowercase(), self.list.len());
            self.list.push(Soundscape::parse(&root, file, &mut self.warnings));
        }
    }

    pub fn get(&self, name: &str) -> Option<&Soundscape> {
        self.index.get(&name.to_ascii_lowercase()).map(|&i| &self.list[i])
    }

    /// Distinct soundscapes (the last definition of each name).
    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    /// Every definition in load order, including ones a later file replaced.
    pub fn definitions(&self) -> &[Soundscape] {
        &self.list
    }

    /// The soundscapes `name` plays through `playsoundscape`, depth first, up
    /// to [`MAX_NESTING`] levels. Names that do not resolve are skipped.
    pub fn nested(&self, name: &str) -> Vec<&Soundscape> {
        let mut out = Vec::new();
        self.collect_nested(name, 0, &mut out);
        out
    }

    fn collect_nested<'a>(&'a self, name: &str, depth: usize, out: &mut Vec<&'a Soundscape>) {
        let Some(s) = self.get(name) else { return };
        if depth >= MAX_NESTING {
            return;
        }
        for n in &s.nested {
            if let Some(child) = self.get(&n.name) {
                out.push(child);
                self.collect_nested(&n.name, depth + 1, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rules_and_overrides() {
        let manifest = r#"soundscapes_manifest { "file" "scripts/a.txt" "file" "scripts/b.txt" }"#;
        let a = r#"
            "Room.Base" { "dsp" "1" "fadetime" "2.5" "playlooping" { "volume" "0.78" "wave" "ambient/hum.wav" "pitch" "85" "attenuation" "0" } }
            "Room.Big"
            {
                "dsp" "104"
                "playrandom"
                {
                    "time" "9.0, 12.0"
                    "volume" "0.3,0.5"
                    "soundlevel" "SNDLVL_80dB"
                    "position" "random"
                    "rndwave" { "wave" ")ambient/drip1.wav" "wave" ")ambient/drip2.wav" }
                }
                "playlooping" { "volume" "1" "wave" "ambient/fan.wav" "origin" "672.5,7 -63.2, 896.0;" }
                "playsoundscape" { "name" "Room.Base" "volume" ".63" "ambientpositionoverride" "2" }
                "bogus" "1"
            }
        "#;
        let b = r#""room.base" { "dsp" "2" }"#;
        let s = Soundscapes::from_manifest(manifest, Some("sp_test"), &mut |p| match p {
            "scripts/a.txt" => Some(a.to_owned()),
            "scripts/b.txt" => Some(b.to_owned()),
            "scripts/soundscapes_sp_test.txt" => Some(r#""Map.Only" { "dsp" "3" }"#.to_owned()),
            _ => None,
        });
        assert_eq!(s.len(), 3);
        assert_eq!(s.definitions().len(), 4);
        // The later definition in b.txt wins.
        assert_eq!(s.get("Room.Base").unwrap().dsp, Some(2));
        assert_eq!(s.get("map.only").unwrap().file, "scripts/soundscapes_sp_test.txt");

        let big = s.get("room.big").unwrap();
        assert_eq!(big.dsp, Some(104));
        let r = &big.random[0];
        assert_eq!(r.waves.len(), 2);
        assert_eq!(r.time, Some(Interval { start: 9.0, end: 12.0 }));
        assert_eq!(r.soundlevel, Some(Interval::fixed(80.0)));
        assert_eq!(r.position, Some(Position::Random));
        // Comma-split like the client: the stray "7 -63.2" part reads as 7.
        assert_eq!(big.looping[0].origin, Some(Vec3::new(672.5, 7.0, 896.0)));
        let n = &big.nested[0];
        assert_eq!(
            (n.name.as_str(), n.volume, n.ambient_position_override),
            ("Room.Base", Some(Interval::fixed(0.63)), Some(2))
        );
        assert_eq!(s.warnings.len(), 1);
        assert_eq!(s.nested("Room.Big").len(), 1);

        let base = &s.definitions()[0];
        assert_eq!(base.looping[0].soundlevel, Some(Interval::fixed(0.0)));
        assert_eq!(base.fadetime, Some(2.5));
    }

    /// Parses every soundscape in the installation (skipped without one) and
    /// reports unknown keys, unresolved nested soundscapes and missing waves.
    #[test]
    fn install_soundscapes_parse() {
        let Some(fs) = GameFileSystem::find_install().and_then(|root| GameFileSystem::portal2(root).ok()) else {
            eprintln!("Portal 2 installation not found; skipping");
            return;
        };
        let s = Soundscapes::load(&fs, None).unwrap();
        assert!(s.missing_files.is_empty(), "missing soundscape files {:?}", s.missing_files);
        let defs = s.definitions();
        let looping: usize = defs.iter().map(|d| d.looping.len()).sum();
        let random: usize = defs.iter().map(|d| d.random.len()).sum();
        let nested: usize = defs.iter().map(|d| d.nested.len()).sum();
        println!(
            "{} soundscapes ({} definitions) from {} files: {looping} looping, {random} random, {nested} nested",
            s.len(),
            defs.len(),
            s.files.len()
        );
        println!("  {} unknown keys", s.warnings.len());
        for w in &s.warnings {
            println!("    {w}");
        }
        let mut unresolved = Vec::new();
        let mut waves = 0;
        let mut missing = Vec::new();
        for d in defs {
            unresolved.extend(
                d.nested.iter().filter(|n| s.get(&n.name).is_none()).map(|n| format!("{} -> {}", d.name, n.name)),
            );
            let names = d.looping.iter().filter_map(|l| l.wave.as_ref()).chain(d.random.iter().flat_map(|r| &r.waves));
            for path in names.filter_map(WaveName::file_path) {
                waves += 1;
                if !fs.exists(&path) {
                    missing.push(format!("{}: {path}", d.name));
                }
            }
        }
        println!("  {} unresolved playsoundscape names: {unresolved:?}", unresolved.len());
        println!("  {waves} wave references, {} missing: {missing:?}", missing.len());
        assert!(s.len() > 100);
        assert!(missing.len() * 10 < waves && unresolved.len() * 10 < nested.max(1));
    }
}
