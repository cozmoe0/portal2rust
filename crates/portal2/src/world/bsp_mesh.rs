//! Turns BSP faces and displacements into renderable triangle batches and
//! packs every face lightmap into one atlas.
//!
//! Bump-mapped faces carry four lightmaps (one unbumped plus three for the
//! tangent-space basis directions). They are packed side by side so the
//! shader can reach basis `k` by offsetting the lightmap UV by `k` blocks; the
//! per-vertex block width (in atlas UV units) travels in vertex colour `r`.

use std::collections::HashMap;

use bevy::math::{Vec2, Vec3};
use source_formats::bsp::{Bsp, Face, surf};

/// Faces with these flags are never drawn.
pub const SKIP_FLAGS: u32 = surf::NODRAW | surf::SKY | surf::SKY2D | surf::SKIP | surf::HINT | surf::TRIGGER;

#[derive(Default)]
pub struct MeshBatch {
    pub texdata_name: String,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub tangents: Vec<[f32; 4]>,
    pub uv0: Vec<[f32; 2]>,
    pub uv1: Vec<[f32; 2]>,
    /// r: bumped-lightmap block width in atlas UV, a: displacement blend alpha.
    pub colors: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
    pub lightmapped: bool,
    pub bumped: bool,
    pub has_blend: bool,
}

struct VertexIn {
    pos: Vec3,
    normal: Vec3,
    tangent: [f32; 4],
    uv0: Vec2,
    uv1: Vec2,
    bump_step: f32,
    alpha: f32,
}

impl MeshBatch {
    fn push_vertex(&mut self, v: VertexIn) -> u32 {
        let i = self.positions.len() as u32;
        self.positions.push(v.pos.to_array());
        self.normals.push(v.normal.to_array());
        self.tangents.push(v.tangent);
        self.uv0.push(v.uv0.to_array());
        self.uv1.push(v.uv1.to_array());
        self.colors.push([v.bump_step, 0.0, 0.0, v.alpha]);
        i
    }
}

pub struct LightmapAtlas {
    pub width: u32,
    pub height: u32,
    /// Linear RGB, row-major.
    pub texels: Vec<[f32; 3]>,
}

#[derive(Clone, Copy)]
struct AtlasRect {
    x: u32,
    y: u32,
    /// Width of one padded lightmap block.
    block_w: u32,
    bumped: bool,
}

/// Batches per brush model, keyed by texdata (material) index.
pub struct BrushModelMeshes {
    pub batches: Vec<MeshBatch>,
}

pub struct WorldGeometry {
    pub models: Vec<BrushModelMeshes>,
    pub atlas: LightmapAtlas,
}

fn face_is_drawn(bsp: &Bsp, face: &Face) -> bool {
    if face.texinfo < 0 {
        return false;
    }
    let ti = &bsp.texinfo[face.texinfo as usize];
    ti.flags & SKIP_FLAGS == 0
}

fn face_has_lightmap(bsp: &Bsp, face: &Face) -> bool {
    face.light_offset >= 0 && !bsp.lighting.is_empty() && bsp.texinfo[face.texinfo as usize].flags & surf::NOLIGHT == 0
}

fn face_is_bumped(bsp: &Bsp, face: &Face) -> bool {
    bsp.texinfo[face.texinfo as usize].flags & surf::BUMPLIGHT != 0
}

