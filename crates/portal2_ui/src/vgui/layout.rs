//! Panel placement from layout files (`resource/ui/*.res`,
//! `scripts/hudlayout.res`), following `Panel::ApplySettings`.
//!
//! `xpos`/`ypos` may start with `r` (from the right / bottom), `c` (from the
//! centre) or `%` (percent of the screen); `wide`/`tall` with `f` (the
//! screen size minus the value) or `%`. Numbers are in 640×480 units for
//! proportional panels. Alignment is against the screen, not the parent
//! (unless `proportionalToParent` is set); the result is relative to the
//! parent.

use source_formats::fs::GameFileSystem;
use source_formats::kv::{self, KeyValues};

use super::scheme::{Symbols, proportional};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    pub fn right(&self) -> i32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    pub fn offset(&self, dx: i32, dy: i32) -> Self {
        Self { x: self.x + dx, y: self.y + dy, ..*self }
    }
}

/// One `xpos`-style value.
fn position(s: &str, align: i32, full: i32, screen_tall: i32, prop: bool) -> i32 {
    let s = s.trim();
    let (mode, rest) = match s.as_bytes().first() {
        Some(b'r' | b'R') => ('r', &s[1..]),
        Some(b'c' | b'C') => ('c', &s[1..]),
        Some(b'%') => ('%', &s[1..]),
        _ => (' ', s),
    };
    if mode == '%' {
        return (kv::parse_f32_lenient(rest) * 0.01 * full as f32) as i32;
    }
    let mut v = kv::parse_i32_lenient(rest);
    if prop {
        v = proportional(v, screen_tall);
    }
    match mode {
        'r' => align - v,
        'c' => align / 2 + v,
        _ => v,
    }
}

/// One `wide`-style value.
fn size(s: &str, align: i32, full: i32, screen_tall: i32, prop: bool) -> i32 {
    let s = s.trim();
    let (mode, rest) = match s.as_bytes().first() {
        Some(b'f' | b'F') => ('f', &s[1..]),
        Some(b'%') => ('%', &s[1..]),
        _ => (' ', s),
    };
    if mode == '%' {
        return (kv::parse_f32_lenient(rest) * 0.01 * full as f32) as i32;
    }
    let mut v = kv::parse_i32_lenient(rest);
    if prop {
        v = proportional(v, screen_tall);
    }
    if mode == 'f' { align - v } else { v }
}

/// The bounds a panel gets from its settings, starting from `current`.
/// `screen` is the screen size; `parent` the parent's bounds (used for
/// `proportionalToParent`).
pub fn apply(settings: &KeyValues, current: Rect, screen: (i32, i32), parent: Rect, prop: bool) -> Rect {
    let to_parent = settings.get_i32("proportionalToParent") == Some(1);
    let (aw, ah) = if to_parent { (parent.w, parent.h) } else { screen };
    let mut r = current;
    if let Some(v) = settings.get_str("xpos") {
        r.x = position(v, aw, screen.0, screen.1, prop);
    }
    if let Some(v) = settings.get_str("ypos") {
        r.y = position(v, ah, screen.1, screen.1, prop);
    }
    if let Some(v) = settings.get_str("wide") {
        r.w = size(v, aw, screen.0, screen.1, prop);
    }
    if let Some(v) = settings.get_str("tall") {
        r.h = size(v, ah, screen.1, screen.1, prop);
    }
    r
}

/// A layout file: the control blocks under its root, in file order.
#[derive(Clone, Debug, Default)]
pub struct ResFile {
    pub controls: Vec<KeyValues>,
}

impl ResFile {
    pub fn load(fs: &GameFileSystem, path: &str, symbols: &Symbols) -> Option<Self> {
        let text = fs.read_string(path)?;
        let doc = symbols.parse(&text);
        let root = doc.roots.into_iter().find(|r| r.is_block())?;
        let controls = root.children().iter().filter(|c| c.is_block()).cloned().collect();
        Some(Self { controls })
    }

    pub fn get(&self, name: &str) -> Option<&KeyValues> {
        self.controls.iter().find(|c| c.key.eq_ignore_ascii_case(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions() {
        let kv = kv::parse(r#""x" { "xpos" "c-250" "ypos" "276" "wide" "500" "tall" "136" }"#).unwrap();
        let r = apply(&kv, Rect::default(), (1600, 900), Rect::new(0, 0, 1600, 900), true);
        assert_eq!(r, Rect::new(800 - 468, 517, 937, 255));
        let kv = kv::parse(r#""x" { "xpos" "r75" "ypos" "10" "wide" "f0" "tall" "%50" }"#).unwrap();
        let r = apply(&kv, Rect::default(), (1600, 900), Rect::new(0, 0, 1600, 900), true);
        assert_eq!(r, Rect::new(1600 - 140, 18, 1600, 450));
    }
}
