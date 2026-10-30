//! The overhaul's lights: what new emitters (portals, lasers) and the map's
//! own fixtures add to the world and model shaders.
//!
//! Producers push [`FxLightData`] into [`FxLights`] every frame (in
//! [`FxProduceSet`]); this module then scores them by how much they can
//! matter to the view (brightness over distance to the eye or to either
//! portal, whose views also show the world), keeps the preset's budget and
//! packs them into the shared buffer (`shaders/fx.wesl` `FxLight`).
//!
//! Light already baked into the lightmaps must not be added twice, so the
//! map's fixtures (`light`, `light_spot` and luminous textures, from the
//! world-light lump) only add specular highlights and volumetric beams: the
//! shaders scale them by how much of the expected light the lightmap shows
//! at the surface, so occluded fixtures do not glint. Portals and lasers are
//! not in the lightmaps and light everything they reach.
//!
//! Lasers become one line light per beam segment (a representative-point
//! tube light, so a beam lights the floor evenly along its length) plus a
//! point light at the emitter and one at the impact.

use bevy::prelude::*;
use portal2_render::fx::{HEADER_FLOATS, LIGHT_FLOATS, MAX_LIGHTS};

use super::material::{FxBuffer, FxFillSet};
use super::{FxSettings, features};
use crate::devices::laser::LaserEmitter;
use crate::portals::{Portal, Portals};
use portal2_core::view::MainCamera;
use portal2_world::{CurrentMap, MapLoaded};

/// [`FxLightData::flags`], shared with `shaders/fx.wesl`.
pub mod light_flags {
    pub const DIFFUSE: u32 = 1;
    pub const SPECULAR: u32 = 1 << 1;
    pub const VOLUMETRIC: u32 = 1 << 2;
    /// A fixture whose direct light the lightmaps already hold.
    pub const FIXTURE: u32 = 1 << 3;
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FxLightShape {
    /// Falls off as 1 / (c + l d + q d^2).
    Point {
        attenuation: Vec3,
    },
    Spot {
        axis: Vec3,
        cos_inner: f32,
        cos_outer: f32,
        exponent: f32,
        attenuation: Vec3,
    },
    /// A tube from the light's position to `end`.
    Line {
        end: Vec3,
    },
    /// A one-sided elliptical disc facing `normal` (portals).
    Ellipse {
        normal: Vec3,
        up: Vec3,
        half_width: f32,
        half_height: f32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FxLightData {
    pub position: Vec3,
    pub shape: FxLightShape,
    /// Linear colour times intensity, on the lightmaps' scale.
    pub color: Vec3,
    /// Nothing is lit beyond this distance (with a smooth fade before it).
    pub range: f32,
    pub flags: u32,
    /// Size of the emitter, which softens highlights and close falloff.
    pub radius: f32,
    /// Scales the light's score against the others.
    pub priority: f32,
    /// Wants one of the overhaul's shadow maps.
    pub shadow: bool,
}

const KIND_POINT: f32 = 0.0;
const KIND_SPOT: f32 = 1.0;
const KIND_LINE: f32 = 2.0;
const KIND_ELLIPSE: f32 = 3.0;

impl FxLightData {
    /// The five vectors of `FxLight` in `shaders/fx.wesl`.
    pub fn pack(&self, shadow_slot: Option<usize>) -> [f32; LIGHT_FLOATS] {
        let p = self.position;
        let (kind, axis, axis_w, param_x, param_w, up) = match self.shape {
            FxLightShape::Point { attenuation } => (KIND_POINT, Vec3::ZERO, -1.0, 1.0, 1.0, attenuation),
            FxLightShape::Spot { axis, cos_inner, cos_outer, exponent, attenuation } => {
                (KIND_SPOT, axis, cos_outer, cos_inner, exponent, attenuation)
            }
            FxLightShape::Line { end } => (KIND_LINE, end, 0.0, 0.0, 0.0, Vec3::ZERO),
            FxLightShape::Ellipse { normal, up, half_width, half_height } => (KIND_ELLIPSE, normal, half_width, half_height, 0.0, up),
        };
        let slot = shadow_slot.map_or(-1.0, |s| s as f32);
        [
            p.x,
            p.y,
            p.z,
            kind,
            self.color.x,
            self.color.y,
            self.color.z,
            self.range,
            axis.x,
            axis.y,
            axis.z,
            axis_w,
            param_x,
            self.flags as f32,
            slot,
            if param_w != 0.0 { param_w } else { self.radius },
            up.x,
            up.y,
            up.z,
            self.radius,
        ]
    }

    /// The nearest point of the light to `p` (for scoring).
    fn nearest(&self, p: Vec3) -> Vec3 {
        match self.shape {
            FxLightShape::Line { end } => closest_on_segment(self.position, end, p),
            _ => self.position,
        }
    }
}

pub fn closest_on_segment(a: Vec3, b: Vec3, p: Vec3) -> Vec3 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
    a + ab * t
}

/// This frame's lights, before scoring.
#[derive(Resource, Default)]
pub struct FxLights {
    pub frame: Vec<FxLightData>,
}

/// The lights that made this frame's cut, in buffer order (for the shadow
/// and volumetric passes).
#[derive(Resource, Default, Clone)]
pub struct FxActiveLights(pub Vec<FxLightData>);

/// Systems that push lights into [`FxLights`].
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct FxProduceSet;

/// The map's light fixtures (from the world-light lump), built on load.
#[derive(Resource, Default)]
struct Fixtures {
    lights: Vec<Fixture>,
    /// The camera's cluster and the clusters it sees, while it stays put.
    cluster: Option<i32>,
    pvs: Vec<u8>,
}

struct Fixture {
    light: FxLightData,
    cluster: i32,
    style: i32,
}

pub struct FxLightsPlugin;

impl Plugin for FxLightsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FxLights>()
            .init_resource::<FxActiveLights>()
            .init_resource::<Fixtures>()
            .configure_sets(PostUpdate, FxProduceSet.before(FxFillSet).after(TransformSystems::Propagate))
            .add_systems(First, clear_lights)
            .add_systems(PreUpdate, load_fixtures.after(portal2_core::sets::LoadMapSet))
            .add_systems(PostUpdate, ((fixture_lights, laser_lights).in_set(FxProduceSet), gather.in_set(FxFillSet)));
    }
}

