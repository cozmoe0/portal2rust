//! The system sets that order the engine's work. Each crate puts its systems
//! in these sets, so a frame keeps the original game's order without the
//! crates having to know each other.

use bevy::prelude::*;

/// Running the console's command buffer, before map loading in `PreUpdate`.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ConsoleSet;

/// Map loading; entity spawning runs after it.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct LoadMapSet;

/// Spawning of map entities; collision is rebuilt before it.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct EntitySpawnSet;

/// Placement of the player relative to the new map's entry landmark (level
/// load, before the first tick).
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct TransitionSet;

/// Creating the player or putting them at the start of a new map.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct PlayerSpawnSet;

/// Player movement, first in every tick (`FixedUpdate`).
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct PlayerMoveSet;

/// Order of work inside one tick, following the original server frame:
/// player movement, then entity thinks, then physics (avian runs in
/// `FixedPostUpdate`), then the event queue in `FixedLast`.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ThinkSet;

/// Servicing of the event queue, at the end of each tick (`FixedLast`):
/// outputs fired during a tick are delivered before the next one starts.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct IoSet;

/// Placement of the main camera at the player's eye.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct PlayerCameraSet;
