//! Forge Game Data (FGD) entity definition reader.
//!
//! Only what the runtime needs is kept: class inheritance, the editor model
//! helper (`studio(...)` / `studioprop(...)`) and keyvalue defaults, which
//! the engine uses to fill in entities that rely on class defaults.

use std::collections::HashMap;

#[derive(Clone, Debug, Default)]
pub struct FgdClass {
    pub name: String,
    pub kind: String,
    pub bases: Vec<String>,
    /// Model from a `studio("...")` / `studioprop("...")` helper, if given.
    pub studio_model: Option<String>,
    /// Lowercase keyvalue name to default value.
    pub defaults: HashMap<String, String>,
    /// Default-on spawnflags bits.
    pub default_spawnflags: u32,
}

#[derive(Clone, Debug, Default)]
pub struct Fgd {
    pub classes: HashMap<String, FgdClass>,
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    At(String),
    Ident(String),
    Str(String),
    Num(String),
    Punct(char),
}

fn tokenize(src: &str) -> Vec<Tok> {
    let b = src.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
        } else if c == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if c == b'"' {
            let start = i + 1;
            i += 1;
            while i < b.len() && b[i] != b'"' {
                i += 1;
            }
            let s = String::from_utf8_lossy(&b[start..i.min(b.len())]).into_owned();
            i += 1;
            out.push(Tok::Str(s));
        } else if c == b'@' {
            let start = i + 1;
            i += 1;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            out.push(Tok::At(String::from_utf8_lossy(&b[start..i]).to_ascii_lowercase()));
        } else if c.is_ascii_digit() || (c == b'-' && b.get(i + 1).is_some_and(|d| d.is_ascii_digit())) {
            let start = i;
            i += 1;
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
                i += 1;
            }
            out.push(Tok::Num(String::from_utf8_lossy(&b[start..i]).into_owned()));
        } else if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'.') {
                i += 1;
            }
            out.push(Tok::Ident(String::from_utf8_lossy(&b[start..i]).into_owned()));
        } else {
            out.push(Tok::Punct(c as char));
            i += 1;
        }
    }
    // Merge "a" + "b" sequences.
    let mut merged: Vec<Tok> = Vec::with_capacity(out.len());
    let mut k = 0;
    while k < out.len() {
        if let (Some(Tok::Punct('+')), Some(Tok::Str(next))) = (out.get(k), out.get(k + 1))
            && let Some(Tok::Str(prev)) = merged.last_mut()
        {
            prev.push_str(next);
            k += 2;
            continue;
        }
        merged.push(out[k].clone());
        k += 1;
    }
    merged
}

impl Fgd {
    /// Parses one FGD file; `include` resolves `@include` file names.
    pub fn parse(src: &str, include: &mut dyn FnMut(&str) -> Option<String>) -> Self {
        let mut fgd = Fgd::default();
        fgd.parse_into(src, include, 0);
        fgd
    }

    fn parse_into(&mut self, src: &str, include: &mut dyn FnMut(&str) -> Option<String>, depth: u32) {
        let toks = tokenize(src);
        let mut i = 0;
        while i < toks.len() {
            let Tok::At(kind) = &toks[i] else {
                i += 1;
                continue;
            };
            i += 1;
            match kind.as_str() {
                "include" => {
                    if let Some(Tok::Str(file)) = toks.get(i) {
                        if depth < 8
                            && let Some(text) = include(file)
                        {
                            self.parse_into(&text, include, depth + 1);
                        }
                        i += 1;
                    }
                }
                k if k.ends_with("class") => {
                    let (class, next) = parse_class(&toks, i, k);
                    i = next;
                    if let Some(class) = class {
                        self.classes.insert(class.name.to_ascii_lowercase(), class);
                    }
                }
                _ => {
                    // @mapsize(...), @MaterialExclusion [...], @AutoVisGroup ... [ ... ]
                    i = skip_directive(&toks, i);
                }
            }
        }
    }

