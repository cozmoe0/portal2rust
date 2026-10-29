//! Foundation of the portal2rust engine.
//!
//! Everything the other crates share lives here: the command line
//! ([`args`]), the server tick clock ([`clock`]), the developer console
//! ([`console`]), the mounted game filesystem ([`vfs`]), Source's coordinate
//! conventions ([`coords`]), the registry of what a saved game holds
//! ([`save`]) and the system sets that order every frame ([`sets`]), plus a
//! small shared vocabulary of entities ([`entity`]), views ([`view`]),
//! render layers ([`layers`]) and messages ([`events`]) that lets the engine
//! crates work together without depending on each other.

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
pub mod vfs;
pub mod view;

use bevy::prelude::*;

/// Counts map loads, so code waiting for a map can tell a new one from the
/// current one. `current` is 0 while no map is loaded.
#[derive(Resource, Default, Debug)]
pub struct MapGeneration {
    pub current: u64,
    last: u64,
}

impl MapGeneration {
    /// A map has been loaded.
    pub fn loaded(&mut self) {
        self.last += 1;
        self.current = self.last;
    }

    /// The map has been unloaded.
    pub fn unloaded(&mut self) {
        self.current = 0;
    }
}

/// The game clock, the game filesystem and the console, plus the shared
/// resources and messages. Added before every other engine plugin.
pub struct CorePlugin;

impl Plugin for CorePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MapGeneration>()
            .init_resource::<entity::EntityRegistry>()
            .init_resource::<view::UiFocus>()
            .add_message::<events::GameEvent>()
            .add_message::<events::CaptionRequest>()
            .add_plugins((clock::ClockPlugin, vfs::VfsPlugin, console::ConsolePlugin));
    }
}
