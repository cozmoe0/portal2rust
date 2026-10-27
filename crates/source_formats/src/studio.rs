//! Combines MDL + VVD + VTX into ready-to-render geometry and resolves
//! include models and external animation blocks.

use std::sync::Arc;

use glam::{Mat4, Quat, Vec3};

use crate::fs::GameFileSystem;
use crate::mdl::Mdl;
use crate::text::{normalize_path, strip_extension};
use crate::vtx::Vtx;
use crate::vvd::{Vertex, Vvd};
use crate::{Error, Result};

/// One drawable piece: vertices + triangles that share a material slot.
#[derive(Clone, Debug)]
pub struct StudioMesh {
    pub body_part: usize,
    pub model: usize,
    /// Index into the model's skin reference table.
    pub material: usize,
    pub vertices: Vec<Vertex>,
    pub tangents: Vec<glam::Vec4>,
    /// Counter-clockwise triangles (already converted from Source winding).
    pub indices: Vec<u32>,
    /// Index of this mesh among all LOD 0 meshes, in the order baked static
    /// prop lighting files list them.
    pub vhv_mesh_index: usize,
    /// MDL vertex index of each optimized (hardware) vertex.
    pub hardware_vertex_ids: Vec<u32>,
}

pub struct StudioModel {
    pub path: String,
    pub mdl: Mdl,
    pub meshes: Vec<StudioMesh>,
    /// Included animation models; sequences with `source == i + 1` live there.
    pub includes: Vec<Arc<StudioModel>>,
    /// External animation block file (`.ani`) contents, if any.
    pub anim_block_data: Option<Vec<u8>>,
    /// Material search candidates per texture: `materials/<dir><name>.vmt`.
    pub material_paths: Vec<Vec<String>>,
    /// Maps this model's bone index to each include's bone index (by name).
    pub include_bone_maps: Vec<Vec<Option<usize>>>,
}

impl StudioModel {
    pub fn load(fs: &GameFileSystem, path: &str) -> Result<Self> {
        Self::load_depth(fs, path, 0)
    }

    fn load_depth(fs: &GameFileSystem, path: &str, depth: u32) -> Result<Self> {
        let path = normalize_path(path);
        let mdl_bytes = fs.read(&path).ok_or_else(|| Error::NotFound(path.clone()))?;
        let mdl = Mdl::parse(mdl_bytes)?;
        let base = strip_extension(&path).to_owned();

        let mut meshes = Vec::new();
        if let (Some(vvd_bytes), Some(vtx_bytes)) =
            (fs.read(&format!("{base}.vvd")), fs.read(&format!("{base}.dx90.vtx")).or_else(|| fs.read(&format!("{base}.vtx"))))
        {
            let vvd = Vvd::parse(&vvd_bytes)?;
            let vtx = Vtx::parse(&vtx_bytes, mdl.version)?;
            let mut vhv_index = 0usize;
            for (bpi, bp) in mdl.body_parts.iter().enumerate() {
                let Some(vbp) = vtx.body_parts.get(bpi) else { continue };
                for (mi, model) in bp.models.iter().enumerate() {
                    let Some(vm) = vbp.models.get(mi) else { continue };
                    let Some(lod) = vm.lods.first() else { continue };
                    for (mei, mesh) in model.meshes.iter().enumerate() {
                        let Some(vmesh) = lod.meshes.get(mei) else { continue };
                        let first = (model.vertex_index + mesh.vertex_offset) as usize;
                        let count = mesh.num_vertices.max(0) as usize;
                        if first + count > vvd.vertices.len() {
                            continue;
                        }
                        let mut indices = Vec::with_capacity(vmesh.indices.len());
                        for tri in vmesh.indices.chunks_exact(3) {
                            if tri.iter().any(|&i| i as usize >= count) {
                                continue;
                            }
                            // Source front faces are clockwise.
                            indices.extend([tri[0], tri[2], tri[1]]);
                        }
                        meshes.push(StudioMesh {
                            body_part: bpi,
                            model: mi,
                            material: mesh.material.max(0) as usize,
                            vertices: vvd.vertices[first..first + count].to_vec(),
                            tangents: vvd.tangents[first..first + count].to_vec(),
                            indices,
                            vhv_mesh_index: vhv_index,
                            hardware_vertex_ids: vmesh.vertex_ids.clone(),
                        });
                        vhv_index += 1;
                    }
                }
            }
        }

        let mut includes = Vec::new();
        if depth < 4 {
            for inc in &mdl.include_models {
                match Self::load_depth(fs, inc, depth + 1) {
                    Ok(m) => includes.push(Arc::new(m)),
                    Err(e) => log::warn!("{path}: include model {inc}: {e}"),
                }
            }
        }
        let include_bone_maps = includes
            .iter()
            .map(|inc| mdl.bones.iter().map(|b| inc.mdl.find_bone(&b.name)).collect())
            .collect();

        let anim_block_data = if mdl.anim_block_name.is_empty() { None } else { fs.read(&mdl.anim_block_name) };

        let material_paths = mdl
            .textures
            .iter()
            .map(|tex| {
                let tex = normalize_path(tex);
                let mut list: Vec<String> = mdl
                    .texture_dirs
                    .iter()
                    .map(|dir| {
                        let dir = normalize_path(dir);
                        let dir = dir.strip_prefix("materials/").unwrap_or(&dir).to_owned();
                        if dir.is_empty() { format!("materials/{tex}.vmt") } else { format!("materials/{dir}/{tex}.vmt") }
                    })
                    .collect();
                list.push(format!("materials/{tex}.vmt"));
                list
            })
            .collect();

        Ok(Self { path, mdl, meshes, includes, anim_block_data, material_paths, include_bone_maps })
    }

