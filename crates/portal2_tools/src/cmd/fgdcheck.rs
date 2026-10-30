//! Parses the installed entity definitions and reports coverage (counts only).
use source_formats::fgd::Fgd;

pub fn main() {
    let root = crate::install_root();
    let bin = root.join("bin");
    let read = |name: &str| std::fs::read(bin.join(name)).ok().map(|b| source_formats::text::decode_text(&b));
    let text = read("portal2.fgd").unwrap();
    let fgd = Fgd::parse(&text, &mut |name| read(name));
    let with_model = fgd.classes.values().filter(|c| c.studio_model.is_some()).count();
    println!("{} classes, {} with an editor model", fgd.classes.len(), with_model);
    for class in [
        "prop_testchamber_door",
        "prop_weighted_cube",
        "prop_button",
        "prop_floor_button",
        "npc_portal_turret_floor",
        "prop_dynamic",
        "func_door",
        "logic_relay",
    ] {
        match fgd.resolve(class) {
            Some(c) => println!(
                "{class}: model {} defaults {} spawnflags {:#x} bases {}",
                c.studio_model.is_some(),
                c.defaults.len(),
                c.default_spawnflags,
                c.bases.len()
            ),
            None => println!("{class}: missing"),
        }
    }
}
