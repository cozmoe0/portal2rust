//! Combines MDL + VVD + VTX into ready-to-render geometry, resolves include
//! models and external animation blocks, and blends sequences into bone
//! poses (blend grids by pose parameter, per-bone weights, delta layers,
//! automatic layers).

use std::borrow::Cow;
use std::sync::Arc;

use glam::{Mat4, Quat, Vec3};

use crate::fs::GameFileSystem;
use crate::mdl::{Mdl, PoseParameter, Sequence, autolayer_flags, seq_flags};
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
    /// Pose parameters of this model followed by those of its includes that
    /// it does not declare itself (merged by name).
    pub pose_parameters: Vec<PoseParameter>,
    /// `AUTOPLAY` sequences (always layered on top) with their cycles per
    /// second.
    pub autoplay: Vec<(usize, f32)>,
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
        let include_bone_maps = includes.iter().map(|inc| mdl.bones.iter().map(|b| inc.mdl.find_bone(&b.name)).collect()).collect();

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

        let mut pose_parameters = mdl.pose_parameters.clone();
        for inc in &includes {
            for p in &inc.pose_parameters {
                if !pose_parameters.iter().any(|q| q.name.eq_ignore_ascii_case(&p.name)) {
                    pose_parameters.push(p.clone());
                }
            }
        }

        let mut model =
            Self { path, mdl, meshes, includes, anim_block_data, material_paths, include_bone_maps, pose_parameters, autoplay: Vec::new() };
        model.autoplay = (0..model.sequence_count())
            .filter(|&i| model.sequence(i).is_some_and(|(m, l)| m.mdl.sequences[l].flags & seq_flags::AUTOPLAY != 0))
            .map(|i| {
                let (fps, frames) = model.sequence_timing(i);
                (i, if frames > 1 { fps / (frames - 1) as f32 } else { 0.0 })
            })
            .collect();
        Ok(model)
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
        (0..self.sequence_count())
            .find(|&i| self.sequence(i).is_some_and(|(m, li)| m.mdl.sequences[li].activity_name.eq_ignore_ascii_case(activity)))
    }

    pub fn find_pose_parameter(&self, name: &str) -> Option<usize> {
        self.pose_parameters.iter().position(|p| p.name.eq_ignore_ascii_case(name))
    }

    /// Setting (0..=1 across the parameter's range) for a pose value;
    /// looping parameters wrap into their range first.
    pub fn pose_setting(&self, index: usize, value: f32) -> f32 {
        let Some(p) = self.pose_parameters.get(index) else { return 0.0 };
        let v = wrap_pose(p, value);
        if p.end == p.start { 0.0 } else { ((v - p.start) / (p.end - p.start)).clamp(0.0, 1.0) }
    }

    /// Pose value of a setting.
    pub fn pose_value(&self, index: usize, setting: f32) -> f32 {
        self.pose_parameters.get(index).map_or(0.0, |p| p.start + (p.end - p.start) * setting)
    }

    /// Settings of a freshly spawned model: every parameter at value 0.
    pub fn default_pose_settings(&self) -> Vec<f32> {
        (0..self.pose_parameters.len()).map(|i| self.pose_setting(i, 0.0)).collect()
    }

    pub fn bind_pose(&self) -> Vec<(Vec3, Quat)> {
        self.mdl.bones.iter().map(|b| (b.pos, b.quat)).collect()
    }

    /// The bind pose with sequence `seq` at `cycle` blended in fully, pose
    /// parameters at their defaults.
    pub fn sample_sequence(&self, seq: usize, cycle: f32, out: &mut [(Vec3, Quat)]) {
        for (o, b) in out.iter_mut().zip(&self.mdl.bones) {
            *o = (b.pos, b.quat);
        }
        self.accumulate(out, seq, cycle, 1.0, &self.default_pose_settings(), 0);
    }

    /// Blends sequence `seq` at `cycle` into `pose` with `weight`, as the
    /// engine accumulates sequences: the sequence's own pose comes from its
    /// blend grid at the pose `settings` (one per [`Self::pose_parameters`]),
    /// is merged bone by bone scaled by the sequence's weight list (delta
    /// sequences add to the pose), and its automatic layers follow.
    pub fn accumulate_sequence(&self, pose: &mut [(Vec3, Quat)], seq: usize, cycle: f32, weight: f32, settings: &[f32]) {
        self.accumulate(pose, seq, cycle, weight, settings, 0);
    }

    fn accumulate(&self, pose: &mut [(Vec3, Quat)], seq: usize, cycle: f32, weight: f32, settings: &[f32], depth: u32) {
        let Some((owner, local)) = self.sequence(seq) else { return };
        if depth > 8 {
            return;
        }
        let weight = weight.clamp(0.0, 1.0);
        let desc = &owner.mdl.sequences[local];
        let mut own = self.sequence_pose(owner, desc, cycle, settings);
        if desc.flags & seq_flags::LOCAL != 0 {
            self.add_layers(&mut own, owner, desc, cycle, 1.0, settings, depth, true);
        }
        self.merge(pose, &own, owner, desc, weight);
        self.add_layers(pose, owner, desc, cycle, weight, settings, depth, false);
    }

    /// Accumulates a sequence's automatic layers (`local`: only its
    /// local-context layers, otherwise only the others). Each layer ramps in
    /// and out over the parent's cycle, or over a pose value.
    #[allow(clippy::too_many_arguments)]
    fn add_layers(
        &self,
        pose: &mut [(Vec3, Quat)],
        owner: &StudioModel,
        desc: &Sequence,
        cycle: f32,
        weight: f32,
        settings: &[f32],
        depth: u32,
        local: bool,
    ) {
        for layer in &desc.autolayers {
            if (layer.flags & autolayer_flags::LOCAL != 0) != local {
                continue;
            }
            let Some(seq) = self.global_sequence(owner, layer.sequence.max(0) as usize) else { continue };
            let (mut layer_cycle, mut layer_weight) = (cycle, weight);
            if layer.start != layer.end {
                let by_pose = !local && layer.flags & autolayer_flags::POSE != 0;
                let index = if by_pose {
                    self.shared_pose_parameter(owner, layer.pose as i32).map_or(0.0, |i| self.pose_value(i, self.setting(settings, i)))
                } else {
                    cycle
                };
                if index < layer.start || index >= layer.end {
                    continue;
                }
                let mut s = if index < layer.peak && layer.start != layer.peak {
                    (index - layer.start) / (layer.peak - layer.start)
                } else if index > layer.tail && layer.end != layer.tail {
                    (layer.end - index) / (layer.end - layer.tail)
                } else {
                    1.0
                };
                if layer.flags & autolayer_flags::SPLINE != 0 {
                    s = s * s * (3.0 - 2.0 * s);
                }
                layer_weight = if layer.flags & autolayer_flags::XFADE != 0 && index > layer.tail {
                    s * weight / (1.0 - weight + s * weight)
                } else if layer.flags & autolayer_flags::NOBLEND != 0 {
                    s
                } else {
                    s * weight
                };
                if !by_pose {
                    layer_cycle = (cycle - layer.start) / (layer.end - layer.start);
                }
            }
            self.accumulate(pose, seq, layer_cycle, layer_weight, settings, depth + 1);
        }
    }

    /// Merges a sequence's own pose into `pose`: each bone by `weight` times
    /// the sequence's weight for it. Delta rotations are scaled and applied
    /// after the bone's rotation (`POST`) or before it; others are slerped.
    fn merge(&self, pose: &mut [(Vec3, Quat)], own: &[(Vec3, Quat)], owner: &StudioModel, desc: &Sequence, weight: f32) {
        if weight <= 0.0 {
            return;
        }
        let map = self.bone_map(owner);
        let delta = desc.flags & seq_flags::DELTA != 0;
        let post = desc.flags & seq_flags::POST != 0;
        for (i, (base, &(p, q))) in pose.iter_mut().zip(own).enumerate() {
            let Some(j) = map.as_ref().map_or(Some(i), |m| m.get(i).copied().flatten()) else { continue };
            let w = weight * desc.weights.get(j).copied().unwrap_or(1.0);
            if w <= 0.0 {
                continue;
            }
            if delta {
                let d = scale_rotation(q, w);
                base.1 = if post { base.1 * d } else { d * base.1 }.normalize();
                base.0 += p * w;
            } else {
                base.1 = base.1.slerp(q, w);
                base.0 = base.0.lerp(p, w);
            }
        }
    }

    /// A sequence's own pose on this model's skeleton: its blend grid sampled
    /// at the pose settings. Interior grid cells blend three corners (the
    /// engine's default triangle blending); bones the owning model lacks keep
    /// the bind pose (no change for delta sequences).
    fn sequence_pose(&self, owner: &StudioModel, desc: &Sequence, cycle: f32, settings: &[f32]) -> Vec<(Vec3, Quat)> {
        let (i0, s0) = self.grid_position(owner, desc, 0, settings);
        let (i1, s1) = self.grid_position(owner, desc, 1, settings);
        let cycle = if desc.flags & seq_flags::CYCLEPOSE != 0 {
            self.shared_pose_parameter(owner, desc.cycle_pose).map_or(0.0, |i| self.setting(settings, i))
        } else if desc.flags & seq_flags::LOOPING != 0 {
            cycle.rem_euclid(1.0)
        } else {
            cycle.clamp(0.0, 1.0)
        };
        let [n0, n1] = desc.group_size.map(|g| g.max(1) as usize);
        let sample = |a: usize, b: usize| {
            let anim = desc.anims.get(b.min(n1 - 1) * n0 + a.min(n0 - 1)).copied().unwrap_or(0).max(0) as usize;
            self.sample_owner_anim(owner, desc, anim, cycle)
        };
        let blend = |a: (usize, usize), b: (usize, usize), t: f32| {
            let mut out = sample(a.0, a.1);
            blend_poses(&mut out, &sample(b.0, b.1), t);
            out
        };
        // Within 0.001 of a grid line counts as on it.
        let edge = |s: f32| {
            if s < 0.001 {
                Some(0)
            } else if s > 0.999 {
                Some(1)
            } else {
                None
            }
        };
        match (edge(s0), edge(s1)) {
            (Some(a), Some(b)) => sample(i0 + a, i1 + b),
            (Some(a), None) => blend((i0 + a, i1), (i0 + a, i1 + 1), s1),
            (None, Some(b)) => blend((i0, i1 + b), (i0 + 1, i1 + b), s0),
            (None, None) => {
                let (corners, w) = triangle(i0, i1, s0, s1);
                let at = |k: usize| (i0 + corners[k].0, i1 + corners[k].1);
                if w[1] >= 0.001 {
                    let mut out = blend(at(0), at(1), w[1] / (w[0] + w[1]));
                    blend_poses(&mut out, &sample(at(2).0, at(2).1), w[2]);
                    out
                } else {
                    blend(at(0), at(2), w[2] / (w[0] + w[2]))
                }
            }
        }
    }

    /// One animation of `owner` at `cycle`, mapped onto this skeleton.
    fn sample_owner_anim(&self, owner: &StudioModel, desc: &Sequence, anim: usize, cycle: f32) -> Vec<(Vec3, Quat)> {
        let delta = desc.flags & seq_flags::DELTA != 0;
        let rest = |b: &crate::mdl::Bone| if delta { (Vec3::ZERO, Quat::IDENTITY) } else { (b.pos, b.quat) };
        let mut out: Vec<(Vec3, Quat)> = self.mdl.bones.iter().map(rest).collect();
        let Some(a) = owner.mdl.anims.get(anim) else { return out };
        let frame = cycle.clamp(0.0, 1.0) * (a.num_frames - 1).max(0) as f32;
        match self.bone_map(owner) {
            None => {
                self.mdl.sample_anim(anim, frame, self.anim_block_data.as_deref(), &mut out);
            }
            Some(map) => {
                let mut theirs: Vec<(Vec3, Quat)> = owner.mdl.bones.iter().map(rest).collect();
                owner.mdl.sample_anim(anim, frame, owner.anim_block_data.as_deref(), &mut theirs);
                for (o, j) in out.iter_mut().zip(map.iter()) {
                    if let Some(&t) = j.and_then(|j| theirs.get(j)) {
                        *o = t;
                    }
                }
            }
        }
        out
    }

    /// Grid line along `axis` of a sequence's blend grid and the fraction
    /// towards the next line, for the current pose settings.
    fn grid_position(&self, owner: &StudioModel, desc: &Sequence, axis: usize, settings: &[f32]) -> (usize, f32) {
        let Some(i) = self.shared_pose_parameter(owner, desc.param_index[axis]) else { return (0, 0.0) };
        let p = &self.pose_parameters[i];
        let v = wrap_pose(p, self.setting(settings, i));
        let n = desc.group_size[axis].max(1) as usize;
        if !desc.pose_keys.is_empty() {
            // Unevenly spaced lines: walk to the first span holding the value.
            let value = p.start + (p.end - p.start) * v;
            let key = |k: usize| desc.pose_keys.get(axis * desc.group_size[0].max(0) as usize + k).copied().unwrap_or(0.0);
            let span = |k: usize| {
                let d = key(k + 1) - key(k);
                if d == 0.0 { 0.0 } else { (value - key(k)) / d }
            };
            let mut index = 0;
            let mut s = span(0);
            while index + 2 < n && s > 1.0 {
                index += 1;
                s = span(index);
            }
            return (index, s.clamp(0.0, 1.0));
        }
        let range = p.end - p.start;
        if range == 0.0 {
            return (0, 0.0);
        }
        let lo = (desc.param_start[axis] - p.start) / range;
        let hi = (desc.param_end[axis] - p.start) / range;
        let mut s = if hi == lo { 0.0 } else { ((v - lo) / (hi - lo)).clamp(0.0, 1.0) };
        let mut index = 0;
        if n > 2 {
            index = (((n - 1) as f32 * s) as usize).min(n - 2);
            s = (n - 1) as f32 * s - index as f32;
        }
        (index, s)
    }

    fn setting(&self, settings: &[f32], index: usize) -> f32 {
        settings.get(index).copied().unwrap_or_else(|| self.pose_setting(index, 0.0))
    }

    /// Index on this model of sequence `local` of `owner` (this model or an
    /// include).
    fn global_sequence(&self, owner: &StudioModel, local: usize) -> Option<usize> {
        if std::ptr::eq(owner, self) {
            return (local < self.mdl.sequences.len()).then_some(local);
        }
        let mut base = self.mdl.sequences.len();
        for inc in &self.includes {
            if let Some(i) = inc.global_sequence(owner, local) {
                return Some(base + i);
            }
            base += inc.sequence_count();
        }
        None
    }

    /// This model's index of pose parameter `local` of `owner`.
    fn shared_pose_parameter(&self, owner: &StudioModel, local: i32) -> Option<usize> {
        let p = owner.mdl.pose_parameters.get(usize::try_from(local).ok()?)?;
        if std::ptr::eq(owner, self) { Some(local as usize) } else { self.find_pose_parameter(&p.name) }
    }

    /// For each of this model's bones, the bone of `owner` with the same name
    /// (`None` when `owner` is this model).
    fn bone_map(&self, owner: &StudioModel) -> Option<Cow<'_, [Option<usize>]>> {
        if std::ptr::eq(owner, self) {
            return None;
        }
        if let Some(i) = self.includes.iter().position(|inc| std::ptr::eq(inc.as_ref(), owner)) {
            return Some(Cow::Borrowed(&self.include_bone_maps[i]));
        }
        Some(Cow::Owned(self.mdl.bones.iter().map(|b| owner.mdl.find_bone(&b.name)).collect()))
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

