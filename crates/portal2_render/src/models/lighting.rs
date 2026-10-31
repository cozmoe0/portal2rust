//! Lighting for dynamic models, after the engine's light cache: the leaf
//! ambient cube at the model plus its strongest few world lights. Lights the
//! compiler already baked into the ambient samples are left out, and visible
//! lights beyond the strongest few are folded into the ambient cube. The sun
//! (`light_environment`'s sky light) counts where the model sees the sky
//! along it.

use bevy::prelude::*;
use source_formats::bsp::{Bsp, WorldLight, surf};

use crate::source_material::{ModelLight, ModelParams};
use crate::textures::TextureCache;
use portal2_core::vfs::GameFs;

/// Local lights per model (`r_worldlights`, 3 by default).
pub const MAX_MODEL_LIGHTS: usize = 3;

/// Weakest light worth considering (`r_worldlightmin`); surface lights are exempt.
const WORLD_LIGHT_MIN: f32 = 0.0002;

/// World light emit types as stored in the BSP.
mod emit {
    pub const SURFACE: i32 = 0;
    pub const POINT: i32 = 1;
    pub const SPOT: i32 = 2;
    pub const SKYLIGHT: i32 = 3;
}

/// Leaf flags: the leaf sees the 3D sky / the 2D sky.
const LEAF_SKY: u16 = 0x1;
const LEAF_SKY2D: u16 = 0x4;

/// World light flag: the light is already part of the leaf ambient samples.
const IN_AMBIENT_CUBE: i32 = 1;

/// Unit directions of the ambient cube faces (+X, -X, +Y, -Y, +Z, -Z).
const CUBE_AXES: [Vec3; 6] = [Vec3::X, Vec3::NEG_X, Vec3::Y, Vec3::NEG_Y, Vec3::Z, Vec3::NEG_Z];

#[derive(Clone, Debug, Default)]
pub struct ModelLighting {
    pub ambient: [Vec3; 6],
    pub lights: Vec<ModelLight>,
}

impl ModelLighting {
    pub fn apply(&self, params: &mut ModelParams) {
        for (dst, src) in params.ambient.iter_mut().zip(self.ambient) {
            *dst = src.extend(1.0);
        }
        params.num_lights = self.lights.len().min(params.lights.len()) as u32;
        for (dst, src) in params.lights.iter_mut().zip(&self.lights) {
            *dst = *src;
        }
    }
}

fn cluster_visible(pvs: &[u8], cluster: i32) -> bool {
    if cluster < 0 {
        return true;
    }
    pvs.get((cluster / 8) as usize).is_some_and(|b| b & (1 << (cluster % 8)) != 0)
}

/// Distance attenuation terms (constant, linear, quadratic) the engine uses.
/// Surface lights are stored without any and fall off with the inverse square;
/// point and spot lights stored without any get the same at map load.
fn attenuation_terms(l: &WorldLight) -> Vec3 {
    let terms = Vec3::new(l.constant_attn, l.linear_attn, l.quadratic_attn);
    if l.kind == emit::SURFACE || terms == Vec3::ZERO { Vec3::Z } else { terms }
}

/// Spot cone falloff exponent; 0 in the file means 1.
fn spot_exponent(l: &WorldLight) -> f32 {
    if l.exponent == 0.0 { 1.0 } else { l.exponent }
}

