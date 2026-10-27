//! Simulation time.
//!
//! Gameplay runs on a fixed tick like the original server: movement, entity
//! thinks, physics and the I/O event queue all see the same `curtime`, which
//! advances by exactly one tick interval per step and restarts with every map.
//! Pausing the game pauses `Time<Virtual>`, which stops the fixed schedules.

use bevy::prelude::*;

/// Ticks per second of the single-player server.
pub const TICK_RATE: f64 = 60.0;
pub const TICK_INTERVAL: f64 = 1.0 / TICK_RATE;

/// Server time of the current map.
#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct GameClock {
    /// Ticks simulated since the map started.
    pub tick: u64,
    /// `tick * TICK_INTERVAL`, in seconds.
    pub curtime: f64,
}

impl GameClock {
    /// Length of one tick in seconds (the scripts' `FrameTime()`).
    pub fn frametime(&self) -> f32 {
        TICK_INTERVAL as f32
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Whether something scheduled for `time` runs on the current tick.
    /// Think times round to the nearest tick, as in the original engine.
    pub fn due(&self, time: f64) -> bool {
        time_to_ticks(time) <= self.tick as i64
    }
}

pub fn time_to_ticks(time: f64) -> i64 {
    (0.5 + time / TICK_INTERVAL).floor() as i64
}

/// Order of work inside one tick, following the original server frame:
/// player movement, then entity thinks, then physics (avian runs in
/// `FixedPostUpdate`), then the event queue in `FixedLast`.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ThinkSet;

pub struct ClockPlugin;

impl Plugin for ClockPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Time::<Fixed>::from_hz(TICK_RATE))
            .init_resource::<GameClock>()
            .configure_sets(FixedUpdate, ThinkSet.after(crate::player::PlayerMoveSet))
            .add_systems(FixedFirst, advance)
            // Children of moving entities (elevator floors, attached
            // triggers) follow their parents every tick, as in the original,
            // not only once per rendered frame.
            .add_systems(
                FixedPreUpdate,
                (
                    bevy::transform::systems::mark_dirty_trees,
                    bevy::transform::systems::propagate_parent_transforms,
                    bevy::transform::systems::sync_simple_transforms,
                )
                    .chain(),
            );
    }
}

fn advance(mut clock: ResMut<GameClock>, queue: Option<ResMut<crate::entities::io::EventQueue>>) {
    clock.tick += 1;
    clock.curtime = clock.tick as f64 * TICK_INTERVAL;
    // Events queued during the tick are timed from its start.
    if let Some(mut q) = queue {
        q.now = clock.curtime;
    }
}
