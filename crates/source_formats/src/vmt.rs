//! Valve Material Type (VMT) parsing: shader name, parameters, proxies,
//! `patch` materials and the conditional parameter prefixes used by Portal 2.

use std::collections::HashMap;

use crate::kv::{self, KeyValues, KvValue};

#[derive(Clone, Debug, Default)]
pub struct Vmt {
    /// Lowercase shader name, e.g. `lightmappedgeneric`.
    pub shader: String,
    /// Lowercase parameter name (including `$` / `%`) to raw value.
    pub params: HashMap<String, String>,
    /// Raw `Proxies` block children.
    pub proxies: Vec<KeyValues>,
}

/// Render settings that VMT conditionals are evaluated against.
#[derive(Clone, Copy, Debug)]
pub struct VmtEnv {
    pub dx_level: u32,
    pub gpu_level: u32,
    pub srgb: bool,
}

impl Default for VmtEnv {
    fn default() -> Self {
        // Portal 2 on PC at the highest settings. `srgb` follows the
        // hardware config's sRGB-correct blending, which the game reports
        // for DX10-class cards (any current GPU).
        Self { dx_level: 95, gpu_level: 3, srgb: true }
    }
}

impl VmtEnv {
    /// Evaluates a single condition such as `>=dx90`, `GPU<2`, `!srgb`, `360`.
    pub fn eval(&self, cond: &str) -> bool {
        let cond = cond.trim();
        if let Some(rest) = cond.strip_prefix('!') {
            return !self.eval(rest);
        }
        let lower = cond.to_ascii_lowercase();
        let (op, rest) = split_op(&lower);
        if let Some(level) = rest.strip_prefix("dx") {
            let level = parse_dx(level);
            let cur = self.dx_level;
            return match op {
                ">=" => cur >= level,
                "<=" => cur <= level,
                ">" => cur > level,
                "<" => cur < level,
                _ => cur == level,
            };
        }
        if let Some(level) = rest.strip_prefix("gpu") {
            let (op2, num) = split_op(level);
            let op = if op.is_empty() { op2 } else { op };
            let n: u32 = num.trim().parse().unwrap_or(0);
            let cur = self.gpu_level;
            return match op {
                ">=" => cur >= n,
                "<=" => cur <= n,
                ">" => cur > n,
                "<" => cur < n,
                _ => cur == n,
            };
        }
        match rest {
            "srgb" | "srgb_pc" => self.srgb,
            "srgb_gameconsole" | "360" | "x360" | "ps3" | "sonyps3" | "gameconsole" | "lowfill" | "osx" | "linux"
            | "posix" => false,
            "hdr" | "ldr" => rest == "hdr",
            // The game skips keys with conditions it does not know.
            _ => false,
        }
    }
}

fn split_op(s: &str) -> (&str, &str) {
    for op in [">=", "<=", ">", "<"] {
        if let Some(rest) = s.strip_prefix(op) {
            return (op, rest);
        }
    }
    ("", s)
}

fn parse_dx(s: &str) -> u32 {
    // "90" -> 90, "90_20b" -> 92, "95" -> 95, "80" -> 80, "9" -> 90
    let main: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    let mut n: u32 = main.parse().unwrap_or(90);
    if n < 10 {
        n *= 10;
    }
    if s.contains("20b") {
        n += 2;
    }
    n
}

/// Splits `"cond?$param"` into `(Some(cond), "$param")`.
fn split_conditional_key(key: &str) -> (Option<&str>, &str) {
    match key.find('?') {
        Some(i) => (Some(&key[..i]), &key[i + 1..]),
        None => (None, key),
    }
}

fn is_fallback_block_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    let n = n.trim_start_matches('!');
    n.starts_with('<')
        || n.starts_with('>')
        || n.starts_with("gpu")
        || n.starts_with("dx")
        || matches!(n, "srgb" | "hdr" | "ldr" | "360" | "x360" | "ps3" | "gameconsole" | "lowfill" | "osx" | "linux")
}

impl Vmt {
    /// Parses VMT text. `include` resolves `patch` includes: it receives a
    /// material path (e.g. `materials/metal/wall.vmt`) and returns its text.
    pub fn parse(text: &str, include: &mut dyn FnMut(&str) -> Option<String>) -> Option<Self> {
        Self::parse_inner(text, include, 0, &VmtEnv::default())
    }

    pub fn parse_with_env(
        text: &str,
        include: &mut dyn FnMut(&str) -> Option<String>,
        env: &VmtEnv,
    ) -> Option<Self> {
        Self::parse_inner(text, include, 0, env)
    }

    fn parse_inner(
        text: &str,
        include: &mut dyn FnMut(&str) -> Option<String>,
        depth: u32,
        env: &VmtEnv,
    ) -> Option<Self> {
        let root = kv::parse(text)?;
        let shader = root.key.to_ascii_lowercase();
        if shader == "patch" {
            if depth > 8 {
                return None;
            }
            let inc = root.get_str("include")?;
            let base_text = include(inc)?;
            let mut base = Self::parse_inner(&base_text, include, depth + 1, env)?;
            for section in ["insert", "replace"] {
                if let Some(block) = root.get(section) {
                    base.apply_params(block, env);
                }
            }
            return Some(base);
        }
        let mut vmt = Vmt { shader, ..Default::default() };
        vmt.apply_params(&root, env);
        Some(vmt)
    }

