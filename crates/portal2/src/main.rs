//! portal2rust: a Portal 2 engine reimplementation on Bevy.
//!
//! Requires a local Portal 2 installation; all game content (maps, models,
//! textures, sounds, scripts) is read from it at runtime. This binary puts
//! the engine crates together into the game and adds the menus, the HUD and
//! the test harness.

mod frontend;
mod harness;

use bevy::prelude::*;
use bevy::window::{PresentMode, WindowResolution};
use portal2_game::{commands, devices, effects, entities, paint, physics, player, portals, save, screen, script, sound, visuals};

use frontend::{gameui, hud};
use harness::{chain, probe, record, roundtrip, screenshot, shots, smoke};

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
                }),
        )
        .insert_resource(entities::io::IoDebug(args.io_debug))
        .insert_resource(args)
        // The order matters: the console comes first, and the entity I/O
        // tables exist before the plugins that register classes into them.
        .add_plugins((
            (portal2_core::CorePlugin, commands::GameCommandsPlugin),
            (
                portal2_render::RenderSetupPlugin,
                portal2_world::WorldPlugin,
                portal2_render::models::ModelsPlugin,
                portal2_world::collision::CollisionPlugin,
            ),
            (entities::EntitiesPlugin, script::ScriptPlugin, player::PlayerPlugin, physics::GamePhysicsPlugin),
            (
                portals::PortalsPlugin,
                devices::DevicesPlugin,
                screen::ScreenPlugin,
                visuals::fog::FogPlugin,
                visuals::projected::ProjectedPlugin,
                visuals::sprites::SpritePlugin,
                visuals::spotlights::SpotlightPlugin,
                visuals::movies::MoviePlugin,
                visuals::progress_sign::ProgressSignPlugin,
                visuals::tonemap::TonemapPlugin,
                visuals::fx::VisualFxPlugin,
                portal2_audio::AudioPlugin,
                sound::SoundEntitiesPlugin,
                portal2_ui::captions::CaptionsPlugin,
            ),
            (
                portal2_world::camera::CameraPlugin,
                screenshot::ScreenshotPlugin,
                shots::ShotsPlugin,
                smoke::SmokePlugin,
                chain::ChainPlugin,
                probe::ProbePlugin,
                record::RecordPlugin,
            ),
            (
                save::SavePlugin,
                roundtrip::RoundTripPlugin,
                portal2_ui::vgui::VguiPlugin,
                portal2_ui::hud::HudLayoutPlugin,
                hud::HudPlugin,
                portal2_ui::video::VideoPlugin,
                gameui::GameUiPlugin,
                portal2_particles::ParticlePlugin,
                effects::EffectsPlugin,
                paint::PaintPlugin,
            ),
        ))
        .run();
}
