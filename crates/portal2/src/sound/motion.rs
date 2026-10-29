//! Movement sounds of trains and brush movers. A `func_tracktrain` plays
//! `StartSound` and loops `MoveSound` while it moves, with the pitch rising
//! from `MoveSoundMinPitch` towards `MoveSoundMaxPitch` as the speed nears
//! 1000 units/s, and plays `StopSound` when it stops. Doors and
//! `func_movelinear` play `noise1` when they start moving (kept looping with
//! `loopmovesound`) and `noise2` when they arrive.

use bevy::prelude::*;

use crate::entities::movers::{Mover, MoverState};
use crate::entities::trains::Train;
use portal2_audio::Emit;
use portal2_core::entity::MapEntity;

const TRAIN_MAX_SPEED: f32 = 1000.0;

#[derive(Component, Default)]
pub struct MotionSound {
    moving: bool,
    channel: Option<u64>,
}

fn key(me: &MapEntity, k: &str) -> Option<String> {
    me.get(k).filter(|s| !s.is_empty()).map(str::to_owned)
}

pub fn motion_sounds(world: &mut World) {
    let trains: Vec<(Entity, bool, f32)> =
        world.query::<(Entity, &Train)>().iter(world).map(|(e, t)| (e, t.moving && t.speed != 0.0, t.speed.abs())).collect();
    let movers: Vec<(Entity, bool)> = world
        .query::<(Entity, &Mover)>()
        .iter(world)
        .map(|(e, m)| (e, matches!(m.state, MoverState::Opening | MoverState::Closing)))
        .collect();
    for (e, moving, speed) in trains {
        let Some(me) = world.get::<MapEntity>(e).cloned() else { continue };
        let was = transition(world, e, moving);
        let pitch = {
            let min = me.get_f32("movesoundminpitch").unwrap_or(60.0);
            let max = me.get_f32("movesoundmaxpitch").unwrap_or(200.0);
            (min + speed * (max - min) / TRAIN_MAX_SPEED).clamp(1.0, 255.0)
        };
        match (was, moving) {
            (false, true) => {
                if let Some(s) = key(&me, "startsound") {
                    portal2_audio::emit(world, &s, Emit { entity: Some(e), ..default() });
                }
                let id = key(&me, "movesound")
                    .and_then(|s| portal2_audio::emit(world, &s, Emit { entity: Some(e), pitch: Some(pitch), ..default() }));
                set_channel(world, e, id);
            }
            (true, true) => {
                if let Some(id) = world.get::<MotionSound>(e).and_then(|m| m.channel) {
                    portal2_audio::adjust(world, id, None, Some(pitch));
                }
            }
            (true, false) => {
                stop_channel(world, e);
                if let Some(s) = key(&me, "stopsound") {
                    portal2_audio::emit(world, &s, Emit { entity: Some(e), ..default() });
                }
            }
            _ => {}
        }
    }
    for (e, moving) in movers {
        let Some(me) = world.get::<MapEntity>(e).cloned() else { continue };
        match (transition(world, e, moving), moving) {
            (false, true) => {
                stop_channel(world, e);
                let id = key(&me, "noise1").and_then(|s| portal2_audio::emit(world, &s, Emit { entity: Some(e), ..default() }));
                let looping = me.get_f32("loopmovesound").unwrap_or(0.0) != 0.0;
                set_channel(world, e, if looping { id } else { None });
            }
            (true, false) => {
                stop_channel(world, e);
                if let Some(s) = key(&me, "noise2") {
                    portal2_audio::emit(world, &s, Emit { entity: Some(e), ..default() });
                }
            }
            _ => {}
        }
    }
}

/// Records the new state and returns the previous one.
fn transition(world: &mut World, e: Entity, moving: bool) -> bool {
    let mut ent = world.entity_mut(e);
    match ent.get_mut::<MotionSound>() {
        Some(mut m) => std::mem::replace(&mut m.moving, moving),
        None => {
            ent.insert(MotionSound { moving, channel: None });
            false
        }
    }
}

fn set_channel(world: &mut World, e: Entity, id: Option<u64>) {
    if let Some(mut m) = world.get_mut::<MotionSound>(e) {
        m.channel = id;
    }
}

fn stop_channel(world: &mut World, e: Entity) {
    if let Some(id) = world.get_mut::<MotionSound>(e).and_then(|mut m| m.channel.take()) {
        portal2_audio::stop_channel(world, id);
    }
}