    /// Resolved definition with inherited defaults and helpers.
    pub fn resolve(&self, classname: &str) -> Option<FgdClass> {
        let mut seen = Vec::new();
        self.resolve_inner(&classname.to_ascii_lowercase(), &mut seen)
    }

    fn resolve_inner(&self, name: &str, seen: &mut Vec<String>) -> Option<FgdClass> {
        if seen.iter().any(|s| s == name) || seen.len() > 32 {
            return None;
        }
        seen.push(name.to_owned());
        let class = self.classes.get(name)?;
        let mut out = FgdClass { name: class.name.clone(), kind: class.kind.clone(), ..Default::default() };
        for base in &class.bases {
            if let Some(b) = self.resolve_inner(&base.to_ascii_lowercase(), seen) {
                out.defaults.extend(b.defaults);
                out.default_spawnflags |= b.default_spawnflags;
                if b.studio_model.is_some() {
                    out.studio_model = b.studio_model;
                }
            }
        }
        out.defaults.extend(class.defaults.iter().map(|(k, v)| (k.clone(), v.clone())));
        out.default_spawnflags |= class.default_spawnflags;
        if class.studio_model.is_some() {
            out.studio_model = class.studio_model.clone();
        }
        out.bases = class.bases.clone();
        Some(out)
    }
}

fn skip_directive(toks: &[Tok], mut i: usize) -> usize {
    // Skip balanced (...) and [...] groups belonging to the directive.
    while i < toks.len() {
        match &toks[i] {
            Tok::At(_) => return i,
            Tok::Punct('(') => i = skip_group(toks, i, '(', ')'),
            Tok::Punct('[') => return skip_group(toks, i, '[', ']'),
            _ => i += 1,
        }
    }
    i
}

