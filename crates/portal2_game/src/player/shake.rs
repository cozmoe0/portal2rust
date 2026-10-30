//! Screen shake from `env_shake`: the server works out how hard each player
//! shakes, the view then jitters with a quickening, fading wobble
//! (`docs/re/gameplay.md`, screen shake).

use bevy::prelude::*;

use super::Player;
use crate::entities::io::{InputCall, InputHandlers};
use portal2_core::entity::MapEntity;
use portal2_core::sets::PlayerCameraSet;

pub struct ShakePlugin;

impl Plugin for ShakePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Shakes>().add_systems(Startup, register_inputs).add_systems(PostUpdate, calc_shake.before(PlayerCameraSet));
    }
}

/// Amplitudes above this are clamped.
const MAX_AMPLITUDE: f32 = 16.0;
/// Shakes running at once on the client.
const MAX_SHAKES: usize = 32;

/// `env_shake` spawn flags.
mod flags {
    pub const GLOBAL: u32 = 1;
    pub const IN_AIR: u32 = 4;
    pub const NO_VIEW: u32 = 32;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShakeCommand {
    Start,
    Stop,
    Amplitude,
    Frequency,
}

#[derive(Clone, Copy, Debug)]
struct Shake {
    end: f32,
    duration: f32,
    amplitude: f32,
    frequency: f32,
    next: f32,
    offset: Vec3,
    angle: f32,
}

/// Running shakes and the view offset they add this frame.
#[derive(Resource)]
pub struct Shakes {
    list: Vec<Shake>,
    pub offset: Vec3,
    pub roll: f32,
    rng: u32,
}

impl Default for Shakes {
    fn default() -> Self {
        Self { list: Vec::new(), offset: Vec3::ZERO, roll: 0.0, rng: 0x9e37_79b9 }
    }
}

impl Shakes {
    fn random(&mut self, lo: f32, hi: f32) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        lo + (hi - lo) * ((self.rng >> 8) as f32 / (1u32 << 24) as f32)
    }

    /// The client side of a shake message.
    fn receive(&mut self, now: f32, command: ShakeCommand, amplitude: f32, frequency: f32, duration: f32) {
        match command {
            ShakeCommand::Start if self.list.len() < MAX_SHAKES => {
                self.list.push(Shake { end: now + duration, duration, amplitude, frequency, next: 0.0, offset: Vec3::ZERO, angle: 0.0 })
            }
            ShakeCommand::Start => {}
            ShakeCommand::Stop => self.list.clear(),
            ShakeCommand::Amplitude => {
                if let Some(s) = self.list.last_mut() {
                    s.amplitude = amplitude;
                }
            }
            ShakeCommand::Frequency => {
                if let Some(s) = self.list.last_mut() {
                    s.frequency = frequency;
                }
            }
        }
    }

    /// Advances every shake by a frame and sums their view offsets.
    fn update(&mut self, now: f32, frame: f32) {
        self.offset = Vec3::ZERO;
        self.roll = 0.0;
        let mut i = self.list.len();
        while i > 0 {
            i -= 1;
            let s = self.list[i];
            if s.end == 0.0 {
                continue;
            }
            if now > s.end || s.duration <= 0.0 || s.amplitude <= 0.0 || s.frequency <= 0.0 {
                self.list.swap_remove(i);
                continue;
            }
            let mut s = s;
            if now > s.next {
                // A new random target every 1 / frequency seconds.
                s.next = now + 1.0 / s.frequency;
                s.offset = Vec3::new(
                    self.random(-s.amplitude, s.amplitude),
                    self.random(-s.amplitude, s.amplitude),
                    self.random(-s.amplitude, s.amplitude),
                );
                s.angle = self.random(-s.amplitude * 0.25, s.amplitude * 0.25);
            }
            // The wobble speeds up as the shake fades out quadratically.
            let fraction = (s.end - now) / s.duration;
            let freq = if fraction != 0.0 { s.frequency / fraction } else { 0.0 };
            let wave = (now * freq).min(1.0e8).sin() * fraction * fraction;
            self.offset += s.offset * wave;
            self.roll += s.angle * wave;
            s.amplitude -= s.amplitude * (frame / (s.duration * s.frequency));
            self.list[i] = s;
        }
    }
}

