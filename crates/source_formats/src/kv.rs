//! Text KeyValues ("KV1") parser.
//!
//! Handles quoted and bare tokens, `//` comments, nested blocks, duplicate
//! keys, platform conditionals such as `[$X360]` / `[!$PS3]`, and records
//! `#include` / `#base` directives so callers can resolve them.

use std::fmt;

#[derive(Clone, PartialEq)]
pub enum KvValue {
    Str(String),
    Block(Vec<KeyValues>),
}

#[derive(Clone, PartialEq)]
pub struct KeyValues {
    pub key: String,
    pub value: KvValue,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IncludeKind {
    Include,
    Base,
}

#[derive(Clone, Debug, Default)]
pub struct Document {
    pub roots: Vec<KeyValues>,
    pub includes: Vec<(IncludeKind, String)>,
}

impl fmt::Debug for KeyValues {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.value {
            KvValue::Str(s) => write!(f, "{:?} {:?}", self.key, s),
            KvValue::Block(c) => f.debug_map().entry(&self.key, c).finish(),
        }
    }
}

impl KeyValues {
    pub fn new_str(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self { key: key.into(), value: KvValue::Str(value.into()) }
    }

    pub fn new_block(key: impl Into<String>, children: Vec<KeyValues>) -> Self {
        Self { key: key.into(), value: KvValue::Block(children) }
    }

    pub fn is_block(&self) -> bool {
        matches!(self.value, KvValue::Block(_))
    }

    pub fn children(&self) -> &[KeyValues] {
        match &self.value {
            KvValue::Block(c) => c,
            KvValue::Str(_) => &[],
        }
    }

    pub fn children_mut(&mut self) -> Option<&mut Vec<KeyValues>> {
        match &mut self.value {
            KvValue::Block(c) => Some(c),
            KvValue::Str(_) => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match &self.value {
            KvValue::Str(s) => Some(s),
            KvValue::Block(_) => None,
        }
    }

    /// First child with a case-insensitive key match.
    pub fn get(&self, key: &str) -> Option<&KeyValues> {
        self.children().iter().find(|c| c.key.eq_ignore_ascii_case(key))
    }

    /// Last child with a matching key (Source uses "last one wins" for duplicates
    /// in most lookups that go through `FindKey` after merges).
    pub fn get_last(&self, key: &str) -> Option<&KeyValues> {
        self.children().iter().rev().find(|c| c.key.eq_ignore_ascii_case(key))
    }

    pub fn get_all<'a>(&'a self, key: &'a str) -> impl Iterator<Item = &'a KeyValues> + 'a {
        self.children().iter().filter(move |c| c.key.eq_ignore_ascii_case(key))
    }

    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(|c| c.as_str())
    }

    pub fn get_f32(&self, key: &str) -> Option<f32> {
        self.get_str(key).map(parse_f32_lenient)
    }

    pub fn get_i32(&self, key: &str) -> Option<i32> {
        self.get_str(key).map(parse_i32_lenient)
    }

    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.get_str(key).map(|s| parse_i32_lenient(s) != 0)
    }

    /// Path lookup with `/` separators, e.g. `"FileSystem/SearchPaths"`.
    pub fn path(&self, path: &str) -> Option<&KeyValues> {
        let mut cur = self;
        for part in path.split('/') {
            cur = cur.get(part)?;
        }
        Some(cur)
    }

    /// Merges `other`'s children into self, replacing string values with the
    /// same key and recursively merging blocks.
    pub fn merge_from(&mut self, other: &KeyValues) {
        let Some(dst) = self.children_mut() else { return };
        for child in other.children() {
            if let Some(existing) = dst.iter_mut().find(|c| c.key.eq_ignore_ascii_case(&child.key)) {
                if existing.is_block() && child.is_block() {
                    existing.merge_from(child);
                } else {
                    existing.value = child.value.clone();
                }
            } else {
                dst.push(child.clone());
            }
        }
    }
}

