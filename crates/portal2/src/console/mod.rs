//! The developer console: console variables, commands, aliases, key binds
//! and the command buffer.
//!
//! Startup follows the original engine: the player's `cfg/config.cfg` (or
//! `config_default.cfg`) runs first, then `cfg/valve.rc`, whose `stuffcmds`
//! runs the `+commands` from the command line once the first map is in.
//! Commands run with full world access; `wait` pauses the buffer for frames.

pub mod binds;
pub mod commands;
pub mod cvars;
pub mod tokenize;

use std::collections::{HashMap, VecDeque};

use bevy::prelude::*;

use crate::vfs::GameFs;
use crate::world::CurrentMap;
use binds::Buttons;
pub use cvars::Cvar;
use cvars::flags;
pub use tokenize::CommandArgs;

/// Handler of a console command.
pub type CommandFn = fn(&mut World, &CommandArgs);

#[derive(Resource, Default)]
pub struct Console {
    cvars: HashMap<String, Cvar>,
    commands: HashMap<String, CommandFn>,
    aliases: HashMap<String, String>,
    /// Lowercase key name -> bound command text.
    pub binds: HashMap<String, String>,
    /// Pending commands; `true` marks text from the game itself (map
    /// entities, scripts), which may run cheat commands like the original
    /// server's own command buffer.
    buffer: VecDeque<(String, bool)>,
    /// Whether the command being executed came from the game itself.
    trusted: bool,
    /// Frames left before the buffer runs again (`wait`).
    wait: u32,
    /// The buffer holds until a requested map has loaded.
    waiting_for_map: bool,
    map_identity: usize,
    /// Command line `+commands`, run by `stuffcmds`.
    pub startup_commands: Vec<String>,
    /// Console output, newest last.
    pub log: Vec<String>,
}

impl Console {
    pub fn register_command(&mut self, name: &str, f: CommandFn) {
        self.commands.insert(name.to_ascii_lowercase(), f);
    }

    pub fn register_cvar(&mut self, name: &str, default: &str, flags: u32) {
        let name = name.to_ascii_lowercase();
        let value = self.cvars.get(&name).map(|c| c.value.clone()).unwrap_or_else(|| default.to_owned());
        self.cvars.insert(
            name.clone(),
            Cvar { name, value, default: default.to_owned(), flags, known: true },
        );
    }

    pub fn cvar(&self, name: &str) -> Option<&Cvar> {
        self.cvars.get(&name.to_ascii_lowercase())
    }

    pub fn f32(&self, name: &str) -> f32 {
        self.cvar(name).map(Cvar::f32).unwrap_or(0.0)
    }

    pub fn i32(&self, name: &str) -> i32 {
        self.cvar(name).map(Cvar::i32).unwrap_or(0)
    }

    pub fn bool(&self, name: &str) -> bool {
        self.cvar(name).is_some_and(Cvar::bool)
    }

    pub fn str(&self, name: &str) -> &str {
        self.cvar(name).map(|c| c.value.as_str()).unwrap_or("")
    }

    /// Cheat commands are allowed: sv_cheats, or text the game issued.
    pub fn cheats(&self) -> bool {
        self.trusted || self.bool("sv_cheats")
    }

    /// Sets a variable, creating unknown ones. Cheat variables need sv_cheats.
    pub fn set(&mut self, name: &str, value: &str) -> Result<(), String> {
        let key = name.to_ascii_lowercase();
        let cheats = self.cheats();
        match self.cvars.get_mut(&key) {
            Some(c) => {
                if c.flags & flags::CHEAT != 0 && !cheats && value != c.default {
                    return Err(format!("Can't change cheat cvar '{key}' unless sv_cheats is 1."));
                }
                c.value = value.to_owned();
            }
            None => {
                self.cvars.insert(
                    key.clone(),
                    Cvar { name: key.clone(), value: value.to_owned(), default: value.to_owned(), flags: 0, known: false },
                );
            }
        }
        if key == "sv_cheats" && value.trim() == "0" {
            self.reset_cheat_cvars();
        }
        Ok(())
    }

    fn reset_cheat_cvars(&mut self) {
        for c in self.cvars.values_mut() {
            if c.flags & flags::CHEAT != 0 {
                c.value = c.default.clone();
            }
        }
    }

    /// Appends text to the command buffer.
    pub fn add_text(&mut self, text: &str) {
        self.buffer.extend(tokenize::split_commands(text).into_iter().map(|c| (c, false)));
    }

    /// Appends text issued by the game itself (see `buffer`).
    pub fn add_trusted_text(&mut self, text: &str) {
        self.buffer.extend(tokenize::split_commands(text).into_iter().map(|c| (c, true)));
    }

    /// Inserts text at the front of the buffer (aliases, `exec`), with the
    /// trust of the command that inserted it.
    pub fn insert_text(&mut self, text: &str) {
        let trusted = self.trusted;
        for cmd in tokenize::split_commands(text).into_iter().rev() {
            self.buffer.push_front((cmd, trusted));
        }
    }

    /// Holds the buffer until the next map has finished loading, so that
    /// commands after `map` see the new level.
    pub fn wait_for_map(&mut self) {
        self.waiting_for_map = true;
    }

