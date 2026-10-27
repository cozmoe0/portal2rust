//! Portal 2 engine reimplementation on Bevy.
//!
//! Requires a local Portal 2 installation; all game content (maps, models,
//! textures, sounds, scripts) is read from it at runtime.

mod args;
mod camera;
mod collision;
mod coords;
mod devices;
mod entities;
mod models;
mod physics;
mod player;
mod portals;
mod probe;
mod render;
mod screenshot;
mod vfs;
mod world;

use bevy::prelude::*;
use bevy::window::{PresentMode, WindowResolution};

fn main() {
    let args = args::Args::from_env();
    App::new()
        .add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "Portal 2 (Rust)".into(),
                        resolution: WindowResolution::new(args.width, args.height),
                        present_mode: PresentMode::AutoVsync,
                        ..default()
                    }),
                    ..default()
                })
                .set(bevy::log::LogPlugin {
                    filter: std::env::var("PORTAL2_LOG").unwrap_or_else(|_| "wgpu=error,naga=warn,bevy_render=info,portal2=info".into()),
                    ..default()
                }),
        )
        .insert_resource(entities::io::IoDebug(args.io_debug))
        .insert_resource(args)
        .add_plugins((
            vfs::VfsPlugin,
            render::RenderSetupPlugin,
            world::WorldPlugin,
            models::ModelsPlugin,
            collision::CollisionPlugin,
            entities::EntitiesPlugin,
            player::PlayerPlugin,
            physics::GamePhysicsPlugin,
            portals::PortalsPlugin,
            devices::DevicesPlugin,
            camera::CameraPlugin,
            screenshot::ScreenshotPlugin,
            probe::ProbePlugin,
        ))
        .run();
}
