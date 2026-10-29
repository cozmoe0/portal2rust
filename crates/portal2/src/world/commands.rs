//! `map` and `screenshot`.

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use portal2_core::console::commands::print;
use portal2_core::console::{CommandArgs, Console};
use portal2_core::vfs::GameFs;

use super::{CurrentMap, LoadMap, MapLoadType};

/// `map <name>` and `screenshot`/`jpeg [name]`.
pub fn register(c: &mut Console) {
    c.register_command("map", map);
    c.register_command("screenshot", screenshot);
    c.register_command("jpeg", screenshot);
}

fn map(world: &mut World, args: &CommandArgs) {
    let name = args.arg(1).trim_end_matches(".bsp").to_ascii_lowercase();
    if name.is_empty() {
        print(world, "map <mapname>");
        return;
    }
    if !world.resource::<GameFs>().exists(&format!("maps/{name}.bsp")) {
        print(world, format!("map load failed: {name} not found or invalid"));
        return;
    }
    world.write_message(LoadMap::new(name, MapLoadType::NewGame));
    world.resource_mut::<Console>().wait_for_map();
}

/// `screenshot` / `jpeg [name]`: saves the frame under `screenshots/`, as
/// `name.png` when given, otherwise numbered after the map.
fn screenshot(world: &mut World, args: &CommandArgs) {
    let map = world.get_resource::<CurrentMap>().map(|m| m.name.clone()).unwrap_or_else(|| "screenshot".into());
    let dir = std::path::Path::new("screenshots");
    let _ = std::fs::create_dir_all(dir);
    let named = Some(args.arg(1)).filter(|n| !n.is_empty()).map(|n| dir.join(format!("{n}.png")));
    let path = named.unwrap_or_else(|| {
        (0..10000).map(|i| dir.join(format!("{map}{i:04}.png"))).find(|p| !p.exists()).unwrap_or_else(|| dir.join(format!("{map}.png")))
    });
    print(world, format!("Wrote {}", path.display()));
    world.spawn(Screenshot::primary_window()).observe(save_to_disk(path));
}