/// Light arriving at `p` from `l` ignoring the receiving surface's
/// orientation, plus the unit direction from `p` to the light.
fn light_at(l: &WorldLight, p: Vec3) -> Option<(f32, Vec3)> {
    let d = l.origin - p;
    let dist = d.length().max(1.0);
    // Surface lights reach no further than their radius (2000 units when the
    // map leaves it unset); the light cache's radius factor (1000) lifts the
    // limit for the other kinds.
    if l.kind == emit::SURFACE && dist > if l.radius >= 1.0 { l.radius } else { 2000.0 } {
        return None;
    }
    let dir = d / dist;
    let a = attenuation_terms(l);
    let mut atten = 1.0 / (a.x + a.y * dist + a.z * dist * dist).max(1e-6);
    let cos = (-dir).dot(l.normal);
    match l.kind {
        emit::SPOT => {
            if cos <= l.stopdot2 {
                return None;
            }
            if cos < l.stopdot {
                atten *= ((cos - l.stopdot2) / (l.stopdot - l.stopdot2).max(1e-4)).powf(spot_exponent(l));
            }
        }
        // Surface lights radiate over the hemisphere in front of them.
        emit::SURFACE if cos <= 0.01 => return None,
        emit::SURFACE => atten *= cos,
        _ => {}
    }
    Some((atten, dir))
}

/// Whether a ray from `p` along `dir` reaches the sky: the first opaque
/// world brush it enters is entered through a sky face (the light cache's
/// trace toward the sun for the sky light).
pub fn sees_sky(bsp: &Bsp, p: Vec3, dir: Vec3) -> bool {
    use source_formats::bsp::contents::{MOVEABLE, OPAQUE, SOLID};
    bsp.first_surface_along(p, p + dir * 56756.0, SOLID | MOVEABLE | OPAQUE).is_some_and(|f| f & (surf::SKY | surf::SKY2D) != 0)
}

/// Whether no opaque world brush lies between a light and the point it
/// lights (the light cache's visibility trace).
fn light_reaches(bsp: &Bsp, light: Vec3, p: Vec3) -> bool {
    use source_formats::bsp::contents::{MOVEABLE, OPAQUE, SOLID};
    bsp.first_surface_along(light, p, SOLID | MOVEABLE | OPAQUE).is_none()
}

fn to_shader_light(l: &WorldLight, scale: f32) -> ModelLight {
    // Surface lights are spot lights whose cone covers the hemisphere with a
    // cosine falloff; the sky light shines along its direction.
    let (kind, inner, outer, exponent) = match l.kind {
        emit::SKYLIGHT => (2.0, 1.0, -1.0, 1.0),
        emit::SPOT => (1.0, l.stopdot, l.stopdot2, spot_exponent(l)),
        emit::SURFACE => (1.0, 1.0, 0.0, 1.0),
        _ => (0.0, 1.0, -1.0, 1.0),
    };
    ModelLight {
        position: l.origin.extend(kind),
        color: (l.intensity * scale).extend(exponent),
        direction: l.normal.extend(inner),
        attenuation: attenuation_terms(l).extend(outer),
    }
}

/// Computes lighting at `p` (usually the model's origin or illumination point).
pub fn compute(bsp: &Bsp, p: Vec3) -> ModelLighting {
    compute_styled(bsp, p, &|_| 1.0)
}

/// Lighting at `p` with each world light scaled by its light style's
/// brightness (`style value / 264`; switched-off lights give none).
pub fn compute_styled(bsp: &Bsp, p: Vec3, style: &dyn Fn(i32) -> f32) -> ModelLighting {
    compute_occluded(bsp, p, style, &|a, b| !light_reaches(bsp, a, b))
}