fn skip_group(toks: &[Tok], mut i: usize, open: char, close: char) -> usize {
    let mut depth = 0;
    while i < toks.len() {
        match &toks[i] {
            Tok::Punct(c) if *c == open => depth += 1,
            Tok::Punct(c) if *c == close => {
                depth -= 1;
                if depth == 0 {
                    return i + 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    i
}

fn parse_class(toks: &[Tok], mut i: usize, kind: &str) -> (Option<FgdClass>, usize) {
    let mut class = FgdClass { kind: kind.to_owned(), ..Default::default() };
    // Helpers until '='.
    while i < toks.len() {
        match &toks[i] {
            Tok::Punct('=') => {
                i += 1;
                break;
            }
            Tok::Ident(helper) => {
                let helper = helper.to_ascii_lowercase();
                let mut args: Vec<String> = Vec::new();
                i += 1;
                if matches!(toks.get(i), Some(Tok::Punct('('))) {
                    let end = skip_group(toks, i, '(', ')');
                    for t in &toks[i + 1..end.saturating_sub(1)] {
                        match t {
                            Tok::Str(s) | Tok::Ident(s) | Tok::Num(s) => args.push(s.clone()),
                            _ => {}
                        }
                    }
                    i = end;
                }
                match helper.as_str() {
                    "base" => class.bases.extend(args),
                    "studio" | "studioprop" => {
                        if let Some(m) = args.first().filter(|m| m.to_ascii_lowercase().ends_with(".mdl")) {
                            class.studio_model = Some(m.clone());
                        }
                    }
                    _ => {}
                }
            }
            Tok::At(_) => return (None, i),
            _ => i += 1,
        }
    }
    let Some(Tok::Ident(name)) = toks.get(i) else { return (None, i) };
    class.name = name.clone();
    i += 1;
    // Optional ": "description"".
    if matches!(toks.get(i), Some(Tok::Punct(':'))) {
        i += 1;
        if matches!(toks.get(i), Some(Tok::Str(_))) {
            i += 1;
        }
    }
    if !matches!(toks.get(i), Some(Tok::Punct('['))) {
        return (Some(class), i);
    }
    let end = skip_group(toks, i, '[', ']');
    parse_body(&toks[i + 1..end.saturating_sub(1)], &mut class);
    (Some(class), end)
}

fn parse_body(toks: &[Tok], class: &mut FgdClass) {
    let mut i = 0;
    while i < toks.len() {
        let Tok::Ident(name) = &toks[i] else {
            i += 1;
            continue;
        };
        let lname = name.to_ascii_lowercase();
        if lname == "input" || lname == "output" {
            // input Name(type) : "desc"
            i += 1;
            while i < toks.len() {
                if let Tok::Punct(')') = toks[i] {
                    i += 1;
                    break;
                }
                i += 1;
            }
            if matches!(toks.get(i), Some(Tok::Punct(':'))) && matches!(toks.get(i + 1), Some(Tok::Str(_))) {
                i += 2;
            }
            continue;
        }
        // keyvalue: name(type) [readonly] [report] : "Display" [: default [: "desc"]] [= [ choices ]]
        if !matches!(toks.get(i + 1), Some(Tok::Punct('('))) {
            i += 1;
            continue;
        }
        let type_end = skip_group(toks, i + 1, '(', ')');
        let kv_type = match toks.get(i + 2) {
            Some(Tok::Ident(t)) => t.to_ascii_lowercase(),
            _ => String::new(),
        };
        i = type_end;
        while matches!(toks.get(i), Some(Tok::Ident(w)) if w.eq_ignore_ascii_case("readonly") || w.eq_ignore_ascii_case("report")) {
            i += 1;
        }
        let mut fields: Vec<Option<String>> = Vec::new();
        while matches!(toks.get(i), Some(Tok::Punct(':'))) {
            i += 1;
            match toks.get(i) {
                Some(Tok::Str(s)) | Some(Tok::Num(s)) => {
                    fields.push(Some(s.clone()));
                    i += 1;
                }
                Some(Tok::Ident(s)) => {
                    fields.push(Some(s.clone()));
                    i += 1;
                }
                _ => fields.push(None),
            }
        }
        // fields[0] = display name, fields[1] = default, fields[2] = description.
        if let Some(Some(default)) = fields.get(1)
            && kv_type != "flags"
        {
            class.defaults.insert(lname.clone(), default.clone());
        }
        if matches!(toks.get(i), Some(Tok::Punct('='))) && matches!(toks.get(i + 1), Some(Tok::Punct('['))) {
            let end = skip_group(toks, i + 1, '[', ']');
            if kv_type == "flags" {
                // value : "label" : default
                let inner = &toks[i + 2..end.saturating_sub(1)];
                let mut k = 0;
                while k < inner.len() {
                    if let Tok::Num(v) = &inner[k] {
                        let bit: u32 = v.parse().unwrap_or(0);
                        let mut j = k + 1;
                        let mut parts = Vec::new();
                        while j + 1 < inner.len() && matches!(inner[j], Tok::Punct(':')) {
                            parts.push(inner[j + 1].clone());
                            j += 2;
                        }
                        if let Some(Tok::Num(d)) = parts.get(1)
                            && d != "0"
                        {
                            class.default_spawnflags |= bit;
                        }
                        k = j;
                    } else {
                        k += 1;
                    }
                }
            }
            i = end;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_classes_with_bases_and_defaults() {
        let src = r#"
            @BaseClass = Targetname [ targetname(target_source) : "Name" : : "The name" ]
            @BaseClass base(Targetname) = Base [ speed(float) : "Speed" : "100" : "desc" ]
            @PointClass base(Base) studio("models/a/b.mdl") = thing : "A " + "thing" [
                skin(integer) : "Skin" : 2
                spawnflags(flags) = [ 1 : "One" : 1 2 : "Two" : 0 ]
                input Open(void) : "Opens"
                output OnOpen(void) : "Fired"
            ]
        "#;
        let fgd = Fgd::parse(src, &mut |_| None);
        let c = fgd.resolve("thing").unwrap();
        assert_eq!(c.studio_model.as_deref(), Some("models/a/b.mdl"));
        assert_eq!(c.defaults.get("speed").map(|s| s.as_str()), Some("100"));
        assert_eq!(c.defaults.get("skin").map(|s| s.as_str()), Some("2"));
        assert_eq!(c.default_spawnflags, 1);
    }
}
