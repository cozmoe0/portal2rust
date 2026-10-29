//! The console's own commands: `exec`, `alias`, `bind`, `find`, `quit`...
//! Commands that act on the game are registered by the plugins that own
//! what they change.

use bevy::prelude::*;

use super::{CommandArgs, Console, binds};

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

/// Prints a line to the console.
pub fn print(world: &mut World, line: impl Into<String>) {
    world.resource_mut::<Console>().print(line);
}

/// Whether a cheat command may run: refuses (with the game's message)
/// unless sv_cheats is on.
pub fn require_cheats(world: &mut World, args: &CommandArgs) -> bool {
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
