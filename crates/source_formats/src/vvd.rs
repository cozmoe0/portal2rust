//! Studio model vertex data (VVD v4).

use glam::{Vec2, Vec3, Vec4};

use crate::reader::Reader;
use crate::{Error, Result};

#[derive(Clone, Copy, Debug, Default)]
pub struct Vertex {
    pub weights: [f32; 3],
    pub bones: [u8; 3],
    pub num_bones: u8,
    pub pos: Vec3,
    pub normal: Vec3,
    pub uv: Vec2,
}

pub struct Vvd {
    pub checksum: i32,
    pub num_lods: i32,
    /// LOD 0 vertices with fixups applied.
    pub vertices: Vec<Vertex>,
    pub tangents: Vec<Vec4>,
}

impl Vvd {
    pub fn parse(data: &[u8]) -> Result<Self> {
        Self::parse_lod(data, 0)
    }

    pub fn parse_lod(data: &[u8], lod: i32) -> Result<Self> {
        let mut r = Reader::new(data);
        let id = r.bytes(4)?;
        if id != b"IDSV" {
            return Err(Error::Magic { expected: "IDSV", found: id.to_vec() });
        }
        let version = r.i32()?;
        if version != 4 {
            return Err(Error::Version(version as i64));
        }
        let checksum = r.i32()?;
        let num_lods = r.i32()?;
        let mut lod_counts = [0i32; 8];
        for c in lod_counts.iter_mut() {
            *c = r.i32()?;
        }
        let num_fixups = r.i32()?;
        let fixup_start = r.i32()? as usize;
        let vertex_start = r.i32()? as usize;
        let tangent_start = r.i32()? as usize;

        let total = lod_counts[0].max(0) as usize;
        let read_vertex = |i: usize| -> Result<Vertex> {
            let mut v = Reader::at(data, vertex_start + i * 48);
            let weights = [v.f32()?, v.f32()?, v.f32()?];
            let bones = [v.u8()?, v.u8()?, v.u8()?];
            let num_bones = v.u8()?;
            Ok(Vertex { weights, bones, num_bones, pos: v.vec3()?, normal: v.vec3()?, uv: v.vec2()? })
        };
        let read_tangent = |i: usize| -> Result<Vec4> {
            let mut t = Reader::at(data, tangent_start + i * 16);
            Ok(Vec4::new(t.f32()?, t.f32()?, t.f32()?, t.f32()?))
        };

        let mut vertices = Vec::with_capacity(total);
        let mut tangents = Vec::with_capacity(total);
        if num_fixups > 0 {
            for f in 0..num_fixups as usize {
                let mut fr = Reader::at(data, fixup_start + f * 12);
                let flod = fr.i32()?;
                let src = fr.i32()? as usize;
                let count = fr.i32()? as usize;
                if flod >= lod {
                    for i in src..src + count {
                        vertices.push(read_vertex(i)?);
                        tangents.push(if tangent_start > 0 { read_tangent(i)? } else { Vec4::X.with_w(1.0) });
                    }
                }
            }
        } else {
            for i in 0..total {
                vertices.push(read_vertex(i)?);
                tangents.push(if tangent_start > 0 { read_tangent(i)? } else { Vec4::X.with_w(1.0) });
            }
        }
        Ok(Self { checksum, num_lods, vertices, tangents })
    }
}
