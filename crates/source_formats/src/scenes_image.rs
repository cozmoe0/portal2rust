//! Compiled scene image (`scenes/scenes.image`): every choreographed scene in
//! binary form, looked up by the CRC32 of its normalised path. The game only
//! loads the first image on its search path, so each DLC image repeats the
//! base game's scenes.
//!
//! Layout (little endian):
//! - header: `VSIF`, version 3, scene count, string count, entry table offset;
//! - one `u32` offset per string right after the header, into a pool of NUL
//!   terminated strings shared by all scenes (deduplicated ignoring case);
//! - entries sorted by CRC: `crc, data offset, data length, summary offset`;
//! - per-scene summary: duration and end of the last speak event in
//!   milliseconds, then a count and pool indices of the sounds it plays;
//! - scene data: a `bvcd` binary scene, usually inside Valve's `LZMA` wrapper.
//!
//! A binary scene keeps only what playback needs: editor settings, curve
//! edges, ramp interpolation types, look-at offsets and tag locks are dropped,
//! and ramp values are quantised to bytes. Its header CRC is the CRC32 of the
//! `.vcd` text it was compiled from.

use std::borrow::Cow;

use crate::fs::GameFileSystem;
use crate::reader::Reader;
use crate::vcd::{
    Actor, CcType, Channel, Curve, CurveType, Event, EventFlags, EventKind, FlexTrack, Sample, Scene, Tag, TagRef,
    TimingTag,
};
use crate::{Error, Result, invalid};

pub const IMAGE_PATH: &str = "scenes/scenes.image";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageEntry {
    /// [`scene_crc`] of the scene's path.
    pub crc: u32,
    pub offset: u32,
    pub length: u32,
    pub summary_offset: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SceneSummary {
    /// Latest start or end of any event, rounded.
    pub duration_ms: u32,
    /// End of the speak event listed last (not necessarily the one ending last).
    pub last_speak_ms: u32,
    pub sounds: Vec<String>,
}

pub struct ScenesImage {
    data: Vec<u8>,
    strings: Vec<u32>,
    entries: Vec<ImageEntry>,
}

impl ScenesImage {
    pub fn parse(data: Vec<u8>) -> Result<Self> {
        let mut r = Reader::new(&data);
        let id = r.bytes(4)?;
        if id != b"VSIF" {
            return Err(Error::Magic { expected: "VSIF", found: id.to_vec() });
        }
        let version = r.i32()?;
        if version != 3 {
            return Err(Error::Version(version as i64));
        }
        let scene_count = r.i32()?.max(0) as usize;
        let string_count = r.i32()?.max(0) as usize;
        let entry_offset = r.u32()? as usize;
        let strings = (0..string_count).map(|_| r.u32()).collect::<Result<Vec<_>>>()?;
        let mut r = Reader::at(&data, entry_offset);
        let mut entries = Vec::with_capacity(scene_count);
        for _ in 0..scene_count {
            entries.push(ImageEntry { crc: r.u32()?, offset: r.u32()?, length: r.u32()?, summary_offset: r.u32()? });
        }
        Ok(Self { data, strings, entries })
    }

    /// The image the game would load: the first `scenes/scenes.image` on the
    /// search path.
    pub fn from_fs(fs: &GameFileSystem) -> Option<Result<Self>> {
        fs.read(IMAGE_PATH).map(Self::parse)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entries(&self) -> &[ImageEntry] {
        &self.entries
    }

    pub fn string_count(&self) -> usize {
        self.strings.len()
    }

    pub fn string(&self, index: usize) -> Option<Cow<'_, str>> {
        let rest = self.data.get(*self.strings.get(index)? as usize..)?;
        let end = rest.iter().position(|&c| c == 0)?;
        Some(String::from_utf8_lossy(&rest[..end]))
    }

    /// Entry index of a scene path such as `scenes/npc/glados/x.vcd`.
    pub fn find(&self, path: &str) -> Option<usize> {
        let crc = scene_crc(path);
        self.entries.binary_search_by_key(&crc, |e| e.crc).ok()
    }

    fn entry(&self, index: usize) -> Result<&ImageEntry> {
        self.entries.get(index).ok_or_else(|| Error::Invalid(format!("scene image: no entry {index}")))
    }

    /// The `bvcd` data of an entry, decompressed if needed.
    pub fn scene_data(&self, index: usize) -> Result<Cow<'_, [u8]>> {
        let e = self.entry(index)?;
        let start = e.offset as usize;
        let blob = self.data.get(start..start + e.length as usize).ok_or(Error::Eof)?;
        if blob.starts_with(b"LZMA") {
            Ok(Cow::Owned(crate::zip::decode_valve_lzma_header(blob)?))
        } else {
            Ok(Cow::Borrowed(blob))
        }
    }

    pub fn summary(&self, index: usize) -> Result<SceneSummary> {
        let e = self.entry(index)?;
        let mut r = Reader::at(&self.data, e.summary_offset as usize);
        let duration_ms = r.u32()?;
        let last_speak_ms = r.u32()?;
        let count = r.i32()?.max(0) as usize;
        let mut sounds = Vec::with_capacity(count);
        for _ in 0..count {
            let i = r.u32()? as usize;
            let s = self.string(i).ok_or_else(|| Error::Invalid(format!("scene image: no string {i}")))?;
            sounds.push(s.into_owned());
        }
        Ok(SceneSummary { duration_ms, last_speak_ms, sounds })
    }

    pub fn scene(&self, index: usize) -> Result<Scene> {
        decode_scene(&self.scene_data(index)?, |i| self.string(i))
    }

    /// CRC32 of the `.vcd` text the entry was compiled from.
    pub fn source_crc(&self, index: usize) -> Result<u32> {
        source_crc(&self.scene_data(index)?)
    }

    pub fn load(&self, path: &str) -> Option<Result<Scene>> {
        self.find(path).map(|i| self.scene(i))
    }
}