    pub fn print(&mut self, line: impl Into<String>) {
        let line = line.into();
        info!("console: {line}");
        self.log.push(line);
        if self.log.len() > 2000 {
            self.log.drain(..500);
        }
    }

    pub fn alias(&self, name: &str) -> Option<&str> {
        self.aliases.get(&name.to_ascii_lowercase()).map(String::as_str)
    }

    pub fn set_alias(&mut self, name: &str, text: &str) {
        self.aliases.insert(name.to_ascii_lowercase(), text.to_owned());
    }

    pub fn command_names(&self) -> impl Iterator<Item = &str> {
        self.commands.keys().map(String::as_str)
    }

    pub fn cvars(&self) -> impl Iterator<Item = &Cvar> {
        self.cvars.values()
    }
}

pub struct ConsolePlugin;

/// Running the command buffer, before map loading in `PreUpdate`.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ConsoleSet;

impl Plugin for ConsolePlugin {
    fn build(&self, app: &mut App) {
        let mut console = Console::default();
        for (name, default, f) in cvars::DEFAULTS {
            console.register_cvar(name, default, *f);
        }
        commands::register(&mut console);
        app.insert_resource(console)
            .init_resource::<Buttons>()
            .configure_sets(PreUpdate, ConsoleSet.before(crate::world::LoadMapSet))
            .add_systems(Startup, queue_startup)
            .add_systems(PreUpdate, (binds::process_binds, run_buffer).chain().in_set(ConsoleSet));
    }
}

/// Queues the configuration files, as the original engine does at startup.
fn queue_startup(mut console: ResMut<Console>, fs: Res<GameFs>, args: Res<crate::args::Args>) {
    console.startup_commands = args.commands.clone();
    let config = if fs.exists("cfg/config.cfg") { "config.cfg" } else { "config_default.cfg" };
    console.add_text(&format!("exec {config}\nexec valve.rc"));
    // The command line's own map loads on startup; everything after
    // `stuffcmds` waits for it.
    if !args.map.is_empty() {
        console.wait_for_map();
    }
}

fn map_identity(world: &World) -> usize {
    world.get_resource::<CurrentMap>().map(|m| std::sync::Arc::as_ptr(&m.bsp) as usize).unwrap_or(0)
}

/// Runs buffered commands until the buffer is empty, a `wait`, or a map load.
pub fn run_buffer(world: &mut World) {
    let identity = map_identity(world);
    {
        let mut console = world.resource_mut::<Console>();
        if console.waiting_for_map {
            if identity == 0 || identity == console.map_identity {
                return;
            }
            console.waiting_for_map = false;
        }
        console.map_identity = identity;
        if console.wait > 0 {
            console.wait -= 1;
            return;
        }
    }
    // Bounded so a runaway alias loop cannot hang the game.
    for _ in 0..4096 {
        let next = {
            let mut console = world.resource_mut::<Console>();
            if console.wait > 0 || console.waiting_for_map {
                break;
            }
            console.buffer.pop_front()
        };
        let Some((text, trusted)) = next else { break };
        world.resource_mut::<Console>().trusted = trusted;
        execute_command(world, &text);
        world.resource_mut::<Console>().trusted = false;
    }
}

/// Runs console text immediately, bypassing the buffer (key binds, scripts).
pub fn execute_now(world: &mut World, text: &str) {
    for cmd in tokenize::split_commands(text) {
        execute_command(world, &cmd);
    }
}

/// Queues console text to run at the next buffer pass.
pub fn queue(world: &mut World, text: &str) {
    world.resource_mut::<Console>().add_text(text);
}

/// Queues console text issued by the game itself (map entities, scripts).
pub fn queue_trusted(world: &mut World, text: &str) {
    world.resource_mut::<Console>().add_trusted_text(text);
}

fn execute_command(world: &mut World, text: &str) {
    let args = CommandArgs::parse(text);
    if args.is_empty() {
        return;
    }
    let name = args.name().to_ascii_lowercase();
    if name == "wait" {
        let frames = args.arg(1).parse::<u32>().unwrap_or(1).max(1);
        world.resource_mut::<Console>().wait = frames;
        return;
    }
    let (alias, command) = {
        let console = world.resource::<Console>();
        (console.alias(&name).map(str::to_owned), console.commands.get(&name).copied())
    };
    if let Some(alias) = alias {
        world.resource_mut::<Console>().insert_text(&alias);
        return;
    }
    if let Some(f) = command {
        f(world, &args);
        return;
    }
    // Buttons without an alias of the same name.
    if let Some(button) = name.strip_prefix('+') {
        world.resource_mut::<Buttons>().press(button, args.arg(1));
        return;
    }
    if let Some(button) = name.strip_prefix('-') {
        world.resource_mut::<Buttons>().release(button, args.arg(1));
        return;
    }
    let mut console = world.resource_mut::<Console>();
    if args.len() == 1 {
        match console.cvar(&name).cloned() {
            Some(c) => console.print(format!("\"{}\" = \"{}\" ( def. \"{}\" )", c.name, c.value, c.default)),
            None => console.print(format!("Unknown command \"{name}\"")),
        }
        return;
    }
    if let Err(e) = console.set(&name, args.arg(1)) {
        console.print(e);
    }
}
