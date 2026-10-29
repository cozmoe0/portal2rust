//! Counts env_sprite / env_glow render modes, materials and spawn flags over
//! the singleplayer maps: `spritemodes`.
use std::collections::BTreeMap;

use source_formats::bsp::Bsp;
use source_formats::fs::GameFileSystem;

fn main() {
    let fs = GameFileSystem::portal2(GameFileSystem::find_install().unwrap()).unwrap();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for p in fs.list("maps", Some("bsp"), false) {
        let name = p.rsplit('/').next().unwrap_or("").to_owned();
        if !name.starts_with("sp_") {
            continue;
        }
        let Some(bytes) = fs.read(&p) else { continue };
        let Ok(bsp) = Bsp::parse(&bytes) else { continue };
        for e in &bsp.entities {
            let class = e.classname();
            if !matches!(class, "env_sprite" | "env_glow" | "env_sprite_oriented" | "env_lightglow" | "point_spotlight" | "beam_spotlight")
            {
                continue;
            }
            let key = format!(
                "{class} mode={} model={} named={} flags={}",
                e.get("rendermode").unwrap_or("-"),
                e.get("model").unwrap_or("-"),
                e.targetname().is_some(),
                e.get("spawnflags").unwrap_or("-")
            );
            *counts.entry(key).or_default() += 1;
        }
    }
    for (k, v) in counts {
        println!("{v:4} {k}");
    }
}