/// Lookup key as the game builds it: lowercase, backslashes, `.vcd` added
/// when the file name has no extension.
pub fn scene_key(path: &str) -> String {
    let mut key: String = path.chars().map(|c| if c == '/' { '\\' } else { c.to_ascii_lowercase() }).collect();
    if !key.rsplit('\\').next().unwrap_or("").contains('.') {
        key.push_str(".vcd");
    }
    key
}

pub fn scene_crc(path: &str) -> u32 {
    crc32(scene_key(path).as_bytes())
}

/// CRC-32 (IEEE 802.3, the zlib variant).
pub fn crc32(data: &[u8]) -> u32 {
    const TABLE: [u32; 256] = {
        let mut table = [0u32; 256];
        let mut i = 0;
        while i < 256 {
            let mut c = i as u32;
            let mut k = 0;
            while k < 8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
                k += 1;
            }
            table[i] = c;
            i += 1;
        }
        table
    };
    !data.iter().fold(!0u32, |c, &b| TABLE[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8))
}

/// Header CRC of a `bvcd` scene: the CRC32 of its source text.
pub fn source_crc(data: &[u8]) -> Result<u32> {
    let mut r = Reader::new(data);
    check_header(&mut r)?;
    r.u32()
}

/// Decodes a `bvcd` scene; `string` resolves indices into the image's pool.
pub fn decode_scene<S: AsRef<str>>(data: &[u8], string: impl Fn(usize) -> Option<S>) -> Result<Scene> {
    decode(data, string).map(|(scene, _)| scene)
}

/// Returns the scene and the number of bytes it used.
fn decode<S: AsRef<str>>(data: &[u8], string: impl Fn(usize) -> Option<S>) -> Result<(Scene, usize)> {
    let mut d = Decoder { r: Reader::new(data), string };
    check_header(&mut d.r)?;
    let _source_crc = d.r.u32()?;
    let mut scene = Scene::default();
    for _ in 0..d.count()? {
        scene.events.push(d.event()?);
    }
    for _ in 0..d.count()? {
        scene.actors.push(d.actor()?);
    }
    scene.ramp = d.curve()?;
    scene.ignore_phonemes = d.r.u8()? != 0;
    Ok((scene, d.r.pos()))
}

fn check_header(r: &mut Reader) -> Result<()> {
    let id = r.bytes(4)?;
    if id != b"bvcd" {
        return Err(Error::Magic { expected: "bvcd", found: id.to_vec() });
    }
    match r.u8()? {
        4 => Ok(()),
        v => Err(Error::Version(v as i64)),
    }
}

