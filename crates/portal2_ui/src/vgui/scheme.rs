//! VGUI schemes (`resource/*scheme.res`): named colours, base settings,
//! fonts and the custom font files they need.
//!
//! Conditionals in schemes and layout files are evaluated against the
//! symbols the game sets for the PC build and the current screen (see
//! `Symbols`). A font name resolves to the first of its numbered entries
//! whose `yres` range holds the screen height (entries without `yres`
//! always match); proportional fonts scale `tall`, `blur` and `scanlines`
//! by screen height / 480.

use std::collections::HashMap;

use source_formats::fs::GameFileSystem;
use source_formats::kv::{self, KeyValues};

use super::font::{FontSpec, flags};
use super::surface::Rgba;

/// The conditional symbols for a screen size, as the game sets them:
/// `WIN32WIDE` when width / height ≥ 1.5999, `WIN32HIDEF` from 720 lines
/// up (`WIN32LODEF` below), English text; console, Mac, Linux and Steam
/// Deck symbols are false.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Symbols {
    pub wide: bool,
    pub hidef: bool,
}

impl Symbols {
    pub fn for_screen(width: i32, height: i32) -> Self {
        Self { wide: width as f32 / height.max(1) as f32 >= 1.5999, hidef: height >= 720 }
    }

    pub fn eval(&self, name: &str) -> bool {
        match name {
            "WIN32" | "WINDOWS" | "PC" | "ENGLISH" => true,
            "WIN32WIDE" => self.wide,
            "WIN32HIDEF" => self.hidef,
            "WIN32LODEF" => !self.hidef,
            _ => false,
        }
    }

    pub fn parse(&self, text: &str) -> kv::Document {
        kv::parse_document_with(text, &|s: &str| self.eval(s))
    }
}

/// `GetProportionalScaledValue`: scales a 640×480-based value to the screen.
pub fn proportional(value: i32, screen_tall: i32) -> i32 {
    (screen_tall as f64 / 480.0 * value as f64) as i32
}

#[derive(Clone, Debug)]
struct FontEntry {
    name: String,
    tall: i32,
    weight: i32,
    flags: u32,
    blur: i32,
    scanlines: i32,
    yres: Option<(i32, i32)>,
}

#[derive(Clone, Debug, Default)]
pub struct Scheme {
    colors: HashMap<String, Rgba>,
    base: HashMap<String, String>,
    fonts: HashMap<String, Vec<FontEntry>>,
    pub custom_font_files: Vec<String>,
}

impl Scheme {
    pub fn load(fs: &GameFileSystem, path: &str, symbols: &Symbols) -> Option<Self> {
        let text = fs.read_string(path)?;
        let doc = symbols.parse(&text);
        let root = doc.roots.iter().find(|r| r.is_block())?;
        let mut scheme = Scheme::default();
        if let Some(colors) = root.get("Colors") {
            for c in colors.children() {
                if let Some(v) = c.as_str().and_then(parse_rgba) {
                    scheme.colors.insert(c.key.to_ascii_lowercase(), v);
                }
            }
        }
        if let Some(base) = root.get("BaseSettings") {
            for s in base.children() {
                if let Some(v) = s.as_str() {
                    scheme.base.entry(s.key.to_ascii_lowercase()).or_insert_with(|| v.to_owned());
                }
            }
        }
        if let Some(fonts) = root.get("Fonts") {
            for f in fonts.children().iter().filter(|f| f.is_block()) {
                let entries = f.children().iter().filter(|e| e.is_block()).map(font_entry).collect::<Vec<_>>();
                scheme.fonts.entry(f.key.to_ascii_lowercase()).or_insert(entries);
            }
        }
        if let Some(files) = root.get("CustomFontFiles") {
            for f in files.children() {
                match f.as_str() {
                    Some(p) => scheme.custom_font_files.push(p.to_owned()),
                    None => {
                        if let Some(p) = f.get_str("font") {
                            scheme.custom_font_files.push(p.to_owned());
                        }
                    }
                }
            }
        }
        Some(scheme)
    }

    /// A named colour or an `"r g b a"` string.
    pub fn color(&self, name: &str) -> Option<Rgba> {
        self.colors.get(&name.to_ascii_lowercase()).copied().or_else(|| parse_rgba(name))
    }

    pub fn setting(&self, key: &str) -> Option<&str> {
        self.base.get(&key.to_ascii_lowercase()).map(|s| s.as_str())
    }

    /// A base setting holding a colour (possibly through another setting
    /// or a named colour).
    pub fn setting_color(&self, key: &str) -> Option<Rgba> {
        let mut v = self.setting(key)?;
        for _ in 0..4 {
            if let Some(c) = self.color(v) {
                return Some(c);
            }
            v = self.setting(v)?;
        }
        None
    }

