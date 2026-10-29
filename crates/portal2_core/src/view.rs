//! The main view and who has the keyboard.

use bevy::prelude::*;

/// Source-space eye position and view angles (pitch, yaw, roll in degrees).
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct ViewAngles {
    pub origin: Vec3,
    pub angles: Vec3,
}

/// The camera that renders the player's view.
#[derive(Component)]
pub struct MainCamera;

/// Whether the menus or the console have the keyboard this frame (key
/// bindings and the player ignore it then).
#[derive(Resource, Default)]
pub struct UiFocus(pub bool);
