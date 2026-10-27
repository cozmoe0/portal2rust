//! Studio model optimized mesh data (VTX v7, `.dx90.vtx`).
//!
//! Only the information needed to rebuild triangle lists is kept: for every
//! body part / model / LOD / mesh, a list of triangles whose indices refer to
//! the MDL mesh's vertices.

use crate::reader::Reader;
use crate::{Error, Result};

pub mod strip_flags {
    pub const IS_TRILIST: u8 = 0x01;
    pub const IS_TRISTRIP: u8 = 0x02;
}

#[derive(Clone, Debug, Default)]
pub struct VtxMesh {
    /// Triangle indices into the MDL mesh's vertex range (`origMeshVertID`).
    pub indices: Vec<u32>,
    /// `origMeshVertID` of every optimized vertex, strip groups concatenated.
    /// Per-vertex data that follows hardware vertex order (baked static prop
    /// lighting) is mapped back to MDL vertices through this list.
    pub vertex_ids: Vec<u32>,
    pub flags: u8,
}

#[derive(Clone, Debug, Default)]
pub struct VtxLod {
    pub meshes: Vec<VtxMesh>,
    pub switch_point: f32,
}

#[derive(Clone, Debug, Default)]
pub struct VtxModel {
    pub lods: Vec<VtxLod>,
}

#[derive(Clone, Debug, Default)]
pub struct VtxBodyPart {
    pub models: Vec<VtxModel>,
}

pub struct Vtx {
    pub checksum: i32,
    pub num_lods: i32,
    pub body_parts: Vec<VtxBodyPart>,
}

impl Vtx {
    /// `mdl_version` selects the strip header layout: version 49 files carry
    /// extra topology fields in strip group and strip headers.
    pub fn parse(data: &[u8], mdl_version: i32) -> Result<Self> {
        let mut r = Reader::new(data);
        let version = r.i32()?;
        if version != 7 {
            return Err(Error::Version(version as i64));
        }
        let _vert_cache = r.i32()?;
        let _max_bones_per_strip = r.u16()?;
        let _max_bones_per_tri = r.u16()?;
        let _max_bones_per_vert = r.i32()?;
        let checksum = r.i32()?;
        let num_lods = r.i32()?;
        let _material_replacement = r.i32()?;
        let num_body_parts = r.i32()?;
        let body_part_offset = r.i32()? as usize;

        // Try the layout implied by the MDL version first, then the other one.
        let primary = mdl_version >= 49;
        match Self::parse_body_parts(data, num_body_parts, body_part_offset, primary) {
            Ok(body_parts) => Ok(Self { checksum, num_lods, body_parts }),
            Err(_) => {
                let body_parts = Self::parse_body_parts(data, num_body_parts, body_part_offset, !primary)?;
                Ok(Self { checksum, num_lods, body_parts })
            }
        }
    }

    fn parse_body_parts(data: &[u8], count: i32, offset: usize, topology: bool) -> Result<Vec<VtxBodyPart>> {
        let sg_size = if topology { 33 } else { 25 };
        let strip_size = if topology { 35 } else { 27 };
        let mut body_parts = Vec::with_capacity(count.max(0) as usize);
        for bp in 0..count.max(0) as usize {
            let bp_off = offset + bp * 8;
            let mut r = Reader::at(data, bp_off);
            let num_models = r.i32()?;
            let model_offset = r.i32()? as usize;
            let mut models = Vec::with_capacity(num_models.max(0) as usize);
            for m in 0..num_models.max(0) as usize {
                let m_off = bp_off + model_offset + m * 8;
                let mut mr = Reader::at(data, m_off);
                let num_lods = mr.i32()?;
                let lod_offset = mr.i32()? as usize;
                let mut lods = Vec::with_capacity(num_lods.max(0) as usize);
                for l in 0..num_lods.max(0) as usize {
                    let l_off = m_off + lod_offset + l * 12;
                    let mut lr = Reader::at(data, l_off);
                    let num_meshes = lr.i32()?;
                    let mesh_offset = lr.i32()? as usize;
                    let switch_point = lr.f32()?;
                    let mut meshes = Vec::with_capacity(num_meshes.max(0) as usize);
                    for me in 0..num_meshes.max(0) as usize {
                        let me_off = l_off + mesh_offset + me * 9;
                        let mut mer = Reader::at(data, me_off);
                        let num_sg = mer.i32()?;
                        let sg_offset = mer.i32()? as usize;
                        let flags = mer.u8()?;
                        let mut mesh = VtxMesh { indices: Vec::new(), vertex_ids: Vec::new(), flags };
                        for sg in 0..num_sg.max(0) as usize {
                            let sg_off = me_off + sg_offset + sg * sg_size;
                            Self::parse_strip_group(data, sg_off, strip_size, &mut mesh)?;
                        }
                        meshes.push(mesh);
                    }
                    lods.push(VtxLod { meshes, switch_point });
                }
                models.push(VtxModel { lods });
            }
            body_parts.push(VtxBodyPart { models });
        }
        Ok(body_parts)
    }

    fn parse_strip_group(data: &[u8], sg_off: usize, strip_size: usize, mesh: &mut VtxMesh) -> Result<()> {
        let mut r = Reader::at(data, sg_off);
        let num_verts = r.i32()?;
        let vert_offset = r.i32()? as usize;
        let num_indices = r.i32()?;
        let index_offset = r.i32()? as usize;
        let num_strips = r.i32()?;
        let strip_offset = r.i32()? as usize;
        if num_verts < 0 || num_indices < 0 || num_strips < 0 {
            return Err(Error::Invalid("vtx: negative counts".into()));
        }
        // VTX vertex: 3 bone weight indices, bone count, original mesh vertex id, 3 bone ids.
        let mut orig = Vec::with_capacity(num_verts as usize);
        for v in 0..num_verts as usize {
            let off = sg_off + vert_offset + v * 9 + 4;
            let b = data.get(off..off + 2).ok_or(Error::Eof)?;
            orig.push(u16::from_le_bytes([b[0], b[1]]) as u32);
        }
        mesh.vertex_ids.extend_from_slice(&orig);
        let mut indices = Vec::with_capacity(num_indices as usize);
        for i in 0..num_indices as usize {
            let off = sg_off + index_offset + i * 2;
            let b = data.get(off..off + 2).ok_or(Error::Eof)?;
            indices.push(u16::from_le_bytes([b[0], b[1]]) as usize);
        }
        for s in 0..num_strips as usize {
            let mut sr = Reader::at(data, sg_off + strip_offset + s * strip_size);
            let n = sr.i32()? as usize;
            let first = sr.i32()? as usize;
            let _num_verts = sr.i32()?;
            let _vert_offset = sr.i32()?;
            let _num_bones = sr.i16()?;
            let flags = sr.u8()?;
            let idx = indices.get(first..first + n).ok_or(Error::Eof)?;
            let map = |i: usize| -> Result<u32> { orig.get(i).copied().ok_or(Error::Eof) };
            if flags & strip_flags::IS_TRISTRIP != 0 {
                for k in 2..idx.len() {
                    let (a, b, c) = if k % 2 == 0 { (idx[k - 2], idx[k - 1], idx[k]) } else { (idx[k - 1], idx[k - 2], idx[k]) };
                    mesh.indices.extend([map(a)?, map(b)?, map(c)?]);
                }
            } else {
                for tri in idx.chunks_exact(3) {
                    mesh.indices.extend([map(tri[0])?, map(tri[1])?, map(tri[2])?]);
                }
            }
        }
        Ok(())
    }
}