fn pack_lightmaps(bsp: &Bsp) -> (LightmapAtlas, HashMap<usize, AtlasRect>) {
    // (face, block width, total width, height, bumped)
    let mut items: Vec<(usize, u32, u32, u32, bool)> = Vec::new();
    let mut area = 0u64;
    for (i, face) in bsp.faces.iter().enumerate() {
        if !face_is_drawn(bsp, face) || !face_has_lightmap(bsp, face) {
            continue;
        }
        let bw = face.lightmap_size[0] as u32 + 1 + 2;
        let h = face.lightmap_size[1] as u32 + 1 + 2;
        let bumped = face_is_bumped(bsp, face);
        let w = if bumped { bw * 4 } else { bw };
        area += (w * h) as u64;
        items.push((i, bw, w, h, bumped));
    }
    items.sort_by(|a, b| b.3.cmp(&a.3).then(b.2.cmp(&a.2)));
    let mut width = 512u32;
    while (width as u64 * width as u64) < area * 2 && width < 8192 {
        width *= 2;
    }
    let mut rects = HashMap::new();
    let (mut x, mut y, mut row_h) = (0u32, 0u32, 0u32);
    for &(face, bw, w, h, bumped) in &items {
        if x + w > width {
            x = 0;
            y += row_h;
            row_h = 0;
        }
        rects.insert(face, AtlasRect { x, y, block_w: bw, bumped });
        x += w;
        row_h = row_h.max(h);
    }
    let height = (y + row_h).max(4).next_multiple_of(4);
    let mut texels = vec![[0.0f32; 3]; (width * height) as usize];
    for &(face_index, bw, _, h, bumped) in &items {
        let face = &bsp.faces[face_index];
        let rect = rects[&face_index];
        let (iw, ih) = (bw - 2, h - 2);
        for block in 0..if bumped { 4 } else { 1 } {
            let Some(samples) = bsp.face_lightmap(face, block) else { continue };
            let bx = rect.x + block as u32 * bw;
            for ty in 0..h {
                for tx in 0..bw {
                    // Clamp into the inner rect so the 1-texel border repeats the edge.
                    let sx = (tx as i32 - 1).clamp(0, iw as i32 - 1) as u32;
                    let sy = (ty as i32 - 1).clamp(0, ih as i32 - 1) as u32;
                    let s = samples[(sy * iw + sx) as usize];
                    texels[((rect.y + ty) * width + bx + tx) as usize] = s.to_array();
                }
            }
        }
    }
    (LightmapAtlas { width, height, texels }, rects)
}

struct FaceMapping {
    tex_s: [f32; 4],
    tex_t: [f32; 4],
    tex_size: Vec2,
    lm_s: [f32; 4],
    lm_t: [f32; 4],
    lm_mins: Vec2,
    rect: Option<AtlasRect>,
    atlas_size: Vec2,
}

impl FaceMapping {
    fn uv0(&self, p: Vec3) -> Vec2 {
        let s = p.x * self.tex_s[0] + p.y * self.tex_s[1] + p.z * self.tex_s[2] + self.tex_s[3];
        let t = p.x * self.tex_t[0] + p.y * self.tex_t[1] + p.z * self.tex_t[2] + self.tex_t[3];
        Vec2::new(s / self.tex_size.x, t / self.tex_size.y)
    }

    /// Luxel coordinates relative to the face's lightmap mins.
    fn luxel(&self, p: Vec3) -> Vec2 {
        let s = p.x * self.lm_s[0] + p.y * self.lm_s[1] + p.z * self.lm_s[2] + self.lm_s[3];
        let t = p.x * self.lm_t[0] + p.y * self.lm_t[1] + p.z * self.lm_t[2] + self.lm_t[3];
        Vec2::new(s, t) - self.lm_mins
    }

    fn luxel_to_uv1(&self, l: Vec2) -> Vec2 {
        match self.rect {
            Some(r) => (Vec2::new(r.x as f32 + 1.5, r.y as f32 + 1.5) + l) / self.atlas_size,
            None => Vec2::ZERO,
        }
    }

    fn bump_step(&self) -> f32 {
        match self.rect {
            Some(r) if r.bumped => r.block_w as f32 / self.atlas_size.x,
            _ => 0.0,
        }
    }

    /// Tangent along the texture S axis; `w` makes cross(N, T) * w point along +T.
    fn tangent(&self, normal: Vec3) -> [f32; 4] {
        let s = Vec3::new(self.tex_s[0], self.tex_s[1], self.tex_s[2]);
        let t = Vec3::new(self.tex_t[0], self.tex_t[1], self.tex_t[2]);
        let tangent = (s - normal * normal.dot(s)).normalize_or(Vec3::X);
        let w = if normal.cross(tangent).dot(t) >= 0.0 { 1.0 } else { -1.0 };
        [tangent.x, tangent.y, tangent.z, w]
    }
}