/// Wraps a value of a looping pose parameter into its range.
fn wrap_pose(p: &PoseParameter, value: f32) -> f32 {
    if p.loop_range == 0.0 {
        return value;
    }
    let shift = p.loop_range - (p.start + p.end + p.loop_range) * 0.5;
    value - ((value + shift) / p.loop_range).floor() * p.loop_range
}

/// Rotation `q` scaled to `t` of its angle; the sign of `w` is kept, so a
/// rotation stored the long way round stays that way.
fn scale_rotation(q: Quat, t: f32) -> Quat {
    let v = Vec3::new(q.x, q.y, q.z);
    let s = v.length().min(1.0);
    let scaled = (s.asin() * t).sin();
    let v = v * (scaled / (s + f32::EPSILON));
    let w = (1.0 - scaled * scaled).max(0.0).sqrt();
    Quat::from_xyzw(v.x, v.y, v.z, if q.w < 0.0 { -w } else { w })
}

/// Moves `a` towards `b` by `t`, bone by bone (positions lerped, rotations
/// blended linearly in the same hemisphere and renormalised).
fn blend_poses(a: &mut [(Vec3, Quat)], b: &[(Vec3, Quat)], t: f32) {
    if t <= 0.0 {
        return;
    }
    for (x, y) in a.iter_mut().zip(b) {
        if t >= 1.0 {
            *x = *y;
            continue;
        }
        let q = if x.1.dot(y.1) < 0.0 { -y.1 } else { y.1 };
        x.1 = (x.1 * (1.0 - t) + q * t).normalize();
        x.0 = x.0.lerp(y.0, t);
    }
}

