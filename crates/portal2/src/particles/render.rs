//! Drawing particle collections: a material per particle material file
//! (`SpriteCard` parameters, base texture and its sheet) and a mesh per
//! renderer, rebuilt every frame from the particles.
//!
//! Sheet frames are chosen the way the game does: each sequence is cut
//! into 512 samples over its length; a particle's sample is its age times
//! the animation rate times 512 (rates are sequence lengths per second, or
//! frames per second with `use animation rate as FPS`, or the particle's
//! lifetime with `animation_fit_lifetime`), held at the end of clamped
//! sequences and wrapped otherwise. Neighbouring frames are blended.

use std::collections::HashMap;
use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, MeshVertexAttribute, MeshVertexBufferLayoutRef, PrimitiveTopology, VertexAttributeValues};
use bevy::pbr::{MaterialPipeline, MaterialPipelineKey};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError, VertexFormat,
};
use bevy::shader::ShaderRef;
use source_formats::vtf::{Sheet, Vtf};

use super::defs::{RendererKind, field};
use super::sim::Collection;
use crate::render::materials::load_vmt;
use crate::render::textures::TextureCache;
use crate::vfs::GameFs;

pub const PARTICLE_SHADER: &str = "embedded://portal2/render/shaders/particle.wesl";

pub const ATTR_CORNER: MeshVertexAttribute = MeshVertexAttribute::new("Particle_Corner", 988_540_101, VertexFormat::Float32x4);
pub const ATTR_SHAPE: MeshVertexAttribute = MeshVertexAttribute::new("Particle_Shape", 988_540_102, VertexFormat::Float32x4);
pub const ATTR_RECT0: MeshVertexAttribute = MeshVertexAttribute::new("Particle_Rect0", 988_540_103, VertexFormat::Float32x4);
pub const ATTR_RECT1: MeshVertexAttribute = MeshVertexAttribute::new("Particle_Rect1", 988_540_104, VertexFormat::Float32x4);
pub const ATTR_RECT2: MeshVertexAttribute = MeshVertexAttribute::new("Particle_Rect2", 988_540_105, VertexFormat::Float32x4);
pub const ATTR_AXIS0: MeshVertexAttribute = MeshVertexAttribute::new("Particle_Axis0", 988_540_106, VertexFormat::Float32x3);
pub const ATTR_AXIS1: MeshVertexAttribute = MeshVertexAttribute::new("Particle_Axis1", 988_540_107, VertexFormat::Float32x3);

pub mod flags {
    pub const ADDITIVE: u32 = 1;
    pub const BLEND_FRAMES: u32 = 2;
    pub const DUAL_SEQUENCE: u32 = 4;
    pub const NO_FOG: u32 = 8;
    pub const MOD2X: u32 = 16;
}

const MODE_SCREEN: f32 = 0.0;
const MODE_SCREEN_Z: f32 = 1.0;
const MODE_WORLD: f32 = 2.0;
const MODE_TRAIL: f32 = 3.0;
const MODE_ROPE: f32 = 4.0;

/// Samples per sheet sequence.
const SAMPLES: i64 = 512;

#[derive(Clone, Copy, Debug, Default, ShaderType)]
pub struct ParticleParams {
    pub params: Vec4,
}

#[derive(Asset, TypePath, AsBindGroup, Clone, Debug)]
pub struct ParticleMaterial {
    #[uniform(0)]
    pub params: ParticleParams,
    #[texture(1)]
    #[sampler(2)]
    pub base: Option<Handle<Image>>,
    pub alpha_mode: AlphaMode,
}

impl Material for ParticleMaterial {
    fn vertex_shader() -> ShaderRef {
        PARTICLE_SHADER.into()
    }