pub fn build_world_geometry(bsp: &Bsp) -> WorldGeometry {
    let (atlas, rects) = pack_lightmaps(bsp);
    let atlas_size = Vec2::new(atlas.width as f32, atlas.height as f32);
    let mut models = Vec::with_capacity(bsp.models.len());
    for model in &bsp.models {
        let mut batches: HashMap<i32, MeshBatch> = HashMap::new();
        for fi in model.first_face..model.first_face + model.num_faces {
            let fi = fi as usize;
            let face = &bsp.faces[fi];
            if !face_is_drawn(bsp, face) {
                continue;
            }
            let ti = &bsp.texinfo[face.texinfo as usize];
            let td = &bsp.texdata[ti.texdata as usize];
            let mapping = FaceMapping {
                tex_s: ti.texture_vecs[0],
                tex_t: ti.texture_vecs[1],
                tex_size: Vec2::new(td.width.max(1) as f32, td.height.max(1) as f32),
                lm_s: ti.lightmap_vecs[0],
                lm_t: ti.lightmap_vecs[1],
                lm_mins: Vec2::new(face.lightmap_mins[0] as f32, face.lightmap_mins[1] as f32),
                rect: rects.get(&fi).copied(),
                atlas_size,
            };
            let batch = batches.entry(ti.texdata).or_insert_with(|| MeshBatch {
                texdata_name: bsp.texdata_strings.get(td.name_index as usize).cloned().unwrap_or_default(),
                ..Default::default()
            });
            batch.lightmapped |= mapping.rect.is_some();
            batch.bumped |= mapping.rect.is_some_and(|r| r.bumped);
            if face.dispinfo >= 0 {
                build_displacement(bsp, face, face.dispinfo as usize, &mapping, batch);
            } else {
                build_face(bsp, face, &mapping, batch);
            }
        }
        let mut batches: Vec<MeshBatch> = batches.into_values().filter(|b| !b.indices.is_empty()).collect();
        batches.sort_by(|a, b| a.texdata_name.cmp(&b.texdata_name));
        models.push(BrushModelMeshes { batches });
    }
    WorldGeometry { models, atlas }
}

fn build_face(bsp: &Bsp, face: &Face, m: &FaceMapping, batch: &mut MeshBatch) {
    let verts = bsp.face_vertices(face);
    if verts.len() < 3 {
        return;
    }
    // The stored plane is the face's own plane (`side` only records how it
    // relates to the node plane), and vertices wind clockwise around it.
    let normal = bsp.planes[face.plane as usize].normal;
    let tangent = m.tangent(normal);
    let bump_step = m.bump_step();
    let base = batch.positions.len() as u32;
    for &p in &verts {
        batch.push_vertex(VertexIn {
            pos: p,
            normal,
            tangent,
            uv0: m.uv0(p),
            uv1: m.luxel_to_uv1(m.luxel(p)),
            bump_step,
            alpha: 1.0,
        });
    }
    // Source polygons wind clockwise seen from the front; Bevy front faces are CCW.
    if face.num_prims > 0 && !bsp.primitives.is_empty() {
        let mut used = false;
        for pi in face.first_prim..face.first_prim + face.num_prims {
            let Some(prim) = bsp.primitives.get(pi as usize) else { continue };
            if prim.vert_count != 0 {
                continue; // extra vertices are not used by Portal 2 maps
            }
            let idx = &bsp.prim_indices[prim.first_index as usize..(prim.first_index + prim.index_count) as usize];
            if prim.kind == 0 {
                for tri in idx.chunks_exact(3) {
                    batch.indices.extend([base + tri[0] as u32, base + tri[2] as u32, base + tri[1] as u32]);
                }
            } else {
                for i in 2..idx.len() {
                    let (a, b, c) = if i % 2 == 0 {
                        (idx[i - 2], idx[i - 1], idx[i])
                    } else {
                        (idx[i - 1], idx[i - 2], idx[i])
                    };
                    batch.indices.extend([base + a as u32, base + c as u32, base + b as u32]);
                }
            }
            used = true;
        }
        if used {
            return;
        }
    }
    for i in 1..verts.len() as u32 - 1 {
        batch.indices.extend([base, base + i + 1, base + i]);
    }
}

