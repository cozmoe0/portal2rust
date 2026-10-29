//! Built-in console commands.

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};

use super::{CommandArgs, Console, binds};
use crate::entities::EntityRegistry;
use crate::entities::io::{EventQueue, EventTarget};
use crate::player::Player;
use crate::world::{CurrentMap, LoadMap, MapLoadType};

pub fn register(c: &mut Console) {
    c.register_command("echo", echo);
    c.register_command("exec", exec);
    c.register_command("alias", alias);
    c.register_command("bind", bind);
    c.register_command("unbind", unbind);
    c.register_command("unbindall", unbindall);
    c.register_command("cmd1", cmd1);
    c.register_command("stuffcmds", stuffcmds);
    c.register_command("find", find);
    c.register_command("cvarlist", cvarlist);
    c.register_command("quit", quit);
    c.register_command("exit", quit);
    c.register_command("map", map);
    c.register_command("changelevel", changelevel);
    c.register_command("setpos", setpos);
    c.register_command("setpos_exact", setpos);
    c.register_command("setang", setang);
    c.register_command("getpos", getpos);
    c.register_command("p2r_stuck", stuck);
    c.register_command("snd_list", snd_list);
    c.register_command("play", play);
    c.register_command("playvol", play);
    c.register_command("noclip", noclip);
    c.register_command("god", god);
    c.register_command("kill", kill);
    c.register_command("explode", kill);
    c.register_command("ent_fire", ent_fire);
    c.register_command("give_portalgun", give_portalgun);
    c.register_command("upgrade_portalgun", upgrade_portalgun);
    c.register_command("upgrade_potatogun", upgrade_portalgun);
    c.register_command("screenshot", screenshot);
    c.register_command("jpeg", screenshot);
    // Accepted without effect until the systems behind them exist.
    for name in [
        "bind_osx",
        "cmd2",
        "startupmenu",
        "joyadvancedupdate",
        "toggleconsole",
        "cancelselect",
        "gameui_activate",
        "swap_ss_input",
        "sv_shutdown",
    ] {
        c.register_command(name, ignore);
    }
}

fn ignore(_world: &mut World, _args: &CommandArgs) {}

/// `kill` / `explode`: the player dies.
fn kill(world: &mut World, _args: &CommandArgs) {
    world.write_message(crate::player::health::DamagePlayer { amount: 1.0e6, push: Vec3::ZERO, source: None });
}

fn print(world: &mut World, line: impl Into<String>) {
    world.resource_mut::<Console>().print(line);
}

/// Cheat commands refuse to run unless sv_cheats is on.
fn cheats(world: &mut World, args: &CommandArgs) -> bool {
    if world.resource::<Console>().cheats() {
        return true;
    }
    print(world, format!("Can't use cheat command {} unless sv_cheats is 1.", args.name()));
    false
}

fn echo(world: &mut World, args: &CommandArgs) {
    print(world, args.rest().to_owned());
}

fn exec(world: &mut World, args: &CommandArgs) {
    let name = args.arg(1);
    if name.is_empty() {
        print(world, "exec <filename>: execute a script file");
        return;
    }
    let file = if name.contains('.') { name.to_owned() } else { format!("{name}.cfg") };
    let text = world.resource::<crate::vfs::GameFs>().read_string(&format!("cfg/{file}"));
    match text {
        Some(text) => world.resource_mut::<Console>().insert_text(&text),
        None => print(world, format!("exec: couldn't exec {file}")),
    }
}

fn alias(world: &mut World, args: &CommandArgs) {
    let mut console = world.resource_mut::<Console>();
    if args.len() < 2 {
        let mut names: Vec<String> = console.aliases.iter().map(|(k, v)| format!("{k} : {v}")).collect();
        names.sort();
        for n in names {
            console.print(n);
        }
        return;
    }
    let text = if args.len() > 2 { args.argv[2..].join(" ") } else { String::new() };
    console.set_alias(args.arg(1), &text);
}