    pub fn setting_i32(&self, key: &str) -> Option<i32> {
        self.setting(key).map(kv::parse_i32_lenient)
    }

    pub fn has_font(&self, name: &str) -> bool {
        self.fonts.contains_key(&name.to_ascii_lowercase())
    }

    /// The font a panel gets for `name` on a screen `screen_tall` lines
    /// high (`proportional` for panels laid out in 640×480 units).
    pub fn font_spec(&self, name: &str, screen_tall: i32, proportional_panel: bool) -> Option<FontSpec> {
        let entries = self.fonts.get(&name.to_ascii_lowercase())?;
        let e = entries.iter().find(|e| match e.yres {
            None => true,
            Some((lo, hi)) => screen_tall >= lo && screen_tall <= hi,
        })?;
        let scale = proportional_panel && e.yres.is_none_or(|(_, hi)| hi > 4000);
        let s = |v: i32| if scale { proportional(v, screen_tall) } else { v };
        let tall = s(e.tall).min(127);
        Some(FontSpec { face: e.name.clone(), tall, weight: e.weight, flags: e.flags, blur: s(e.blur), scanlines: s(e.scanlines) })
    }
}

fn font_entry(e: &KeyValues) -> FontEntry {
    let b = |k: &str| e.get_str(k).is_some_and(|v| kv::parse_i32_lenient(v) != 0);
    let mut f = 0;
    for (key, flag) in [
        ("italic", flags::ITALIC),
        ("underline", flags::UNDERLINE),
        ("strikeout", flags::STRIKEOUT),
        ("symbol", flags::SYMBOL),
        ("antialias", flags::ANTIALIAS),
        ("dropshadow", flags::DROPSHADOW),
        ("outline", flags::OUTLINE),
        ("custom", flags::CUSTOM),
        ("bitmap", flags::BITMAP),
        ("rotary", flags::ROTARY),
        ("additive", flags::ADDITIVE),
    ] {
        if b(key) {
            f |= flag;
        }
    }
    let i = |k: &str| e.get_str(k).map(kv::parse_i32_lenient).unwrap_or(0);
    let yres = e.get_str("yres").and_then(|s| {
        let mut it = s.split_whitespace().map(kv::parse_i32_lenient);
        let lo = it.next().unwrap_or(0);
        let hi = it.next().filter(|v| *v != 0).unwrap_or(lo);
        (lo != 0).then_some((lo, hi))
    });
    FontEntry {
        name: e.get_str("name").unwrap_or("").to_owned(),
        tall: i("tall"),
        weight: i("weight"),
        flags: f,
        blur: i("blur"),
        scanlines: i("scanlines"),
        yres,
    }
}

/// `"r g b a"` (alpha optional, default 255).
pub fn parse_rgba(s: &str) -> Option<Rgba> {
    let v: Vec<i32> = s.split_whitespace().map(kv::parse_i32_lenient).collect();
    if v.len() < 3 || !s.trim_start().starts_with(|c: char| c.is_ascii_digit() || c == '-') {
        return None;
    }
    let c = |i: usize| v.get(i).copied().unwrap_or(255).clamp(0, 255) as u8;
    Some([c(0), c(1), c(2), c(3)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_and_scaling() {
        assert_eq!(parse_rgba("255 155 0 255"), Some([255, 155, 0, 255]));
        assert_eq!(parse_rgba("16 16 16"), Some([16, 16, 16, 255]));
        assert_eq!(parse_rgba("Blank"), None);
        assert_eq!(proportional(22, 900), 41);
        assert_eq!(proportional(88, 1080), 198);
        let s = Symbols::for_screen(1600, 900);
        assert!(s.wide && s.hidef && !s.eval("WIN32LODEF"));
        assert!(!Symbols::for_screen(1024, 768).wide);
    }

    #[test]
    fn installed_scheme() {
        let Some(root) = GameFileSystem::find_install() else { return };
        let Ok(fs) = GameFileSystem::portal2(root) else { return };
        let scheme = Scheme::load(&fs, "resource/basemodui_scheme.res", &Symbols::for_screen(1600, 900)).unwrap();
        let f = scheme.font_spec("MainMenuItem", 900, true).unwrap();
        assert_eq!((f.face.as_str(), f.tall, f.weight), ("UniversLTStd-Cn", 41, 400));
        assert!(f.flags & flags::ANTIALIAS != 0);
        // DialogButton differs between low and high definition screens.
        assert_eq!(scheme.font_spec("DialogButton", 900, true).unwrap().face, "UniversLTStd-LightCn");
        assert_eq!(scheme.setting_color("MainMenuButton.TextColor"), Some([120, 120, 120, 255]));
        assert!(scheme.custom_font_files.iter().any(|f| f.ends_with("universc.vfont")));
    }
}