/// As [`compute_styled`], with `blocked(light, point)` telling which lights
/// something hides from the point (the world and static props in the game).
pub fn compute_occluded(bsp: &Bsp, p: Vec3, style: &dyn Fn(i32) -> f32, blocked: &dyn Fn(Vec3, Vec3) -> bool) -> ModelLighting {
    let leaf_cube = bsp.ambient_cube(p);
    // Lights flagged as part of the ambient samples only count when the cube
    // really comes from those samples.
    let skip_baked = leaf_cube.is_some();
    let mut ambient = leaf_cube.unwrap_or([Vec3::splat(0.02); 6]);
    let leaf = bsp.find_leaf(p);
    let pvs = match (&bsp.visibility, bsp.leafs.get(leaf)) {
        (Some(vis), Some(l)) if l.cluster >= 0 => Some(vis.pvs(l.cluster as usize)),
        _ => None,
    };
    let luma = |c: Vec3| c.dot(Vec3::new(0.299, 0.587, 0.114));
    let leaf_flags = bsp.leafs.get(leaf).map_or(0, |l| l.flags);
    let sky = bsp.world_lights.iter().filter(|l| l.kind == emit::SKYLIGHT).find_map(|l| {
        let scale = style(l.style);
        let toward = -l.normal.normalize_or_zero();
        (scale > 0.0 && leaf_flags & (LEAF_SKY | LEAF_SKY2D) != 0 && sees_sky(bsp, p, toward))
            .then(|| (luma(l.intensity) * scale, 1.0, toward, l, scale))
    });
    let mut candidates: Vec<(f32, f32, Vec3, &WorldLight, f32)> = bsp
        .world_lights
        .iter()
        .filter(|l| matches!(l.kind, emit::POINT | emit::SPOT | emit::SURFACE))
        .filter(|l| !(skip_baked && l.flags & IN_AMBIENT_CUBE != 0))
        .filter(|l| pvs.as_ref().is_none_or(|v| cluster_visible(v, l.cluster)))
        .filter_map(|l| {
            let scale = style(l.style);
            if scale <= 0.0 {
                return None;
            }
            light_at(l, p).map(|(atten, dir)| (luma(l.intensity) * scale * atten, atten, dir, l, scale))
        })
        .filter(|(score, _, _, l, _)| l.kind == emit::SURFACE || *score >= WORLD_LIGHT_MIN)
        // Lights hidden from the point give it nothing.
        .filter(|(_, _, _, l, _)| !blocked(l.origin, p))
        .chain(sky)
        .collect();
    candidates.sort_by(|a, b| b.0.total_cmp(&a.0));
    // Lights that do not make the cut still brighten the ambient cube.
    for (_, atten, dir, l, scale) in candidates.iter().skip(MAX_MODEL_LIGHTS) {
        for (face, axis) in ambient.iter_mut().zip(CUBE_AXES) {
            *face += l.intensity * (scale * atten * axis.dot(*dir).max(0.0));
        }
    }
    let lights = candidates.iter().take(MAX_MODEL_LIGHTS).map(|&(_, _, _, l, scale)| to_shader_light(l, scale)).collect();
    ModelLighting { ambient, lights }
}

/// The baked cube map the engine binds for a model's `env_cubemap` materials:
/// the sample nearest the model's lighting origin, by straight distance (no
/// visibility test), loaded in its HDR version when there is one.
pub fn nearest_cubemap(
    bsp: &Bsp,
    map_name: &str,
    p: Vec3,
    fs: &GameFs,
    textures: &mut TextureCache,
    images: &mut Assets<Image>,
) -> Option<Handle<Image>> {
    let cube = nearest_cubemap_path(bsp, map_name, p)?;
    textures
        .load(fs, images, &format!("{cube}.hdr.vtf"), false)
        .or_else(|| textures.load(fs, images, &format!("{cube}.vtf"), true))
        .map(|t| t.handle)
}

/// Path of the baked cube map closest to `p`, if the map has any. Ties go to
/// the earlier sample, as in the engine.
pub fn nearest_cubemap_path(bsp: &Bsp, map_name: &str, p: Vec3) -> Option<String> {
    let c = bsp.cubemaps.iter().min_by(|a, b| {
        let da = Vec3::new(a.origin[0] as f32, a.origin[1] as f32, a.origin[2] as f32).distance_squared(p);
        let db = Vec3::new(b.origin[0] as f32, b.origin[1] as f32, b.origin[2] as f32).distance_squared(p);
        da.total_cmp(&db)
    })?;
    Some(format!("materials/maps/{map_name}/c{}_{}_{}", c.origin[0], c.origin[1], c.origin[2]))
}