    fn fragment_shader() -> ShaderRef {
        PARTICLE_SHADER.into()
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
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        layout: &MeshVertexBufferLayoutRef,
        _key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        let vertex = layout.0.get_layout(&[
            Mesh::ATTRIBUTE_POSITION.at_shader_location(0),
            ATTR_CORNER.at_shader_location(1),
            ATTR_SHAPE.at_shader_location(2),
            ATTR_RECT0.at_shader_location(3),
            ATTR_RECT1.at_shader_location(4),
            ATTR_RECT2.at_shader_location(5),
            ATTR_AXIS0.at_shader_location(6),
            ATTR_AXIS1.at_shader_location(7),
            Mesh::ATTRIBUTE_COLOR.at_shader_location(8),
        ])?;
        descriptor.vertex.buffers = vec![vertex];
        descriptor.primitive.cull_mode = None;
        if let Some(depth) = descriptor.depth_stencil.as_mut() {
            depth.depth_write_enabled = Some(false);
        }
        Ok(())
    }
}

/// One sheet sequence, by sample.
#[derive(Clone, Debug)]
struct SequenceSamples {
    clamp: bool,
    /// Frame time spans (start, length) and rectangles.
    frames: Vec<(f32, f32, [f32; 4])>,
    total: f32,
}

#[derive(Clone, Debug, Default)]
pub struct SheetSampler {
    sequences: HashMap<u32, SequenceSamples>,
    first: Option<u32>,
}

/// The whole texture.
const FULL: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

impl SheetSampler {
    pub fn new(sheet: &Sheet) -> Self {
        let mut sequences = HashMap::new();
        for s in &sheet.sequences {
            let mut t = 0.0;
            let frames = s
                .frames
                .iter()
                .map(|f| {
                    let start = t;
                    t += f.display_time.max(0.0);
                    (start, f.display_time.max(0.0), f.rects.first().copied().unwrap_or(FULL))
                })
                .collect::<Vec<_>>();
            sequences.insert(s.number, SequenceSamples { clamp: s.clamps(), total: t.max(1e-6), frames });
        }
        let first = sheet.sequences.first().map(|s| s.number);
        Self { sequences, first }
    }

    /// Frames in a sequence (its length in frame units).
    pub fn frame_span(&self, seq: u32) -> f32 {
        self.sequence(seq).map(|s| s.total).unwrap_or(1.0)
    }

    fn sequence(&self, seq: u32) -> Option<&SequenceSamples> {
        self.sequences.get(&seq).or_else(|| self.first.and_then(|f| self.sequences.get(&f)))
    }

    /// Rectangles of the frame at `cycles` sequence lengths in, the next
    /// frame and how far between them.
    pub fn sample(&self, seq: u32, cycles: f32) -> ([f32; 4], [f32; 4], f32) {
        let Some(s) = self.sequence(seq) else { return (FULL, FULL, 0.0) };
        if s.frames.len() <= 1 {
            let r = s.frames.first().map(|f| f.2).unwrap_or(FULL);
            return (r, r, 0.0);
        }
        let mut index = (cycles * SAMPLES as f32) as i64;
        if s.clamp {
            index = index.clamp(0, SAMPLES - 1);
        } else {
            index &= SAMPLES - 1;
        }
        let time = index as f32 / SAMPLES as f32 * s.total;
        let k = s.frames.iter().rposition(|f| f.0 <= time).unwrap_or(0);
        let (start, len, r0) = s.frames[k];
        let next = if k + 1 < s.frames.len() {
            k + 1
        } else if s.clamp {
            k
        } else {
            0
        };
        let blend = if len > 0.0 { ((time - start) / len).clamp(0.0, 1.0) } else { 0.0 };
        (r0, s.frames[next].2, blend)
    }

    /// `(sequence number, frame count)` for `Lifetime From Sequence`.
    pub fn frame_counts(&self) -> Vec<(u32, f32)> {
        self.sequences.iter().map(|(n, s)| (*n, s.frames.len() as f32)).collect()
    }
}

/// A loaded particle material.
#[derive(Clone)]
pub struct Look {
    pub material: Handle<ParticleMaterial>,
    pub sheet: Arc<SheetSampler>,
    pub additive: bool,
    /// Drawn with a shader this renderer does not have (refraction).
    pub hidden: bool,
}

/// Particle materials by name.
#[derive(Resource, Default)]
pub struct Looks {
    by_name: HashMap<String, Look>,
}