fn clear_lights(mut lights: ResMut<FxLights>) {
    lights.frame.clear();
}

/// World-light emit types.
mod emit {
    pub const SURFACE: i32 = 0;
    pub const POINT: i32 = 1;
    pub const SPOT: i32 = 2;
}

/// Fixture highlights are a share of the light the fixture casts.
const FIXTURE_SPECULAR: f32 = 1.0;
/// The faintest fixture light kept, at its range.
const FIXTURE_CUTOFF: f32 = 0.004;

fn load_fixtures(mut loaded: MessageReader<MapLoaded>, map: Option<Res<CurrentMap>>, mut fixtures: ResMut<Fixtures>) {
    if loaded.read().last().is_none() {
        return;
    }
    *fixtures = Fixtures::default();
    let Some(map) = map else { return };
    let luma = |c: Vec3| c.dot(Vec3::new(0.299, 0.587, 0.114));
    for l in &map.bsp.world_lights {
        if !matches!(l.kind, emit::POINT | emit::SPOT | emit::SURFACE) {
            continue;
        }
        let mut attenuation = Vec3::new(l.constant_attn, l.linear_attn, l.quadratic_attn);
        if l.kind == emit::SURFACE || attenuation == Vec3::ZERO {
            attenuation = Vec3::Z;
        }
        // The distance where the light fades under the cutoff.
        let i = luma(l.intensity) / FIXTURE_CUTOFF;
        let (c, lin, q) = (attenuation.x, attenuation.y, attenuation.z);
        let range = if q > 0.0 {
            (-lin + (lin * lin - 4.0 * q * (c - i)).max(0.0).sqrt()) / (2.0 * q)
        } else if lin > 0.0 {
            (i - c) / lin
        } else {
            1500.0
        };
        let range = range.clamp(32.0, 1500.0);
        let shape = match l.kind {
            emit::SPOT => FxLightShape::Spot {
                axis: l.normal,
                cos_inner: l.stopdot,
                cos_outer: l.stopdot2,
                exponent: if l.exponent == 0.0 { 1.0 } else { l.exponent },
                attenuation,
            },
            // Luminous textures radiate over their front hemisphere.
            emit::SURFACE => FxLightShape::Spot { axis: l.normal, cos_inner: 1.0, cos_outer: 0.0, exponent: 1.0, attenuation },
            _ => FxLightShape::Point { attenuation },
        };
        let mut flags = light_flags::SPECULAR | light_flags::FIXTURE;
        if l.kind == emit::SPOT {
            flags |= light_flags::VOLUMETRIC;
        }
        fixtures.lights.push(Fixture {
            light: FxLightData {
                position: l.origin,
                shape,
                color: l.intensity * FIXTURE_SPECULAR,
                range,
                flags,
                radius: 4.0,
                priority: 0.5,
                shadow: false,
            },
            cluster: l.cluster,
            style: l.style,
        });
    }
    info!("{} light fixtures for the visual overhaul", fixtures.lights.len());
}

