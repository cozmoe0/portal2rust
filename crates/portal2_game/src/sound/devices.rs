//! Loops test chamber devices keep running while they are on (server.dll):
//! tractor beams spin `VFX.TbeamEmitterSpinLp` and hum their polarity
//! (`VFX.TBeamPosPolarity` / `VFX.TBeamNegPolarity`), light bridges glow
//! `VFX.BridgeGlow`, and the fizzlers share one `VFX.FizzlerLp` (the game's
//! `fizzler_multiorigin_sound_player`), heard from the nearest point of the
//! nearest fizzler that is on.

use std::collections::HashMap;

use bevy::prelude::*;

use crate::devices::bridge::LightBridge;
use crate::devices::funnel::Funnel;
use crate::entities::triggers::{Trigger, TriggerKind};
use crate::player::Player;
use portal2_audio::SoundRequest;
use portal2_core::entity::BrushBounds;
use portal2_world::MapObject;

const FUNNEL_SPIN: &str = "VFX.TbeamEmitterSpinLp";
const BRIDGE_GLOW: &str = "VFX.BridgeGlow";
const FIZZLER_LOOP: &str = "VFX.FizzlerLp";

fn polarity(force: f32) -> &'static str {
    if force >= 0.0 { "VFX.TBeamPosPolarity" } else { "VFX.TBeamNegPolarity" }
}

/// The loops each device is playing.
#[derive(Default)]
pub struct DeviceLoops {
    playing: HashMap<Entity, Vec<&'static str>>,
    fizzler: Option<Entity>,
}

pub fn device_loops(
    mut commands: Commands,
    funnels: Query<(Entity, &Funnel)>,
    bridges: Query<(Entity, &LightBridge)>,
    fizzlers: Query<(&Trigger, &BrushBounds, &GlobalTransform)>,
    players: Query<&Player>,
    mut transforms: Query<&mut Transform, Without<Player>>,
    mut sounds: MessageWriter<SoundRequest>,
    mut state: Local<DeviceLoops>,
) {
    let mut wanted: HashMap<Entity, Vec<&'static str>> = HashMap::new();
    for (e, f) in &funnels {
        if f.enabled {
            wanted.insert(e, vec![FUNNEL_SPIN, polarity(f.force)]);
        }
    }
    for (e, b) in &bridges {
        if b.enabled {
            wanted.insert(e, vec![BRIDGE_GLOW]);
        }
    }
    let DeviceLoops { playing, fizzler } = &mut *state;
    playing.retain(|e, names| {
        let keep = wanted.get(e);
        for name in names.iter().filter(|n| keep.is_none_or(|k| !k.contains(n))) {
            sounds.write(SoundRequest::stop(name, *e));
        }
        keep.is_some()
    });
    for (e, names) in wanted {
        let now = playing.entry(e).or_default();
        for name in &names {
            if !now.contains(name) {
                sounds.write(SoundRequest::on(name, e));
            }
        }
        *now = names;
    }

    // The fizzler loop follows the player along the nearest fizzler.
    let ear = players.single().ok().map(|p| p.state.origin + Vec3::Z * p.state.eye_offset());
    let nearest = ear.and_then(|ear| {
        fizzlers
            .iter()
            .filter(|(t, _, _)| t.enabled && matches!(t.kind, TriggerKind::Cleanser))
            .map(|(_, b, gt)| {
                let (lo, hi) = (gt.transform_point(b.mins), gt.transform_point(b.maxs));
                ear.clamp(lo.min(hi), lo.max(hi))
            })
            .min_by(|a, b| a.distance_squared(ear).total_cmp(&b.distance_squared(ear)))
    });
    // A new map took the old source with it.
    if fizzler.is_some_and(|s| transforms.get(s).is_err()) {
        *fizzler = None;
    }
    match (nearest, *fizzler) {
        (Some(p), Some(s)) => {
            if let Ok(mut t) = transforms.get_mut(s) {
                t.translation = p;
            }
        }
        (Some(p), None) => {
            let s = commands.spawn((Name::new("fizzler loop"), Transform::from_translation(p), MapObject)).id();
            sounds.write(SoundRequest::on(FIZZLER_LOOP, s));
            *fizzler = Some(s);
        }
        (None, Some(s)) => {
            sounds.write(SoundRequest::stop(FIZZLER_LOOP, s));
            commands.entity(s).despawn();
            *fizzler = None;
        }
        (None, None) => {}
    }
}