impl Looks {
    pub fn get(
        &mut self,
        name: &str,
        fs: &GameFs,
        textures: &mut TextureCache,
        images: &mut Assets<Image>,
        materials: &mut Assets<ParticleMaterial>,
    ) -> Look {
        if let Some(l) = self.by_name.get(name) {
            return l.clone();
        }
        let look = load_look(name, fs, textures, images, materials);
        self.by_name.insert(name.to_owned(), look.clone());
        look
    }

    pub fn clear(&mut self) {
        self.by_name.clear();
    }
}

fn load_look(
    name: &str,
    fs: &GameFs,
    textures: &mut TextureCache,
    images: &mut Assets<Image>,
    materials: &mut Assets<ParticleMaterial>,
) -> Look {
    let vmt = load_vmt(fs, &format!("materials/{name}.vmt"));
    let shader = vmt.as_ref().map(|v| v.shader.to_ascii_lowercase()).unwrap_or_default();
    let get = |k: &str| vmt.as_ref().is_some_and(|v| v.get_bool(k));
    // `$addself` adds the texture in, scaled by its amount.
    let add_self = vmt.as_ref().and_then(|v| v.get_f32("$addself")).unwrap_or(0.0);
    let additive = get("$additive") || add_self > 0.0;
    let mut f = 0u32;
    if additive {
        f |= flags::ADDITIVE;
    }
    // Frame blending is on unless the material turns it off.
    if vmt.as_ref().and_then(|v| v.get("$blendframes")).is_none_or(|b| b.trim() != "0") {
        f |= flags::BLEND_FRAMES;
    }
    if get("$dualsequence") {
        f |= flags::DUAL_SEQUENCE;
    }
    if get("$nofog") {
        f |= flags::NO_FOG;
    }
    if get("$mod2x") {
        f |= flags::MOD2X;
    }
    let mut overbright = vmt.as_ref().and_then(|v| v.get_f32("$overbrightfactor")).unwrap_or(1.0);
    if add_self > 0.0 && !get("$additive") {
        overbright *= add_self;
    }
    let blend_mode = vmt.as_ref().and_then(|v| v.get_f32("$sequence_blend_mode")).unwrap_or(0.0);
    let path = vmt.as_ref().and_then(|v| v.texture_path("$basetexture"));
    let srgb = !get("$nosrgb");
    let base = path.as_ref().and_then(|p| textures.load(fs, images, p, srgb)).map(|t| t.handle);
    let sheet = path
        .as_ref()
        .and_then(|p| fs.read(p))
        .and_then(|b| Vtf::parse(&b).ok())
        .and_then(|v| v.sheet)
        .map(|s| SheetSampler::new(&s))
        .unwrap_or_default();
    let material = materials.add(ParticleMaterial {
        params: ParticleParams { params: Vec4::new(overbright, f as f32, blend_mode, 0.0) },
        base,
        alpha_mode: if additive { AlphaMode::Add } else { AlphaMode::Blend },
    });
    Look {
        material,
        sheet: Arc::new(sheet),
        additive,
        hidden: shader == "refract" || vmt.is_none(),
    }
}

/// Vertex arrays for one renderer's particles.
#[derive(Default)]
pub struct Batch {
    pos: Vec<[f32; 3]>,
    corner: Vec<[f32; 4]>,
    shape: Vec<[f32; 4]>,
    rect0: Vec<[f32; 4]>,
    rect1: Vec<[f32; 4]>,
    rect2: Vec<[f32; 4]>,
    axis0: Vec<[f32; 3]>,
    axis1: Vec<[f32; 3]>,
    color: Vec<[f32; 4]>,
    indices: Vec<u32>,
}

const CORNERS: [[f32; 2]; 4] = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];

struct Vert {
    pos: Vec3,
    mode: f32,
    blend: f32,
    shape: [f32; 4],
    rects: [[f32; 4]; 3],
    axis0: Vec3,
    axis1: Vec3,
    color: [f32; 4],
}