fn bind(world: &mut World, args: &CommandArgs) {
    let key = args.arg(1).to_ascii_lowercase();
    if !binds::is_key_name(&key) {
        print(world, format!("\"{}\" isn't a valid key", args.arg(1)));
        return;
    }
    let mut console = world.resource_mut::<Console>();
    if args.len() == 2 {
        let line = match console.binds.get(&key) {
            Some(b) => format!("\"{key}\" = \"{b}\""),
            None => format!("\"{key}\" is not bound"),
        };
        console.print(line);
        return;
    }
    let command = args.argv[2..].join(" ");
    console.binds.insert(key, command);
}

fn unbind(world: &mut World, args: &CommandArgs) {
    let key = args.arg(1).to_ascii_lowercase();
    world.resource_mut::<Console>().binds.remove(&key);
}

fn unbindall(world: &mut World, _args: &CommandArgs) {
    world.resource_mut::<Console>().binds.clear();
}

/// `cmd1 <command>` addresses the first split-screen player, which is us.
fn cmd1(world: &mut World, args: &CommandArgs) {
    let rest = args.args_raw.clone();
    super::execute_now(world, &rest);
}

fn stuffcmds(world: &mut World, _args: &CommandArgs) {
    let mut console = world.resource_mut::<Console>();
    let text = console.startup_commands.join("\n");
    console.insert_text(&text);
}

fn find(world: &mut World, args: &CommandArgs) {
    let needle = args.arg(1).to_ascii_lowercase();
    let mut console = world.resource_mut::<Console>();
    let mut lines: Vec<String> = console
        .cvars()
        .filter(|c| c.known && c.name.contains(&needle))
        .map(|c| format!("\"{}\" = \"{}\"", c.name, c.value))
        .chain(console.command_names().filter(|n| n.contains(&needle)).map(|n| format!("\"{n}\" (command)")))
        .collect();
    lines.sort();
    for l in lines {
        console.print(l);
    }
}

fn cvarlist(world: &mut World, _args: &CommandArgs) {
    let mut console = world.resource_mut::<Console>();
    let known = console.cvars().filter(|c| c.known).count();
    let commands = console.command_names().count();
    console.print(format!("{known} convars / {commands} concommands"));
}

fn quit(world: &mut World, _args: &CommandArgs) {
    world.write_message(AppExit::Success);
}

fn map(world: &mut World, args: &CommandArgs) {
    let name = args.arg(1).trim_end_matches(".bsp").to_ascii_lowercase();
    if name.is_empty() {
        print(world, "map <mapname>");
        return;
    }
    if !world.resource::<crate::vfs::GameFs>().exists(&format!("maps/{name}.bsp")) {
        print(world, format!("map load failed: {name} not found or invalid"));
        return;
    }
    world.write_message(LoadMap::new(name, MapLoadType::NewGame));
    world.resource_mut::<Console>().wait_for_map();
}

fn changelevel(world: &mut World, args: &CommandArgs) {
    if args.len() < 2 {
        print(world, "changelevel <mapname>");
        return;
    }
    crate::entities::transition::change_level(world, args.arg(1));
    world.resource_mut::<Console>().wait_for_map();
}

fn player_mut(world: &mut World) -> Option<Mut<'_, Player>> {
    let e = world.resource::<EntityRegistry>().player?;
    world.get_mut::<Player>(e)
}

fn setpos(world: &mut World, args: &CommandArgs) {
    if !cheats(world, args) {
        return;
    }
    if args.len() < 3 {
        print(world, "Usage:  setpos x y <z optional>");
        return;
    }
    let Some(mut p) = player_mut(world) else { return };
    let x = args.arg_f32(1).unwrap_or(0.0);
    let y = args.arg_f32(2).unwrap_or(0.0);
    let z = args.arg_f32(3).unwrap_or(p.state.origin.z);
    p.state.origin = Vec3::new(x, y, z);
    p.previous_origin = p.state.origin;
    p.state.velocity = Vec3::ZERO;
    p.ground_ref = None;
}

fn setang(world: &mut World, args: &CommandArgs) {
    if !cheats(world, args) {
        return;
    }
    if args.len() < 3 {
        print(world, "Usage:  setang pitch yaw <roll optional>");
        return;
    }
    let mut input = world.resource_mut::<crate::player::PlayerInput>();
    let roll = args.arg_f32(3).unwrap_or(input.input.angles.z);
    input.input.angles = Vec3::new(args.arg_f32(1).unwrap_or(0.0), args.arg_f32(2).unwrap_or(0.0), roll);
}