fn build_displacement(bsp: &Bsp, face: &Face, disp_index: usize, m: &FaceMapping, batch: &mut MeshBatch) {
    let disp = &bsp.disp_infos[disp_index];
    let corners = bsp.face_vertices(face);
    if corners.len() != 4 {
        return;
    }
    let start = (0..4)
        .min_by(|&a, &b| {
            corners[a]
                .distance_squared(disp.start_position)
                .total_cmp(&corners[b].distance_squared(disp.start_position))
        })
        .unwrap_or(0);
    let c = [corners[start], corners[(start + 1) % 4], corners[(start + 2) % 4], corners[(start + 3) % 4]];
    let lux = c.map(|p| m.luxel(p));
    let n = disp.side_verts();
    let denom = (n - 1) as f32;
    let mut pos = Vec::with_capacity(n * n);
    let mut base_pos = Vec::with_capacity(n * n);
    let mut lm = Vec::with_capacity(n * n);
    let mut alpha = Vec::with_capacity(n * n);
    for i in 0..n {
        let fi = i as f32 / denom;
        let a = c[0].lerp(c[1], fi);
        let b = c[3].lerp(c[2], fi);
        let la = lux[0].lerp(lux[1], fi);
        let lb = lux[3].lerp(lux[2], fi);
        for j in 0..n {
            let fj = j as f32 / denom;
            let p = a.lerp(b, fj);
            let dv = &bsp.disp_verts[disp.disp_vert_start as usize + i * n + j];
            base_pos.push(p);
            pos.push(p + dv.vec * dv.dist);
            lm.push(la.lerp(lb, fj));
            alpha.push((dv.alpha / 255.0).clamp(0.0, 1.0));
        }
    }
    if alpha.iter().any(|&a| a > 0.0) {
        batch.has_blend = true;
    }
    let face_normal = bsp.planes[face.plane as usize].normal;
    // Decide winding from the undisplaced grid so it matches the face orientation.
    let grid_normal = (base_pos[n] - base_pos[0]).cross(base_pos[1] - base_pos[0]);
    let flip = grid_normal.dot(face_normal) < 0.0;
    let bump_step = m.bump_step();

    let at = |i: usize, j: usize| pos[i * n + j];
    let base = batch.positions.len() as u32;
    for i in 0..n {
        for j in 0..n {
            let di = at((i + 1).min(n - 1), j) - at(i.saturating_sub(1), j);
            let dj = at(i, (j + 1).min(n - 1)) - at(i, j.saturating_sub(1));
            let mut nrm = di.cross(dj).normalize_or_zero();
            if flip {
                nrm = -nrm;
            }
            if nrm == Vec3::ZERO {
                nrm = face_normal;
            }
            let k = i * n + j;
            batch.push_vertex(VertexIn {
                pos: pos[k],
                normal: nrm,
                tangent: m.tangent(nrm),
                uv0: m.uv0(base_pos[k]),
                uv1: m.luxel_to_uv1(lm[k]),
                bump_step,
                alpha: alpha[k],
            });
        }
    }
    for i in 0..n - 1 {
        for j in 0..n - 1 {
            let v00 = base + (i * n + j) as u32;
            let v01 = v00 + 1;
            let v10 = base + ((i + 1) * n + j) as u32;
            let v11 = v10 + 1;
            let (t1, t2) = if (i + j) % 2 == 0 {
                ([v00, v10, v11], [v00, v11, v01])
            } else {
                ([v00, v10, v01], [v01, v10, v11])
            };
            // Both triangles are counter-clockwise around the grid normal
            // (row x column); flip them when that faces away from the surface.
            for t in [t1, t2] {
                if flip {
                    batch.indices.extend([t[0], t[2], t[1]]);
                } else {
                    batch.indices.extend([t[0], t[1], t[2]]);
                }
            }
        }
    }
}
