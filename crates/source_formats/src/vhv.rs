//! Baked static prop vertex lighting (`sp_hdr_N.vhv` inside map pakfiles).

use glam::Vec3;

use crate::reader::Reader;
use crate::{Error, Result};

pub struct Vhv {
    /// Linear RGB per hardware vertex, one list per LOD 0 mesh.
    pub meshes: Vec<Vec<Vec3>>,
}

impl Vhv {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        let version = r.i32()?;
        if version != 2 {
            return Err(Error::Version(version as i64));
        }
        let _checksum = r.u32()?;
        let _flags = r.u32()?;
        let vertex_size = r.u32()? as usize;
        let _total = r.u32()?;
        let num_meshes = r.i32()?.max(0) as usize;
        let mut meshes = Vec::new();
        for m in 0..num_meshes {
            let mut h = Reader::at(data, 40 + m * 28);
            let lod = h.u32()?;
            let count = h.u32()? as usize;
            let offset = h.u32()? as usize;
            if lod != 0 {
                continue;
            }
            let mut colors = Vec::with_capacity(count);
            for v in 0..count {
                let o = offset + v * vertex_size.max(4);
                let c = data.get(o..o + 4).ok_or(Error::Eof)?;
                colors.push(decode(c));
            }
            meshes.push(colors);
        }
        Ok(Self { meshes })
    }
}

/// Colours are stored BGRA in gamma space, pre-divided by an overbright
/// factor of two.
fn decode(c: &[u8]) -> Vec3 {
    let g = |v: u8| (v as f32 * 2.0 / 255.0).powf(2.2);
    Vec3::new(g(c[2]), g(c[1]), g(c[0]))
}