fn cluster_visible(pvs: &[u8], cluster: i32) -> bool {
    cluster < 0 || pvs.get((cluster / 8) as usize).is_some_and(|b| b & (1 << (cluster % 8)) != 0)
}

/// Fixtures in the clusters the camera can see, at their light style.
fn fixture_lights(
    settings: Res<FxSettings>,
    map: Option<Res<CurrentMap>>,
    styles: Option<Res<portal2_world::lightstyles::LightStyles>>,
    camera: Query<&GlobalTransform, With<MainCamera>>,
    mut fixtures: ResMut<Fixtures>,
    mut lights: ResMut<FxLights>,
) {
    if !settings.has(features::LIGHTS) || fixtures.lights.is_empty() {
        return;
    }
    let (Some(map), Ok(camera)) = (map, camera.single()) else { return };
    let leaf = map.bsp.find_leaf(camera.translation());
    let cluster = map.bsp.leafs.get(leaf).map_or(-1, |l| l.cluster as i32);
    if fixtures.cluster != Some(cluster) {
        fixtures.pvs = match &map.bsp.visibility {
            Some(vis) if cluster >= 0 => vis.pvs(cluster as usize),
            _ => Vec::new(),
        };
        fixtures.cluster = Some(cluster);
    }
    let all_visible = fixtures.pvs.is_empty();
    for f in &fixtures.lights {
        if !all_visible && !cluster_visible(&fixtures.pvs, f.cluster) {
            continue;
        }
        let scale = styles.as_ref().map_or(1.0, |s| s.value(f.style));
        if scale <= 0.0 {
            continue;
        }
        let mut light = f.light;
        light.color *= scale;
        lights.frame.push(light);
    }
}

/// The normal beam's colour (its sprite is red), and the lethal one's.
const LASER_COLOR: Vec3 = Vec3::new(1.0, 0.1, 0.06);
const LETHAL_COLOR: Vec3 = Vec3::new(0.2, 1.0, 0.25);
const LASER_BEAM_INTENSITY: f32 = 350.0;
const LASER_POINT_INTENSITY: f32 = 700.0;
const LASER_RANGE: f32 = 150.0;

