//! Studio model collision (`.phy`): convex pieces of each solid, converted to
//! Source units, plus the trailing keyvalue text.
//!
//! Layout: a 16-byte file header, then per solid a size-prefixed block that
//! starts with a "VPHY" surface header followed by a compact surface whose
//! ledges are convex hulls (triangles over a shared point array).

use glam::Vec3;

use crate::reader::Reader;
use crate::{Error, Result};

/// Physics geometry is stored in metres with a Y-up basis.
const METERS_PER_INCH: f32 = 0.0254;

fn to_source(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(x, z, -y) / METERS_PER_INCH
}

#[derive(Clone, Debug, Default)]
pub struct ConvexPiece {
    pub points: Vec<Vec3>,
    pub triangles: Vec<[u16; 3]>,
}

#[derive(Clone, Debug, Default)]
pub struct Solid {
    pub pieces: Vec<ConvexPiece>,
}

#[derive(Clone, Debug, Default)]
pub struct Phy {
    pub solids: Vec<Solid>,
    pub keyvalues: String,
}

impl Phy {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        let header_size = r.i32()? as usize;
        let _id = r.i32()?;
        let solid_count = r.i32()?.max(0) as usize;
        r.seek(header_size);
        let mut solids = Vec::with_capacity(solid_count);
        for _ in 0..solid_count {
            let size = r.i32()? as usize;
            let start = r.pos();
            let block = data.get(start..start + size).ok_or(Error::Eof)?;
            solids.push(parse_solid(block).unwrap_or_default());
            r.seek(start + size);
        }
        let keyvalues = String::from_utf8_lossy(&data[r.pos().min(data.len())..]).trim_end_matches('\0').to_owned();
        Ok(Self { solids, keyvalues })
    }

    /// Parses a single compact surface as found in BSP physics lumps.
    pub fn parse_surface(block: &[u8]) -> Option<Solid> {
        parse_solid(block)
    }
}

fn parse_solid(block: &[u8]) -> Option<Solid> {
    // "VPHY" header (28 bytes) precedes the compact surface in current files.
    let surface_start = if block.get(0..4) == Some(b"VPHY") { 28 } else { 0 };
    let surf = block.get(surface_start..)?;
    // Compact surface header: mass centre, inertia, radius, deviation/size,
    // ledge tree root offset and three reserved words (one holds "IVPS").
    let mut r = Reader::new(surf);
    r.skip(12 + 12 + 4 + 4).ok()?;
    let tree_root = r.i32().ok()?;
    let mut pieces = Vec::new();
    if tree_root > 0 {
        // Walk the ledge tree: leaf nodes point at the convex pieces.
        let mut stack = vec![tree_root as usize];
        let mut guard = 0;
        while let Some(node) = stack.pop() {
            guard += 1;
            if guard > 100_000 {
                break;
            }
            let mut n = Reader::at(surf, node);
            let right = n.i32().ok()?;
            let ledge = n.i32().ok()?;
            if right == 0 {
                let at = node as isize + ledge as isize;
                if at >= 0
                    && let Some(piece) = parse_ledge(surf, at as usize)
                {
                    pieces.push(piece);
                }
            } else {
                // Left child follows the node directly; the right one is offset.
                stack.push(node + 28);
                stack.push((node as isize + right as isize) as usize);
            }
        }
    } else if let Some(piece) = parse_ledge(surf, 48) {
        pieces.push(piece);
    }
    Some(Solid { pieces })
}

fn parse_ledge(surf: &[u8], offset: usize) -> Option<ConvexPiece> {
    let mut l = Reader::at(surf, offset);
    let point_offset = l.i32().ok()?;
    let _client = l.i32().ok()?;
    let _flags = l.u32().ok()?;
    let n_triangles = l.i16().ok()?.max(0) as usize;
    let points_at = offset as isize + point_offset as isize;
    if n_triangles == 0 || points_at < 0 {
        return None;
    }
    // Triangles index the point array the solid's ledges share; keep only
    // this ledge's points, renumbered in order of first use.
    let mut points = Vec::new();
    let mut remap = std::collections::HashMap::new();
    let mut triangles = Vec::with_capacity(n_triangles);
    for t in 0..n_triangles {
        let mut tr = Reader::at(surf, offset + 16 + t * 16 + 4);
        let mut tri = [0u16; 3];
        for v in tri.iter_mut() {
            let index = (tr.u32().ok()? & 0xFFFF) as usize;
            *v = match remap.get(&index) {
                Some(&local) => local,
                None => {
                    let mut pr = Reader::at(surf, points_at as usize + index * 16);
                    let (x, y, z) = (pr.f32().ok()?, pr.f32().ok()?, pr.f32().ok()?);
                    let local = points.len() as u16;
                    points.push(to_source(x, y, z));
                    remap.insert(index, local);
                    local
                }
            };
        }
        triangles.push(tri);
    }
    Some(ConvexPiece { points, triangles })
}