impl Batch {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    fn quad(&mut self, v: &Vert) {
        let base = self.pos.len() as u32;
        for c in CORNERS {
            self.pos.push(v.pos.to_array());
            self.corner.push([c[0], c[1], v.mode, v.blend]);
            self.shape.push(v.shape);
            self.rect0.push(v.rects[0]);
            self.rect1.push(v.rects[1]);
            self.rect2.push(v.rects[2]);
            self.axis0.push(v.axis0.to_array());
            self.axis1.push(v.axis1.to_array());
            self.color.push(v.color);
        }
        self.indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    /// Writes the arrays into `mesh`, replacing what it held.
    pub fn write(self, mesh: &mut Mesh) {
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, self.pos);
        mesh.insert_attribute(ATTR_CORNER, VertexAttributeValues::Float32x4(self.corner));
        mesh.insert_attribute(ATTR_SHAPE, VertexAttributeValues::Float32x4(self.shape));
        mesh.insert_attribute(ATTR_RECT0, VertexAttributeValues::Float32x4(self.rect0));
        mesh.insert_attribute(ATTR_RECT1, VertexAttributeValues::Float32x4(self.rect1));
        mesh.insert_attribute(ATTR_RECT2, VertexAttributeValues::Float32x4(self.rect2));
        mesh.insert_attribute(ATTR_AXIS0, VertexAttributeValues::Float32x3(self.axis0));
        mesh.insert_attribute(ATTR_AXIS1, VertexAttributeValues::Float32x3(self.axis1));
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, self.color);
        mesh.insert_indices(Indices::U32(self.indices));
    }
}

/// A mesh with one invisible (zero-size) quad, until particles fill it.
pub fn empty_mesh() -> Mesh {
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD);
    let mut batch = Batch::default();
    batch.quad(&Vert {
        pos: Vec3::ZERO,
        mode: MODE_SCREEN,
        blend: 0.0,
        shape: [0.0; 4],
        rects: [FULL; 3],
        axis0: Vec3::ZERO,
        axis1: Vec3::ZERO,
        color: [0.0; 4],
    });
    batch.write(&mut mesh);
    mesh
}

/// How much of a renderer's visibility proxy the view sees, as alpha and
/// radius scales: the visible share is mapped from `input` to 0..1, which
/// picks between the scale minimum and maximum.
pub fn visibility_scale(v: &super::defs::Visibility, visible: f32) -> Seen {
    let span = v.input.1 - v.input.0;
    let t = if span.abs() < 1e-6 { if visible >= v.input.1 { 1.0 } else { 0.0 } } else { ((visible - v.input.0) / span).clamp(0.0, 1.0) };
    Seen {
        alpha: v.alpha.0 + (v.alpha.1 - v.alpha.0) * t,
        radius: v.radius_scale.0 + (v.radius_scale.1 - v.radius_scale.0) * t,
    }
}

/// What the view sees of a renderer's proxy (0..1), applied to alpha and radius.
#[derive(Clone, Copy, Debug)]
pub struct Seen {
    pub alpha: f32,
    pub radius: f32,
}

impl Default for Seen {
    fn default() -> Self {
        Self { alpha: 1.0, radius: 1.0 }
    }
}

