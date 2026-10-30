//! Ropes and cables (`move_rope` / `keyframe_rope`). Every keyframe with a
//! `NextKey` hangs a rope from itself to that keyframe, using its own
//! settings.
//!
//! The shape comes from the game's rope simulation, run until it settles:
//! `Type` 0, 1 and 2 give 10, 4 and 2 nodes, spaced evenly on the straight
//! line to start with; springs between them are (distance + `Slack` - 100)
//! / (nodes - 1) long (integer division) and only pull. Each 0.02 s step
//! moves the nodes by verlet integration (velocity kept at 98%, gravity
//! -1500 units/s²) and then relaxes the springs three times, moving both
//! ends of an over-long spring halfway, pinning the end points after each
//! pass. The game settles new ropes for five seconds; so do we. A rope is
//! drawn as a Catmull-Rom spline through the nodes with `Subdiv` extra
//! points per segment, `Width` times `rope_smooth_enlarge` (1.4) wide,
//! facing the view and at least two pixels across
//! (`rope_min_pixel_diameter`), lit by the average of the ambient light
//! cube at each point.

use portal2_core::layers::SKYBOX_LAYER;
use std::collections::HashMap;

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::RenderLayers;
use bevy::mesh::{Indices, MeshVertexBufferLayoutRef, PrimitiveTopology};
use bevy::pbr::{MaterialPipeline, MaterialPipelineKey};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError};
use bevy::shader::ShaderRef;
use source_formats::bsp::{Bsp, Entity as MapEntity};

use super::sky::{SkyChecked, Skybox3d};
use super::{CurrentMap, MapLoaded, MapObject};
use portal2_core::sets::LoadMapSet;
use portal2_core::vfs::GameFs;
use portal2_render::materials::load_vmt;
use portal2_render::textures::TextureCache;

pub const ROPE_SHADER: &str = "embedded://portal2_render/shaders/rope.wesl";

const TIME_STEP: f32 = 0.02;
const SETTLE_TIME: f32 = 5.0;
const DAMPING: f32 = 0.98;
const GRAVITY: Vec3 = Vec3::new(0.0, 0.0, -1500.0);
const SPRING_PASSES: usize = 3;
/// Subtracted from distance + slack (`ROPESLACK_FUDGEFACTOR`).
const SLACK_FUDGE: i32 = -100;
/// `rope_min_pixel_diameter`.
const MIN_PIXELS: f32 = 2.0;
/// `rope_smooth_enlarge`: ropes are drawn this much wider than `Width`.
const ENLARGE: f32 = 1.4;

#[derive(Clone, Copy, Debug, Default, ShaderType)]
pub struct RopeParams {
    pub params: Vec4,
}

#[derive(Asset, TypePath, AsBindGroup, Clone, Debug)]
pub struct RopeMaterial {
    #[uniform(0)]
    pub params: RopeParams,
    #[texture(1)]
    #[sampler(2)]
    pub base: Option<Handle<Image>>,
}

impl Material for RopeMaterial {
    fn vertex_shader() -> ShaderRef {
        ROPE_SHADER.into()
    }

    fn fragment_shader() -> ShaderRef {
        ROPE_SHADER.into()
    }

    fn enable_prepass() -> bool {
        false
    }

    fn enable_shadows() -> bool {
        false
    }

    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}

pub struct RopePlugin;

impl Plugin for RopePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<RopeMaterial>::default()).add_systems(PreUpdate, spawn_ropes.after(LoadMapSet));
    }
}

/// Node count for a rope `Type`.
fn node_count(kind: i32) -> usize {
    match kind {
        0 => 10,
        1 => 4,
        _ => 2,
    }
}

