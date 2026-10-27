//! Player health: damage from hazards, regeneration, and death.

use bevy::prelude::*;

use super::{Player, PlayerMoveSet};
use crate::entities::triggers::PlayerDied;
use crate::world::PlayerStart;

pub const MAX_HEALTH: f32 = 100.0;
/// Seconds without damage before health starts coming back.
const REGEN_DELAY: f64 = 1.0;
const REGEN_RATE: f32 = 25.0;

pub struct HealthPlugin;

impl Plugin for HealthPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<DamagePlayer>()
            .add_systems(Update, reset_health)
            .add_systems(FixedUpdate, apply_damage.after(PlayerMoveSet));
    }
}

#[derive(Component, Debug)]
pub struct Health {
    pub current: f32,
    pub last_damage: f64,
    pub dead: bool,
}

impl Default for Health {
    fn default() -> Self {
        Self { current: MAX_HEALTH, last_damage: f64::MIN, dead: false }
    }
}

/// Damage to the player, with an optional shove.
#[derive(Message, Clone, Copy, Debug)]
pub struct DamagePlayer {
    pub amount: f32,
    pub push: Vec3,
}

fn reset_health(
    mut commands: Commands,
    start: Option<Res<PlayerStart>>,
    mut players: Query<(Entity, Option<&mut Health>), With<Player>>,
) {
    for (e, health) in &mut players {
        match health {
            Some(mut h) if start.as_ref().is_some_and(|s| s.is_changed()) => *h = Health::default(),
            Some(_) => {}
            None => {
                commands.entity(e).insert(Health::default());
            }
        }
    }
}

fn apply_damage(
    time: Res<Time>,
    mut damage: MessageReader<DamagePlayer>,
    mut players: Query<(&mut Player, &mut Health)>,
    mut deaths: MessageWriter<PlayerDied>,
) {
    let now = time.elapsed_secs_f64();
    let Ok((mut p, mut h)) = players.single_mut() else { return };
    if p.noclip {
        damage.clear();
        return;
    }
    for d in damage.read() {
        if h.dead {
            continue;
        }
        h.current -= d.amount;
        h.last_damage = now;
        p.state.velocity += d.push;
        if h.current <= 0.0 {
            h.dead = true;
            deaths.write(PlayerDied);
        }
    }
    if !h.dead && now - h.last_damage > REGEN_DELAY {
        h.current = (h.current + REGEN_RATE * time.delta_secs()).min(MAX_HEALTH);
    }
}