/// Fills `batch` with renderer `r` of collection `c`, positions relative
/// to `origin`, sorted back to front from `eye` when the definition sorts.
pub fn build(batch: &mut Batch, c: &Collection, r: usize, look: &Look, origin: Vec3, eye: Vec3, seen: Seen) {
    let Some(renderer) = c.def.renderers.get(r) else { return };
    let p = &c.particles;
    if p.count == 0 {
        return;
    }
    let mut order: Vec<usize> = (0..p.count).collect();
    if c.def.sort && !look.additive {
        let d: Vec<f32> = (0..p.count).map(|i| p.v[field::POSITION][i].distance_squared(eye)).collect();
        order.sort_by(|a, b| d[*b].total_cmp(&d[*a]));
    }
    let sheet = &look.sheet;
    let colour = |i: usize| {
        let c = p.v[field::COLOR][i];
        [c.x, c.y, c.z, (p.s[field::ALPHA][i] * seen.alpha).clamp(0.0, 1.0)]
    };
    match renderer.kind {
        RendererKind::Sprites { rate, fit_lifetime, orientation, orientation_cp, second_rate, fps } => {
            let cp = c.cps[orientation_cp.clamp(0, 63) as usize];
            for &i in &order {
                let radius = p.s[field::RADIUS][i] * seen.radius;
                if radius <= 0.0 {
                    continue;
                }
                let age = p.age(i, c.time).max(0.0);
                let seq = p.s[field::SEQUENCE][i].max(0.0) as u32;
                let cycles = if fit_lifetime {
                    let life = p.s[field::LIFE][i];
                    if life > 0.0 { age / life } else { 0.0 }
                } else if fps {
                    age * rate / sheet.frame_span(seq)
                } else {
                    age * rate
                };
                let (r0, r1, blend) = sheet.sample(seq, cycles);
                let seq1 = p.s[field::SEQUENCE1][i].max(0.0) as u32;
                let (r2, _, _) = sheet.sample(seq1, age * second_rate);
                let roll = p.s[field::ROLL][i];
                let (mode, axis0, axis1) = match orientation {
                    1 => (MODE_SCREEN_Z, Vec3::ZERO, Vec3::ZERO),
                    2 | 3 => {
                        let (x, y) = if orientation == 2 && orientation_cp >= 0 {
                            (cp.forward, cp.left)
                        } else if orientation == 2 {
                            (Vec3::X, Vec3::Y)
                        } else {
                            let n = p.v[field::NORMAL][i].normalize_or(Vec3::Z);
                            let x = n.any_orthonormal_vector();
                            (x, n.cross(x))
                        };
                        let (s, co) = roll.sin_cos();
                        (MODE_WORLD, x * co + y * s, y * co - x * s)
                    }
                    _ => (MODE_SCREEN, Vec3::ZERO, Vec3::ZERO),
                };
                batch.quad(&Vert {
                    pos: p.v[field::POSITION][i] - origin,
                    mode,
                    blend,
                    shape: [radius, roll, 1.0, 0.0],
                    rects: [r0, r1, r2],
                    axis0,
                    axis1,
                    color: colour(i),
                });
            }
        }
        RendererKind::Trail { rate, length_fade_in, max_length, min_length, constrain_radius, ignore_dt, tail_scale } => {
            let per_second = if ignore_dt || c.dt <= 0.0 { 1.0 } else { 1.0 / c.dt };
            for &i in &order {
                let pos = p.v[field::POSITION][i];
                let back = p.v[field::PREV_POSITION][i] - pos;
                let age = p.age(i, c.time).max(0.0);
                let fade = if length_fade_in > 0.0 { (age / length_fade_in).min(1.0) } else { 1.0 };
                let mut length = back.length() * fade * per_second * p.s[field::TRAIL_LENGTH][i];
                if length <= 0.0 {
                    continue;
                }
                length = length.min(max_length);
                if min_length > length {
                    length = min_length;
                }
                let mut radius = p.s[field::RADIUS][i] * seen.radius;
                if constrain_radius {
                    radius = radius.min(length);
                }
                // Trails are a radius wide in all.
                let radius = radius * 0.5;
                let seq = p.s[field::SEQUENCE][i].max(0.0) as u32;
                let (r0, r1, blend) = sheet.sample(seq, age * rate);
                let tail = pos + back.normalize_or_zero() * length;
                batch.quad(&Vert {
                    pos: pos - origin,
                    mode: MODE_TRAIL,
                    blend,
                    shape: [radius, 0.0, tail_scale[3], 0.0],
                    rects: [r0, r1, r0],
                    axis0: tail - origin,
                    axis1: Vec3::new(tail_scale[0], tail_scale[1], tail_scale[2]),
                    color: colour(i),
                });
            }
        }
        RendererKind::Rope { subdivisions, texel_size, scroll_rate, offset } => {
            if p.count < 2 {
                return;
            }
            // Particles in creation order make the rope.
            let mut chain: Vec<usize> = (0..p.count).collect();
            chain.sort_by_key(|&i| p.id[i]);
            let pts: Vec<(Vec3, f32, [f32; 4])> =
                chain.iter().map(|&i| (p.v[field::POSITION][i], p.s[field::RADIUS][i] * seen.radius, colour(i))).collect();
            let mut v = offset + scroll_rate * c.time;
            let mut prev_point: Option<Vec3> = None;
            let mut prev_index: Option<u32> = None;
            let steps = subdivisions.max(1) as usize;
            for s in 0..pts.len() - 1 {
                let p0 = pts[s.saturating_sub(1)].0;
                let (p1, w1, c1) = pts[s];
                let (p2, w2, c2) = pts[s + 1];
                let p3 = pts[(s + 2).min(pts.len() - 1)].0;
                let last = s + 2 == pts.len();
                for k in 0..steps + usize::from(last) {
                    let t = k as f32 / steps as f32;
                    let point = catmull_rom(p0, p1, p2, p3, t);
                    let dir = catmull_rom(p0, p1, p2, p3, (t + 0.01).min(1.0)) - catmull_rom(p0, p1, p2, p3, (t - 0.01).max(0.0));
                    if let Some(prev) = prev_point {
                        v += prev.distance(point) / (texel_size * 16.0);
                    }
                    prev_point = Some(point);
                    let width = w1 + (w2 - w1) * t;
                    let col = [
                        c1[0] + (c2[0] - c1[0]) * t,
                        c1[1] + (c2[1] - c1[1]) * t,
                        c1[2] + (c2[2] - c1[2]) * t,
                        c1[3] + (c2[3] - c1[3]) * t,
                    ];
                    let base = batch.pos.len() as u32;
                    for side in [0.0f32, 1.0] {
                        batch.pos.push((point - origin).to_array());
                        batch.corner.push([side, 0.0, MODE_ROPE, 0.0]);
                        batch.shape.push([width, 0.0, v, 0.0]);
                        batch.rect0.push(FULL);
                        batch.rect1.push(FULL);
                        batch.rect2.push(FULL);
                        batch.axis0.push(dir.normalize_or(Vec3::Z).to_array());
                        batch.axis1.push([1.0; 3]);
                        batch.color.push(col);
                    }
                    if let Some(prev) = prev_index {
                        batch.indices.extend([prev, prev + 1, base + 1, prev, base + 1, base]);
                    }
                    prev_index = Some(base);
                }
            }
        }
        RendererKind::Blobs | RendererKind::Unsupported(_) => {}
    }
}

