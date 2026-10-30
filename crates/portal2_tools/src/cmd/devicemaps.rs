//! Counts puzzle-device entities per single-player map (counts only).
use source_formats::bsp::Bsp;

const CLASSES: [(&str, &str); 7] = [
    ("env_portal_laser", "laser"),
    ("prop_laser_catcher", "catcher"),
    ("prop_laser_relay", "relay"),
    ("prop_wall_projector", "bridge"),
    ("prop_tractor_beam", "funnel"),
    ("trigger_catapult", "catapult"),
    ("npc_portal_turret_floor", "turret"),
];

pub fn main() {
    let fs = crate::game_fs();
    let mut maps: Vec<String> = fs.list("maps", Some("bsp"), false).into_iter().filter(|m| m.contains("/sp_")).collect();
    maps.sort();
    for m in &maps {
        let Ok(bsp) = Bsp::parse(&fs.read(m).unwrap()) else { continue };
        let counts: Vec<String> = CLASSES
            .iter()
            .filter_map(|(class, label)| {
                let n = bsp.entities.iter().filter(|e| e.classname() == *class).count();
                (n > 0).then(|| format!("{label}={n}"))
            })
            .collect();
        if !counts.is_empty() {
            let name = m.rsplit('/').next().unwrap_or(m).trim_end_matches(".bsp");
            println!("{name}: {}", counts.join(" "));
        }
    }
}
