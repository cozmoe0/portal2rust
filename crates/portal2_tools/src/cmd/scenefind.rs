//! Lists compiled scenes with an actor (and optionally an event kind)
//! matching the arguments, named by the paths SP maps and scripts reference:
//! `scenefind <actor> [kind]`.
use std::collections::HashMap;

use source_formats::bsp::Bsp;
use source_formats::scenes_image::{ScenesImage, scene_crc};

pub fn main() {
    let actor = crate::args().nth(1).expect("usage: scenefind <actor> [kind]").to_ascii_lowercase();
    let kind = crate::args().nth(2);
    let fs = crate::game_fs();
    let Some(Ok(image)) = ScenesImage::from_fs(&fs) else { return };
    // Scene paths named by SP map entities and by scripts.
    let mut names: HashMap<u32, String> = HashMap::new();
    for m in fs.list("maps", Some("bsp"), false).into_iter().filter(|m| m.contains("/sp_")) {
        let Some(data) = fs.read(&m) else { continue };
        let Ok(bsp) = Bsp::parse(&data) else { continue };
        let map = m.rsplit('/').next().unwrap_or(&m).trim_end_matches(".bsp").to_owned();
        for e in &bsp.entities {
            if let Some(s) = e.get("scenefile") {
                names.insert(scene_crc(s), format!("{s} [{map}]"));
            }
        }
    }
    for p in fs.list("scripts/vscripts", Some("nut"), true) {
        let Some(text) = fs.read_string(&p) else { continue };
        for part in text.split('"').filter(|s| s.to_ascii_lowercase().ends_with(".vcd")) {
            names.entry(scene_crc(part)).or_insert_with(|| format!("{part} [{p}]"));
        }
    }
    for i in 0..image.len() {
        let Ok(scene) = image.scene(i) else { continue };
        for a in scene.actors.iter().filter(|a| a.name.eq_ignore_ascii_case(&actor)) {
            let events: Vec<_> =
                a.channels.iter().flat_map(|c| &c.events).filter(|e| kind.as_deref().is_none_or(|k| e.kind.name() == k)).collect();
            if events.is_empty() {
                continue;
            }
            let crc = image.entries()[i].crc;
            let name = names.get(&crc).cloned().unwrap_or_else(|| format!("{crc:08x} (not referenced by SP maps or scripts)"));
            println!(
                "{name}: {} events, e.g. {:?}",
                events.len(),
                events.iter().take(3).map(|e| (e.kind.name(), e.param.as_str(), e.start, e.end)).collect::<Vec<_>>()
            );
        }
    }
}