/// `atof`-like parsing: leading numeric prefix, garbage ignored, 0 on failure.
pub fn parse_f32_lenient(s: &str) -> f32 {
    let s = s.trim();
    let mut end = 0;
    let bytes = s.as_bytes();
    let mut seen_digit = false;
    let mut seen_dot = false;
    let mut seen_exp = false;
    while end < bytes.len() {
        let c = bytes[end];
        let ok = match c {
            b'0'..=b'9' => {
                seen_digit = true;
                true
            }
            b'+' | b'-' => end == 0 || matches!(bytes[end - 1], b'e' | b'E'),
            b'.' if !seen_dot && !seen_exp => {
                seen_dot = true;
                true
            }
            b'e' | b'E' if seen_digit && !seen_exp => {
                seen_exp = true;
                true
            }
            _ => false,
        };
        if !ok {
            break;
        }
        end += 1;
    }
    let mut t = &s[..end];
    while !t.is_empty() && matches!(t.as_bytes()[t.len() - 1], b'e' | b'E' | b'+' | b'-') {
        t = &t[..t.len() - 1];
    }
    t.parse::<f32>().unwrap_or(0.0)
}

pub fn parse_i32_lenient(s: &str) -> i32 {
    let s = s.trim();
    let bytes = s.as_bytes();
    let mut end = 0;
    while end < bytes.len() && (bytes[end].is_ascii_digit() || (end == 0 && matches!(bytes[end], b'-' | b'+'))) {
        end += 1;
    }
    match s[..end].parse::<i64>() {
        Ok(v) => v as i32,
        Err(_) => parse_f32_lenient(s) as i32,
    }
}

/// Parses whitespace separated floats ("1 2 3" or "[1 2 3]" or "{255 0 0}").
pub fn parse_floats(s: &str) -> Vec<f32> {
    s.split(|c: char| c.is_whitespace() || c == '[' || c == ']' || c == '{' || c == '}' || c == ',')
        .filter(|p| !p.is_empty())
        .map(parse_f32_lenient)
        .collect()
}

pub fn parse_vec3(s: &str) -> Option<glam::Vec3> {
    let v = parse_floats(s);
    (v.len() >= 3).then(|| glam::Vec3::new(v[0], v[1], v[2]))
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Str(String, bool), // value, was_quoted
    Open,
    Close,
    Cond(String),
}

struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
    /// Truth of each `$SYMBOL` in conditionals.
    symbols: &'a dyn Fn(&str) -> bool,
}

impl<'a> Lexer<'a> {
    fn next(&mut self) -> Option<Tok> {
        loop {
            while self.pos < self.src.len() && (self.src[self.pos] as char).is_ascii_whitespace() {
                self.pos += 1;
            }
            if self.pos >= self.src.len() {
                return None;
            }
            if self.src[self.pos] == b'/' && self.src.get(self.pos + 1) == Some(&b'/') {
                while self.pos < self.src.len() && self.src[self.pos] != b'\n' {
                    self.pos += 1;
                }
                continue;
            }
            break;
        }
        let c = self.src[self.pos];
        match c {
            b'{' => {
                self.pos += 1;
                Some(Tok::Open)
            }
            b'}' => {
                self.pos += 1;
                Some(Tok::Close)
            }
            b'"' => {
                self.pos += 1;
                let start = self.pos;
                while self.pos < self.src.len() && self.src[self.pos] != b'"' {
                    self.pos += 1;
                }
                let s = String::from_utf8_lossy(&self.src[start..self.pos]).into_owned();
                self.pos += 1; // closing quote (may run past end on truncated input)
                Some(Tok::Str(s, true))
            }
            b'[' => {
                let start = self.pos + 1;
                while self.pos < self.src.len() && self.src[self.pos] != b']' {
                    self.pos += 1;
                }
                let s = String::from_utf8_lossy(&self.src[start..self.pos.min(self.src.len())]).into_owned();
                self.pos += 1;
                Some(Tok::Cond(s))
            }
            _ => {
                let start = self.pos;
                while self.pos < self.src.len() {
                    let ch = self.src[self.pos];
                    if (ch as char).is_ascii_whitespace() || ch == b'{' || ch == b'}' || ch == b'"' {
                        break;
                    }
                    // A bare token directly followed by a comment ends the token.
                    if ch == b'/' && self.src.get(self.pos + 1) == Some(&b'/') {
                        break;
                    }
                    self.pos += 1;
                }
                Some(Tok::Str(String::from_utf8_lossy(&self.src[start..self.pos]).into_owned(), false))
            }
        }
    }
}

