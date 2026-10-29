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
    /// `info_overlay` decals, by material, in render order.
    pub overlays: Vec<MeshBatch>,
    /// World faces and overlays inside the 3D sky box's area (left out of
    /// `models[0]` and `overlays`).
    pub skybox: Vec<MeshBatch>,
    pub skybox_overlays: Vec<MeshBatch>,
    pub atlas: LightmapAtlas,
    /// Atlas texel holding luxel (0, 0) of each lightmapped face's flat
    /// lightmap, by face index (paint maps share this layout).
    pub face_luxels: HashMap<usize, [u32; 2]>,
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

fn face_mapping(bsp: &Bsp, fi: usize, rects: &HashMap<usize, AtlasRect>, atlas_size: Vec2) -> FaceMapping {
    let face = &bsp.faces[fi];
    let ti = &bsp.texinfo[face.texinfo as usize];
    let td = &bsp.texdata[ti.texdata as usize];
    FaceMapping {
        tex_s: ti.texture_vecs[0],
        tex_t: ti.texture_vecs[1],
        tex_size: Vec2::new(td.width.max(1) as f32, td.height.max(1) as f32),
        lm_s: ti.lightmap_vecs[0],
        lm_t: ti.lightmap_vecs[1],
        lm_mins: Vec2::new(face.lightmap_mins[0] as f32, face.lightmap_mins[1] as f32),
        rect: rects.get(&fi).copied(),
        atlas_size,
    }
}

