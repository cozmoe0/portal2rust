//! Sequence playback for studio models: advances the cycle, blends the
//! sequence, scene gesture layers and the model's autoplay sequences (by pose
//! parameter) into joint entities, and reports sequence completion and
//! events.

use std::sync::Arc;

use bevy::prelude::*;
use source_formats::mdl::seq_flags;

use super::LoadedModel;

pub struct AnimationPlugin;

impl Plugin for AnimationPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<AnimationFinished>()
            .add_message::<AnimationEvent>()
            .add_systems(PostUpdate, animate_models.before(TransformSystems::Propagate));
    }
}

/// Sent when a non-looping sequence reaches its end.
#[derive(Message, Clone, Debug)]
pub struct AnimationFinished {
    pub entity: Entity,
    pub sequence: usize,
}

/// Animation events (sounds, effects) crossed during playback.
#[derive(Message, Clone, Debug)]
pub struct AnimationEvent {
    pub entity: Entity,
    pub event: i32,
    pub name: String,
    pub options: String,
}

/// A sequence blended over the base one (scene gestures), accumulated like
/// any other sequence with `weight`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layer {
    pub sequence: usize,
    pub cycle: f32,
    pub weight: f32,
}

#[derive(Component)]
pub struct StudioAnimator {
    pub model: Arc<LoadedModel>,
    /// Joint entities in bone order.
    pub joints: Vec<Entity>,
    pub sequence: usize,
    /// Normalised playback position, 0..=1.
    pub cycle: f32,
    pub rate: f32,
    pub looping: bool,
    pub finished: bool,
    /// Sequence to return to after a non-looping one finishes.
    pub default_sequence: Option<usize>,
    /// Layered sequences, applied in order after the base sequence.
    layers: Vec<Layer>,
    /// Pose parameter settings (0..=1 across each parameter's range), in
    /// the order of the model's pose parameters.
    pose: Vec<f32>,
    /// Pose needs re-sampling even if the cycle did not move.
    dirty: bool,
}

impl StudioAnimator {
    pub fn new(model: Arc<LoadedModel>, joints: Vec<Entity>) -> Self {
        let pose = model.studio.default_pose_settings();
        let mut a = Self {
            model,
            joints,
            sequence: 0,
            cycle: 0.0,
            rate: 1.0,
            looping: false,
            finished: false,
            default_sequence: None,
            layers: Vec::new(),
            pose,
            dirty: true,
        };
        a.set_sequence_index(0);
        a
    }

    fn sequence_flags(&self, index: usize) -> i32 {
        self.model.studio.sequence(index).map(|(m, l)| m.mdl.sequences[l].flags).unwrap_or(0)
    }

    pub fn set_sequence_index(&mut self, index: usize) {
        if index >= self.model.studio.sequence_count() {
            return;
        }
        self.sequence = index;
        self.cycle = 0.0;
        self.finished = false;
        self.looping = self.sequence_flags(index) & seq_flags::LOOPING != 0;
        self.dirty = true;
    }

    /// Starts a sequence by name; returns false when the model lacks it.
    pub fn set_sequence(&mut self, name: &str) -> bool {
        match self.model.studio.find_sequence(name) {
            Some(i) => {
                self.set_sequence_index(i);
                true
            }
            None => false,
        }
    }

    pub fn set_default_sequence(&mut self, name: &str) -> bool {
        match self.model.studio.find_sequence(name) {
            Some(i) => {
                self.default_sequence = Some(i);
                true
            }
            None => false,
        }
    }

    pub fn layers(&self) -> &[Layer] {
        &self.layers
    }

    pub fn set_layers(&mut self, layers: Vec<Layer>) {
        if self.layers != layers {
            self.layers = layers;
            self.dirty = true;
        }
    }

    /// Sets pose parameter `name` to `value` (in the parameter's own units);
    /// false when the model has no such parameter.
    pub fn set_pose_parameter(&mut self, name: &str, value: f32) -> bool {
        let Some(i) = self.model.studio.find_pose_parameter(name) else { return false };
        let setting = self.model.studio.pose_setting(i, value);
        if self.pose.get(i) != Some(&setting) {
            self.pose[i] = setting;
            self.dirty = true;
        }
        true
    }