/// The symbols that hold for the PC build of the game when nothing more
/// specific (screen size, language) is known.
pub fn pc_symbol(name: &str) -> bool {
    matches!(name, "WIN32" | "WINDOWS" | "PC")
}

/// Evaluates a platform conditional for the PC build of the game.
pub fn eval_condition(cond: &str) -> bool {
    eval_condition_with(cond, &pc_symbol)
}

/// Evaluates a conditional such as `$WIN32 && !$X360`; `symbols` gives the
/// value of each (upper-case) symbol name.
pub fn eval_condition_with(cond: &str, symbols: &dyn Fn(&str) -> bool) -> bool {
    // Grammar: expr := term (('||'|'&&') term)* ; term := '!'? '$'NAME | '(' expr ')'
    fn term(s: &[u8], i: &mut usize, symbols: &dyn Fn(&str) -> bool) -> bool {
        skip_ws(s, i);
        let mut negate = false;
        while *i < s.len() && s[*i] == b'!' {
            negate = !negate;
            *i += 1;
            skip_ws(s, i);
        }
        let v = if *i < s.len() && s[*i] == b'(' {
            *i += 1;
            let v = expr(s, i, symbols);
            skip_ws(s, i);
            if *i < s.len() && s[*i] == b')' {
                *i += 1;
            }
            v
        } else {
            if *i < s.len() && s[*i] == b'$' {
                *i += 1;
            }
            let start = *i;
            while *i < s.len() && (s[*i].is_ascii_alphanumeric() || s[*i] == b'_') {
                *i += 1;
            }
            let name = std::str::from_utf8(&s[start..*i]).unwrap_or("").to_ascii_uppercase();
            symbols(&name)
        };
        v != negate
    }
    fn skip_ws(s: &[u8], i: &mut usize) {
        while *i < s.len() && s[*i].is_ascii_whitespace() {
            *i += 1;
        }
    }
    fn expr(s: &[u8], i: &mut usize, symbols: &dyn Fn(&str) -> bool) -> bool {
        let mut v = term(s, i, symbols);
        loop {
            skip_ws(s, i);
            if s[*i..].starts_with(b"||") {
                *i += 2;
                let r = term(s, i, symbols);
                v = v || r;
            } else if s[*i..].starts_with(b"&&") {
                *i += 2;
                let r = term(s, i, symbols);
                v = v && r;
            } else {
                return v;
            }
        }
    }
    let s = cond.as_bytes();
    let mut i = 0;
    expr(s, &mut i, symbols)
}

/// Parses a KeyValues document.
pub fn parse_document(text: &str) -> Document {
    parse_document_with(text, &pc_symbol)
}

/// Parses a KeyValues document, keeping the entries whose conditionals hold
/// under `symbols`.
pub fn parse_document_with(text: &str, symbols: &dyn Fn(&str) -> bool) -> Document {
    let mut lx = Lexer { src: text.as_bytes(), pos: 0, symbols };
    let mut doc = Document::default();
    let mut peeked: Option<Tok> = None;
    let next = |lx: &mut Lexer, peeked: &mut Option<Tok>| peeked.take().or_else(|| lx.next());

    // Top level is a sequence of `key value` or `key { ... }` entries.
    loop {
        let Some(tok) = next(&mut lx, &mut peeked) else { break };
        match tok {
            Tok::Str(key, quoted) => {
                if !quoted && (key.eq_ignore_ascii_case("#include") || key.eq_ignore_ascii_case("#base")) {
                    if let Some(Tok::Str(file, _)) = next(&mut lx, &mut peeked) {
                        let kind = if key.eq_ignore_ascii_case("#base") { IncludeKind::Base } else { IncludeKind::Include };
                        doc.includes.push((kind, file));
                    }
                    continue;
                }
                if let Some(node) = parse_entry(key, &mut lx, &mut peeked) {
                    doc.roots.push(node);
                }
            }
            Tok::Open => {
                // Anonymous top-level block (entity lumps); parse and keep with empty key.
                let children = parse_block_body(&mut lx, &mut peeked);
                doc.roots.push(KeyValues::new_block(String::new(), children));
            }
            Tok::Close | Tok::Cond(_) => {}
        }
    }
    doc
}

