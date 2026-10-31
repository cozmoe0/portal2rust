//! How deep the water is under each point of the map's water surfaces, for
//! water one sees into: the game measures its fog through the scene's depth
//! behind the surface, which a big flat water plane over uneven ground (the
//! puddles in `sp_a1_wakeup`'s GLaDOS chamber) needs per pixel. At map load
//! the world's triangles are rasterised from above into a grid over the
//! water faces: each cell keeps the highest ground under its water surface,
//! stored as the depth below that surface.

use bevy::asset::RenderAssetUsages;
use bevy::image::{Image, ImageSampler};
use bevy::math::{Vec2, Vec3, Vec4};
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use source_formats::bsp::{Bsp, surf};

use crate::bsp_mesh::MeshBatch;

/// World units per cell.
const CELL: f32 = 4.0;
/// Largest grid side, in cells.
const MAX_SIDE: usize = 1024;
/// Depth given where nothing lies under the water within reach.
const MAX_DEPTH: f32 = 512.0;

/// The depth map and the world rectangle it covers (min x, min y, size x,
/// size y).
pub struct WaterDepthMap {
    pub image: Image,
    pub rect: Vec4,
}

/// Builds the depth map under the world's (model 0) water faces, if it has
/// any; `batches` are the world's meshes, `water` names their water
/// materials.
pub fn build(bsp: &Bsp, batches: &[MeshBatch], water: &dyn Fn(&str) -> bool) -> Option<WaterDepthMap> {
    let world = bsp.models.first()?;
    let faces = bsp.faces.get(world.first_face.max(0) as usize..(world.first_face + world.num_faces).max(0) as usize)?;
    let polys: Vec<Vec<Vec3>> = faces
        .iter()
        .filter(|f| f.dispinfo < 0 && bsp.texinfo.get(f.texinfo.max(0) as usize).is_some_and(|t| t.flags & surf::WARP != 0))
        .map(|f| bsp.face_vertices(f))
        .filter(|v| v.len() >= 3)
        .collect();
    if polys.is_empty() {
        return None;
    }
    let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
    for p in polys.iter().flatten() {
        lo = lo.min(p.truncate());
        hi = hi.max(p.truncate());
    }
    let cell = CELL.max((hi - lo).max_element() / MAX_SIDE as f32);
    let (w, h) = (((hi.x - lo.x) / cell).ceil().max(1.0) as usize, ((hi.y - lo.y) / cell).ceil().max(1.0) as usize);
    let center = |x: usize, y: usize| lo + Vec2::new((x as f32 + 0.5) * cell, (y as f32 + 0.5) * cell);
    // The water surface's height over each cell (none where there is none).
    let mut surface = vec![f32::NAN; w * h];
    for poly in &polys {
        for i in 1..poly.len() - 1 {
            raster(&[poly[0], poly[i], poly[i + 1]], lo, cell, w, h, &center, |k, z| {
                if surface[k].is_nan() || z > surface[k] {
                    surface[k] = z;
                }
            });
        }
    }
    // The highest ground under it.
    let mut ground = vec![f32::MIN; w * h];
    for b in batches.iter().filter(|b| !water(&b.texdata_name)) {
        for t in b.indices.as_chunks::<3>().0 {
            let tri = t.map(|i| Vec3::from_array(b.positions[i as usize]));
            raster(&tri, lo, cell, w, h, &center, |k, z| {
                let top = surface[k];
                if !top.is_nan() && z <= top + 0.5 && z > ground[k] {
                    ground[k] = z;
                }
            });
        }
    }
    let mut data = Vec::with_capacity(w * h * 2);
    for k in 0..w * h {
        let depth = if surface[k].is_nan() {
            0.0
        } else if ground[k] == f32::MIN {
            MAX_DEPTH
        } else {
            (surface[k] - ground[k]).clamp(0.0, MAX_DEPTH)
        };
        data.extend_from_slice(&half::f16::from_f32(depth).to_le_bytes());
    }
    let mut image = Image::new(
        Extent3d { width: w as u32, height: h as u32, depth_or_array_layers: 1 },
        TextureDimension::D2,
        data,
        TextureFormat::R16Float,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::linear();
    Some(WaterDepthMap { image, rect: Vec4::new(lo.x, lo.y, w as f32 * cell, h as f32 * cell) })
}

/// Calls `put(cell index, height)` for each cell whose centre the triangle
/// covers seen from above, with the triangle's height there.
fn raster(
    tri: &[Vec3; 3],
    lo: Vec2,
    cell: f32,
    w: usize,
    h: usize,
    center: &dyn Fn(usize, usize) -> Vec2,
    mut put: impl FnMut(usize, f32),
) {
    let [a, b, c] = *tri;
    let area = (b - a).truncate().perp_dot((c - a).truncate());
    if area.abs() < 1e-6 {
        return;
    }
    let (min, max) = (a.min(b).min(c).truncate(), a.max(b).max(c).truncate());
    let x0 = (((min.x - lo.x) / cell).floor().max(0.0)) as usize;
    let y0 = (((min.y - lo.y) / cell).floor().max(0.0)) as usize;
    let x1 = ((((max.x - lo.x) / cell).ceil()) as usize).min(w);
    let y1 = ((((max.y - lo.y) / cell).ceil()) as usize).min(h);
    for y in y0..y1 {
        for x in x0..x1 {
            let p = center(x, y);
            let w1 = (p - a.truncate()).perp_dot((c - a).truncate()) / area;
            let w2 = (b - a).truncate().perp_dot(p - a.truncate()) / area;
            let w0 = 1.0 - w1 - w2;
            if w0 < -1e-4 || w1 < -1e-4 || w2 < -1e-4 {
                continue;
            }
            put(y * w + x, a.z * w0 + b.z * w1 + c.z * w2);
        }
    }
}
