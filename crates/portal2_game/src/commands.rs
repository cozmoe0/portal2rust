//! Console commands that act on the game: `changelevel`, `setpos`,
//! `noclip`, `god`, `kill`, `ent_fire`, the portal gun upgrades...

use bevy::prelude::*;
use portal2_core::console::commands::{print, require_cheats as cheats};
use portal2_core::console::{CommandArgs, Console};

use crate::entities::io::{EventQueue, EventTarget};
use crate::player::Player;
use portal2_core::entity::EntityRegistry;

pub struct GameCommandsPlugin;

impl Plugin for GameCommandsPlugin {
    fn build(&self, app: &mut App) {
        register(&mut app.world_mut().resource_mut::<Console>());
    }
}

fn register(c: &mut Console) {
    c.register_command("changelevel", changelevel);
    c.register_command("setpos", setpos);
    c.register_command("setpos_exact", setpos);
    c.register_command("setang", setang);
    c.register_command("getpos", getpos);
    c.register_command("p2r_stuck", stuck);
    c.register_command("noclip", noclip);
    c.register_command("god", god);
    c.register_command("kill", kill);
    c.register_command("explode", kill);
    c.register_command("ent_fire", ent_fire);
    c.register_command("give", give);
    c.register_command("give_portalgun", give_portalgun);
    c.register_command("upgrade_portalgun", upgrade_portalgun);
    c.register_command("upgrade_potatogun", upgrade_portalgun);
}

/// `kill` / `explode`: the player dies.
fn kill(world: &mut World, _args: &CommandArgs) {
    world.write_message(crate::player::health::DamagePlayer { amount: 1.0e6, push: Vec3::ZERO, source: None });
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

/// Development aid: lists the solid static volumes the player's hull
/// overlaps.
fn stuck(world: &mut World, _args: &CommandArgs) {
    let Some(state) = player_mut(world).map(|p| p.state) else { return };
    let (mins, maxs) = state.hull();
    let lines: Vec<String> = {
        let cw = world.resource::<portal2_world::collision::CollisionWorld>();
        cw.statics
            .iter()
            .filter(|v| v.contents & portal2_world::collision::mask::PLAYER_SOLID != 0 && v.overlaps_box(state.origin, mins, maxs))
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

/// `give <classname>`: hands the player an item. The only one Portal 2's
/// player carries is `weapon_portalgun`, which (as the game constructs it)
/// fires the first portal only; a gun already held keeps what it had. Maps
/// give the gun this way through a `point_servercommand`.
fn give(world: &mut World, args: &CommandArgs) {
    if !cheats(world, args) {
        return;
    }
    match args.arg(1).to_ascii_lowercase().as_str() {
        "weapon_portalgun" => crate::portals::gun::enable_portals(world, [true, false]),
        other => print(world, format!("give: {other} is not an item the player can hold")),
    }
}

fn give_portalgun(world: &mut World, args: &CommandArgs) {
    if cheats(world, args) {
        crate::portals::gun::enable_portals(world, [true, false]);
    }
}

fn upgrade_portalgun(world: &mut World, args: &CommandArgs) {
    if cheats(world, args) {
        crate::portals::gun::enable_portals(world, [true, true]);
    }
}