struct Decoder<'a, F> {
    r: Reader<'a>,
    string: F,
}

impl<S: AsRef<str>, F: Fn(usize) -> Option<S>> Decoder<'_, F> {
    fn str(&mut self) -> Result<String> {
        let index = self.r.u16()? as usize;
        match (self.string)(index) {
            Some(s) => Ok(s.as_ref().to_owned()),
            None => invalid(format!("bvcd: string {index} out of range")),
        }
    }

    fn count(&mut self) -> Result<usize> {
        Ok(self.r.u8()? as usize)
    }

    /// 0..1 value stored as a byte.
    fn unit(&mut self) -> Result<f32> {
        Ok(self.r.u8()? as f32 / 255.0)
    }

    fn actor(&mut self) -> Result<Actor> {
        let name = self.str()?;
        let mut channels = Vec::new();
        for _ in 0..self.count()? {
            channels.push(self.channel()?);
        }
        let active = self.r.u8()? == 1;
        Ok(Actor { name, channels, active, faceposer_model: None, unknown: Vec::new() })
    }

    fn channel(&mut self) -> Result<Channel> {
        let name = self.str()?;
        let mut events = Vec::new();
        for _ in 0..self.count()? {
            events.push(self.event()?);
        }
        let active = self.r.u8()? == 1;
        Ok(Channel { name, events, active, unknown: Vec::new() })
    }

    /// Ramps keep `time: f32, value: u8` per sample and no interpolation type.
    fn curve(&mut self) -> Result<Curve> {
        let mut samples = Vec::new();
        for _ in 0..self.count()? {
            samples.push(Sample { time: self.r.f32()?, value: self.unit()?, curve: CurveType::default() });
        }
        Ok(Curve { samples, left_edge: None, right_edge: None })
    }

    /// Flex samples add a `u16` interpolation type.
    fn flex_samples(&mut self, count: usize) -> Result<Vec<Sample>> {
        (0..count)
            .map(|_| -> Result<Sample> {
                Ok(Sample { time: self.r.f32()?, value: self.unit()?, curve: CurveType::from_bits(self.r.u16()?) })
            })
            .collect()
    }

    fn event(&mut self) -> Result<Event> {
        let kind = EventKind::from_index(self.r.u8()?);
        let name = self.str()?;
        let start = self.r.f32()?;
        let end = self.r.f32()?;
        let param = self.str()?;
        // The game trims trailing whitespace from these two on load.
        let param2 = trim_param(self.str()?);
        let param3 = trim_param(self.str()?);
        let ramp = self.curve()?;
        let f = self.r.u8()?;
        let flags = EventFlags {
            resume_condition: f & 1 != 0,
            lock_body_facing: f & 2 != 0,
            fixed_length: f & 4 != 0,
            active: f & 8 != 0,
            force_short_movement: f & 16 != 0,
            play_over_script: f & 32 != 0,
            sync_to_following_gesture: false,
        };
        let distance_to_target = self.r.f32()?;
        let mut ev = Event {
            kind,
            name,
            start,
            end,
            param,
            param2,
            param3,
            ramp,
            flags,
            distance_to_target,
            ..Event::default()
        };
        for _ in 0..self.count()? {
            let name = self.str()?;
            ev.tags.push(Tag { name, value: self.unit()? });
        }
        for _ in 0..self.count()? {
            let name = self.str()?;
            ev.timing_tags.push(TimingTag { name, value: self.unit()?, locked: false });
        }
        // Absolute tags: playback then shifted, values in 1/4096 steps.
        for shifted in [false, true] {
            for _ in 0..self.count()? {
                let name = self.str()?;
                let tag = Tag { name, value: self.r.u16()? as f32 / 4096.0 };
                if shifted { ev.shifted_tags.push(tag) } else { ev.playback_tags.push(tag) }
            }
        }
        if ev.kind == EventKind::Gesture {
            let duration = self.r.f32()?;
            ev.sequence_duration = (duration != -1.0).then_some(duration);
        }
        if self.r.u8()? == 1 {
            let tag = self.str()?;
            let wav = self.str()?;
            ev.relative_tag = Some(TagRef { tag, wav });
        }
        for _ in 0..self.count()? {
            let name = self.str()?;
            let f = self.r.u8()?;
            let (min, max) = (self.r.f32()?, self.r.f32()?);
            let mut track = FlexTrack { name, active: f & 1 != 0, combo: f & 2 != 0, min, max, ..FlexTrack::default() };
            let count = self.r.i16()?.max(0) as usize;
            track.samples = self.flex_samples(count)?;
            if track.combo {
                let count = self.r.u16()? as usize;
                track.balance = self.flex_samples(count)?;
            }
            ev.flex_tracks.push(track);
        }
        if ev.kind == EventKind::Loop {
            ev.loop_count = (self.r.i8()? as i32).max(-1);
        }
        if ev.kind == EventKind::Speak {
            ev.cc.kind = CcType::from_index(self.r.u8()?);
            ev.cc.token = self.str()?;
            let f = self.r.u8()?;
            ev.cc.using_combined_file = f & 1 != 0;
            ev.cc.combined_uses_gender = f & 2 != 0;
            ev.cc.no_attenuate = f & 4 != 0;
        }
        Ok(ev)
    }
}

