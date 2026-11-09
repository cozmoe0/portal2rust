//! portal2rust: a Portal 2 engine reimplementation on Bevy.
//!
//! Requires a local Portal 2 installation; all game content (maps, models,
//! textures, sounds, scripts) is read from it at runtime. This binary puts
//! the engine crates together into the game and adds the menus, the HUD and
//! the test harness.
//!
//! Built for the browser (`wasm32`), the game reads its files from the
//! machine serving the page instead (see `portal2_web` and
//! `portal2_core::web`): `main` waits for the page's loader, then starts
//! the same app on a canvas.

mod frontend;
mod harness;
#[cfg(target_arch = "wasm32")]
mod web;

use bevy::prelude::*;
#[cfg(not(target_arch = "wasm32"))]
use bevy::window::{PresentMode, WindowResolution};
use portal2_game::{commands, devices, effects, entities, paint, physics, player, portals, save, screen, script, sound, visuals};

use frontend::{gameui, hud};
use harness::{chain, probe, record, roundtrip, screenshot, shots, smoke};

fn main() {
    #[cfg(target_arch = "wasm32")]
    web::start(run);
    #[cfg(not(target_arch = "wasm32"))]
    run();
}

fn primary_window(args: &portal2_core::args::Args) -> Window {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = args;
        web::window()
    }
    #[cfg(not(target_arch = "wasm32"))]
    Window {
        title: "Portal 2 (Rust)".into(),
        // Physical pixels, so screenshots match the reference captures.
        resolution: WindowResolution::new(args.width, args.height).with_scale_factor_override(1.0),
        // Benchmarks measure frame times without waiting for the display.
        present_mode: if args.bench.is_some() || args.novsync { PresentMode::AutoNoVsync } else { PresentMode::AutoVsync },
        ..default()
    }
}

fn run() {
    let mut args = portal2_core::args::Args::from_env();
    shots::start_on_first_shot(&mut args);
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin { primary_window: Some(primary_window(&args)), ..default() }).set(
        bevy::log::LogPlugin {
            filter: std::env::var("PORTAL2_LOG").unwrap_or_else(|_| "wgpu=error,naga=warn,bevy_render=info,portal2=info".into()),
            ..default()
        },
    ))
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
    ));
    #[cfg(target_arch = "wasm32")]
    app.add_plugins(web::WebPlugin);
    app.run();
}
