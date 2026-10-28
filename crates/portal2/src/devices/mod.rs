//! Puzzle devices: lasers and their receivers, light bridges, excursion
//! funnels, faith plates, turrets and security cameras. Shared here: tracing beams that pass
//! through linked portals, and the glowing effect material.

pub mod bridge;
pub mod catapult;
pub mod funnel;
pub mod laser;
pub mod security_camera;
pub mod turret;

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;

use crate::collision::Physics;
use crate::portals::placement::{PortalFrame, transfer};
use crate::portals::{Portal, Portals};

pub const EFFECT_SHADER: &str = "embedded://portal2/render/shaders/effect.wesl";

pub struct DevicesPlugin;

impl Plugin for DevicesPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<EffectMaterial>::default())
            .add_systems(Update, animate_effects)
            .add_plugins((
                laser::LaserPlugin,
                bridge::BridgePlugin,
                funnel::FunnelPlugin,
                catapult::CatapultPlugin,
                turret::TurretPlugin,
                security_camera::SecurityCameraPlugin,
            ));
    }
}

#[derive(Clone, Copy, Debug, Default, ShaderType)]
pub struct EffectParams {
    pub color: Vec4,
    pub params: Vec4,
}

#[derive(Asset, TypePath, AsBindGroup, Clone, Debug)]
pub struct EffectMaterial {
    #[uniform(0)]
    pub effect: EffectParams,
    pub alpha_mode: AlphaMode,
}

impl EffectMaterial {
    /// Additive glow (lasers, tracers).
    pub fn glow(color: Vec3, scroll: f32, density: f32) -> Self {
        Self {
            // Zero alpha: Bevy's additive mode is premultiplied blending.
            effect: EffectParams { color: color.extend(0.0), params: Vec4::new(0.0, scroll, density, 0.6) },
            alpha_mode: AlphaMode::Add,
        }
    }

    /// See-through surface (bridges, funnels).
    pub fn translucent(color: Vec3, opacity: f32, scroll: f32, density: f32) -> Self {
        Self {
            effect: EffectParams { color: color.extend(opacity), params: Vec4::new(0.0, scroll, density, 0.15) },
            alpha_mode: AlphaMode::Blend,
        }
    }
}

impl Material for EffectMaterial {
    fn fragment_shader() -> ShaderRef {
        EFFECT_SHADER.into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        self.alpha_mode
    }

    fn enable_prepass() -> bool {
        false
    }

    fn enable_shadows() -> bool {
        false
    }

