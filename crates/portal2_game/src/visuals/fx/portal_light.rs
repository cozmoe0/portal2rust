//! Light from the portals' burning rims onto the walls, floor and props
//! around them.
//!
//! Every portal entity gets a [`PortalLight`] when it appears (and loses it
//! with the entity). Each frame it becomes a one-sided elliptical area light
//! the size of the opening, a little in front of the wall, in the portal's
//! colour. Its strength follows the opening animation and the static: an
//! unlinked portal (or one whose partner just moved), full of fire, is
//! brighter than a clear one whose rim alone burns. A slow, smooth flicker
//! keeps it alive without strobing.

use bevy::prelude::*;
use portal2_world::collision::portal::{HALF_HEIGHT, HALF_WIDTH};

use super::lights::{FxLightData, FxLightShape, FxLights, FxProduceSet, light_flags};
use super::{FxSettings, features};
use crate::portals::{Portal, PortalColors, Portals};

/// Light from a clear portal's rim, and the extra from a fire-filled one.
const RIM_INTENSITY: f32 = 700.0;
const FILL_INTENSITY: f32 = 550.0;
const RANGE: f32 = 420.0;
/// How far in front of the wall the light sits (the portal's quad is 0.4
/// in front).
const OFFSET: f32 = 0.5;
/// The burning hole is a little smaller than the portal's quad (the rim's
/// coordinates run 7.5% past it, see `portal.wesl`).
const HOLE_SCALE: f32 = 1.0 / 1.075;

/// The light state of one portal.
#[derive(Component, Clone, Copy, Debug)]
pub struct PortalLight {
    /// Flicker phase, so the two portals do not pulse together.
    pub phase: f32,
}

pub struct PortalLightingPlugin;

impl Plugin for PortalLightingPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostUpdate, (attach, portal_lights).chain().in_set(FxProduceSet));
    }
}

fn attach(mut commands: Commands, new: Query<(Entity, &Portal), Without<PortalLight>>) {
    for (e, p) in &new {
        commands.entity(e).insert(PortalLight { phase: p.slot as f32 * 2.17 });
    }
}

/// A smooth flicker around 1 (three slow incommensurate sines).
pub fn flicker(t: f32, phase: f32) -> f32 {
    1.0 + 0.035 * (t * 7.1 + phase).sin() + 0.02 * (t * 12.7 + phase * 1.7).sin() + 0.015 * (t * 3.3 + phase * 0.6).sin()
}

/// Brightness of a portal from its opening and static (as `animate_portals`).
pub fn portal_energy(now: f64, opened_at: f64, last_opened: f64, linked: bool) -> f32 {
    let open = ((now - opened_at) * 2.0).clamp(0.0, 1.0) as f32;
    let static_amount = if linked { (1.0 - (now - last_opened)).clamp(0.0, 1.0) as f32 } else { 1.0 };
    open * open * (RIM_INTENSITY + FILL_INTENSITY * static_amount)
}

fn portal_lights(
    settings: Res<FxSettings>,
    time: Res<Time>,
    colors: Res<PortalColors>,
    portals: Res<Portals>,
    query: Query<(&Portal, Option<&PortalLight>)>,
    mut lights: ResMut<FxLights>,
) {
    if !settings.has(features::LIGHTS) {
        return;
    }
    let now = time.elapsed_secs_f64();
    let open: Vec<(&Portal, Option<&PortalLight>)> = portals.entities.iter().flatten().filter_map(|e| query.get(*e).ok()).collect();
    let linked = open.len() == 2;
    let last_opened = open.iter().map(|(p, _)| p.opened_at).fold(f64::MIN, f64::max);
    for (p, light) in &open {
        let mut energy = portal_energy(now, p.opened_at, last_opened, linked);
        if settings.has(features::PROCEDURAL) {
            energy *= flicker(now as f32, light.map_or(0.0, |l| l.phase));
        }
        if energy <= 0.0 {
            continue;
        }
        // Equal brightness for both colours, keeping their hue.
        let c = colors.0[p.slot.min(1)];
        let color = c / c.dot(Vec3::new(0.2126, 0.7152, 0.0722)).max(1e-3) * 0.5;
        lights.frame.push(FxLightData {
            position: p.frame.center + p.frame.forward * OFFSET,
            shape: FxLightShape::Ellipse {
                normal: p.frame.forward,
                up: p.frame.up,
                half_width: HALF_WIDTH * HOLE_SCALE,
                half_height: HALF_HEIGHT * HOLE_SCALE,
            },
            color: color * energy,
            range: RANGE,
            flags: light_flags::DIFFUSE | light_flags::SPECULAR | light_flags::VOLUMETRIC,
            radius: HALF_WIDTH,
            priority: 3.0,
            shadow: true,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn energy_follows_opening_and_static() {
        // Nothing before it opens, full after half a second.
        assert_eq!(portal_energy(10.0, 10.0, 10.0, false), 0.0);
        let unlinked = portal_energy(11.0, 10.0, 10.0, false);
        assert!((unlinked - (RIM_INTENSITY + FILL_INTENSITY)).abs() < 1e-3);
        // A linked pair clears to the rim alone a second after the last move.
        let clear = portal_energy(20.0, 10.0, 12.0, true);
        assert!((clear - RIM_INTENSITY).abs() < 1e-3);
        assert!(portal_energy(12.5, 10.0, 12.0, true) > clear);
        for i in 0..1000 {
            let f = flicker(i as f32 * 0.01, 1.0);
            assert!((0.9..=1.1).contains(&f));
        }
    }
}
