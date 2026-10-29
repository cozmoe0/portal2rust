//! `--record <dir>`: save every rendered frame of a stretch of play as a
//! numbered PNG, then exit. Used to make the clips in the README
//! (`docs/assets/make_clip.py` turns the frames into an animated WebP).
//!
//! Time advances by exactly one frame interval (`--record-fps`, default 30)
//! per rendered frame however long a capture takes, so the frames play back
//! smoothly. Capture starts once the map clock reads `--record-start`
//! seconds and lasts `--record-seconds` of game time; drive the player with
//! `--autopilot` meanwhile.

use std::path::PathBuf;
use std::time::Duration;

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use bevy::time::TimeUpdateStrategy;

use crate::args::Args;
use crate::clock::GameClock;
use crate::world::CurrentMap;

pub struct RecordPlugin;

impl Plugin for RecordPlugin {
    fn build(&self, app: &mut App) {
        let Some(args) = app.world().get_resource::<Args>() else { return };
        if args.record.is_none() {
            return;
        }
        let step = Duration::from_secs_f64(1.0 / f64::from(args.record_fps.max(1.0)));
        app.insert_resource(TimeUpdateStrategy::ManualDuration(step)).add_systems(Update, record);
    }
}

#[derive(Default)]
struct Recorder {
    /// Frames requested so far.
    frame: u32,
    /// Frames to wait after the last request before exiting, so pending
    /// captures reach the disk.
    drain: Option<u32>,
}

fn record(
    mut commands: Commands,
    args: Res<Args>,
    clock: Res<GameClock>,
    map: Option<Res<CurrentMap>>,
    mut state: Local<Recorder>,
    mut exit: MessageWriter<AppExit>,
) {
    let Some(dir) = &args.record else { return };
    if let Some(left) = &mut state.drain {
        if *left == 0 {
            exit.write(AppExit::Success);
        }
        *left = left.saturating_sub(1);
        return;
    }
    if map.is_none() || clock.curtime < f64::from(args.record_start) {
        return;
    }
    let total = (args.record_seconds * args.record_fps).round() as u32;
    if state.frame == 0 {
        let _ = std::fs::create_dir_all(dir);
        info!("recording {total} frames to {dir}");
    }
    let path = PathBuf::from(dir).join(format!("{:05}.png", state.frame));
    commands.spawn(Screenshot::primary_window()).observe(save_to_disk(path));
    state.frame += 1;
    if state.frame >= total {
        state.drain = Some(30);
    }
}