    fn specialize(
        _pipeline: &bevy::pbr::MaterialPipeline,
        descriptor: &mut bevy::render::render_resource::RenderPipelineDescriptor,
        _layout: &bevy::mesh::MeshVertexBufferLayoutRef,
        _key: bevy::pbr::MaterialPipelineKey<Self>,
    ) -> Result<(), bevy::render::render_resource::SpecializedMeshPipelineError> {
        // Effects are visible from both sides.
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}

fn animate_effects(time: Res<Time>, mut materials: ResMut<Assets<EffectMaterial>>) {
    let t = time.elapsed_secs();
    for (_, m) in materials.iter_mut() {
        m.effect.params.x = t;
    }
}

/// One straight piece of a beam.
#[derive(Clone, Copy, Debug)]
pub struct BeamSegment {
    pub start: Vec3,
    pub end: Vec3,
    pub dir: Vec3,
    /// Entity hit at `end`, if any (None for world or portals).
    pub hit_entity: Option<Entity>,
    /// Whether the segment ended by entering a portal.
    pub through_portal: bool,
}

/// An entity and everything it is mounted on: beams never hit their own
/// emitter or the brushes carrying it.
pub fn with_ancestors(e: Entity, parents: &Query<&ChildOf>) -> Vec<Entity> {
    let mut out = vec![e];
    let mut cur = e;
    while let Ok(p) = parents.get(cur) {
        cur = p.parent();
        out.push(cur);
    }
    out
}

/// Linked portal frames, if both portals exist.
pub fn linked_frames(portals: &Portals, query: &Query<&Portal>) -> Option<[PortalFrame; 2]> {
    let a = query.get(portals.entities[0]?).ok()?;
    let b = query.get(portals.entities[1]?).ok()?;
    Some([a.frame, b.frame])
}

/// Traces a beam from `start` along `dir`, continuing out of the linked
/// portal whenever it lands inside an opening. `redirect` may continue the
/// beam from an entity it hits (reflection cubes); it returns the new start
/// and direction.
pub fn trace_beam(
    physics: &Physics,
    frames: Option<[PortalFrame; 2]>,
    start: Vec3,
    dir: Vec3,
    mask: u32,
    ignore: &[Entity],
    max_segments: usize,
    mut redirect: impl FnMut(Entity) -> Option<(Vec3, Vec3)>,
) -> Vec<BeamSegment> {
    let mut out = Vec::new();
    let mut start = start;
    let mut dir = dir.normalize_or(Vec3::X);
    let mut skip: Vec<Entity> = ignore.to_vec();
    for _ in 0..max_segments {
        let tr = physics.trace(start, start + dir * 16384.0, Vec3::ZERO, Vec3::ZERO, mask, &skip);
        let end = tr.end;
        let mut seg = BeamSegment {
            start,
            end,
            dir,
            hit_entity: tr.entity,
            through_portal: false,
        };
        // Into an opening: continue from the other portal.
        if let Some(frames) = frames
            && tr.fraction < 1.0
            && let Some(slot) = (0..2).find(|&s| {
                let l = frames[s].to_local(end);
                l.x.abs() < 2.0 && frames[s].in_opening(l, 0.0) && dir.dot(frames[s].forward) < 0.0
            })
        {
            seg.through_portal = true;
            seg.hit_entity = None;
            out.push(seg);
            let m = transfer(&frames[slot], &frames[1 - slot]);
            dir = m.transform_vector3(dir).normalize_or(Vec3::X);
            start = m.transform_point3(end) + dir * 1.0;
            skip = ignore.to_vec();
            continue;
        }
        out.push(seg);
        let Some(hit) = tr.entity else { break };
        let Some((next_start, next_dir)) = redirect(hit) else { break };
        start = next_start;
        dir = next_dir.normalize_or(Vec3::X);
        // Do not immediately hit the redirecting entity again.
        skip = ignore.iter().copied().chain(std::iter::once(hit)).collect();
    }
    out
}

/// Unit cylinder along +Z from 0 to 1 with U around and V along its length.
pub fn tube_mesh(sides: u32) -> Mesh {
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut uvs = Vec::new();
    let mut indices = Vec::new();
    for i in 0..=sides {
        let a = i as f32 / sides as f32 * std::f32::consts::TAU;
        let (s, c) = a.sin_cos();
        for z in [0.0, 1.0] {
            positions.push([c, s, z]);
            normals.push([c, s, 0.0]);
            // U wraps twice around so the soft edge darkens the silhouette.
            uvs.push([(i as f32 / sides as f32 * 2.0).fract(), z]);
        }
    }
    for i in 0..sides {
        let b = i * 2;
        indices.extend([b, b + 2, b + 1, b + 1, b + 2, b + 3]);
    }
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
        .with_inserted_indices(Indices::U32(indices))
}

/// Transform placing a unit +Z mesh along a segment with the given width
/// (X) and height (Y) scale, rolled so local X stays horizontal.
pub fn segment_transform(start: Vec3, end: Vec3, width: f32, height: f32) -> Transform {
    let d = end - start;
    let len = d.length().max(0.01);
    let z = d / len;
    let x = if z.z.abs() > 0.99 { Vec3::X } else { Vec3::Z.cross(z).normalize() };
    let y = z.cross(x);
    Transform {
        translation: start,
        rotation: Quat::from_mat3(&Mat3::from_cols(x, y, z)),
        scale: Vec3::new(width, height, len),
    }
}