    /// First material path that exists for a texture slot.
    pub fn resolve_material(&self, fs: &GameFileSystem, texture: usize) -> Option<String> {
        self.material_paths.get(texture)?.iter().find(|p| fs.exists(p)).cloned()
    }

    /// All sequences visible on this model: its own followed by included ones.
    pub fn sequence_count(&self) -> usize {
        self.mdl.sequences.len() + self.includes.iter().map(|i| i.sequence_count()).sum::<usize>()
    }

    /// Resolves a global sequence index to (owning model, local index).
    pub fn sequence(&self, mut index: usize) -> Option<(&StudioModel, usize)> {
        if index < self.mdl.sequences.len() {
            return Some((self, index));
        }
        index -= self.mdl.sequences.len();
        for inc in &self.includes {
            let n = inc.sequence_count();
            if index < n {
                return inc.sequence(index);
            }
            index -= n;
        }
        None
    }

    pub fn find_sequence(&self, name: &str) -> Option<usize> {
        let mut base = 0;
        if let Some(i) = self.mdl.find_sequence(name) {
            return Some(i);
        }
        base += self.mdl.sequences.len();
        for inc in &self.includes {
            if let Some(i) = inc.find_sequence(name) {
                return Some(base + i);
            }
            base += inc.sequence_count();
        }
        None
    }

    pub fn find_sequence_by_activity(&self, activity: &str) -> Option<usize> {
        (0..self.sequence_count()).find(|&i| {
            self.sequence(i).is_some_and(|(m, li)| m.mdl.sequences[li].activity_name.eq_ignore_ascii_case(activity))
        })
    }

    /// Samples a sequence (first blend entry) at `cycle` (0..1) into local bone
    /// transforms for this model's skeleton.
    pub fn sample_sequence(&self, seq: usize, cycle: f32, out: &mut [(Vec3, Quat)]) {
        for (i, b) in self.mdl.bones.iter().enumerate().take(out.len()) {
            out[i] = (b.pos, b.quat);
        }
        let Some((owner, local)) = self.sequence(seq) else { return };
        let s = &owner.mdl.sequences[local];
        let Some(&anim) = s.anims.first() else { return };
        let anim = anim.max(0) as usize;
        let Some(desc) = owner.mdl.anims.get(anim) else { return };
        let frames = (desc.num_frames - 1).max(0) as f32;
        let frame = cycle.clamp(0.0, 1.0) * frames;
        if std::ptr::eq(owner, self) {
            self.mdl.sample_anim(anim, frame, self.anim_block_data.as_deref(), out);
        } else {
            let mut tmp = vec![(Vec3::ZERO, Quat::IDENTITY); owner.mdl.bones.len()];
            owner.mdl.sample_anim(anim, frame, owner.anim_block_data.as_deref(), &mut tmp);
            // Map by bone name onto our skeleton.
            for (i, b) in self.mdl.bones.iter().enumerate().take(out.len()) {
                if let Some(j) = owner.mdl.find_bone(&b.name) {
                    out[i] = tmp[j];
                }
            }
        }
    }

    /// Frames-per-second and frame count of a sequence's first animation.
    pub fn sequence_timing(&self, seq: usize) -> (f32, i32) {
        let Some((owner, local)) = self.sequence(seq) else { return (30.0, 1) };
        let s = &owner.mdl.sequences[local];
        let anim = s.anims.first().copied().unwrap_or(0).max(0) as usize;
        owner.mdl.anims.get(anim).map(|a| (a.fps.max(0.001), a.num_frames.max(1))).unwrap_or((30.0, 1))
    }

    /// Model-space bone matrices from local transforms.
    pub fn bone_matrices(&self, local: &[(Vec3, Quat)]) -> Vec<Mat4> {
        let mut world: Vec<Mat4> = Vec::with_capacity(self.mdl.bones.len());
        for (i, b) in self.mdl.bones.iter().enumerate() {
            let (p, q) = local.get(i).copied().unwrap_or((b.pos, b.quat));
            let m = Mat4::from_rotation_translation(q, p);
            let m = if b.parent >= 0 && (b.parent as usize) < world.len() { world[b.parent as usize] * m } else { m };
            world.push(m);
        }
        world
    }
}