    /// Current value of pose parameter `name`, in its own units.
    pub fn pose_parameter(&self, name: &str) -> Option<f32> {
        let i = self.model.studio.find_pose_parameter(name)?;
        Some(self.model.studio.pose_value(i, *self.pose.get(i)?))
    }

    /// Fade-in and fade-out times of a sequence (seconds).
    pub fn sequence_fades(&self, index: usize) -> (f32, f32) {
        self.model.studio.sequence(index).map(|(m, l)| (m.mdl.sequences[l].fade_in, m.mdl.sequences[l].fade_out)).unwrap_or((0.2, 0.2))
    }

    pub fn sequence_name(&self) -> String {
        self.model.studio.sequence(self.sequence).map(|(m, l)| m.mdl.sequences[l].label.clone()).unwrap_or_default()
    }

    /// Duration of the current sequence in seconds at rate 1.
    pub fn duration(&self) -> f32 {
        let (fps, frames) = self.model.studio.sequence_timing(self.sequence);
        ((frames - 1).max(1) as f32) / fps
    }
}

fn animate_models(
    time: Res<Time>,
    mut animators: Query<(Entity, &mut StudioAnimator)>,
    mut transforms: Query<&mut Transform>,
    mut finished: MessageWriter<AnimationFinished>,
    mut events: MessageWriter<AnimationEvent>,
) {
    let dt = time.delta_secs();
    let now = time.elapsed_secs();
    for (entity, mut anim) in &mut animators {
        let duration = anim.duration();
        let previous = anim.cycle;
        if !anim.finished && anim.rate != 0.0 && duration > 0.0 {
            let mut cycle = anim.cycle + dt * anim.rate / duration;
            if anim.looping {
                cycle = cycle.rem_euclid(1.0);
            } else if cycle >= 1.0 {
                cycle = 1.0;
                anim.finished = true;
                finished.write(AnimationFinished { entity, sequence: anim.sequence });
            }
            anim.cycle = cycle;
            anim.dirty = true;
            // Report events whose cycle was crossed this frame.
            if let Some((owner, local)) = anim.model.studio.sequence(anim.sequence) {
                for e in &owner.mdl.sequences[local].events {
                    let crossed = if cycle >= previous {
                        e.cycle > previous && e.cycle <= cycle
                    } else {
                        e.cycle > previous || e.cycle <= cycle
                    };
                    if crossed {
                        events.write(AnimationEvent {
                            entity,
                            event: e.event,
                            name: e.name.clone(),
                            options: e.options.clone(),
                        });
                    }
                }
            }
            if anim.finished
                && let Some(default) = anim.default_sequence
                && default != anim.sequence
            {
                anim.set_sequence_index(default);
            }
        }
        // Autoplay sequences run on the clock, so they keep the pose moving.
        let studio = &anim.model.studio;
        if studio.autoplay.iter().any(|&(_, cps)| cps > 0.0) {
            anim.dirty = true;
        }
        if !anim.dirty || anim.joints.is_empty() {
            continue;
        }
        anim.dirty = false;
        let studio = &anim.model.studio;
        let mut pose = studio.bind_pose();
        studio.accumulate_sequence(&mut pose, anim.sequence, anim.cycle, 1.0, &anim.pose);
        for layer in &anim.layers {
            studio.accumulate_sequence(&mut pose, layer.sequence, layer.cycle, layer.weight, &anim.pose);
        }
        for &(seq, cps) in &studio.autoplay {
            studio.accumulate_sequence(&mut pose, seq, (now * cps).fract(), 1.0, &anim.pose);
        }
        for (i, &joint) in anim.joints.iter().enumerate() {
            if let (Some(&(p, q)), Ok(mut t)) = (pose.get(i), transforms.get_mut(joint)) {
                t.translation = p;
                t.rotation = q;
            }
        }
    }
}
