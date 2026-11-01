//! Sounds the game itself starts: `ambient_generic` and soundscape
//! entities, the player's footsteps, physics props hitting things and the
//! move sounds of doors and platforms. They play through the sound engine
//! in `portal2_audio`.

pub mod ambient;
pub mod footsteps;
pub mod impacts;
pub mod motion;
pub mod soundscape;

use bevy::prelude::*;
use portal2_audio::AudioSet;

pub struct SoundEntitiesPlugin;

impl Plugin for SoundEntitiesPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostUpdate, ambient::start_pending.in_set(AudioSet::Start))
            .add_systems(Update, (soundscape::update_soundscapes, footsteps::player_footsteps, motion::motion_sounds))
            .add_systems(FixedUpdate, impacts::impact_sounds);
        ambient::register(app);
        soundscape::register(app);
    }
}