fn getpos(world: &mut World, _args: &CommandArgs) {
    let Some(p) = player_mut(world).map(|p| p.state.origin) else { return };
    let a = world.resource::<crate::player::PlayerInput>().input.angles;
    print(world, format!("setpos {:.6} {:.6} {:.6};setang {:.6} {:.6} {:.6}", p.x, p.y, p.z, a.x, a.y, a.z));
}

/// Development aid: lists the solid static volumes and entities the player's
/// hull overlaps.
/// Lists the playing sound channels.
fn snd_list(world: &mut World, _args: &CommandArgs) {
    let lines = crate::audio::list(world);
    print(world, format!("{} channels", lines.len()));
    for l in lines {
        print(world, l);
    }
}

/// `play <sound>` / `playvol <sound> <volume>`: plays a sound without
/// spatialisation.
fn play(world: &mut World, args: &CommandArgs) {
    let name = args.arg(1).to_owned();
    if name.is_empty() {
        return;
    }
    let volume = Some(args.arg(2)).filter(|v| !v.is_empty()).map(source_formats::kv::parse_f32_lenient);
    crate::audio::emit(world, &name, crate::audio::Emit { volume, soundlevel: Some(0.0), ..Default::default() });
}

fn stuck(world: &mut World, _args: &CommandArgs) {
    let Some(state) = player_mut(world).map(|p| p.state) else { return };
    let (mins, maxs) = state.hull();
    let lines: Vec<String> = {
        let cw = world.resource::<crate::collision::CollisionWorld>();
        cw.statics
            .iter()
            .filter(|v| v.contents & crate::collision::mask::PLAYER_SOLID != 0 && v.overlaps_box(state.origin, mins, maxs))
            .map(|v| {
                format!(
                    "static volume {:?}..{:?} contents {:#x} surface {:?}",
                    v.mins,
                    v.maxs,
                    v.contents,
                    v.planes.first().map(|p| p.surface)
                )
            })
            .collect()
    };
    print(world, format!("player hull at {:?}: {} static volumes overlap", state.origin, lines.len()));
    for l in lines {
        print(world, l);
    }
}

fn noclip(world: &mut World, args: &CommandArgs) {
    if !cheats(world, args) {
        return;
    }
    let Some(mut p) = player_mut(world) else { return };
    p.noclip = !p.noclip;
    let on = p.noclip;
    print(world, format!("noclip {}", if on { "ON" } else { "OFF" }));
}

fn god(world: &mut World, args: &CommandArgs) {
    if !cheats(world, args) {
        return;
    }
    let Some(e) = world.resource::<EntityRegistry>().player else { return };
    let Some(mut h) = world.get_mut::<crate::player::health::Health>(e) else { return };
    h.god = !h.god;
    let on = h.god;
    print(world, format!("godmode {}", if on { "ON" } else { "OFF" }));
}

/// `ent_fire <target> <input> [parameter] [delay]`
fn ent_fire(world: &mut World, args: &CommandArgs) {
    if !cheats(world, args) {
        return;
    }
    if args.len() < 3 {
        print(world, "Usage: ent_fire <target> <input> [parameter] [delay]");
        return;
    }
    let target = args.arg(1).to_ascii_lowercase();
    let param = args.arg(3).to_owned();
    let delay = args.arg_f32(4).unwrap_or(0.0);
    let player = world.resource::<EntityRegistry>().player;
    world.resource_mut::<EventQueue>().push(delay, EventTarget::Name(target), args.arg(2), &param, player, player);
}

fn gun(world: &mut World) -> Mut<'_, crate::portals::gun::PortalGun> {
    world.resource_mut::<crate::portals::gun::PortalGun>()
}

fn give_portalgun(world: &mut World, args: &CommandArgs) {
    if cheats(world, args) {
        gun(world).can_fire[0] = true;
    }
}

fn upgrade_portalgun(world: &mut World, args: &CommandArgs) {
    if cheats(world, args) {
        gun(world).can_fire = [true, true];
    }
}

/// Saves the window to `screenshots/<map>NNNN.png` under the working directory.
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