/// The settled nodes of a rope from `start` to `end`.
pub fn settle(start: Vec3, end: Vec3, nodes: usize, slack: i32, lock_end: bool) -> Vec<Vec3> {
    let nodes = nodes.max(2);
    let length = start.distance(end) as i32;
    let rest = ((length + slack + SLACK_FUDGE) / (nodes as i32 - 1)).max(0) as f32;
    let mut pos: Vec<Vec3> = (0..nodes).map(|i| start.lerp(end, i as f32 / (nodes - 1) as f32)).collect();
    let mut prev = pos.clone();
    let steps = (SETTLE_TIME / TIME_STEP).ceil() as usize;
    for _ in 0..steps {
        for i in 0..nodes {
            let p = pos[i];
            pos[i] = p + (p - prev[i]) * DAMPING + GRAVITY * (TIME_STEP * TIME_STEP);
            prev[i] = p;
        }
        for _ in 0..SPRING_PASSES {
            for i in 0..nodes - 1 {
                let d = pos[i] - pos[i + 1];
                let len2 = d.length_squared();
                if len2 > rest * rest {
                    let fix = d * (1.0 - rest / len2.sqrt()) * 0.5;
                    pos[i] -= fix;
                    pos[i + 1] += fix;
                }
            }
            pos[0] = start;
            prev[0] = start;
            if lock_end {
                pos[nodes - 1] = end;
                prev[nodes - 1] = end;
            }
        }
    }
    pos
}

/// Catmull-Rom points through `nodes`, with `subdiv` extra points between
/// each pair.
fn spline(nodes: &[Vec3], subdiv: usize) -> Vec<Vec3> {
    let n = nodes.len();
    let at = |i: isize| nodes[i.clamp(0, n as isize - 1) as usize];
    let mut out = Vec::with_capacity((n - 1) * (subdiv + 1) + 1);
    for i in 0..n - 1 {
        let (p0, p1, p2, p3) = (at(i as isize - 1), at(i as isize), at(i as isize + 1), at(i as isize + 2));
        for k in 0..=subdiv {
            let t = k as f32 / (subdiv + 1) as f32;
            let (t2, t3) = (t * t, t * t * t);
            out.push(0.5 * (2.0 * p1 + (p2 - p0) * t + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2 + (3.0 * p1 - p0 - 3.0 * p2 + p3) * t3));
        }
    }
    out.push(nodes[n - 1]);
    out
}

#[derive(Default)]
struct RopeBatch {
    positions: Vec<[f32; 3]>,
    directions: Vec<[f32; 3]>,
    uv: Vec<[f32; 2]>,
    size: Vec<[f32; 2]>,
    colors: Vec<[f32; 4]>,
    indices: Vec<u32>,
}

impl RopeBatch {
    fn add(&mut self, points: &[Vec3], half_width: f32, texture_scale: f32, bsp: &Bsp) {
        let mut v = 0.0;
        for (i, &p) in points.iter().enumerate() {
            let prev = points[i.saturating_sub(1)];
            let next = points[(i + 1).min(points.len() - 1)];
            let dir = (next - prev).normalize_or(Vec3::Z);
            if i > 0 {
                v += p.distance(prev) * texture_scale / (half_width * 8.0).max(1.0);
            }
            let light = bsp.ambient_cube(p).map(|c| c.iter().copied().sum::<Vec3>() / 6.0).unwrap_or(Vec3::splat(0.2));
            let base = self.positions.len() as u32;
            for side in [-1.0f32, 1.0] {
                self.positions.push(p.to_array());
                self.directions.push(dir.to_array());
                self.uv.push([side, v]);
                self.size.push([half_width, 0.0]);
                self.colors.push([light.x, light.y, light.z, 1.0]);
            }
            if i + 1 < points.len() {
                self.indices.extend([base, base + 1, base + 3, base, base + 3, base + 2]);
            }
        }
    }

    fn into_mesh(self) -> Mesh {
        Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD)
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.positions)
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, self.directions)
            .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, self.uv)
            .with_inserted_attribute(Mesh::ATTRIBUTE_UV_1, self.size)
            .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, self.colors)
            .with_inserted_indices(Indices::U32(self.indices))
    }
}

fn is_rope(e: &MapEntity) -> bool {
    matches!(e.classname(), "move_rope" | "keyframe_rope")
}