/// Overlays, as the engine builds them: the overlay's quad lies in the plane
/// through its origin spanned by basis U (packed into the z of its first
/// three UV points) and V = normal x U; it is clipped against each face it
/// lists, in that plane, and the pieces are projected onto the faces along
/// the normal. Texture coordinates run from (U0, V0) at the first corner
/// through (U0, V1), (U1, V1) to (U1, V0); the pieces take the faces'
/// lightmaps. On displacements the quad is clipped against each triangle of
/// the undisplaced grid and the pieces follow the displaced surface.
fn build_overlays(
    bsp: &Bsp,
    rects: &HashMap<usize, AtlasRect>,
    atlas_size: Vec2,
    in_skybox: &dyn Fn(usize) -> bool,
) -> (Vec<MeshBatch>, Vec<MeshBatch>) {
    let mut batches: Vec<(u16, i32, bool, MeshBatch)> = Vec::new();
    for ov in &bsp.overlays {
        let Some(ti) = bsp.texinfo.get(ov.texinfo.max(0) as usize) else { continue };
        let Some(td) = bsp.texdata.get(ti.texdata as usize) else { continue };
        let n = ov.basis_normal.normalize_or_zero();
        let u = Vec3::new(ov.uv_points[0].z, ov.uv_points[1].z, ov.uv_points[2].z);
        if u == Vec3::ZERO || n == Vec3::ZERO {
            continue;
        }
        let v = n.cross(u).normalize_or_zero();
        let quad: [Vec2; 4] = std::array::from_fn(|i| ov.uv_points[i].truncate());
        let tex: [Vec2; 4] =
            [Vec2::new(ov.u[0], ov.v[0]), Vec2::new(ov.u[0], ov.v[1]), Vec2::new(ov.u[1], ov.v[1]), Vec2::new(ov.u[1], ov.v[0])];
        let sky = ov.faces.first().is_some_and(|&f| in_skybox(f.max(0) as usize));
        let key = (ov.render_order, ti.texdata, sky);
        let pos = match batches.iter().position(|(o, t, s, _)| (*o, *t, *s) == key) {
            Some(i) => i,
            None => {
                let name = bsp.texdata_strings.get(td.name_index as usize).cloned().unwrap_or_default();
                batches.push((key.0, key.1, key.2, MeshBatch { texdata_name: name, ..Default::default() }));
                batches.len() - 1
            }
        };
        let before = batches[pos].3.indices.len();
        for &fi in &ov.faces {
            let Some(face) = bsp.faces.get(fi.max(0) as usize) else { continue };
            if face.dispinfo >= 0 {
                let m = face_mapping(bsp, fi as usize, rects, atlas_size);
                let Some(g) = disp_grid(bsp, face, face.dispinfo as usize, &m) else { continue };
                let to2d = |p: Vec3| Vec2::new((p - ov.origin).dot(u), (p - ov.origin).dot(v));
                let batch = &mut batches[pos].3;
                batch.lightmapped |= m.rect.is_some();
                batch.bumped |= m.rect.is_some_and(|r| r.bumped);
                for t in &g.triangles {
                    let tri2 = t.map(|k| to2d(g.base_pos[k]));
                    let clipped = clip_convex(&quad, &tri2);
                    if clipped.len() < 3 {
                        continue;
                    }
                    let d = (tri2[1] - tri2[0]).perp_dot(tri2[2] - tri2[0]);
                    if d.abs() < 1e-6 {
                        continue;
                    }
                    let base = batch.positions.len() as u32;
                    for q in &clipped {
                        let w1 = (*q - tri2[0]).perp_dot(tri2[2] - tri2[0]) / d;
                        let w2 = (tri2[1] - tri2[0]).perp_dot(*q - tri2[0]) / d;
                        let w = [1.0 - w1 - w2, w1, w2];
                        let mix3 = |f: &dyn Fn(usize) -> Vec3| (0..3).map(|i| f(t[i]) * w[i]).sum::<Vec3>();
                        let p = mix3(&|k| g.pos[k]);
                        let nrm = mix3(&|k| g.normals[k]).normalize_or(Vec3::Z);
                        let lux = (0..3).map(|i| g.lm[t[i]] * w[i]).sum::<Vec2>();
                        batch.push_vertex(VertexIn {
                            pos: p,
                            normal: nrm,
                            tangent: m.tangent(nrm),
                            uv0: quad_texcoord(&quad, &tex, *q),
                            uv1: m.luxel_to_uv1(lux),
                            bump_step: m.bump_step(),
                            alpha: 1.0,
                        });
                    }
                    // Grid triangles are already front-facing for Bevy.
                    for i in 1..clipped.len() as u32 - 1 {
                        batch.indices.extend([base, base + i, base + i + 1]);
                    }
                }
                continue;
            }
            let verts = bsp.face_vertices(face);
            if verts.len() < 3 {
                continue;
            }
            let plane = &bsp.planes[face.plane as usize];
            let to2d = |p: Vec3| Vec2::new((p - ov.origin).dot(u), (p - ov.origin).dot(v));
            let poly: Vec<Vec2> = verts.iter().map(|&p| to2d(p)).collect();
            let clipped = clip_convex(&quad, &poly);
            if clipped.len() < 3 {
                continue;
            }
            let denom = plane.normal.dot(n);
            if denom.abs() < 1e-4 {
                continue;
            }
            let m = face_mapping(bsp, fi as usize, rects, atlas_size);
            let tangent = {
                let t = (u - plane.normal * plane.normal.dot(u)).normalize_or(Vec3::X);
                let w = if plane.normal.cross(t).dot(v) >= 0.0 { 1.0 } else { -1.0 };
                [t.x, t.y, t.z, w]
            };
            let batch = &mut batches[pos].3;
            batch.lightmapped |= m.rect.is_some();
            batch.bumped |= m.rect.is_some_and(|r| r.bumped);
            let base = batch.positions.len() as u32;
            for q in &clipped {
                let on_plane = ov.origin + u * q.x + v * q.y;
                let t = (plane.dist - plane.normal.dot(on_plane)) / denom;
                let p = on_plane + n * t;
                batch.push_vertex(VertexIn {
                    pos: p,
                    normal: plane.normal,
                    tangent,
                    uv0: quad_texcoord(&quad, &tex, *q),
                    uv1: m.luxel_to_uv1(m.luxel(p)),
                    bump_step: m.bump_step(),
                    alpha: 1.0,
                });
            }
            // The clipped polygon follows the face's winding (clockwise in
            // Source); flip it to Bevy's counter-clockwise fronts.
            for i in 1..clipped.len() as u32 - 1 {
                batch.indices.extend([base, base + i + 1, base + i]);
            }
        }
        log::trace!(
            "overlay {} {}: {} faces, {} triangles, origin {:?} normal {:?} u {:?} quad {:?}",
            ov.id,
            batches[pos].3.texdata_name,
            ov.faces.len(),
            (batches[pos].3.indices.len() - before) / 3,
            ov.origin,
            n,
            u,
            quad
        );
    }
    batches.sort_by_key(|(order, _, _, _)| *order);
    let (sky, world): (Vec<_>, Vec<_>) = batches.into_iter().filter(|(_, _, _, b)| !b.indices.is_empty()).partition(|(_, _, sky, _)| *sky);
    (world.into_iter().map(|(_, _, _, b)| b).collect(), sky.into_iter().map(|(_, _, _, b)| b).collect())
}

