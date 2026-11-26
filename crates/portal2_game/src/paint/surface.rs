//! The merged liquid surface paint blobs are drawn with, standing in for the
//! game's Blobulator (client.dll): every blob is a metaball, and nearby blobs
//! flow together into one surface, so a sprayer's stream reads as a pour and
//! a paint bomb's twenty blobs as one wobbling ball.
//!
//! The game hands the Blobulator each blob's centre, its radius `r` (blob
//! units) and a field scale `1 / r²`. Sizes are in units of
//! `paintblob_isosurface_box_width` (8): a lone blob of radius 1 shows
//! `r_paintblob_blr_render_radius` (1.3) of them across its radius, and its
//! field reaches `r_paintblob_blr_cutoff_radius` (5.5). Both are scaled by
//! `r_paintblob_blr_scale`. The surface is sampled every
//! `r_paintblob_highres_cube` (0.8) box widths in the main view.
//!
//! The Blobulator's own falloff is not ported. This uses the smooth
//! `(1 - d²/R²)²` metaball with the game's radii, with the surface where a
//! lone blob's field crosses at its render radius. The surface is built by
//! surface nets on a sparse grid, with normals from the field's gradient.

use bevy::platform::collections::HashMap;
use bevy::prelude::*;

/// One blob for the surface: centre, radius in blob units, linear colour.
#[derive(Clone, Copy, Debug)]
pub struct Particle {
    pub center: Vec3,
    pub radius: f32,
    pub color: [f32; 4],
}

/// Surface sizes (see the module notes).
#[derive(Clone, Copy, Debug)]
pub struct SurfaceParams {
    pub box_width: f32,
    pub render_radius: f32,
    pub cutoff_radius: f32,
    pub cell: f32,
}

impl SurfaceParams {
    pub fn from_console(console: &portal2_core::console::Console) -> Self {
        let scale = console.f32("r_paintblob_blr_scale").max(0.01);
        let box_width = console.f32("paintblob_isosurface_box_width").max(0.5) * scale;
        Self {
            box_width,
            render_radius: console.f32("r_paintblob_blr_render_radius").max(0.01),
            cutoff_radius: console.f32("r_paintblob_blr_cutoff_radius").max(0.02),
            cell: (console.f32("r_paintblob_highres_cube") * box_width).max(1.0),
        }
    }

    /// The field value the surface sits at.
    fn threshold(&self) -> f32 {
        let t = self.render_radius / self.cutoff_radius;
        let k = (1.0 - t * t).max(0.0);
        k * k
    }
}

/// A blob's field in world terms: centre, `1 / (r × box × cutoff)²`, reach.
#[derive(Clone, Copy)]
struct Source {
    center: Vec3,
    inv_reach2: f32,
    reach: f32,
    color: [f32; 4],
}

/// Triangles of the merged surface, in world space.
#[derive(Default)]
pub struct SurfaceMesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub colors: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
}

/// Blobs bucketed by position, for summing the field at a point.
struct Buckets {
    size: f32,
    map: HashMap<IVec3, Vec<u32>>,
}

impl Buckets {
    fn new(sources: &[Source]) -> Self {
        let size = sources.iter().map(|s| s.reach).fold(1.0f32, f32::max);
        let mut map: HashMap<IVec3, Vec<u32>> = HashMap::default();
        for (i, s) in sources.iter().enumerate() {
            map.entry((s.center / size).floor().as_ivec3()).or_default().push(i as u32);
        }
        Self { size, map }
    }

    #[cfg(test)]
    fn near(&self, p: Vec3) -> impl Iterator<Item = u32> + '_ {
        let c = (p / self.size).floor().as_ivec3();
        (-1..=1).flat_map(move |x| (-1..=1).flat_map(move |y| (-1..=1).map(move |z| c + IVec3::new(x, y, z))))
            .filter_map(|k| self.map.get(&k))
            .flatten()
            .copied()
    }
}

/// `(1 - x)²` for `x = d² / reach²` under 1.
fn kernel(x: f32) -> f32 {
    if x >= 1.0 { 0.0 } else { (1.0 - x) * (1.0 - x) }
}

