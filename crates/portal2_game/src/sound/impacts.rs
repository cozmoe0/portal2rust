//! Physics impact sounds, as Source's `PhysCollisionSound` plays them: a
//! physics prop that hits something at 70 units/s or more (relative speed
//! along the contact normal), at least 0.05 s after its last collision,
//! plays its surface property's `impacthard` sound, or `impactsoft` when
//! what it hit is softer (`audiohardnessfactor`) than its
//! `impactHardThreshold` or the hit was slower than its
//! `audioHardMinVelocity`. The volume is the entry's times
//! `(speed / 320)²`, at most full. Each prop in a collision plays its own
//! sound; the world plays none and counts as the `default` surface.

use std::collections::HashMap;

use avian3d::prelude::*;
use bevy::prelude::*;

use super::footsteps::SurfaceProps;
use crate::entities::props::PhysicsProp;
use portal2_audio::SoundRequest;
use portal2_core::clock::GameClock;
use portal2_core::entity::MapEntity;
use portal2_core::vfs::GameFs;

const MIN_SPEED: f32 = 70.0;
const FULL_VOLUME_SPEED: f32 = 320.0;
const MIN_INTERVAL: f64 = 0.05;

/// A prop's velocity before the last physics step and when it last
/// collided.
#[derive(Component, Default)]
pub struct ImpactState {
    velocity: Vec3,
    last: f64,
}

#[derive(Default)]
pub struct ImpactSurfaces {
    props: Option<SurfaceProps>,
    /// Model path to its surface property.
    models: HashMap<String, String>,
}

impl ImpactSurfaces {
    fn of(&mut self, fs: &GameFs, me: &MapEntity) -> String {
        let Some(path) = me.studio_model() else { return "default".into() };
        self.models
            .entry(path.to_ascii_lowercase())
            .or_insert_with(|| {
                fs.read(&path)
                    .and_then(|d| source_formats::mdl::Mdl::parse(d).ok())
                    .map(|m| m.surface_prop.to_ascii_lowercase())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "default".into())
            })
            .clone()
    }
}

#[allow(clippy::too_many_arguments)]
pub fn impact_sounds(
    clock: Res<GameClock>,
    fs: Res<GameFs>,
    mut starts: MessageReader<CollisionStart>,
    contacts: Res<ContactGraph>,
    mut props: Query<(Entity, &MapEntity, &LinearVelocity, Option<&mut ImpactState>), With<PhysicsProp>>,
    mut commands: Commands,
    mut sounds: MessageWriter<SoundRequest>,
    mut surfaces: Local<ImpactSurfaces>,
) {
    let now = clock.curtime;
    let before = |e: Entity| props.get(e).ok().map(|(_, _, v, s)| s.map_or(v.0, |s| s.velocity));
    // (prop, what it hit, speed)
    let mut hits = Vec::new();
    for ev in starts.read() {
        let a = ev.body1.unwrap_or(ev.collider1);
        let b = ev.body2.unwrap_or(ev.collider2);
        let (va, vb) = (before(a), before(b));
        if va.is_none() && vb.is_none() {
            continue;
        }
        let relative = va.unwrap_or(Vec3::ZERO) - vb.unwrap_or(Vec3::ZERO);
        let normal = contacts.get(ev.collider1, ev.collider2).and_then(|(_, pair)| pair.manifolds.first()).map(|m| m.normal);
        let speed = normal.map_or(relative.length(), |n| relative.dot(n).abs());
        for (prop, other) in [(a, b), (b, a)] {
            if before(prop).is_some() {
                hits.push((prop, other, speed));
            }
        }
    }
    if surfaces.props.is_none() {
        surfaces.props = Some(SurfaceProps::load(&fs.0));
    }
    for (prop, other, speed) in hits {
        let hit_surface = props.get(other).ok().map(|(_, me, _, _)| me.clone());
        let Ok((_, me, _, state)) = props.get_mut(prop) else { continue };
        let me = me.clone();
        let Some(mut state) = state else { continue };
        let since = now - state.last;
        state.last = now;
        if since < MIN_INTERVAL || speed < MIN_SPEED {
            continue;
        }
        let own_name = surfaces.of(&fs, &me);
        let hit_name = hit_surface.map_or_else(|| "default".to_owned(), |m| surfaces.of(&fs, &m));
        let table = surfaces.props.as_mut().expect("loaded above");
        let (own, hit) = (table.sounds(&own_name), table.sounds(&hit_name));
        let Some(hard) = own.impact_hard.clone() else { continue };
        let soft = hit.hardness < own.hard_threshold || (own.hard_min_velocity > 0.0 && own.hard_min_velocity > speed);
        let name = if soft { own.impact_soft.clone().unwrap_or(hard) } else { hard };
        let volume = (speed * speed / (FULL_VOLUME_SPEED * FULL_VOLUME_SPEED)).min(1.0);
        trace!("impact: {prop} {name} speed {speed:.0} volume {volume:.2}");
        let mut request = SoundRequest::on(&name, prop);
        request.params.volume_scale = Some(volume);
        sounds.write(request);
    }
    for (e, _, v, state) in &mut props {
        match state {
            Some(mut s) => s.velocity = v.0,
            None => {
                commands.entity(e).insert(ImpactState { velocity: v.0, last: f64::MIN });
            }
        }
    }
}
