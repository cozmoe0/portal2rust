//! Portal 2 engine reimplementation on Bevy.
//!
//! Requires a local Portal 2 installation; all game content (maps, models,
//! textures, sounds, scripts) is read from it at runtime.

mod audio;
mod camera;
mod captions;
mod chain;
mod collision;
mod commands;
mod devices;
mod entities;
mod gameui;
mod hud;
mod models;
mod paint;
mod particles;
mod physics;
mod player;
mod portals;
mod probe;
mod record;
mod render;
mod save;
mod screen;
mod screenshot;
mod script;
mod shots;
mod smoke;
mod vgui;
mod video;
mod world;

use bevy::prelude::*;
use bevy::window::{PresentMode, WindowResolution};

fn main() {
    let mut args = portal2_core::args::Args::from_env();
    shots::start_on_first_shot(&mut args);
    App::new()
        .add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "Portal 2 (Rust)".into(),
                        // Physical pixels, so screenshots match the reference captures.
                        resolution: WindowResolution::new(args.width, args.height).with_scale_factor_override(1.0),
                        // Benchmarks measure frame times without waiting for the display.
                        present_mode: if args.bench.is_some() { PresentMode::AutoNoVsync } else { PresentMode::AutoVsync },
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
            (portal2_core::CorePlugin, commands::GameCommandsPlugin),
            (render::RenderSetupPlugin, world::WorldPlugin, models::ModelsPlugin, collision::CollisionPlugin),
            (entities::EntitiesPlugin, script::ScriptPlugin, player::PlayerPlugin, physics::GamePhysicsPlugin),
            (
                portals::PortalsPlugin,
                devices::DevicesPlugin,
                screen::ScreenPlugin,
                render::fog::FogPlugin,
                render::projected::ProjectedPlugin,
                render::sprites::SpritePlugin,
                render::spotlights::SpotlightPlugin,
                render::movies::MoviePlugin,
                render::tonemap::TonemapPlugin,
                audio::AudioPlugin,
                captions::CaptionsPlugin,
            ),
            (
                camera::CameraPlugin,
                screenshot::ScreenshotPlugin,
                shots::ShotsPlugin,
                smoke::SmokePlugin,
                chain::ChainPlugin,
                probe::ProbePlugin,
                record::RecordPlugin,
            ),
            (
                save::SavePlugin,
                vgui::VguiPlugin,
                hud::HudPlugin,
                video::VideoPlugin,
                gameui::GameUiPlugin,
                particles::ParticlePlugin,
                paint::PaintPlugin,
            ),
        ))
        .run();
}