fn trim_param(s: String) -> String {
    if s.ends_with(' ') { s.trim_end().to_owned() } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_and_keys() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(scene_key("Scenes/NPC/Glados/Hello"), "scenes\\npc\\glados\\hello.vcd");
        assert_eq!(scene_key("scenes/a.b/c.vcd"), "scenes\\a.b\\c.vcd");
        assert_eq!(scene_crc("scenes/x.vcd"), crc32(b"scenes\\x.vcd"));
    }

    /// Little writer for hand-made binary scenes.
    #[derive(Default)]
    struct Writer {
        out: Vec<u8>,
        pool: Vec<String>,
    }

    impl Writer {
        fn u8(&mut self, v: u8) -> &mut Self {
            self.out.push(v);
            self
        }
        fn u16(&mut self, v: u16) -> &mut Self {
            self.out.extend_from_slice(&v.to_le_bytes());
            self
        }
        fn f32(&mut self, v: f32) -> &mut Self {
            self.out.extend_from_slice(&v.to_le_bytes());
            self
        }
        fn s(&mut self, s: &str) -> &mut Self {
            let i = self.pool.iter().position(|p| p == s).unwrap_or_else(|| {
                self.pool.push(s.to_owned());
                self.pool.len() - 1
            });
            self.u16(i as u16)
        }
        /// Common event prefix: no ramp, tags or flex unless written after.
        fn event(&mut self, kind: u8, name: &str, start: f32, end: f32, params: [&str; 3]) -> &mut Self {
            self.u8(kind).s(name).f32(start).f32(end).s(params[0]).s(params[1]).s(params[2])
        }
    }

    #[test]
    fn decodes_hand_made_scene() {
        let mut w = Writer::default();
        w.out.extend_from_slice(b"bvcd");
        w.u8(4).f32(0.0); // version, source crc
        // One scene-level stop point.
        w.u8(1).event(14, "pause", 2.0, -1.0, ["noaction", "", ""]);
        w.u8(0).u8(8).f32(0.0).u8(0).u8(0).u8(0).u8(0).u8(0).u8(0);
        // One actor, one channel, four events.
        w.u8(1).s("Hero").u8(1).s("main").u8(4);
        // speak with a ramp, a tag, cc settings
        w.event(5, "line", 0.5, 2.5, ["hero.line", "75dB  ", ""]);
        w.u8(2).f32(0.0).u8(0).f32(1.0).u8(255);
        w.u8(4 | 8).f32(0.0);
        w.u8(1).s("beat").u8(51);
        w.u8(0).u8(0).u8(0);
        w.u8(0).u8(0); // no relative tag, no flex
        w.u8(1).s("hero.line_cc").u8(2 | 4);
        // gesture with absolute tags, sequence duration and a relative tag
        w.event(6, "wave", 1.0, 2.0, ["g_wave", "", ""]);
        w.u8(0).u8(8 | 2).f32(12.5);
        w.u8(0).u8(0);
        w.u8(1).s("apex").u16(2048).u8(1).s("apex").u16(1024);
        w.f32(0.75);
        w.u8(1).s("beat").s("hero.line");
        w.u8(0);
        // flex animation with a combo track, then a loop
        w.event(10, "face", 0.0, 4.0, ["", "", ""]);
        w.u8(0).u8(8).f32(0.0).u8(0).u8(0).u8(0).u8(0).u8(0);
        w.u8(1).s("blink").u8(3).f32(0.0).f32(1.0);
        w.u16(1).f32(0.5).u8(255).u16(0x0b0f);
        w.u16(1).f32(0.5).u8(0).u16(0);
        w.event(12, "again", 3.0, -1.0, ["1.0", "", ""]);
        w.u8(0).u8(8).f32(0.0).u8(0).u8(0).u8(0).u8(0).u8(0).u8(0);
        w.u8(3); // loop count
        w.u8(1); // channel active
        w.u8(0); // actor inactive
        w.u8(1).f32(0.0).u8(255); // scene ramp
        w.u8(1); // ignore phonemes
        let pool = w.pool.clone();
        let (scene, used) = decode(&w.out, |i| pool.get(i)).unwrap();
        assert_eq!(used, w.out.len());
        assert_eq!(scene.events[0].kind, EventKind::StopPoint);
        assert!(!scene.events[0].has_duration());
        let actor = &scene.actors[0];
        assert!(!actor.active && actor.channels[0].active);
        let [speak, wave, face, again] = &actor.channels[0].events[..] else { panic!() };
        assert_eq!(speak.param2, "75dB");
        assert_eq!(speak.ramp.samples[1].value, 1.0);
        assert!(speak.flags.fixed_length && speak.flags.active && !speak.flags.resume_condition);
        assert_eq!(speak.tags[0].value, 0.2);
        assert_eq!(speak.cc.kind, CcType::Slave);
        assert_eq!(speak.cc.token, "hero.line_cc");
        assert!(speak.cc.combined_uses_gender && speak.cc.no_attenuate && !speak.cc.using_combined_file);
        assert!(wave.flags.lock_body_facing);
        assert_eq!(wave.distance_to_target, 12.5);
        assert_eq!((wave.playback_tags[0].value, wave.shifted_tags[0].value), (0.5, 0.25));
        assert_eq!(wave.sequence_duration, Some(0.75));
        assert_eq!(wave.relative_tag, Some(TagRef { tag: "beat".into(), wav: "hero.line".into() }));
        let blink = &face.flex_tracks[0];
        assert!(blink.active && blink.combo);
        assert_eq!(blink.samples[0].curve.bits(), 0x0b0f);
        assert_eq!(blink.balance[0].value, 0.0);
        assert_eq!((again.kind.clone(), again.loop_count), (EventKind::Loop, 3));
        assert!(scene.ignore_phonemes);
        assert_eq!(scene.ramp.samples.len(), 1);
        assert_eq!(source_crc(&w.out).unwrap(), 0);
    }

    /// Images found directly in each game directory of the install.
    fn installed_images(root: &std::path::Path) -> Vec<(String, ScenesImage)> {
        let mut out = Vec::new();
        let mut dirs: Vec<_> = std::fs::read_dir(root).unwrap().flatten().map(|e| e.path()).collect();
        dirs.sort();
        for dir in dirs {
            if let Ok(data) = std::fs::read(dir.join(IMAGE_PATH)) {
                let game = dir.file_name().unwrap().to_string_lossy().into_owned();
                let image = ScenesImage::parse(data).unwrap_or_else(|e| panic!("{game}: {e}"));
                out.push((game, image));
            }
        }
        out
    }

    /// Every entry of every installed image decodes completely, and every
    /// `.vcd` in a game directory's VPK is indexed by that directory's image.
    #[test]
    fn installed_images_decode() {
        let Some(root) = GameFileSystem::find_install() else {
            eprintln!("skipping: Portal 2 install not found");
            return;
        };
        let images = installed_images(&root);
        assert!(!images.is_empty());
        for (game, image) in &images {
            let mut kinds = std::collections::BTreeMap::<String, usize>::new();
            let mut compressed = 0;
            for (i, e) in image.entries().iter().enumerate() {
                assert!(i == 0 || image.entries()[i - 1].crc < e.crc, "{game}: entries not sorted");
                let data = image.scene_data(i).unwrap_or_else(|err| panic!("{game} #{i}: {err}"));
                compressed += matches!(data, Cow::Owned(_)) as usize;
                let (scene, used) =
                    decode(&data, |s| image.string(s)).unwrap_or_else(|err| panic!("{game} #{i}: {err}"));
                assert_eq!(used, data.len(), "{game} #{i}: trailing bytes");
                let summary = image.summary(i).unwrap();
                // The summary's "last speak" is the end of the speak event that
                // comes last in the scene, not the one that ends last.
                let last_speak =
                    scene.all_events().filter(|e| e.kind == EventKind::Speak).last().map_or(0.0, |e| e.end);
                let ms = |s: f32| s * 1000.0;
                let (end, speak) = (ms(scene.end_time()), ms(last_speak));
                assert!((end - summary.duration_ms as f32).abs() <= 1.0, "{game} #{i}: {end} {summary:?}");
                assert!((speak - summary.last_speak_ms as f32).abs() <= 1.0, "{game} #{i}: {speak} {summary:?}");
                for e in scene.all_events() {
                    *kinds.entry(e.kind.name().to_owned()).or_default() += 1;
                }
            }
            eprintln!("{game}: {} scenes ({compressed} compressed), events by type: {kinds:?}", image.len());
        }
        let vpks = crate::vcd::tests::installed_vpks(&root);
        for (game, image) in &images {
            let Some((_, vpk)) = vpks.iter().find(|(g, _)| g == game) else { continue };
            for path in vpk.paths().filter(|p| p.starts_with("scenes/") && p.ends_with(".vcd")) {
                assert!(image.find(path).is_some(), "{game}: {path} missing from image");
            }
        }
    }

    /// For every installed `.vcd` whose text matches an image entry's source
    /// CRC, the text and compiled versions describe the same scene.
    #[test]
    fn installed_text_and_image_agree() {
        let Some(root) = GameFileSystem::find_install() else {
            eprintln!("skipping: Portal 2 install not found");
            return;
        };
        let vpks = crate::vcd::tests::installed_vpks(&root);
        for (game, image) in installed_images(&root) {
            let (mut compared, mut stale, mut differ) = (0, Vec::new(), Vec::new());
            for (vpk_game, vpk) in &vpks {
                for path in vpk.paths().filter(|p| p.starts_with("scenes/") && p.ends_with(".vcd")) {
                    let Some(index) = image.find(path) else { continue };
                    let bytes = vpk.read(path).unwrap();
                    if image.source_crc(index).unwrap() != crc32(&bytes) {
                        stale.push(format!("{vpk_game}/{path}"));
                        continue;
                    }
                    let text = Scene::parse(&crate::text::decode_text(&bytes)).unwrap();
                    let compiled = image.scene(index).unwrap();
                    compared += 1;
                    if let Err(why) = compare(&text, &compiled) {
                        differ.push(format!("{vpk_game}/{path}: {why}"));
                    }
                }
            }
            eprintln!(
                "{game}: compared {compared} scenes, {} differ, {} texts differ from the image's source",
                differ.len(),
                stale.len()
            );
            for s in stale.iter().take(10) {
                eprintln!("  stale: {s}");
            }
            assert!(compared > 0, "{game}: nothing compared");
            assert!(differ.is_empty(), "{game}: {:#?}", &differ[..differ.len().min(20)]);
        }
    }

    /// Strings compare ignoring case (the image pool folds case); values
    /// allow for byte (ramps, tags) and 1/4096 (absolute tags) quantisation.
    fn compare(text: &Scene, bin: &Scene) -> std::result::Result<(), String> {
        let same = |a: &str, b: &str| a.trim_end().eq_ignore_ascii_case(b.trim_end());
        let near = |a: f32, b: f32, eps: f32| (a - b).abs() <= eps;
        let check = |ok: bool, what: &dyn Fn() -> String| if ok { Ok(()) } else { Err(what()) };
        let events = |t: &[Event], b: &[Event], at: &str| -> std::result::Result<(), String> {
            check(t.len() == b.len(), &|| format!("{at}: {} vs {} events", t.len(), b.len()))?;
            for (x, y) in t.iter().zip(b) {
                let at = format!("{at}/{}", x.name);
                check(same(&x.name, &y.name) && x.kind == y.kind, &|| {
                    format!("{at}: {:?} vs {:?} {:?}", x.kind, y.kind, y.name)
                })?;
                check(x.start == y.start && x.end == y.end, &|| {
                    format!("{at}: time {}..{} vs {}..{}", x.start, x.end, y.start, y.end)
                })?;
                check(same(&x.param, &y.param) && same(&x.param2, &y.param2) && same(&x.param3, &y.param3), &|| {
                    format!(
                        "{at}: params {:?} vs {:?}",
                        [&x.param, &x.param2, &x.param3],
                        [&y.param, &y.param2, &y.param3]
                    )
                })?;
                let mut flags = x.flags;
                flags.sync_to_following_gesture = false;
                check(flags == y.flags, &|| format!("{at}: flags {:?} vs {:?}", x.flags, y.flags))?;
                check(x.distance_to_target == y.distance_to_target, &|| format!("{at}: distance"))?;
                let ramp_ok = x.ramp.samples.len() == y.ramp.samples.len()
                    && x.ramp
                        .samples
                        .iter()
                        .zip(&y.ramp.samples)
                        .all(|(p, q)| p.time == q.time && near(p.value, q.value, 1.0 / 255.0));
                check(ramp_ok, &|| format!("{at}: ramp {:?} vs {:?}", x.ramp.samples, y.ramp.samples))?;
                let tags_ok = |p: &[Tag], q: &[Tag], eps: f32| {
                    p.len() == q.len()
                        && p.iter().zip(q).all(|(a, b)| same(&a.name, &b.name) && near(a.value, b.value, eps))
                };
                check(tags_ok(&x.tags, &y.tags, 1.0 / 255.0), &|| format!("{at}: tags {:?} vs {:?}", x.tags, y.tags))?;
                check(
                    tags_ok(&x.playback_tags, &y.playback_tags, 1.0 / 4096.0)
                        && tags_ok(&x.shifted_tags, &y.shifted_tags, 1.0 / 4096.0),
                    &|| format!("{at}: absolute tags {:?} vs {:?}", x.playback_tags, y.playback_tags),
                )?;
                check(x.timing_tags.len() == y.timing_tags.len(), &|| format!("{at}: timing tags"))?;
                check(x.relative_tag.is_some() == y.relative_tag.is_some(), &|| format!("{at}: relative tag"))?;
                check(x.flex_tracks.len() == y.flex_tracks.len(), &|| format!("{at}: flex tracks"))?;
                if x.kind == EventKind::Gesture {
                    let (a, b) = (x.sequence_duration.unwrap_or(0.0), y.sequence_duration.unwrap_or(0.0));
                    check(a == b, &|| {
                        format!("{at}: sequence duration {:?} vs {:?}", x.sequence_duration, y.sequence_duration)
                    })?;
                }
                if x.kind == EventKind::Loop {
                    check(x.loop_count == y.loop_count, &|| format!("{at}: loop count"))?;
                }
                if x.kind == EventKind::Speak {
                    check(
                        x.cc.kind == y.cc.kind
                            && same(&x.cc.token, &y.cc.token)
                            && (x.cc.using_combined_file, x.cc.combined_uses_gender, x.cc.no_attenuate)
                                == (y.cc.using_combined_file, y.cc.combined_uses_gender, y.cc.no_attenuate),
                        &|| format!("{at}: cc {:?} vs {:?}", x.cc, y.cc),
                    )?;
                }
            }
            Ok(())
        };
        events(&text.events, &bin.events, "scene")?;
        check(text.actors.len() == bin.actors.len(), &|| {
            format!("{} vs {} actors", text.actors.len(), bin.actors.len())
        })?;
        for (a, b) in text.actors.iter().zip(&bin.actors) {
            check(same(&a.name, &b.name) && a.active == b.active, &|| format!("actor {:?} vs {:?}", a.name, b.name))?;
            check(a.channels.len() == b.channels.len(), &|| format!("{}: channel count", a.name))?;
            for (c, d) in a.channels.iter().zip(&b.channels) {
                let at = format!("{}/{}", a.name, c.name);
                check(same(&c.name, &d.name) && c.active == d.active, &|| format!("{at}: channel {:?}", d.name))?;
                events(&c.events, &d.events, &at)?;
            }
        }
        check(text.ramp.samples.len() == bin.ramp.samples.len(), &|| "scene ramp".to_owned())?;
        check(text.ignore_phonemes == bin.ignore_phonemes, &|| "ignorePhonemes".to_owned())
    }
}