    fn apply_params(&mut self, block: &KeyValues, env: &VmtEnv) {
        for child in block.children() {
            match &child.value {
                KvValue::Str(value) => {
                    let (cond, name) = split_conditional_key(&child.key);
                    if cond.is_some_and(|c| !env.eval(c)) {
                        continue;
                    }
                    self.params.insert(name.to_ascii_lowercase(), value.clone());
                }
                KvValue::Block(children) => {
                    if child.key.eq_ignore_ascii_case("proxies") {
                        self.proxies.extend(children.iter().cloned());
                    } else if is_fallback_block_name(&child.key) {
                        if env.eval(&child.key) {
                            self.apply_params(child, env);
                        }
                    } else if child.key.contains('?') {
                        let (cond, _) = split_conditional_key(&child.key);
                        if cond.is_none_or(|c| env.eval(c)) {
                            self.apply_params(child, env);
                        }
                    }
                    // Shader-specific fallback blocks (e.g. "LightmappedGeneric_DX8") are ignored.
                }
            }
        }
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.params.get(&name.to_ascii_lowercase()).map(|s| s.as_str())
    }

    pub fn get_f32(&self, name: &str) -> Option<f32> {
        self.get(name).map(|s| {
            let v = kv::parse_floats(s);
            v.first().copied().unwrap_or_else(|| kv::parse_f32_lenient(s))
        })
    }

    pub fn get_bool(&self, name: &str) -> bool {
        self.get(name).is_some_and(|s| kv::parse_f32_lenient(s) != 0.0)
    }

    /// Parses a color/vector parameter: `[1 2 3]`, `{255 0 0}` (0-255) or a scalar.
    pub fn get_vec3(&self, name: &str) -> Option<glam::Vec3> {
        let raw = self.get(name)?;
        let v = kv::parse_floats(raw);
        let scale = if raw.trim_start().starts_with('{') { 1.0 / 255.0 } else { 1.0 };
        match v.len() {
            0 => None,
            1 | 2 => Some(glam::Vec3::splat(v[0] * scale)),
            _ => Some(glam::Vec3::new(v[0], v[1], v[2]) * scale),
        }
    }

    /// Texture path parameters, normalised to `materials/<name>.vtf`.
    pub fn texture_path(&self, name: &str) -> Option<String> {
        let raw = self.get(name)?.trim();
        if raw.is_empty() {
            return None;
        }
        let p = crate::text::normalize_path(raw);
        let p = crate::text::strip_extension(&p).to_owned();
        let p = p.strip_prefix("materials/").unwrap_or(&p).to_owned();
        Some(format!("materials/{p}.vtf"))
    }
}

/// Normalises a material name from a BSP or MDL to `materials/<name>.vmt`.
pub fn material_path(name: &str) -> String {
    let p = crate::text::normalize_path(name);
    let p = if crate::text::extension(&p) == "vmt" { crate::text::strip_extension(&p).to_owned() } else { p };
    let p = p.strip_prefix("materials/").unwrap_or(&p).to_owned();
    format!("materials/{p}.vmt")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conditionals() {
        let env = VmtEnv::default();
        assert!(env.eval(">=dx90"));
        assert!(!env.eval("<dx90"));
        assert!(env.eval("GPU>=2"));
        assert!(!env.eval("GPU<2"));
        assert!(!env.eval("360"));
        assert!(env.eval("!360"));
        assert!(env.eval("srgb") && env.eval("srgb_pc") && !env.eval("!srgb"));
        assert!(!env.eval("SonyPS3") && !env.eval("srgb_gameconsole"));
        let glass = r#""LightmappedGeneric" { "$alpha" 1 "srgb?$alpha" "0.5" "SonyPS3?$alpha" ".7" }"#;
        let vmt = Vmt::parse(glass, &mut |_| None).unwrap();
        assert_eq!(vmt.get_f32("$alpha"), Some(0.5));
    }

    #[test]
    fn patch() {
        let base = r#""LightmappedGeneric" { "$basetexture" "a/b" "$envmap" "env_cubemap" }"#;
        let patch = r#""patch" { "include" "materials/a/b.vmt" "replace" { "$envmap" "maps/x/c0_0_0" } }"#;
        let vmt = Vmt::parse(patch, &mut |p| (p == "materials/a/b.vmt").then(|| base.to_owned())).unwrap();
        assert_eq!(vmt.shader, "lightmappedgeneric");
        assert_eq!(vmt.get("$envmap"), Some("maps/x/c0_0_0"));
        assert_eq!(vmt.texture_path("$basetexture").as_deref(), Some("materials/a/b.vtf"));
    }
}