/// Convenience: parse and return the first root.
pub fn parse(text: &str) -> Option<KeyValues> {
    parse_document(text).roots.into_iter().next()
}

fn parse_entry(key: String, lx: &mut Lexer, peeked: &mut Option<Tok>) -> Option<KeyValues> {
    let tok = peeked.take().or_else(|| lx.next())?;
    let (node, cond_ok) = match tok {
        Tok::Open => {
            let children = parse_block_body(lx, peeked);
            let ok = take_condition(lx, peeked);
            (KeyValues::new_block(key, children), ok)
        }
        Tok::Str(value, _) => {
            let ok = take_condition(lx, peeked);
            // `key value [cond] { ... }` never happens; but `key [cond] { ... }` does.
            (KeyValues::new_str(key, value), ok)
        }
        Tok::Cond(c) => {
            // Condition between key and block: "key" [$X360] { ... }
            let ok = eval_condition_with(&c, lx.symbols);
            let inner = parse_entry(key, lx, peeked)?;
            return ok.then_some(inner);
        }
        Tok::Close => {
            *peeked = Some(Tok::Close);
            return None;
        }
    };
    cond_ok.then_some(node)
}

fn take_condition(lx: &mut Lexer, peeked: &mut Option<Tok>) -> bool {
    let tok = peeked.take().or_else(|| lx.next());
    match tok {
        Some(Tok::Cond(c)) => eval_condition_with(&c, lx.symbols),
        other => {
            *peeked = other;
            true
        }
    }
}

fn parse_block_body(lx: &mut Lexer, peeked: &mut Option<Tok>) -> Vec<KeyValues> {
    let mut out = Vec::new();
    loop {
        let Some(tok) = peeked.take().or_else(|| lx.next()) else { break };
        match tok {
            Tok::Close => break,
            Tok::Str(key, _) => {
                if let Some(node) = parse_entry(key, lx, peeked) {
                    out.push(node);
                }
            }
            Tok::Open => {
                let children = parse_block_body(lx, peeked);
                out.push(KeyValues::new_block(String::new(), children));
            }
            Tok::Cond(_) => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic() {
        let doc = parse_document(
            r#"
            "LightmappedGeneric"
            {
                "$basetexture" "metal/wall" // comment
                $surfaceprop metal
                "$envmap" "env_cubemap" [!$X360]
                "$foo" "bar" [$X360]
                "Proxies" { "AnimatedTexture" { "animatedtexturevar" "$basetexture" } }
            }
            "#,
        );
        let root = &doc.roots[0];
        assert_eq!(root.key, "LightmappedGeneric");
        assert_eq!(root.get_str("$basetexture"), Some("metal/wall"));
        assert_eq!(root.get_str("$surfaceprop"), Some("metal"));
        assert_eq!(root.get_str("$envmap"), Some("env_cubemap"));
        assert!(root.get("$foo").is_none());
        assert!(root.path("Proxies/AnimatedTexture").is_some());
    }

    #[test]
    fn symbols() {
        let text = r#""a" { "x" "1" [$WIN32WIDE] "x" "2" [!$WIN32WIDE] "y" [$WIN32HIDEF && !$DECK] { "z" "3" } }"#;
        let wide = |s: &str| matches!(s, "WIN32" | "WIN32WIDE" | "WIN32HIDEF");
        let root = &parse_document_with(text, &wide).roots[0];
        assert_eq!(root.get_str("x"), Some("1"));
        assert_eq!(root.path("y/z").and_then(|k| k.as_str()), Some("3"));
        let root = &parse_document(text).roots[0];
        assert_eq!(root.get_str("x"), Some("2"));
        assert!(root.get("y").is_none());
    }

    #[test]
    fn lenient_numbers() {
        assert_eq!(parse_f32_lenient("1.5f"), 1.5);
        assert_eq!(parse_f32_lenient(" -3 "), -3.0);
        assert_eq!(parse_i32_lenient("12abc"), 12);
        assert_eq!(parse_floats("[1 2 3]"), vec![1.0, 2.0, 3.0]);
    }
}