fn spawn_ropes(
    mut loaded: MessageReader<MapLoaded>,
    map: Option<Res<CurrentMap>>,
    sky: Option<Res<Skybox3d>>,
    fs: Res<GameFs>,
    mut textures: ResMut<TextureCache>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<RopeMaterial>>,
    mut commands: Commands,
) {
    if loaded.read().last().is_none() {
        return;
    }
    let Some(map) = map else { return };
    let bsp = &map.bsp;
    let by_name: HashMap<String, &MapEntity> =
        bsp.entities.iter().filter(|e| is_rope(e)).filter_map(|e| Some((e.targetname()?.to_ascii_lowercase(), e))).collect();
    // (material, in the 3D sky box) -> batch
    let mut batches: HashMap<(String, bool), RopeBatch> = HashMap::new();
    let sky_area = sky.map(|s| s.area);
    for e in bsp.entities.iter().filter(|e| is_rope(e)) {
        let Some(next) = e.get("NextKey").and_then(|n| by_name.get(&n.to_ascii_lowercase())) else { continue };
        // Ropes riding on moving parents are not simulated.
        if e.get("parentname").is_some_and(|p| !p.is_empty()) || next.get("parentname").is_some_and(|p| !p.is_empty()) {
            continue;
        }
        let (start, end) = (e.origin(), next.origin());
        let nodes = node_count(e.get_i32("Type").unwrap_or(0));
        let slack = e.get_i32("Slack").unwrap_or(25);
        let lock_end = e.get_i32("Dangling").unwrap_or(0) == 0;
        let settled = settle(start, end, nodes, slack, lock_end);
        let subdiv = e.get_i32("Subdiv").unwrap_or(2).clamp(0, 8) as usize;
        let points = spline(&settled, subdiv);
        let half_width = e.get_f32("Width").unwrap_or(2.0).max(0.1) * 0.5 * ENLARGE;
        let texture_scale = e.get_f32("TextureScale").unwrap_or(1.0);
        let material = e.get("RopeMaterial").unwrap_or("cable/cable.vmt").to_ascii_lowercase();
        let mid = settled[settled.len() / 2];
        let in_sky = sky_area.is_some_and(|a| bsp.leafs.get(bsp.find_leaf(mid)).is_some_and(|l| l.area == a));
        batches.entry((material, in_sky)).or_default().add(&points, half_width, texture_scale, bsp);
    }
    let mut cache: HashMap<String, Handle<RopeMaterial>> = HashMap::new();
    for ((material, in_sky), batch) in batches {
        let handle = cache
            .entry(material.clone())
            .or_insert_with(|| {
                let path = format!("materials/{}", material.trim_start_matches("materials/"));
                let path = if path.ends_with(".vmt") { path } else { format!("{path}.vmt") };
                let base = load_vmt(&fs, &path)
                    .and_then(|vmt| vmt.texture_path("$basetexture"))
                    .and_then(|t| textures.load(&fs, &mut images, &t, true))
                    .map(|t| t.handle);
                materials.add(RopeMaterial { params: RopeParams { params: Vec4::new(MIN_PIXELS, 0.0, 0.0, 0.0) }, base })
            })
            .clone();
        let mut rope = commands.spawn((
            Name::new("ropes"),
            MapObject,
            SkyChecked,
            Mesh3d(meshes.add(batch.into_mesh())),
            MeshMaterial3d(handle),
            Transform::IDENTITY,
        ));
        if in_sky {
            rope.insert(RenderLayers::layer(SKYBOX_LAYER));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hangs_with_slack() {
        // 300 units apart with 200 slack: the middle sags well below the ends.
        let nodes = settle(Vec3::ZERO, Vec3::new(300.0, 0.0, 0.0), 10, 200, true);
        assert_eq!(nodes.first(), Some(&Vec3::ZERO));
        assert_eq!(nodes.last(), Some(&Vec3::new(300.0, 0.0, 0.0)));
        let sag = nodes[5].z;
        assert!(sag < -60.0 && sag > -200.0, "sag {sag}");
        // Springs never end up much longer than their rest length ((300 + 200 - 100) / 9 = 44).
        for w in nodes.windows(2) {
            assert!(w[0].distance(w[1]) < 46.0);
        }
    }

    #[test]
    fn tight_rope_stays_straight() {
        let nodes = settle(Vec3::ZERO, Vec3::new(500.0, 0.0, 0.0), 10, 0, true);
        assert!(nodes.iter().all(|p| p.z.abs() < 20.0));
    }
}