#[cfg(test)]
fn field(sources: &[Source], buckets: &Buckets, p: Vec3) -> f32 {
    buckets
        .near(p)
        .map(|i| {
            let s = &sources[i as usize];
            kernel((p - s.center).length_squared() * s.inv_reach2)
        })
        .sum()
}

/// Outward normal and blended colour at `p`, from the blobs listed in
/// `near`.
fn shade(sources: &[Source], near: &[u32], p: Vec3) -> (Vec3, [f32; 4]) {
    let mut grad = Vec3::ZERO;
    let mut color = Vec4::ZERO;
    let mut weight = 0.0;
    for &i in near {
        let s = &sources[i as usize];
        let d = p - s.center;
        let x = d.length_squared() * s.inv_reach2;
        if x >= 1.0 {
            continue;
        }
        // d/dp (1 - x)² = -2 (1 - x) × 2 d / reach².
        grad += d * (-4.0 * (1.0 - x) * s.inv_reach2);
        let k = kernel(x);
        color += Vec4::from_array(s.color) * k;
        weight += k;
    }
    let color = if weight > 0.0 { (color / weight).to_array() } else { [1.0; 4] };
    ((-grad).normalize_or(Vec3::Z), color)
}

/// Grid points are sampled in bricks of 8³, each summing the field from
/// one short list of the blobs that reach it.
const BRICK: i32 = 8;
const BRICK_POINTS: usize = (BRICK * BRICK * BRICK) as usize;

struct Brick {
    values: Box<[f32; BRICK_POINTS]>,
    /// Which points are sampled (the rest count as outside).
    set: [u64; BRICK_POINTS / 64],
    /// How many of them are inside the liquid.
    inside: u32,
    /// The blobs reaching the brick or its cells' far corners.
    near: Vec<u32>,
}

impl Brick {
    fn new() -> Self {
        Self { values: Box::new([0.0; BRICK_POINTS]), set: [0; BRICK_POINTS / 64], inside: 0, near: Vec::new() }
    }

    fn get(&self, i: usize) -> f32 {
        if self.is_set(i) { self.values[i] } else { 0.0 }
    }

    fn is_set(&self, i: usize) -> bool {
        self.set[i >> 6] >> (i & 63) & 1 == 1
    }

    fn mark(&mut self, i: usize) {
        self.set[i >> 6] |= 1 << (i & 63);
    }
}

/// The brick holding grid point `k`, and the point's index in it.
fn split(k: IVec3) -> (IVec3, usize) {
    let b = k.div_euclid(IVec3::splat(BRICK));
    let l = k - b * BRICK;
    (b, (l.x + l.y * BRICK + l.z * BRICK * BRICK) as usize)
}

fn local(i: usize) -> IVec3 {
    let i = i as i32;
    IVec3::new(i % BRICK, (i / BRICK) % BRICK, i / (BRICK * BRICK))
}

/// A brick and the 26 around it, for reading grid points just past its
/// edges without a lookup each.
struct Around<'a> {
    base: IVec3,
    bricks: [Option<&'a Brick>; 27],
}

impl<'a> Around<'a> {
    fn new(bricks: &'a HashMap<IVec3, Brick>, b: IVec3) -> Self {
        let mut out = [None; 27];
        for (n, slot) in out.iter_mut().enumerate() {
            let o = IVec3::new(n as i32 % 3, (n as i32 / 3) % 3, n as i32 / 9) - IVec3::ONE;
            *slot = bricks.get(&(b + o));
        }
        Self { base: b * BRICK, bricks: out }
    }

    /// The field at grid point `k` (0 if not sampled), for points within one
    /// brick of this one.
    fn value(&self, k: IVec3) -> f32 {
        let l = k - self.base;
        let o = l.div_euclid(IVec3::splat(BRICK)) + IVec3::ONE;
        let Some(brick) = self.bricks[(o.x + o.y * 3 + o.z * 9) as usize] else { return 0.0 };
        let (_, i) = split(k);
        if brick.is_set(i) { brick.values[i] } else { 0.0 }
    }
}