fn laser_lights(settings: Res<FxSettings>, emitters: Query<&LaserEmitter>, mut lights: ResMut<FxLights>) {
    if !settings.has(features::LIGHTS) {
        return;
    }
    for e in &emitters {
        if !e.on || e.segments.is_empty() {
            continue;
        }
        let color = if e.lethal { LETHAL_COLOR } else { LASER_COLOR };
        let both = light_flags::DIFFUSE | light_flags::SPECULAR;
        for s in &e.segments {
            if s.start.distance_squared(s.end) < 1.0 {
                continue;
            }
            lights.frame.push(FxLightData {
                position: s.start,
                shape: FxLightShape::Line { end: s.end },
                color: color * LASER_BEAM_INTENSITY,
                range: LASER_RANGE,
                flags: both,
                radius: 6.0,
                priority: 2.0,
                shadow: false,
            });
        }
        let first = e.segments[0];
        lights.frame.push(FxLightData {
            position: first.start + first.dir * 4.0,
            shape: FxLightShape::Point { attenuation: Vec3::Z },
            color: color * LASER_POINT_INTENSITY,
            range: LASER_RANGE * 1.25,
            flags: both,
            radius: 8.0,
            priority: 2.5,
            shadow: false,
        });
        // The last segment ends on whatever stops the beam (not a portal).
        if let Some(last) = e.segments.last().filter(|s| !s.through_portal) {
            lights.frame.push(FxLightData {
                position: last.end - last.dir * 3.0,
                shape: FxLightShape::Point { attenuation: Vec3::Z },
                color: color * LASER_POINT_INTENSITY * 0.7,
                range: LASER_RANGE,
                flags: both,
                radius: 6.0,
                priority: 2.0,
                shadow: false,
            });
        }
    }
}

/// How much a light can matter: priority times brightness over squared
/// distance to the nearest place the player looks from.
fn score(light: &FxLightData, viewpoints: &[Vec3]) -> f32 {
    let d = viewpoints.iter().map(|v| light.nearest(*v).distance(*v)).fold(f32::MAX, f32::min);
    let reach = (d - light.range).max(0.0) + 64.0;
    let luma = light.color.dot(Vec3::new(0.299, 0.587, 0.114));
    light.priority * luma / (reach * reach)
}

pub(super) fn gather(
    settings: Res<FxSettings>,
    lights: Res<FxLights>,
    camera: Query<&GlobalTransform, With<MainCamera>>,
    portals: Res<Portals>,
    portal_query: Query<&Portal>,
    mut active: ResMut<FxActiveLights>,
    mut buffer: ResMut<FxBuffer>,
) {
    active.0.clear();
    if settings.has(features::LIGHTS) {
        let mut viewpoints: Vec<Vec3> = camera.single().map(|c| c.translation()).into_iter().collect();
        viewpoints.extend(portals.entities.iter().flatten().filter_map(|e| portal_query.get(*e).ok()).map(|p| p.frame.center));
        let mut scored: Vec<(f32, &FxLightData)> =
            lights.frame.iter().map(|l| (score(l, &viewpoints), l)).filter(|(s, _)| *s > 0.0).collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        active.0.extend(scored.iter().take(settings.light_budget.min(MAX_LIGHTS)).map(|(_, l)| **l));
    }
    let data = &mut buffer.data;
    data[2] = active.0.len() as f32;
    for (i, light) in active.0.iter().enumerate() {
        let o = HEADER_FLOATS + i * LIGHT_FLOATS;
        data[o..o + LIGHT_FLOATS].copy_from_slice(&light.pack(None));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packing_and_scoring() {
        let line = FxLightData {
            position: Vec3::ZERO,
            shape: FxLightShape::Line { end: Vec3::new(100.0, 0.0, 0.0) },
            color: Vec3::ONE,
            range: 100.0,
            flags: light_flags::DIFFUSE,
            radius: 2.0,
            priority: 1.0,
            shadow: false,
        };
        let packed = line.pack(Some(3));
        assert_eq!(packed.len(), 20);
        assert_eq!(packed[3], KIND_LINE);
        assert_eq!(&packed[8..11], &[100.0, 0.0, 0.0]);
        assert_eq!(packed[13], light_flags::DIFFUSE as f32);
        assert_eq!(packed[14], 3.0);
        // A line is as near as its closest point.
        assert_eq!(closest_on_segment(Vec3::ZERO, Vec3::X * 100.0, Vec3::new(50.0, 10.0, 0.0)), Vec3::new(50.0, 0.0, 0.0));
        let near = score(&line, &[Vec3::new(50.0, 10.0, 0.0)]);
        let far = score(&line, &[Vec3::new(50.0, 1000.0, 0.0)]);
        assert!(near > far && far > 0.0);
        // The buffer holds a header and the most lights any preset uses.
        assert_eq!(HEADER_FLOATS % 4, 0);
        assert_eq!(LIGHT_FLOATS % 4, 0);
    }
}