fn catmull_rom(p0: Vec3, p1: Vec3, p2: Vec3, p3: Vec3, t: f32) -> Vec3 {
    let t2 = t * t;
    let t3 = t2 * t;
    0.5 * ((2.0 * p1) + (-p0 + p2) * t + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2 + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3)
}

#[cfg(test)]
mod tests {
    use super::*;
    use source_formats::vtf::{SheetFrame, SheetSequence};

    fn sheet(clamp: bool) -> SheetSampler {
        let frame = |u: f32| SheetFrame { display_time: 1.0, rects: vec![[u, 0.0, u + 0.25, 1.0]] };
        SheetSampler::new(&Sheet {
            sequences: vec![SheetSequence {
                number: 3,
                flags: u32::from(clamp),
                total_time: 4.0,
                frames: vec![frame(0.0), frame(0.25), frame(0.5), frame(0.75)],
            }],
        })
    }

    #[test]
    fn samples_walk_frames_and_wrap() {
        let s = sheet(false);
        let (r0, r1, b) = s.sample(3, 0.0);
        assert_eq!((r0[0], r1[0], b), (0.0, 0.25, 0.0));
        let (r0, r1, b) = s.sample(3, 0.375);
        assert_eq!((r0[0], r1[0]), (0.25, 0.5));
        assert!((b - 0.5).abs() < 0.01);
        // The last frame blends into the first again; a full cycle wraps.
        let (r0, r1, _) = s.sample(3, 0.9);
        assert_eq!((r0[0], r1[0]), (0.75, 0.0));
        assert_eq!(s.sample(3, 1.0).0[0], 0.0);
    }

    #[test]
    fn clamped_sequences_hold_the_last_frame() {
        let s = sheet(true);
        let (r0, r1, _) = s.sample(3, 7.0);
        assert_eq!((r0[0], r1[0]), (0.75, 0.75));
        // Unknown sequence numbers use the first sequence.
        assert_eq!(s.sample(9, 0.0).0[0], 0.0);
    }
}
