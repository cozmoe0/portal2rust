//! Player health: damage from hazards, regeneration, and death.

use bevy::prelude::*;

use super::{Player, PlayerMoveSet};
use crate::clock::GameClock;
use crate::entities::triggers::PlayerDied;
use crate::world::PlayerStart;

pub const MAX_HEALTH: f32 = 100.0;
/// Seconds without damage before health starts coming back
/// (`sv_regeneration_wait_time`).
const REGEN_DELAY: f64 = 1.0;
/// One point of health per 60 Hz tick after that.
const REGEN_RATE: f32 = 60.0;

pub struct HealthPlugin;

impl Plugin for HealthPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<DamagePlayer>()
            .add_systems(Update, reset_health)
            .add_systems(FixedUpdate, apply_damage.after(PlayerMoveSet));
    }
}

#[derive(Component, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Health {
    pub current: f32,
    pub last_damage: f64,
    pub dead: bool,
    /// `god`: ignore all damage.
    pub god: bool,
}

impl Default for Health {
    fn default() -> Self {
        Self { current: MAX_HEALTH, last_damage: f64::MIN, dead: false, god: false }
    }
}

/// Damage to the player, with an optional shove.
#[derive(Message, Clone, Copy, Debug)]
pub struct DamagePlayer {
    pub amount: f32,
    pub push: Vec3,
    /// What dealt it (logged when the damage kills).
    pub source: Option<Entity>,
}

fn reset_health(
    mut commands: Commands,
    start: Option<Res<PlayerStart>>,
    mut players: Query<(Entity, Option<&mut Health>), With<Player>>,
) {
    for (e, health) in &mut players {
        match health {
            Some(mut h) if start.as_ref().is_some_and(|s| s.is_changed()) => *h = Health { god: h.god, ..default() },
            Some(_) => {}
            None => {
                commands.entity(e).insert(Health::default());
            }
        }
    }
}

fn apply_damage(
    clock: Res<GameClock>,
    mut damage: MessageReader<DamagePlayer>,
    mut players: Query<(&mut Player, &mut Health)>,
    names: Query<&Name>,
    mut deaths: MessageWriter<PlayerDied>,
) {
    let now = clock.curtime;
    let Ok((mut p, mut h)) = players.single_mut() else { return };
    if p.noclip || h.god {
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
            let killer = d.source.and_then(|s| names.get(s).ok()).map(|n| n.as_str().to_owned());
            info!("player killed by {}", killer.as_deref().unwrap_or("unknown"));
            deaths.write(PlayerDied);
        }
    }
    if !h.dead && now - h.last_damage > REGEN_DELAY {
        h.current = (h.current + REGEN_RATE * clock.frametime()).min(MAX_HEALTH);
    }
}

impl crate::save::Restore for Health {}