/// Builds the merged surface of `particles`.
pub fn build(particles: &[Particle], params: &SurfaceParams) -> SurfaceMesh {
    let mut out = SurfaceMesh::default();
    if particles.is_empty() {
        return out;
    }
    let sources: Vec<Source> = particles
        .iter()
        .map(|p| {
            let reach = p.radius.clamp(0.001, 1.0) * params.box_width * params.cutoff_radius;
            Source { center: p.center, inv_reach2: 1.0 / (reach * reach), reach, color: p.color }
        })
        .collect();
    let buckets = Buckets::new(&sources);
    let threshold = params.threshold();
    let cell = params.cell;
    // Mark the grid around every blob, out to a little more than twice its
    // own drawn radius: far enough for the bridges between neighbours.
    let mut bricks: HashMap<IVec3, Brick> = HashMap::default();
    for p in particles {
        let r = p.radius.clamp(0.001, 1.0) * params.box_width;
        let bound = (r * params.render_radius * 2.2 + cell).min(r * params.cutoff_radius);
        let lo = ((p.center - bound) / cell).floor().as_ivec3();
        let hi = ((p.center + bound) / cell).ceil().as_ivec3();
        let (blo, bhi) = (lo.div_euclid(IVec3::splat(BRICK)), hi.div_euclid(IVec3::splat(BRICK)));
        for bx in blo.x..=bhi.x {
            for by in blo.y..=bhi.y {
                for bz in blo.z..=bhi.z {
                    let bb = IVec3::new(bx, by, bz);
                    let start = (bb * BRICK).max(lo);
                    let end = (bb * BRICK + (BRICK - 1)).min(hi);
                    let brick = bricks.entry(bb).or_insert_with(Brick::new);
                    for x in start.x..=end.x {
                        for y in start.y..=end.y {
                            for z in start.z..=end.z {
                                let k = IVec3::new(x, y, z);
                                if (k.as_vec3() * cell).distance_squared(p.center) <= bound * bound {
                                    brick.mark(split(k).1);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    // Sum the field at the marked points.
    let mut near: Vec<Source> = Vec::new();
    for (bb, brick) in bricks.iter_mut() {
        let min = (*bb * BRICK).as_vec3() * cell;
        // Its cells reach one point past its last row.
        let max = (*bb * BRICK + BRICK).as_vec3() * cell;
        near.clear();
        let (klo, khi) = (((min - buckets.size) / buckets.size).floor().as_ivec3(), ((max + buckets.size) / buckets.size).floor().as_ivec3());
        for x in klo.x..=khi.x {
            for y in klo.y..=khi.y {
                for z in klo.z..=khi.z {
                    let Some(list) = buckets.map.get(&IVec3::new(x, y, z)) else { continue };
                    for &i in list {
                        let s = sources[i as usize];
                        if s.center.clamp(min, max).distance_squared(s.center) < s.reach * s.reach {
                            near.push(s);
                            brick.near.push(i);
                        }
                    }
                }
            }
        }
        for i in 0..BRICK_POINTS {
            if !brick.is_set(i) {
                continue;
            }
            let q = (*bb * BRICK + local(i)).as_vec3() * cell;
            let v: f32 = near.iter().map(|s| kernel((q - s.center).length_squared() * s.inv_reach2)).sum();
            brick.values[i] = v;
            brick.inside += (v >= threshold) as u32;
        }
    }
    const CORNERS: [IVec3; 8] = [
        IVec3::new(0, 0, 0),
        IVec3::new(1, 0, 0),
        IVec3::new(0, 1, 0),
        IVec3::new(1, 1, 0),
        IVec3::new(0, 0, 1),
        IVec3::new(1, 0, 1),
        IVec3::new(0, 1, 1),
        IVec3::new(1, 1, 1),
    ];
    const EDGES: [(usize, usize); 12] =
        [(0, 1), (2, 3), (4, 5), (6, 7), (0, 2), (1, 3), (4, 6), (5, 7), (0, 4), (1, 5), (2, 6), (3, 7)];
    // One vertex per cell the surface passes through, at the mean of its
    // edge crossings. Every such cell has a sampled corner, so its lowest
    // corner's brick exists.
    let mut vertex_of: HashMap<IVec3, u32> = HashMap::default();
    const SIDE: usize = BRICK as usize + 1;
    let mut block = [0.0f32; SIDE * SIDE * SIDE];
    for (bb, brick) in &bricks {
        // The brick's cells reach one point into the bricks above it on
        // each axis; nothing to do unless some of those points are inside.
        let ahead: [Option<&Brick>; 8] = CORNERS.map(|o| bricks.get(&(*bb + o)));
        if ahead.iter().flatten().all(|b| b.inside == 0) {
            continue;
        }
        let n = BRICK as usize;
        let mut n_in = 0;
        for z in 0..SIDE {
            for y in 0..SIDE {
                for x in 0..SIDE {
                    let o = (x / n) + (y / n) * 2 + (z / n) * 4;
                    let i = (x % n) + (y % n) * n + (z % n) * n * n;
                    let v = ahead[o].map_or(0.0, |b| b.get(i));
                    n_in += (v >= threshold) as usize;
                    block[x + y * SIDE + z * SIDE * SIDE] = v;
                }
            }
        }
        if n_in == 0 || n_in == block.len() {
            continue;
        }
        for i in 0..BRICK_POINTS {
            let l = local(i);
            let c = *bb * BRICK + l;
            let v: [f32; 8] = CORNERS.map(|d| {
                let q = (l + d).as_uvec3();
                block[q.x as usize + q.y as usize * SIDE + q.z as usize * SIDE * SIDE]
            });
            let n_in = v.iter().filter(|x| **x >= threshold).count();
            if n_in == 0 || n_in == 8 {
                continue;
            }
            let mut sum = Vec3::ZERO;
            let mut n = 0.0;
            for (a, b) in EDGES {
                let (va, vb) = (v[a], v[b]);
                if (va >= threshold) == (vb >= threshold) {
                    continue;
                }
                let t = ((threshold - va) / (vb - va)).clamp(0.0, 1.0);
                sum += CORNERS[a].as_vec3().lerp(CORNERS[b].as_vec3(), t);
                n += 1.0;
            }
            let pos = (c.as_vec3() + sum / n) * cell;
            let (normal, color) = shade(&sources, &brick.near, pos);
            vertex_of.insert(c, out.positions.len() as u32);
            out.positions.push(pos.to_array());
            out.normals.push(normal.to_array());
            out.colors.push(color);
        }
    }
    // A quad across every grid edge the surface crosses, joining the four
    // cells around it, wound to face out of the liquid.
    let axes = [IVec3::X, IVec3::Y, IVec3::Z];
    for (bb, brick) in &bricks {
        if brick.inside == 0 {
            continue;
        }
        let around = Around::new(&bricks, *bb);
        for i in 0..BRICK_POINTS {
            if !brick.is_set(i) || brick.values[i] < threshold {
                continue;
            }
            let p = *bb * BRICK + local(i);
            for (ai, &a) in axes.iter().enumerate() {
                let (u, w) = (axes[(ai + 1) % 3], axes[(ai + 2) % 3]);
                for (lower, other, out_dir) in [(p, p + a, 1.0f32), (p - a, p - a, -1.0)] {
                    if around.value(other) >= threshold {
                        continue;
                    }
                    let cells = [lower, lower - u, lower - u - w, lower - w];
                    let Some(q) = cells.iter().map(|c| vertex_of.get(c).copied()).collect::<Option<Vec<u32>>>() else { continue };
                    // Each triangle on its own: a quad folded across a saddle
                    // would turn one half inward if wound as a whole.
                    for tri in [[q[0], q[1], q[2]], [q[0], q[2], q[3]]] {
                        let [p0, p1, p2] = tri.map(|i| Vec3::from_array(out.positions[i as usize]));
                        let normal: Vec3 = tri.iter().map(|&i| Vec3::from_array(out.normals[i as usize])).sum();
                        let face = (p1 - p0).cross(p2 - p0);
                        let outward = if normal.length_squared() > 1e-6 { normal } else { a.as_vec3() * out_dir };
                        if face.dot(outward) >= 0.0 {
                            out.indices.extend(tri);
                        } else {
                            out.indices.extend([tri[0], tri[2], tri[1]]);
                        }
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> SurfaceParams {
        SurfaceParams { box_width: 8.0, render_radius: 1.3, cutoff_radius: 5.5, cell: 6.4 }
    }

    /// A lone blob of radius 1 is a ball of about 1.3 box widths.
    #[test]
    fn lone_blob_is_a_ball_of_its_render_radius() {
        let m = build(&[Particle { center: Vec3::new(3.0, -2.0, 1.0), radius: 1.0, color: [1.0; 4] }], &params());
        assert!(!m.indices.is_empty());
        for p in &m.positions {
            let r = Vec3::from_array(*p).distance(Vec3::new(3.0, -2.0, 1.0));
            assert!((r - 10.4).abs() < 2.5, "vertex at radius {r}");
        }
        // Normals point away from the centre.
        for (p, n) in m.positions.iter().zip(&m.normals) {
            let d = Vec3::from_array(*p) - Vec3::new(3.0, -2.0, 1.0);
            assert!(d.normalize().dot(Vec3::from_array(*n)) > 0.9);
        }
    }

    /// Two blobs a little apart flow into one surface: the point between
    /// them is inside.
    #[test]
    fn neighbours_merge() {
        let a = Particle { center: Vec3::ZERO, radius: 0.7, color: [1.0; 4] };
        let b = Particle { center: Vec3::new(18.0, 0.0, 0.0), radius: 0.7, color: [1.0; 4] };
        let p = params();
        let sources: Vec<Source> = [a, b]
            .iter()
            .map(|q| {
                let reach = q.radius * p.box_width * p.cutoff_radius;
                Source { center: q.center, inv_reach2: 1.0 / (reach * reach), reach, color: q.color }
            })
            .collect();
        let buckets = Buckets::new(&sources);
        assert!(field(&sources, &buckets, Vec3::new(9.0, 0.0, 0.0)) >= p.threshold());
        // Each alone shows 0.7 × 10.4 ≈ 7.3 units, less than half the gap.
        assert!(kernel((9.0f32 * 9.0) * sources[0].inv_reach2) < p.threshold());
    }

    /// Every triangle's winding agrees with the outward normals.
    #[test]
    fn faces_point_out() {
        let blobs: Vec<Particle> = (0..6)
            .map(|i| Particle { center: Vec3::new(i as f32 * 9.0, (i % 2) as f32 * 5.0, 0.0), radius: 0.6, color: [1.0; 4] })
            .collect();
        let m = build(&blobs, &params());
        assert!(!m.indices.is_empty());
        let mut wrong = 0;
        for t in m.indices.chunks(3) {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| Vec3::from_array(m.positions[i as usize]));
            let face = (b - a).cross(c - a);
            let n = Vec3::from_array(m.normals[t[0] as usize]) + Vec3::from_array(m.normals[t[1] as usize]) + Vec3::from_array(m.normals[t[2] as usize]);
            if face.dot(n) <= 0.0 {
                wrong += 1;
            }
        }
        assert_eq!(wrong, 0, "of {} triangles", m.indices.len() / 3);
    }
}

#[cfg(test)]
mod timing {
    use super::*;

    #[test]
    #[ignore]
    fn time_stream() {
        let blobs: Vec<Particle> = (0..250)
            .map(|i| {
                let t = i as f32 * 0.02;
                Particle { center: Vec3::new(t * 300.0, (i % 7) as f32, 400.0 - 0.5 * 800.0 * t * t), radius: 0.6, color: [1.0; 4] }
            })
            .collect();
        for cell in [6.4f32, 4.8, 3.2] {
            let p = SurfaceParams { box_width: 8.0, render_radius: 1.3, cutoff_radius: 5.5, cell };
            let start = std::time::Instant::now();
            let mut tris = 0;
            for _ in 0..10 {
                tris = build(&blobs, &p).indices.len() / 3;
            }
            eprintln!("cell {cell}: {:.2} ms, {tris} triangles", start.elapsed().as_secs_f64() * 100.0);
        }
    }
}