/// The BSP area of every face: from the leaves that list it, or for faces
/// no leaf lists (displacements, brush models), the area just in front of
/// the face's centre.
pub fn face_areas(bsp: &Bsp) -> Vec<u16> {
    let mut areas = vec![u16::MAX; bsp.faces.len()];
    for leaf in &bsp.leafs {
        let first = leaf.first_leaf_face as usize;
        for &lf in bsp.leaf_faces.get(first..first + leaf.num_leaf_faces as usize).unwrap_or(&[]) {
            if let Some(a) = areas.get_mut(lf as usize)
                && *a == u16::MAX
            {
                *a = leaf.area;
            }
        }
    }
    for (fi, face) in bsp.faces.iter().enumerate() {
        if areas[fi] != u16::MAX {
            continue;
        }
        let verts = bsp.face_vertices(face);
        if verts.is_empty() {
            continue;
        }
        let centre = verts.iter().copied().sum::<Vec3>() / verts.len() as f32;
        let normal = bsp.planes[face.plane as usize].normal;
        if let Some(leaf) = bsp.leafs.get(bsp.find_leaf(centre + normal * 2.0)) {
            areas[fi] = leaf.area;
        }
    }
    areas
}

/// Sutherland-Hodgman: `subject` clipped to the convex polygon `clip`
/// (either winding); the result winds like `clip`.
fn clip_convex(subject: &[Vec2], clip: &[Vec2]) -> Vec<Vec2> {
    let area: f32 = (0..clip.len()).map(|i| clip[i].perp_dot(clip[(i + 1) % clip.len()])).sum();
    let sign = if area >= 0.0 { 1.0 } else { -1.0 };
    let sub_area: f32 = (0..subject.len()).map(|i| subject[i].perp_dot(subject[(i + 1) % subject.len()])).sum();
    let mut out: Vec<Vec2> = if sub_area * sign >= 0.0 { subject.to_vec() } else { subject.iter().rev().copied().collect() };
    for i in 0..clip.len() {
        let (a, b) = (clip[i], clip[(i + 1) % clip.len()]);
        let inside = |p: Vec2| (b - a).perp_dot(p - a) * sign >= -1e-4;
        let input = std::mem::take(&mut out);
        for j in 0..input.len() {
            let (p, q) = (input[j], input[(j + 1) % input.len()]);
            let (pi, qi) = (inside(p), inside(q));
            if pi {
                out.push(p);
            }
            if pi != qi {
                let d = (b - a).perp_dot(q - p);
                if d.abs() > 1e-9 {
                    let t = (b - a).perp_dot(a - p) / d;
                    out.push(p + (q - p) * t);
                }
            }
        }
        if out.len() < 3 {
            return Vec::new();
        }
    }
    out
}

/// Texture coordinate at `p` inside the overlay quad, by barycentric
/// interpolation over its two triangles (0,1,2) and (0,2,3).
fn quad_texcoord(quad: &[Vec2; 4], tex: &[Vec2; 4], p: Vec2) -> Vec2 {
    let bary = |a: Vec2, b: Vec2, c: Vec2| {
        let d = (b - a).perp_dot(c - a);
        if d.abs() < 1e-9 {
            return None;
        }
        let w1 = (p - a).perp_dot(c - a) / d;
        let w2 = (b - a).perp_dot(p - a) / d;
        Some((1.0 - w1 - w2, w1, w2))
    };
    let tri = |i: [usize; 3]| bary(quad[i[0]], quad[i[1]], quad[i[2]]).map(|w| (i, w));
    let first = tri([0, 1, 2]);
    let pick = match (first, tri([0, 2, 3])) {
        (Some((i, w)), _) if w.0 >= -1e-3 && w.1 >= -1e-3 && w.2 >= -1e-3 => Some((i, w)),
        (_, Some(second)) => Some(second),
        (f, None) => f,
    };
    match pick {
        Some((i, (w0, w1, w2))) => tex[i[0]] * w0 + tex[i[1]] * w1 + tex[i[2]] * w2,
        None => tex[0],
    }
}

