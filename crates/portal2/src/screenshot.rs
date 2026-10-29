//! `--screenshot <path>`: capture the window after a number of frames and exit.
//! Used for automated visual checks.

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};

use portal2_core::args::Args;

pub struct ScreenshotPlugin;

impl Plugin for ScreenshotPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, auto_screenshot);
    }
}

fn auto_screenshot(mut commands: Commands, args: Res<Args>, mut frame: Local<u32>, mut exit: MessageWriter<AppExit>) {
    let Some(path) = &args.screenshot else { return };
    *frame += 1;
    if *frame == args.frames {
        info!("capturing screenshot to {path}");
        commands.spawn(Screenshot::primary_window()).observe(save_to_disk(path.clone()));
    }
    if *frame == args.frames + 20 {
        exit.write(AppExit::Success);
    }
}
