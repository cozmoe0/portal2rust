//! Counts the entities and surfaces visual-parity work cares about, per
//! singleplayer map: `m8survey [map ...]`.
use std::collections::BTreeMap;

use source_formats::bsp::Bsp;
use source_formats::fs::GameFileSystem;

const CLASSES: &[&str] = &[
    "move_rope",
    "keyframe_rope",
    "env_sprite",
    "env_glow",
    "env_lightglow",
    "point_spotlight",
    "beam_spotlight",
    "env_beam",
    "env_projectedtexture",
    "vgui_movie_display",
    "func_water_analog",
    "water_lod_control",
    "sky_camera",
    "env_fog_controller",
    "color_correction",
    "env_tonemap_controller",
    "func_dustmotes",
    "func_smokevolume",
    "env_smokestack",
    "env_steam",
    "info_particle_system",
];

fn main() {
    let fs = GameFileSystem::portal2(GameFileSystem::find_install().unwrap()).unwrap();
    let mut maps: Vec<String> = std::env::args().skip(1).collect();
    if maps.is_empty() {
        maps = fs
            .list("maps", Some("bsp"), false)
            .into_iter()
            .filter_map(|p| {
                let name = p.rsplit('/').next()?.trim_end_matches(".bsp").to_owned();
                name.starts_with("sp_").then_some(name)
            })
            .collect();
        maps.sort();
    }
    let mut totals: BTreeMap<&str, usize> = BTreeMap::new();
    for map in maps {
        let Some(bytes) = fs.read(&format!("maps/{map}.bsp")) else { continue };
        let Ok(bsp) = Bsp::parse(&bytes) else { continue };
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for e in &bsp.entities {
            let class = e.get("classname").unwrap_or("");
            if let Some(c) = CLASSES.iter().find(|c| **c == class) {
                *counts.entry(c).or_default() += 1;
                *totals.entry(c).or_default() += 1;
            }
        }
        // Faces whose material uses the Water shader.
        let mut water = BTreeMap::new();
        for f in &bsp.faces {
            let Some(ti) = bsp.texinfo.get(f.texinfo as usize) else { continue };
            let Some(td) = bsp.texdata.get(ti.texdata as usize) else { continue };
            let name = bsp.texdata_strings.get(td.name_index as usize).cloned().unwrap_or_default();
            let lower = name.to_ascii_lowercase();
            if lower.contains("water") || lower.contains("goo") || lower.contains("toxic") {
                *water.entry(lower).or_insert(0usize) += 1;
            }
        }
        let summary: Vec<String> = counts.iter().map(|(k, v)| format!("{k}={v}")).collect();
        println!("{map:26} {}  water {:?}", summary.join(" "), water);
    }
    println!("totals {totals:?}");
}
