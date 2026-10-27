//! Lighting for dynamic models: the baked ambient cube nearest the model plus
//! the strongest few world lights that are potentially visible from it.

use bevy::prelude::*;
use source_formats::bsp::{Bsp, WorldLight};

use crate::render::source_material::{ModelLight, ModelParams};

pub const MAX_MODEL_LIGHTS: usize = 4;

/// World light emit types as stored in the BSP.
mod emit {
    pub const SURFACE: i32 = 0;
    pub const POINT: i32 = 1;
    pub const SPOT: i32 = 2;
}

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
        params.num_lights = self.lights.len().min(MAX_MODEL_LIGHTS) as u32;
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

/// Estimated light reaching `p` from `l`, ignoring surface orientation.
fn light_strength(l: &WorldLight, p: Vec3) -> f32 {
    let d = l.origin - p;
    let dist = d.length().max(1.0);
    if l.radius > 0.0 && dist > l.radius {
        return 0.0;
    }
    let mut atten = 1.0 / (l.constant_attn + l.linear_attn * dist + l.quadratic_attn * dist * dist).max(1e-4);
    let dir = d / dist;
    match l.kind {
        emit::SPOT => {
            let dot = (-dir).dot(l.normal);
            if dot < l.stopdot2 {
                return 0.0;
            }
            if dot < l.stopdot {
                atten *= (dot - l.stopdot2) / (l.stopdot - l.stopdot2).max(1e-4);
            }
        }
        emit::SURFACE => atten *= (-dir).dot(l.normal).max(0.0),
        _ => {}
    }
    let i = l.intensity;
    (i.x * 0.2125 + i.y * 0.7154 + i.z * 0.0721) * atten
}

fn to_shader_light(l: &WorldLight) -> ModelLight {
    let (kind, inner, outer) = match l.kind {
        emit::SPOT => (1.0, l.stopdot, l.stopdot2),
        // Surface lights radiate over a hemisphere with cosine falloff.
        emit::SURFACE => (1.0, 1.0, 0.0),
        _ => (0.0, 1.0, -1.0),
    };
    ModelLight {
        position: l.origin.extend(kind),
        color: l.intensity.extend(0.0),
        direction: l.normal.extend(inner),
        attenuation: Vec4::new(l.constant_attn, l.linear_attn, l.quadratic_attn, outer),
    }
}

/// Computes lighting at `p` (usually the model's origin or illumination point).
pub fn compute(bsp: &Bsp, p: Vec3) -> ModelLighting {
    let ambient = bsp.ambient_cube(p).unwrap_or([Vec3::splat(0.02); 6]);
    let leaf = bsp.find_leaf(p);
    let pvs = match (&bsp.visibility, bsp.leafs.get(leaf)) {
        (Some(vis), Some(l)) if l.cluster >= 0 => Some(vis.pvs(l.cluster as usize)),
        _ => None,
    };
    let mut candidates: Vec<(f32, &WorldLight)> = bsp
        .world_lights
        .iter()
        .filter(|l| matches!(l.kind, emit::POINT | emit::SPOT | emit::SURFACE))
        .filter(|l| pvs.as_ref().is_none_or(|v| cluster_visible(v, l.cluster)))
        .map(|l| (light_strength(l, p), l))
        .filter(|(s, _)| *s > 1e-4)
        .collect();
    candidates.sort_by(|a, b| b.0.total_cmp(&a.0));
    let lights = candidates.iter().take(MAX_MODEL_LIGHTS).map(|(_, l)| to_shader_light(l)).collect();
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
