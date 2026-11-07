//! Compiled close captions: `resource/closecaption_<language>.dat` (sound
//! effects and dialogue) and `resource/subtitles_<language>.dat` (dialogue).
//!
//! A `VCCD` file holds a directory of CRC-32 hashes of the lowercase token (the
//! sound entry name) pointing into fixed-size blocks of NUL-terminated UTF-16LE
//! text. The client loads every copy of both files along the game search
//! paths, close captions first; [`Captions`] keeps them in that order and the
//! first match wins. [`parse_tags`] splits caption text into `<tag>` commands.

use std::collections::HashMap;

use crate::fs::GameFileSystem;
use crate::reader::Reader;
use crate::text::decode_utf16le;
use crate::{Error, Result, invalid};

const CRC_TABLE: [u32; 256] = {
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

/// Standard (zlib) CRC-32.
pub fn crc32(data: &[u8]) -> u32 {
    !data.iter().fold(!0u32, |c, &b| CRC_TABLE[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8))
}

/// Directory hash of a caption token: CRC-32 of its ASCII-lowercased bytes.
pub fn token_hash(token: &str) -> u32 {
    crc32(token.to_ascii_lowercase().as_bytes())
}

#[derive(Clone, Copy, Debug)]
struct Entry {
    block: u32,
    offset: u16,
    length: u16,
}

/// One compiled caption file.
pub struct CaptionFile {
    pub version: i32,
    pub block_size: usize,
    entries: HashMap<u32, Entry>,
    data: Vec<u8>,
    data_offset: usize,
}

impl CaptionFile {
    pub fn parse(data: Vec<u8>) -> Result<Self> {
        let mut r = Reader::new(&data);
        let magic = r.bytes(4)?;
        if magic != b"VCCD" {
            return Err(Error::Magic { expected: "VCCD", found: magic.to_vec() });
        }
        let version = r.i32()?;
        if version != 1 {
            return Err(Error::Version(version as i64));
        }
        let blocks = r.i32()?.max(0) as usize;
        let block_size = r.i32()?.max(0) as usize;
        let count = r.i32()?;
        let data_offset = r.i32()?.max(0) as usize;
        if !(0..=0x10000).contains(&count) {
            return invalid(format!("caption directory of {count} entries"));
        }
        let mut entries = HashMap::with_capacity(count as usize);
        for _ in 0..count {
            let hash = r.u32()?;
            let entry = Entry { block: r.u32()?, offset: r.u16()?, length: r.u16()? };
            if entry.block as usize >= blocks || entry.offset as usize + entry.length as usize > block_size {
                return invalid(format!("caption {hash:08x} outside its block"));
            }
            entries.entry(hash).or_insert(entry);
        }
        if data.len() < data_offset + blocks * block_size {
            return Err(Error::Eof);
        }
        Ok(Self { version, block_size, entries, data, data_offset })
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn contains(&self, token: &str) -> bool {
        self.entries.contains_key(&token_hash(token))
    }

    pub fn lookup(&self, token: &str) -> Option<String> {
        self.lookup_hash(token_hash(token))
    }

    pub fn lookup_hash(&self, hash: u32) -> Option<String> {
        let e = self.entries.get(&hash)?;
        let start = self.data_offset + e.block as usize * self.block_size + e.offset as usize;
        let text = decode_utf16le(&self.data[start..start + e.length as usize]);
        Some(text.trim_end_matches('\0').to_owned())
    }

    /// Directory hashes, in no particular order.
    pub fn hashes(&self) -> impl Iterator<Item = u32> + '_ {
        self.entries.keys().copied()
    }
}

/// The caption dictionaries for one language, in lookup order.
#[derive(Default)]
pub struct Captions {
    /// Each file with the path it was read from.
    pub files: Vec<(String, CaptionFile)>,
}

impl Captions {
    /// Loads `closecaption_<language>.dat` from every search path that has
    /// it, then `subtitles_<language>.dat` the same way. Either kind falls back
    /// to English when no copy exists for `language`. Unreadable or corrupt
    /// files are skipped with a warning.
    pub fn load(fs: &GameFileSystem, language: &str) -> Self {
        let mut captions = Self::default();
        for kind in ["closecaption", "subtitles"] {
            if !captions.load_kind(fs, kind, language) && !language.eq_ignore_ascii_case("english") {
                captions.load_kind(fs, kind, "english");
            }
        }
        captions
    }

    fn load_kind(&mut self, fs: &GameFileSystem, kind: &str, language: &str) -> bool {
        let rel = format!("resource/{kind}_{language}.dat");
        let mut found = false;
        // Archives and map pakfiles are searched through `fs` below.
        for name in fs.loose_dirs() {
            let Some(bytes) = fs.read_root(&format!("{name}/{rel}")) else { continue };
            found |= self.add_bytes(format!("{name}/{rel}"), bytes);
        }
        if !found
            && let Some(source) = fs.locate(&rel)
            && let Some(bytes) = fs.read(&rel)
        {
            found = self.add_bytes(format!("{source}:{rel}"), bytes);
        }
        found
    }

    fn add_bytes(&mut self, name: String, bytes: Vec<u8>) -> bool {
        match CaptionFile::parse(bytes) {
            Ok(file) => {
                self.files.push((name, file));
                true
            }
            Err(e) => {
                log::warn!("bad caption file {name}: {e}");
                false
            }
        }
    }

    pub fn add(&mut self, name: impl Into<String>, file: CaptionFile) {
        self.files.push((name.into(), file));
    }

    pub fn lookup(&self, token: &str) -> Option<String> {
        let hash = token_hash(token);
        self.files.iter().find_map(|(_, f)| f.lookup_hash(hash))
    }

    pub fn contains(&self, token: &str) -> bool {
        let hash = token_hash(token);
        self.files.iter().any(|(_, f)| f.entries.contains_key(&hash))
    }

    /// Total directory entries over all files.
    pub fn len(&self) -> usize {
        self.files.iter().map(|(_, f)| f.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.files.iter().all(|(_, f)| f.is_empty())
    }
}

/// Formatting and timing commands inside caption text.
#[derive(Clone, Debug, PartialEq)]
pub enum Tag {
    /// `<clr:r,g,b>` pushes a text colour; `<clr>` without values pops it. The
    /// client may replace the colour by the speaker's entry in
    /// `resource/captioning_colors.txt` when the tag is followed by `Name:`.
    Color(Option<[u8; 3]>),
    /// `<playerclr:r,g,b:r,g,b>` the first colour when the local player emitted
    /// the sound, the second otherwise.
    PlayerColor { player: [u8; 3], other: [u8; 3] },
    /// `<B>` toggles bold.
    Bold,
    /// `<I>` toggles italic.
    Italic,
    /// `<cr>` line break.
    LineBreak,
    /// `<sfx>` sound-effect caption, hidden when only subtitles are shown.
    Sfx,
    /// `<low>` low-priority caption.
    Low,
    /// `<len:n>` keeps the caption on screen for `n` seconds.
    Length(f32),
    /// `<delay:n>` text after the tag appears `n` seconds after the caption starts.
    Delay(f32),
    /// `<norepeat:n>` the caption is not shown again within `n` seconds.
    NoRepeat(f32),
    /// Anything else, ignored by the client.
    Other { name: String, args: String },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Span {
    Text(String),
    Tag(Tag),
}

fn parse_rgb(s: &str) -> Option<[u8; 3]> {
    let mut it = s.split(',').map(|p| p.trim().parse::<i32>().ok().map(|v| v.clamp(0, 255) as u8));
    let rgb = [it.next()??, it.next()??, it.next()??];
    it.next().is_none().then_some(rgb)
}

fn tag(name: &str, args: &str) -> Tag {
    let seconds = || crate::kv::parse_f32_lenient(args);
    let other = || Tag::Other { name: name.to_owned(), args: args.to_owned() };
    match name.to_ascii_lowercase().as_str() {
        "clr" if args.is_empty() => Tag::Color(None),
        "clr" => parse_rgb(args).map_or_else(other, |c| Tag::Color(Some(c))),
        "playerclr" => args
            .split_once(':')
            .and_then(|(a, b)| Some(Tag::PlayerColor { player: parse_rgb(a)?, other: parse_rgb(b)? }))
            .unwrap_or_else(other),
        "b" => Tag::Bold,
        "i" => Tag::Italic,
        "cr" => Tag::LineBreak,
        "sfx" => Tag::Sfx,
        "low" => Tag::Low,
        "len" => Tag::Length(seconds()),
        "delay" => Tag::Delay(seconds()),
        "norepeat" => Tag::NoRepeat(seconds()),
        _ => other(),
    }
}

/// Splits caption text into text runs and tags. A `<` without a closing `>`
/// stays text. Tag names match case-insensitively (the client itself expects
/// `B`/`I` upper case and the rest lower case).
pub fn parse_tags(text: &str) -> Vec<Span> {
    let mut out = Vec::new();
    let mut run = String::new();
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        let Some(close) = rest[open..].find('>') else { break };
        run.push_str(&rest[..open]);
        let inner = &rest[open + 1..open + close];
        let (name, args) = inner.split_once(':').unwrap_or((inner, ""));
        if !run.is_empty() {
            out.push(Span::Text(std::mem::take(&mut run)));
        }
        out.push(Span::Tag(tag(name.trim(), args.trim())));
        rest = &rest[open + close + 1..];
    }
    run.push_str(rest);
    if !run.is_empty() {
        out.push(Span::Text(run));
    }
    out
}

/// Caption text with every tag removed.
pub fn plain_text(text: &str) -> String {
    parse_tags(text)
        .into_iter()
        .filter_map(|s| match s {
            Span::Text(t) => Some(t),
            Span::Tag(Tag::LineBreak) => Some("\n".to_owned()),
            Span::Tag(_) => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compile(entries: &[(&str, &str)], block_size: usize) -> Vec<u8> {
        let mut blocks: Vec<Vec<u8>> = vec![Vec::new()];
        let mut dir = Vec::new();
        for (token, text) in entries {
            let mut bytes: Vec<u8> = text.encode_utf16().chain([0]).flat_map(|u| u.to_le_bytes()).collect();
            if blocks.last().unwrap().len() + bytes.len() > block_size {
                blocks.push(Vec::new());
            }
            let block = blocks.len() - 1;
            dir.push((token_hash(token), block as u32, blocks[block].len() as u16, bytes.len() as u16));
            blocks[block].append(&mut bytes);
        }
        let data_offset = (24 + dir.len() * 12).next_multiple_of(512);
        let mut out = b"VCCD".to_vec();
        for v in [1, blocks.len() as i32, block_size as i32, dir.len() as i32, data_offset as i32] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for (hash, block, offset, length) in dir {
            out.extend_from_slice(&hash.to_le_bytes());
            out.extend_from_slice(&block.to_le_bytes());
            out.extend_from_slice(&offset.to_le_bytes());
            out.extend_from_slice(&length.to_le_bytes());
        }
        out.resize(data_offset, 0);
        for mut b in blocks {
            b.resize(block_size, 0);
            out.extend(b);
        }
        out
    }

    #[test]
    fn crc() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(token_hash("Npc.Hello"), crc32(b"npc.hello"));
    }

    #[test]
    fn lookup_across_files() {
        let a = CaptionFile::parse(compile(&[("Test.One", "<sfx>[Beep]"), ("Test.Two", "two")], 32)).unwrap();
        let b = CaptionFile::parse(compile(&[("test.two", "shadowed"), ("Test.Three", "three")], 64)).unwrap();
        let mut c = Captions::default();
        c.add("a", a);
        c.add("b", b);
        assert_eq!(c.lookup("TEST.ONE").as_deref(), Some("<sfx>[Beep]"));
        assert_eq!(c.lookup("test.two").as_deref(), Some("two"));
        assert_eq!(c.lookup("test.three").as_deref(), Some("three"));
        assert_eq!(c.lookup("test.four"), None);
        assert_eq!(c.len(), 4);
        assert!(CaptionFile::parse(b"VCCD\x02\0\0\0".to_vec()).is_err());
    }

    #[test]
    fn tags() {
        let spans = parse_tags("<clr:250,231,181><norepeat:5>Name: Hello<cr>world <len:2.5><B>x<clr><unknown:1> <3");
        assert_eq!(
            spans,
            vec![
                Span::Tag(Tag::Color(Some([250, 231, 181]))),
                Span::Tag(Tag::NoRepeat(5.0)),
                Span::Text("Name: Hello".into()),
                Span::Tag(Tag::LineBreak),
                Span::Text("world ".into()),
                Span::Tag(Tag::Length(2.5)),
                Span::Tag(Tag::Bold),
                Span::Text("x".into()),
                Span::Tag(Tag::Color(None)),
                Span::Tag(Tag::Other { name: "unknown".into(), args: "1".into() }),
                Span::Text(" <3".into()),
            ]
        );
        assert_eq!(
            parse_tags("<playerclr:255,255,255:100,100,100><delay:1.5><sfx><low>")[..2],
            [Span::Tag(Tag::PlayerColor { player: [255; 3], other: [100; 3] }), Span::Tag(Tag::Delay(1.5))]
        );
        assert_eq!(plain_text("<clr:1,2,3>A<cr>B"), "A\nB");
    }

    /// Sound entry names in the string pool of `scenes/scenes.image` (the
    /// compiled choreography scenes). Only the pool is read here.
    fn scene_strings(data: &[u8]) -> Vec<String> {
        let mut r = Reader::new(data);
        if r.bytes(4).ok() != Some(b"VSIF") {
            return Vec::new();
        }
        let _version = r.i32().unwrap();
        let _scenes = r.i32().unwrap();
        let strings = r.i32().unwrap().max(0) as usize;
        r.skip(4).unwrap();
        (0..strings).filter_map(|_| r.u32().ok()).map(|off| crate::reader::cstr_at(data, off as usize)).collect()
    }

    /// Loads the English captions (skipped without an installation), checks
    /// that the voice lines named by sound scripts and scenes have captions and
    /// spot-checks GLaDOS and Wheatley lines.
    #[test]
    fn install_captions_cover_voice_lines() {
        use crate::soundscript::SoundScripts;
        let Some(fs) = GameFileSystem::find_install().and_then(|root| GameFileSystem::portal2(root).ok()) else {
            eprintln!("Portal 2 installation not found; skipping");
            return;
        };
        let captions = Captions::load(&fs, "english");
        for (name, f) in &captions.files {
            println!("{name}: {} captions", f.len());
        }
        assert!(captions.files.len() >= 2 && captions.len() > 3000);
        let scripts = SoundScripts::load(&fs).unwrap();

        // Speaker token prefixes and the label their subtitles start with.
        for (prefix, label) in [("glados.", "GLaDOS:"), ("sphere03.", "Wheatley:"), ("announcer.", "Announcer:")] {
            let tokens: Vec<&str> = scripts
                .iter()
                .filter(|e| e.name.to_ascii_lowercase().starts_with(prefix))
                .filter(|e| e.waves.iter().filter_map(|w| w.file_path()).any(|p| fs.exists(&p)))
                .map(|e| e.name.as_str())
                .collect();
            let texts: Vec<String> = tokens.iter().filter_map(|t| captions.lookup(t)).collect();
            let labelled = texts.iter().filter(|t| plain_text(t).trim_start().starts_with(label)).count();
            let coloured = texts.iter().filter(|t| parse_tags(t).iter().any(|s| matches!(s, Span::Tag(Tag::Color(Some(_)))))).count();
            println!(
                "{prefix}*: {} playable entries, {} captioned, {labelled} labelled {label:?}, {coloured} coloured",
                tokens.len(),
                texts.len()
            );
            assert!(tokens.len() > 50, "too few {prefix} entries");
            assert!(texts.len() * 100 >= tokens.len() * 95, "{prefix} caption coverage too low");
            assert!(labelled * 100 >= texts.len() * 95 && coloured * 100 >= texts.len() * 95);
            // Spot checks spread over the captioned lines.
            let captioned: Vec<&&str> = tokens.iter().filter(|t| captions.contains(t)).collect();
            for t in captioned.iter().step_by(captioned.len() / 6) {
                let text = captions.lookup(t).unwrap();
                let plain = plain_text(&text);
                assert!(plain.len() > label.len() + 2, "{t}: empty caption");
                println!("  {t}: {} chars, speaker {:?}", plain.len(), plain.split(':').next().unwrap_or(""));
            }
        }

        let image = fs.read("scenes/scenes.image").expect("scenes/scenes.image");
        let mut scene_tokens: Vec<String> = scene_strings(&image).into_iter().filter(|s| scripts.get(s).is_some()).collect();
        scene_tokens.sort_by_key(|s| s.to_ascii_lowercase());
        scene_tokens.dedup_by_key(|s| s.to_ascii_lowercase());
        let voiced: Vec<&String> = scene_tokens
            .iter()
            .filter(|t| scripts.get(t).is_some_and(|e| e.waves.iter().any(|w| w.path.to_ascii_lowercase().starts_with("vo/"))))
            .collect();
        let uncaptioned: Vec<&&String> = voiced.iter().filter(|t| !captions.contains(t)).collect();
        println!(
            "scenes.image names {} sound entries, {} of them voice-over; {} without captions: {:?}",
            scene_tokens.len(),
            voiced.len(),
            uncaptioned.len(),
            uncaptioned.iter().take(20).collect::<Vec<_>>()
        );
        assert!(voiced.len() > 500);
        assert!(uncaptioned.len() * 100 < voiced.len() * 5, "scene voice lines missing captions");
    }
}
