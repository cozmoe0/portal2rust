//! Lighting for dynamic models, after the engine's light cache: the leaf
//! ambient cube at the model plus its strongest few world lights. Lights the
//! compiler already baked into the ambient samples are left out, and visible
//! lights beyond the strongest few are folded into the ambient cube.

use bevy::prelude::*;
use source_formats::bsp::{Bsp, WorldLight};

use crate::render::source_material::{ModelLight, ModelParams};

/// Local lights per model (`r_worldlights`, 3 by default).
pub const MAX_MODEL_LIGHTS: usize = 3;

/// Weakest light worth considering (`r_worldlightmin`); surface lights are exempt.
const WORLD_LIGHT_MIN: f32 = 0.0002;

/// World light emit types as stored in the BSP.
mod emit {
    pub const SURFACE: i32 = 0;
    pub const POINT: i32 = 1;
    pub const SPOT: i32 = 2;
}

/// World light flag: the light is already part of the leaf ambient samples.
const IN_AMBIENT_CUBE: i32 = 1;

/// Unit directions of the ambient cube faces (+X, -X, +Y, -Y, +Z, -Z).
const CUBE_AXES: [Vec3; 6] = [Vec3::X, Vec3::NEG_X, Vec3::Y, Vec3::NEG_Y, Vec3::Z, Vec3::NEG_Z];

#[derive(Clone, Debug, Default)]
pub struct ModelLighting {
    pub ambient: [Vec3; 6],
    pub lights: Vec<ModelLight>,
    pub envmap: Option<Handle<Image>>,
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

fn to_shader_light(l: &WorldLight) -> ModelLight {
    // Surface lights are spot lights whose cone covers the hemisphere with a
    // cosine falloff.
    let (kind, inner, outer, exponent) = match l.kind {
        emit::SPOT => (1.0, l.stopdot, l.stopdot2, spot_exponent(l)),
        emit::SURFACE => (1.0, 1.0, 0.0, 1.0),
        _ => (0.0, 1.0, -1.0, 1.0),
    };
    ModelLight {
        position: l.origin.extend(kind),
        color: l.intensity.extend(exponent),
        direction: l.normal.extend(inner),
        attenuation: attenuation_terms(l).extend(outer),
    }
}

/// Computes lighting at `p` (usually the model's origin or illumination point).
pub fn compute(bsp: &Bsp, p: Vec3) -> ModelLighting {
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
    let mut candidates: Vec<(f32, f32, Vec3, &WorldLight)> = bsp
        .world_lights
        .iter()
        .filter(|l| matches!(l.kind, emit::POINT | emit::SPOT | emit::SURFACE))
        .filter(|l| !(skip_baked && l.flags & IN_AMBIENT_CUBE != 0))
        .filter(|l| pvs.as_ref().is_none_or(|v| cluster_visible(v, l.cluster)))
        .filter_map(|l| light_at(l, p).map(|(atten, dir)| (luma(l.intensity) * atten, atten, dir, l)))
        .filter(|(score, .., l)| l.kind == emit::SURFACE || *score >= WORLD_LIGHT_MIN)
        .collect();
    candidates.sort_by(|a, b| b.0.total_cmp(&a.0));
    // Lights that do not make the cut still brighten the ambient cube.
    for (_, atten, dir, l) in candidates.iter().skip(MAX_MODEL_LIGHTS) {
        for (face, axis) in ambient.iter_mut().zip(CUBE_AXES) {
            *face += l.intensity * (atten * axis.dot(*dir).max(0.0));
        }
    }
    let lights = candidates.iter().take(MAX_MODEL_LIGHTS).map(|(.., l)| to_shader_light(l)).collect();
    ModelLighting { ambient, lights, envmap: None }
}

/// Path of the baked cube map closest to `p`, if the map has any.
pub fn nearest_cubemap_path(bsp: &Bsp, map_name: &str, p: Vec3) -> Option<String> {
    let c = bsp.cubemaps.iter().min_by(|a, b| {
        let da = Vec3::new(a.origin[0] as f32, a.origin[1] as f32, a.origin[2] as f32).distance_squared(p);
        let db = Vec3::new(b.origin[0] as f32, b.origin[1] as f32, b.origin[2] as f32).distance_squared(p);
        da.total_cmp(&db)
    })?;
    Some(format!("materials/maps/{map_name}/c{}_{}_{}", c.origin[0], c.origin[1], c.origin[2]))
}
