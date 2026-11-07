//! Foundation of the portal2rust engine.
//!
//! Everything the other crates share lives here: the command line
//! ([`args`]), the server tick clock ([`clock`]), the developer console
//! ([`console`]), the mounted game filesystem ([`vfs`]), Source's coordinate
//! conventions ([`coords`]), the registry of what a saved game holds
//! ([`save`]) and the system sets that order every frame ([`sets`]), plus a
//! small shared vocabulary of entities ([`entity`]), views ([`view`]),
//! render layers ([`layers`]) and messages ([`events`]) that lets the engine
//! crates work together without depending on each other. The player's own
//! files go through [`userfs`], and the browser build's link to its page is
//! [`web`].

pub mod args;
pub mod clock;
pub mod console;
pub mod coords;
pub mod entity;
pub mod events;
pub mod layers;
pub mod rng;
pub mod save;
pub mod sets;
pub mod userfs;
pub mod vfs;
pub mod view;
#[cfg(target_arch = "wasm32")]
pub mod web;

use bevy::prelude::*;

/// Which map is loaded: its name, and a generation that changes with every
/// load so code waiting for a map can tell a new one from the current one
/// (0 and an empty name while no map is loaded).
#[derive(Resource, Default, Debug)]
pub struct LoadedMap {
    pub generation: u64,
    pub name: String,
    last: u64,
}

impl LoadedMap {
    /// A map has been loaded.
    pub fn loaded(&mut self, name: &str) {
        self.last += 1;
        self.generation = self.last;
        self.name = name.to_owned();
    }

    /// The map has been unloaded.
    pub fn unloaded(&mut self) {
        self.generation = 0;
        self.name.clear();
    }
}

/// The game clock, the game filesystem and the console, plus the shared
/// resources and messages. Added before every other engine plugin.
pub struct CorePlugin;

impl Plugin for CorePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LoadedMap>()
            .init_resource::<entity::EntityRegistry>()
            .init_resource::<view::UiFocus>()
            .add_message::<events::GameEvent>()
            .add_message::<events::CaptionRequest>()
            .add_plugins((clock::ClockPlugin, vfs::VfsPlugin, console::ConsolePlugin));
    }
}