/// The triangle of blend grid cell (`i0`, `i1`) that holds the fractions
/// (`s0`, `s1`): three corner offsets and their barycentric weights. The
/// diagonal alternates from cell to cell.
fn triangle(i0: usize, i1: usize, s0: f32, s1: f32) -> ([(usize, usize); 3], [f32; 3]) {
    let (corners, w0, w1) = if (i0 + i1) % 2 == 1 {
        if s0 + s1 > 1.0 { ([(1, 0), (1, 1), (0, 1)], 1.0 - s1, s0 + s1 - 1.0) } else { ([(0, 1), (0, 0), (1, 0)], s1, 1.0 - s0 - s1) }
    } else if s0 <= s1 {
        ([(1, 1), (0, 1), (0, 0)], s0, s1 - s0)
    } else {
        ([(0, 0), (1, 0), (1, 1)], 1.0 - s0, s0 - s1)
    };
    let w1 = if w1 < 0.001 { 0.0 } else { w1 };
    (corners, [w0, w1, 1.0 - w0 - w1])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn install() -> Option<GameFileSystem> {
        GameFileSystem::portal2(GameFileSystem::find_install()?).ok()
    }

    #[test]
    fn triangle_weights_hit_the_corners() {
        for (i0, i1) in [(0, 0), (1, 0), (0, 1), (3, 2)] {
            for (s0, s1, corner) in [(0.0, 0.0, (0, 0)), (1.0, 0.0, (1, 0)), (0.0, 1.0, (0, 1)), (1.0, 1.0, (1, 1))] {
                let (c, w) = triangle(i0, i1, s0, s1);
                let at: Vec<_> = c.iter().zip(w).filter(|(_, w)| *w > 0.999).map(|(c, _)| *c).collect();
                assert_eq!(at, vec![corner], "cell ({i0},{i1}) at ({s0},{s1}): {c:?} {w:?}");
            }
            let (_, w) = triangle(i0, i1, 0.3, 0.6);
            assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-6 && w.iter().all(|&w| w >= 0.0));
        }
    }

    #[test]
    fn rotation_scaling() {
        let q = Quat::from_rotation_z(1.0);
        let close = |a: Quat, b: Quat| (a - b).length() < 1e-5;
        assert!(close(scale_rotation(q, 0.5), Quat::from_rotation_z(0.5)));
        assert!(close(scale_rotation(q, 0.0), Quat::IDENTITY));
        assert!(close(scale_rotation(q, 1.0), q));
    }

    /// Security cameras aim through autoplay delta grids on `aim_yaw` /
    /// `aim_pitch`; at the default settings (value 0) the aim layers sit on
    /// their centre line and a full yaw reaches the last column.
    #[test]
    fn camera_aim_grid() {
        let Some(fs) = install() else { return };
        let Ok(m) = StudioModel::load(&fs, "models/props/security_camera.mdl") else { return };
        let yaw = m.find_pose_parameter("aim_yaw").expect("aim_yaw");
        assert_eq!(m.default_pose_settings()[yaw], 0.5);
        assert!(m.autoplay.iter().any(|&(s, _)| m.sequence(s).is_some_and(|(o, l)| o.mdl.sequences[l].label == "auto_leftright")));
        let (owner, local) = m.sequence(m.find_sequence("auto_leftright").unwrap()).unwrap();
        let desc = &owner.mdl.sequences[local];
        // Unevenly keyed columns: the walk stops at the first span that
        // reaches the value, so the centre reads as the end of span 1.
        assert!(!desc.pose_keys.is_empty());
        let mut settings = m.default_pose_settings();
        assert_eq!(m.grid_position(owner, desc, 0, &settings), (1, 1.0));
        settings[yaw] = m.pose_setting(yaw, 180.0);
        assert_eq!(m.grid_position(owner, desc, 0, &settings), (3, 1.0));
        settings[yaw] = m.pose_setting(yaw, 45.0);
        let (i, s) = m.grid_position(owner, desc, 0, &settings);
        assert_eq!(i, 2);
        assert!((s - 0.5).abs() < 1e-5);
        // The yaw deltas are relative to the -180 degree pose: the swivel
        // bone turns by yaw + 180 about its Y axis, so the camera faces
        // forward at the default and the autoplay layers must be applied.
        let idle = m.find_sequence("idle").unwrap();
        let swivel = m.mdl.find_bone("LR_Rot1").unwrap();
        let turn = |yaw_degrees: f32| {
            let mut settings = m.default_pose_settings();
            settings[yaw] = m.pose_setting(yaw, yaw_degrees);
            let mut p = m.bind_pose();
            m.accumulate_sequence(&mut p, idle, 0.0, 1.0, &settings);
            for &(s, _) in &m.autoplay {
                m.accumulate_sequence(&mut p, s, 0.0, 1.0, &settings);
            }
            p[swivel].1
        };
        for yaw_degrees in [0.0f32, 45.0, -90.0, 120.0] {
            let want = Quat::from_rotation_y((yaw_degrees + 180.0).to_radians());
            let got = turn(yaw_degrees);
            assert!(got.angle_between(want) < 0.02, "yaw {yaw_degrees}: {got:?}");
        }
    }

    /// Wheatley's model takes its aim parameters from the animation include.
    #[test]
    fn include_pose_parameters_merge() {
        let Some(fs) = install() else { return };
        let Ok(m) = StudioModel::load(&fs, "models/npcs/personality_sphere/personality_sphere_skins.mdl") else { return };
        assert!(m.mdl.pose_parameters.is_empty());
        assert!(m.find_pose_parameter("aim_yaw").is_some() && m.find_pose_parameter("aim_pitch").is_some());
    }
}
