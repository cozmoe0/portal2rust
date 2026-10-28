//! Portal 2 engine reimplementation on Bevy.
//!
//! Requires a local Portal 2 installation; all game content (maps, models,
//! textures, sounds, scripts) is read from it at runtime.

mod args;
mod audio;
mod camera;
mod captions;
mod chain;
mod clock;
mod collision;
mod console;
mod coords;
mod devices;
mod entities;
mod gameui;
mod hud;
mod models;
mod physics;
mod player;
mod portals;
mod probe;
mod render;
mod save;
mod script;
mod screen;
mod screenshot;
mod shots;
mod smoke;
mod vfs;
mod vgui;
mod video;
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
                        // Physical pixels, so screenshots match the reference captures.
                        resolution: WindowResolution::new(args.width, args.height).with_scale_factor_override(1.0),
                        present_mode: PresentMode::AutoVsync,
                        ..default()
                    }),
                    ..default()
                })
                .set(bevy::log::LogPlugin {
                    filter: std::env::var("PORTAL2_LOG").unwrap_or_else(|_| "wgpu=error,naga=warn,bevy_render=info,portal2=info".into()),
                    ..default()
                })
                // Sound has its own mixer (crate::audio).
                .disable::<bevy::audio::AudioPlugin>(),
        )
        .insert_resource(entities::io::IoDebug(args.io_debug))
        .insert_resource(args)
        .add_plugins((
            (clock::ClockPlugin, vfs::VfsPlugin, console::ConsolePlugin),
            (render::RenderSetupPlugin, world::WorldPlugin, models::ModelsPlugin, collision::CollisionPlugin),
            (entities::EntitiesPlugin, script::ScriptPlugin, player::PlayerPlugin, physics::GamePhysicsPlugin),
            (portals::PortalsPlugin, devices::DevicesPlugin, screen::ScreenPlugin, render::fog::FogPlugin, audio::AudioPlugin, captions::CaptionsPlugin),
            (camera::CameraPlugin, screenshot::ScreenshotPlugin, shots::ShotsPlugin, smoke::SmokePlugin, chain::ChainPlugin, probe::ProbePlugin),
            (save::SavePlugin, vgui::VguiPlugin, hud::HudPlugin, video::VideoPlugin, gameui::GameUiPlugin),
        ))
        .run();
}
