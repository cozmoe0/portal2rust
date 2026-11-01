//! Surface profiles for the visual overhaul: roughness, reflectance, whether
//! a material may turn wet, foliage, and self-illumination strength.
//!
//! The data is `fx_surfaces.txt` (KeyValues, embedded): a material starts
//! from its surface property's game material (from the game's
//! `scripts/surfaceproperties*.txt`, following `base` inheritance), then
//! its surface property, then each path pattern it contains, later entries
//! overriding earlier ones key by key. Profiles are worked out once per
//! material when it is built and travel in its `fx` parameters.

use std::collections::HashMap;
use std::sync::OnceLock;

use bevy::prelude::*;
use source_formats::kv::{self, KeyValues};

use portal2_core::vfs::GameFs;

const PROFILES: &str = include_str!("fx_surfaces.txt");

/// Bits of [`SurfaceProfile::flags`] (and the shaders' `fx.w`).
pub mod profile_flags {
    pub const FOLIAGE: u32 = 1;
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceProfile {
    pub roughness: f32,
    pub f0: f32,
    pub wet: f32,
    pub flags: u32,
    pub emissive: f32,
}

impl Default for SurfaceProfile {
    fn default() -> Self {
        Self { roughness: 0.75, f0: 0.04, wet: 0.0, flags: 0, emissive: 1.0 }
    }
}

impl SurfaceProfile {
    fn apply(&mut self, block: &KeyValues) {
        if let Some(v) = block.get_f32("roughness") {
            self.roughness = v.clamp(0.02, 1.0);
        }
        if let Some(v) = block.get_f32("f0") {
            self.f0 = v.clamp(0.0, 1.0);
        }
        if let Some(v) = block.get_f32("wet") {
            self.wet = v.clamp(0.0, 1.0);
        }
        if let Some(v) = block.get_f32("emissive") {
            self.emissive = v.max(0.0);
        }
        match block.get_f32("foliage") {
            Some(v) if v != 0.0 => self.flags |= profile_flags::FOLIAGE,
            Some(_) => self.flags &= !profile_flags::FOLIAGE,
            None => {}
        }
    }

    /// The material parameters' `fx` vector: roughness, f0, wet, flags.
    pub fn params(&self) -> Vec4 {
        Vec4::new(self.roughness, self.f0, self.wet, self.flags as f32)
    }
}

struct Profiles {
    game_materials: HashMap<String, KeyValues>,
    surface_props: HashMap<String, KeyValues>,
    paths: Vec<(String, KeyValues)>,
}

fn section(root: &KeyValues, name: &str) -> Vec<(String, KeyValues)> {
    root.get(name).map(|s| s.children().iter().map(|c| (c.key.to_ascii_lowercase(), c.clone())).collect()).unwrap_or_default()
}

fn profiles() -> &'static Profiles {
    static PARSED: OnceLock<Profiles> = OnceLock::new();
    PARSED.get_or_init(|| {
        let root = kv::parse(PROFILES).unwrap_or_else(|| KeyValues::new_block("FxSurfaces", Vec::new()));
        Profiles {
            game_materials: section(&root, "gamematerial").into_iter().collect(),
            surface_props: section(&root, "surfaceprop").into_iter().collect(),
            paths: section(&root, "path"),
        }
    })
}

/// Surface property name to game material letter, from the game's files.
fn game_materials(fs: &GameFs) -> &'static HashMap<String, String> {
    static TABLE: OnceLock<HashMap<String, String>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut raw: HashMap<String, HashMap<String, String>> = HashMap::new();
        if let Some(manifest) = fs.read_string("scripts/surfaceproperties_manifest.txt").and_then(|t| kv::parse(&t)) {
            for file in manifest.get_all("file").filter_map(|f| f.as_str()) {
                let Some(text) = fs.read_string(file) else { continue };
                for root in kv::parse_document(&text).roots {
                    let entry = raw.entry(root.key.to_ascii_lowercase()).or_default();
                    for c in root.children() {
                        if let Some(v) = c.as_str() {
                            entry.insert(c.key.to_ascii_lowercase(), v.to_owned());
                        }
                    }
                }
            }
        }
        let lookup = |name: &str| {
            let mut name = name.to_owned();
            for _ in 0..8 {
                let props = raw.get(&name)?;
                if let Some(g) = props.get("gamematerial") {
                    return Some(g.to_ascii_uppercase());
                }
                name = props.get("base")?.to_ascii_lowercase();
            }
            None
        };
        raw.keys().filter_map(|k| lookup(k).map(|g| (k.clone(), g))).collect()
    })
}

/// The game material letter of surface property `surface_prop` (unknown
/// names are `default`, as in the game).
pub fn game_material(fs: &GameFs, surface_prop: &str) -> Option<&'static str> {
    let table = game_materials(fs);
    table.get(&surface_prop.to_ascii_lowercase()).or_else(|| table.get("default")).map(String::as_str)
}

/// The profile of the material at `path` with surface property `surface_prop`.
pub fn profile(fs: &GameFs, path: &str, surface_prop: &str) -> SurfaceProfile {
    profile_with(path, surface_prop, game_material(fs, surface_prop))
}

fn profile_with(path: &str, surface_prop: &str, game_material: Option<&str>) -> SurfaceProfile {
    let p = profiles();
    let mut out = SurfaceProfile::default();
    if let Some(block) = game_material.and_then(|g| p.game_materials.get(&g.to_ascii_lowercase())) {
        out.apply(block);
    }
    if let Some(block) = p.surface_props.get(surface_prop) {
        out.apply(block);
    }
    let path = path.replace('\\', "/").to_ascii_lowercase();
    for (pattern, block) in &p.paths {
        if path.contains(pattern.as_str()) {
            out.apply(block);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layered_profiles() {
        // Concrete floors may turn wet; glass never; vines are foliage.
        let concrete = profile_with("materials/concrete/concrete_floor_01.vmt", "concrete", Some("C"));
        assert!(concrete.wet > 0.9 && concrete.roughness > 0.8);
        let glass = profile_with("materials/glass/glasswindow.vmt", "glass", Some("Y"));
        assert_eq!(glass.wet, 0.0);
        assert!(glass.roughness < 0.1);
        let vine = profile_with("materials/models/props_foliage/vine_large.vmt", "default", Some("C"));
        assert_eq!(vine.flags & profile_flags::FOLIAGE, profile_flags::FOLIAGE);
        // Signs glow and stay dry whatever they are made of.
        let sign = profile_with("materials/signage/indicator_lights/indicator_lights_floor.vmt", "metal", Some("M"));
        assert_eq!(sign.wet, 0.0);
        assert!(sign.emissive >= 1.0);
        // Painted metal is not a mirror.
        let panel = profile_with("materials/metal/metalwall_01.vmt", "metalpanel", Some("M"));
        assert!(panel.f0 < 0.2 && panel.roughness >= 0.4);
        // Unknown materials fall back to the defaults.
        assert_eq!(profile_with("materials/x.vmt", "x", None), SurfaceProfile::default());
    }
}