/// The server side: how strongly the player shakes for a shake at
/// `center` (none when outside `radius`, or airborne unless `air`).
pub fn screen_shake(
    world: &mut World,
    center: Vec3,
    amplitude: f32,
    frequency: f32,
    duration: f32,
    radius: f32,
    command: ShakeCommand,
    air: bool,
) {
    let amplitude = amplitude.min(MAX_AMPLITUDE);
    let Some(state) = world.query::<&Player>().iter(world).next().map(|p| p.state) else { return };
    if command == ShakeCommand::Start && !air && !state.on_ground {
        return;
    }
    let local = if radius > 0.0 {
        let (mins, maxs) = state.hull();
        let distance = center.distance(state.origin + (mins + maxs) * 0.5);
        if radius >= distance { (1.0 - distance / radius) * amplitude } else { -1.0 }
    } else {
        amplitude
    };
    if local < 0.0 {
        return;
    }
    let now = world.resource::<Time<Virtual>>().elapsed_secs();
    world.resource_mut::<Shakes>().receive(now, command, local, frequency, duration);
}

fn register_inputs(world: &mut World) {
    world.resource_mut::<InputHandlers>().register("env_shake", shake_input);
}

fn shake_input(world: &mut World, call: &InputCall) -> bool {
    let Some(me) = world.get::<MapEntity>(call.target).cloned() else { return false };
    let origin = world.get::<GlobalTransform>(call.target).map(|t| t.translation()).unwrap_or_else(|| me.origin());
    let spawnflags = me.spawnflags();
    let radius = if spawnflags & flags::GLOBAL != 0 { 0.0 } else { me.get_f32("radius").unwrap_or(0.0) };
    let air = spawnflags & flags::IN_AIR != 0;
    let amplitude = me.get_f32("amplitude").unwrap_or(0.0);
    let frequency = me.get_f32("frequency").unwrap_or(0.0);
    let duration = me.get_f32("duration").unwrap_or(0.0);
    let param = source_formats::kv::parse_f32_lenient(&call.param);
    let (command, amplitude, frequency) = match call.input.as_str() {
        "startshake" => (ShakeCommand::Start, amplitude, frequency),
        "stopshake" => (ShakeCommand::Stop, amplitude, frequency),
        "amplitude" => (ShakeCommand::Amplitude, param, frequency),
        "frequency" => (ShakeCommand::Frequency, amplitude, param),
        _ => return false,
    };
    // The new values also apply to later shakes from this entity.
    if let Some(mut me) = world.get_mut::<MapEntity>(call.target) {
        match command {
            ShakeCommand::Amplitude => me.set("amplitude", &call.param),
            ShakeCommand::Frequency => me.set("frequency", &call.param),
            _ => {}
        }
    }
    // Shakes that only move physics or ropes leave the view alone.
    if spawnflags & flags::NO_VIEW == 0 {
        screen_shake(world, origin, amplitude, frequency, duration, radius, command, air);
    }
    true
}

fn calc_shake(time: Res<Time<Virtual>>, mut shakes: ResMut<Shakes>) {
    let now = time.elapsed_secs();
    shakes.update(now, time.delta_secs());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shake_fades_out_and_ends() {
        let mut s = Shakes::default();
        s.receive(0.0, ShakeCommand::Start, 8.0, 20.0, 1.0);
        let mut peak: f32 = 0.0;
        let mut t = 0.0;
        while t < 0.5 {
            s.update(t, 1.0 / 60.0);
            peak = peak.max(s.offset.length());
            t += 1.0 / 60.0;
        }
        assert!(peak > 0.1 && peak < 8.0 * 1.8, "{peak}");
        s.update(1.01, 1.0 / 60.0);
        assert!(s.list.is_empty() && s.offset == Vec3::ZERO);
    }
}