/// Builds every brush model's batches; with `skybox_area`, the world faces
/// and overlays in that BSP area go to `skybox` / `skybox_overlays`.
pub fn build_world_geometry(bsp: &Bsp, skybox_area: Option<u16>) -> WorldGeometry {
    let (atlas, rects) = pack_lightmaps(bsp);
    let atlas_size = Vec2::new(atlas.width as f32, atlas.height as f32);
    let areas = skybox_area.map(|_| face_areas(bsp)).unwrap_or_default();
    let in_skybox = |fi: usize| skybox_area.is_some_and(|a| areas.get(fi) == Some(&a));
    let mut models = Vec::with_capacity(bsp.models.len());
    let mut sky_batches: HashMap<i32, MeshBatch> = HashMap::new();
    for (mi, model) in bsp.models.iter().enumerate() {
        let mut batches: HashMap<i32, MeshBatch> = HashMap::new();
        for fi in model.first_face..model.first_face + model.num_faces {
            let fi = fi as usize;
            let face = &bsp.faces[fi];
            if !face_is_drawn(bsp, face) {
                continue;
            }
            let target = if mi == 0 && in_skybox(fi) { &mut sky_batches } else { &mut batches };
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
            let batch = target.entry(ti.texdata).or_insert_with(|| MeshBatch {
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
    let (overlays, skybox_overlays) = build_overlays(bsp, &rects, atlas_size, &in_skybox);
    let mut skybox: Vec<MeshBatch> = sky_batches.into_values().filter(|b| !b.indices.is_empty()).collect();
    skybox.sort_by(|a, b| a.texdata_name.cmp(&b.texdata_name));
    let face_luxels = rects.iter().map(|(&f, r)| (f, [r.x + 1, r.y + 1])).collect();
    WorldGeometry { models, overlays, skybox, skybox_overlays, atlas, face_luxels }
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
        batch.push_vertex(VertexIn { pos: p, normal, tangent, uv0: m.uv0(p), uv1: m.luxel_to_uv1(m.luxel(p)), bump_step, alpha: 1.0 });
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
                    let (a, b, c) = if i % 2 == 0 { (idx[i - 2], idx[i - 1], idx[i]) } else { (idx[i - 1], idx[i - 2], idx[i]) };
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

/// A displacement's vertex grid: undisplaced and displaced positions,
/// lightmap luxels, blend alpha, normals, and its triangles (grid indices,
/// counter-clockwise from the front).
struct DispGrid {
    base_pos: Vec<Vec3>,
    pos: Vec<Vec3>,
    lm: Vec<Vec2>,
    alpha: Vec<f32>,
    normals: Vec<Vec3>,
    triangles: Vec<[usize; 3]>,
}

fn disp_grid(bsp: &Bsp, face: &Face, disp_index: usize, m: &FaceMapping) -> Option<DispGrid> {
    let disp = &bsp.disp_infos[disp_index];
    let corners = bsp.face_vertices(face);
    if corners.len() != 4 {
        return None;
    }
    let start = (0..4)
        .min_by(|&a, &b| corners[a].distance_squared(disp.start_position).total_cmp(&corners[b].distance_squared(disp.start_position)))
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
    let face_normal = bsp.planes[face.plane as usize].normal;
    // Decide winding from the undisplaced grid so it matches the face orientation.
    let grid_normal = (base_pos[n] - base_pos[0]).cross(base_pos[1] - base_pos[0]);
    let flip = grid_normal.dot(face_normal) < 0.0;
    let at = |i: usize, j: usize| pos[i * n + j];
    let mut normals = Vec::with_capacity(n * n);
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
            normals.push(nrm);
        }
    }
    let mut triangles = Vec::with_capacity((n - 1) * (n - 1) * 2);
    for i in 0..n - 1 {
        for j in 0..n - 1 {
            let v00 = i * n + j;
            let v01 = v00 + 1;
            let v10 = (i + 1) * n + j;
            let v11 = v10 + 1;
            let (t1, t2) = if (i + j) % 2 == 0 { ([v00, v10, v11], [v00, v11, v01]) } else { ([v00, v10, v01], [v01, v10, v11]) };
            // Both triangles are counter-clockwise around the grid normal
            // (row x column); flip them when that faces away from the surface.
            for t in [t1, t2] {
                triangles.push(if flip { [t[0], t[2], t[1]] } else { t });
            }
        }
    }
    Some(DispGrid { base_pos, pos, lm, alpha, normals, triangles })
}

fn build_displacement(bsp: &Bsp, face: &Face, disp_index: usize, m: &FaceMapping, batch: &mut MeshBatch) {
    let Some(g) = disp_grid(bsp, face, disp_index, m) else { return };
    if g.alpha.iter().any(|&a| a > 0.0) {
        batch.has_blend = true;
    }
    let bump_step = m.bump_step();
    let base = batch.positions.len() as u32;
    for k in 0..g.pos.len() {
        batch.push_vertex(VertexIn {
            pos: g.pos[k],
            normal: g.normals[k],
            tangent: m.tangent(g.normals[k]),
            uv0: m.uv0(g.base_pos[k]),
            uv1: m.luxel_to_uv1(g.lm[k]),
            bump_step,
            alpha: g.alpha[k],
        });
    }
    for t in &g.triangles {
        batch.indices.extend(t.map(|k| base + k as u32));
    }
}
