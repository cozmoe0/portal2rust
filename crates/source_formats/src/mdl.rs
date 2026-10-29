//! Studio model (MDL v44-49) reader: skeleton, body parts, materials, skins,
//! attachments, sequences, pose parameters and compressed animations.

use glam::{Mat4, Quat, Vec3};

use crate::reader::{Reader, cstr_at};
use crate::{Error, Result};

pub mod anim_flags {
    pub const RAWPOS: u8 = 0x01;
    pub const RAWROT: u8 = 0x02;
    pub const ANIMPOS: u8 = 0x04;
    pub const ANIMROT: u8 = 0x08;
    pub const DELTA: u8 = 0x10;
    pub const RAWROT2: u8 = 0x20;
}

pub mod seq_flags {
    pub const LOOPING: i32 = 0x0001;
    pub const SNAP: i32 = 0x0002;
    /// Adds to the pose instead of replacing it.
    pub const DELTA: i32 = 0x0004;
    /// Always layered on top, whatever else plays.
    pub const AUTOPLAY: i32 = 0x0008;
    /// Delta applied in the bone's own frame (after the base rotation).
    pub const POST: i32 = 0x0010;
    pub const ALLZEROS: i32 = 0x0020;
    /// Stored as whole frames (per-bone constants and fixed-size frames)
    /// rather than run-length channels.
    pub const FRAMEANIM: i32 = 0x0040;
    /// The cycle is the setting of pose parameter `cycle_pose`.
    pub const CYCLEPOSE: i32 = 0x0080;
    /// The cycle follows the clock.
    pub const REALTIME: i32 = 0x0100;
    /// Has local-context layers (applied to its own pose before blending).
    pub const LOCAL: i32 = 0x0200;
    pub const HIDDEN: i32 = 0x0800;
    /// Blends in world space.
    pub const WORLD: i32 = 0x4000;
}

/// Flags of a sequence's automatic layers.
pub mod autolayer_flags {
    /// Ramp is smoothed (3t² - 2t³).
    pub const SPLINE: i32 = 0x0040;
    /// Ramp-out is pre-biased for a second layer cross-fading in.
    pub const XFADE: i32 = 0x0080;
    /// Ignores the parent sequence's weight.
    pub const NOBLEND: i32 = 0x0200;
    /// Applied to the parent sequence's own pose (local context).
    pub const LOCAL: i32 = 0x1000;
    /// Ramp follows a pose parameter instead of the cycle.
    pub const POSE: i32 = 0x4000;
}

#[derive(Clone, Debug)]
pub struct Bone {
    pub name: String,
    pub parent: i32,
    pub pos: Vec3,
    pub quat: Quat,
    /// Euler angles (x, y, z radians) matching `quat`; base for ANIMROT data.
    pub rot: Vec3,
    pub pos_scale: Vec3,
    pub rot_scale: Vec3,
    /// Bind pose: model space to bone space (3x4 stored row-major).
    pub pose_to_bone: Mat4,
    pub flags: i32,
    pub surface_prop: String,
    pub contents: i32,
    pub physics_bone: i32,
}

#[derive(Clone, Debug)]
pub struct Attachment {
    pub name: String,
    pub bone: i32,
    pub local: Mat4,
}

#[derive(Clone, Debug)]
pub struct Mesh {
    pub material: i32,
    pub num_vertices: i32,
    pub vertex_offset: i32,
    pub center: Vec3,
}

#[derive(Clone, Debug)]
pub struct Model {
    pub name: String,
    pub bounding_radius: f32,
    pub num_vertices: i32,
    /// Index of the first vertex of this model in the VVD vertex array.
    pub vertex_index: i32,
    pub meshes: Vec<Mesh>,
}

#[derive(Clone, Debug)]
pub struct BodyPart {
    pub name: String,
    pub base: i32,
    pub models: Vec<Model>,
}

#[derive(Clone, Debug)]
pub struct PoseParameter {
    pub name: String,
    pub flags: i32,
    pub start: f32,
    pub end: f32,
    pub loop_range: f32,
}

#[derive(Clone, Debug)]
pub struct Event {
    pub cycle: f32,
    pub event: i32,
    pub kind: i32,
    pub options: String,
    pub name: String,
}

/// A sequence layered automatically while its parent sequence plays.
#[derive(Clone, Debug, PartialEq)]
pub struct AutoLayer {
    /// Sequence index within the model that owns the parent sequence.
    pub sequence: i16,
    /// Pose parameter index within that model (for `POSE` layers).
    pub pose: i16,
    pub flags: i32,
    /// Ramp over the parent's cycle (or the pose value): influence starts at
    /// `start`, is full from `peak` to `tail` and ends at `end`.
    pub start: f32,
    pub peak: f32,
    pub tail: f32,
    pub end: f32,
}

#[derive(Clone, Debug)]
pub struct Sequence {
    pub label: String,
    pub activity_name: String,
    pub flags: i32,
    pub activity: i32,
    pub act_weight: i32,
    pub events: Vec<Event>,
    pub bbmin: Vec3,
    pub bbmax: Vec3,
    /// Blend grid of animation indices (groupsize[0] x groupsize[1]).
    pub anims: Vec<i16>,
    pub group_size: [i32; 2],
    /// Pose parameter (of the owning model) along each grid axis, or -1.
    pub param_index: [i32; 2],
    /// Pose values at the first and last grid column/row.
    pub param_start: [f32; 2],
    pub param_end: [f32; 2],
    /// Pose values of each grid line when they are not evenly spaced
    /// (`groupsize[0]` for axis 0, then `groupsize[1]` for axis 1).
    pub pose_keys: Vec<f32>,
    /// Blend weight per bone of the owning model (empty: all 1).
    pub weights: Vec<f32>,
    pub autolayers: Vec<AutoLayer>,
    /// Pose parameter that gives the cycle of `CYCLEPOSE` sequences.
    pub cycle_pose: i32,
    pub fade_in: f32,
    pub fade_out: f32,
    pub entry_node: i32,
    pub exit_node: i32,
    pub node_flags: i32,
    pub next_seq: i32,
    pub keyvalues: String,
    /// Which loaded model (0 = this one, n = include n-1) owns the animations.
    pub source: usize,
}

#[derive(Clone, Debug)]
pub struct AnimSection {
    pub anim_block: i32,
    pub anim_index: i32,
}

#[derive(Clone, Debug)]
pub struct AnimDesc {
    pub name: String,
    pub fps: f32,
    pub flags: i32,
    pub num_frames: i32,
    pub anim_block: i32,
    /// Offset of the bone data, relative to the animdesc (or to the block).
    pub anim_index: i32,
    /// Absolute offset of the animdesc in the file.
    pub offset: usize,
    pub section_frames: i32,
    pub sections: Vec<AnimSection>,
    pub movements: Vec<Movement>,
}

#[derive(Clone, Debug)]
pub struct Movement {
    pub end_frame: i32,
    pub motion_flags: i32,
    pub v0: f32,
    pub v1: f32,
    pub angle: f32,
    pub vector: Vec3,
    pub position: Vec3,
}

#[derive(Clone, Debug)]
pub struct HitBox {
    pub bone: i32,
    pub group: i32,
    pub bbmin: Vec3,
    pub bbmax: Vec3,
}

pub struct Mdl {
    pub data: Vec<u8>,
    pub version: i32,
    pub checksum: i32,
    pub name: String,
    pub eye_position: Vec3,
    pub illum_position: Vec3,
    pub hull_min: Vec3,
    pub hull_max: Vec3,
    pub view_bbmin: Vec3,
    pub view_bbmax: Vec3,
    pub flags: i32,
    pub bones: Vec<Bone>,
    pub hitboxes: Vec<HitBox>,
    pub anims: Vec<AnimDesc>,
    pub sequences: Vec<Sequence>,
    pub textures: Vec<String>,
    pub texture_dirs: Vec<String>,
    /// skins[family][skinref] = texture index.
    pub skins: Vec<Vec<i16>>,
    pub body_parts: Vec<BodyPart>,
    pub attachments: Vec<Attachment>,
    pub pose_parameters: Vec<PoseParameter>,
    pub surface_prop: String,
    pub keyvalues: String,
    pub mass: f32,
    pub contents: i32,
    pub include_models: Vec<String>,
    pub anim_block_name: String,
    pub anim_blocks: Vec<(i32, i32)>,
}

fn mat3x4(r: &mut Reader) -> Result<Mat4> {
    let mut m = [[0f32; 4]; 3];
    for row in m.iter_mut() {
        for v in row.iter_mut() {
            *v = r.f32()?;
        }
    }
    Ok(Mat4::from_cols_array(&[
        m[0][0], m[1][0], m[2][0], 0.0, m[0][1], m[1][1], m[2][1], 0.0, m[0][2], m[1][2], m[2][2], 0.0, m[0][3],
        m[1][3], m[2][3], 1.0,
    ]))
}

impl Mdl {
    pub fn parse(data: Vec<u8>) -> Result<Self> {
        let d = &data[..];
        let mut r = Reader::new(d);
        let id = r.bytes(4)?;
        if id != b"IDST" {
            return Err(Error::Magic { expected: "IDST", found: id.to_vec() });
        }
        let version = r.i32()?;
        if !(44..=49).contains(&version) {
            return Err(Error::Version(version as i64));
        }
        let checksum = r.i32()?;
        let name = r.fixed_str(64)?;
        let _length = r.i32()?;
        let eye_position = r.vec3()?;
        let illum_position = r.vec3()?;
        let hull_min = r.vec3()?;
        let hull_max = r.vec3()?;
        let view_bbmin = r.vec3()?;
        let view_bbmax = r.vec3()?;
        let flags = r.i32()?;
        let (num_bones, bone_index) = (r.i32()?, r.i32()?);
        let (_num_bone_controllers, _bone_controller_index) = (r.i32()?, r.i32()?);
        let (num_hitbox_sets, hitbox_set_index) = (r.i32()?, r.i32()?);
        let (num_anims, anim_index) = (r.i32()?, r.i32()?);
        let (num_seqs, seq_index) = (r.i32()?, r.i32()?);
        let (_activity_list_version, _events_indexed) = (r.i32()?, r.i32()?);
        let (num_textures, texture_index) = (r.i32()?, r.i32()?);
        let (num_cd_textures, cd_texture_index) = (r.i32()?, r.i32()?);
        let (num_skin_ref, num_skin_families, skin_index) = (r.i32()?, r.i32()?, r.i32()?);
        let (num_body_parts, body_part_index) = (r.i32()?, r.i32()?);
        let (num_attachments, attachment_index) = (r.i32()?, r.i32()?);
        r.skip(12)?; // nodes
        r.skip(8 * 5)?; // flexdesc, flexcontrollers, flexrules, ikchains, mouths
        let (num_pose_params, pose_param_index) = (r.i32()?, r.i32()?);
        let surface_prop_index = r.i32()?;
        let (keyvalue_index, keyvalue_size) = (r.i32()?, r.i32()?);
        r.skip(8)?; // ik autoplay locks
        let mass = r.f32()?;
        let contents = r.i32()?;
        let (num_include_models, include_model_index) = (r.i32()?, r.i32()?);
        let _virtual_model = r.i32()?;
        let anim_block_name_index = r.i32()?;
        let (num_anim_blocks, anim_block_index) = (r.i32()?, r.i32()?);

        let mut bones = Vec::with_capacity(num_bones.max(0) as usize);
        for i in 0..num_bones.max(0) as usize {
            let base = bone_index as usize + i * 216;
            let mut b = Reader::at(d, base);
            let name = cstr_at(d, base + b.i32()? as usize);
            let parent = b.i32()?;
            b.skip(24)?;
            let pos = b.vec3()?;
            let quat = b.quat()?;
            let rot = b.vec3()?;
            let pos_scale = b.vec3()?;
            let rot_scale = b.vec3()?;
            let pose_to_bone = mat3x4(&mut b)?;
            b.skip(16)?; // alignment quaternion
            let flags = b.i32()?;
            b.skip(8)?; // proctype, procindex
            let physics_bone = b.i32()?;
            let sp = b.i32()?;
            let contents = b.i32()?;
            bones.push(Bone {
                name,
                parent,
                pos,
                quat,
                rot,
                pos_scale,
                rot_scale,
                pose_to_bone,
                flags,
                surface_prop: cstr_at(d, base + sp as usize),
                contents,
                physics_bone,
            });
        }

        let mut hitboxes = Vec::new();
        if num_hitbox_sets > 0 {
            let set = hitbox_set_index as usize;
            let mut s = Reader::at(d, set + 4);
            let n = s.i32()?;
            let idx = s.i32()?;
            for i in 0..n.max(0) as usize {
                let mut h = Reader::at(d, set + idx as usize + i * 68);
                hitboxes.push(HitBox { bone: h.i32()?, group: h.i32()?, bbmin: h.vec3()?, bbmax: h.vec3()? });
            }
        }

        let mut anims = Vec::with_capacity(num_anims.max(0) as usize);
        for i in 0..num_anims.max(0) as usize {
            let base = anim_index as usize + i * 100;
            let mut a = Reader::at(d, base + 4);
            let name = cstr_at(d, base + a.i32()? as usize);
            let fps = a.f32()?;
            let aflags = a.i32()?;
            let num_frames = a.i32()?;
            let num_movements = a.i32()?;
            let movement_index = a.i32()?;
            a.skip(24)?;
            let anim_block = a.i32()?;
            let anim_idx = a.i32()?;
            a.skip(20)?; // ik rules, local hierarchy
            let section_index = a.i32()?;
            let section_frames = a.i32()?;
            let mut sections = Vec::new();
            if section_frames > 0 {
                let count = num_frames / section_frames + 2;
                for s in 0..count as usize {
                    let mut sr = Reader::at(d, base + section_index as usize + s * 8);
                    sections.push(AnimSection { anim_block: sr.i32()?, anim_index: sr.i32()? });
                }
            }
            let mut movements = Vec::new();
            for m in 0..num_movements.max(0) as usize {
                let mut mr = Reader::at(d, base + movement_index as usize + m * 44);
                movements.push(Movement {
                    end_frame: mr.i32()?,
                    motion_flags: mr.i32()?,
                    v0: mr.f32()?,
                    v1: mr.f32()?,
                    angle: mr.f32()?,
                    vector: mr.vec3()?,
                    position: mr.vec3()?,
                });
            }
            anims.push(AnimDesc {
                name,
                fps,
                flags: aflags,
                num_frames,
                anim_block,
                anim_index: anim_idx,
                offset: base,
                section_frames,
                sections,
                movements,
            });
        }

        let mut sequences = Vec::with_capacity(num_seqs.max(0) as usize);
        for i in 0..num_seqs.max(0) as usize {
            let base = seq_index as usize + i * 212;
            let mut s = Reader::at(d, base + 4);
            let label = cstr_at(d, base + s.i32()? as usize);
            let activity_name = cstr_at(d, base + s.i32()? as usize);
            let sflags = s.i32()?;
            let activity = s.i32()?;
            let act_weight = s.i32()?;
            let num_events = s.i32()?;
            let event_index = s.i32()?;
            let bbmin = s.vec3()?;
            let bbmax = s.vec3()?;
            let _num_blends = s.i32()?;
            let anim_index_index = s.i32()?;
            let _movement_index = s.i32()?;
            let group_size = [s.i32()?, s.i32()?];
            let param_index = [s.i32()?, s.i32()?];
            let param_start = [s.f32()?, s.f32()?];
            let param_end = [s.f32()?, s.f32()?];
            let _param_parent = s.i32()?;
            let fade_in = s.f32()?;
            let fade_out = s.f32()?;
            let entry_node = s.i32()?;
            let exit_node = s.i32()?;
            let node_flags = s.i32()?;
            s.skip(12)?; // entry/exit phase, last frame
            let next_seq = s.i32()?;
            s.skip(8)?; // pose, ik rules
            let num_autolayers = s.i32()?;
            let autolayer_index = s.i32()?;
            let weightlist_index = s.i32()?;
            let posekey_index = s.i32()?;
            s.skip(8)?; // ik locks
            let kv_index = s.i32()?;
            let kv_size = s.i32()?;
            let cycle_pose = s.i32()?;
            let mut autolayers = Vec::with_capacity(num_autolayers.max(0) as usize);
            for l in 0..num_autolayers.max(0) as usize {
                let mut lr = Reader::at(d, base + autolayer_index as usize + l * 24);
                autolayers.push(AutoLayer {
                    sequence: lr.i16()?,
                    pose: lr.i16()?,
                    flags: lr.i32()?,
                    start: lr.f32()?,
                    peak: lr.f32()?,
                    tail: lr.f32()?,
                    end: lr.f32()?,
                });
            }
            let mut weights = Vec::new();
            if weightlist_index > 0 {
                let mut wr = Reader::at(d, base + weightlist_index as usize);
                for _ in 0..num_bones.max(0) {
                    weights.push(wr.f32()?);
                }
            }
            let mut pose_keys = Vec::new();
            if posekey_index > 0 {
                let mut pr = Reader::at(d, base + posekey_index as usize);
                for _ in 0..group_size[0].max(0) + group_size[1].max(0) {
                    pose_keys.push(pr.f32()?);
                }
            }
            let mut events = Vec::with_capacity(num_events.max(0) as usize);
            for e in 0..num_events.max(0) as usize {
                let eb = base + event_index as usize + e * 80;
                let mut er = Reader::at(d, eb);
                let cycle = er.f32()?;
                let event = er.i32()?;
                let kind = er.i32()?;
                let options = er.fixed_str(64)?;
                let name_off = er.i32()?;
                events.push(Event { cycle, event, kind, options, name: cstr_at(d, eb + name_off as usize) });
            }
            let count = (group_size[0].max(1) * group_size[1].max(1)) as usize;
            let mut anims_grid = Vec::with_capacity(count);
            let mut ar = Reader::at(d, base + anim_index_index as usize);
            for _ in 0..count {
                anims_grid.push(ar.i16()?);
            }
            let keyvalues = if kv_size > 0 {
                String::from_utf8_lossy(d.get(base + kv_index as usize..base + (kv_index + kv_size) as usize).unwrap_or(&[]))
                    .trim_end_matches('\0')
                    .to_owned()
            } else {
                String::new()
            };
            sequences.push(Sequence {
                label,
                activity_name,
                flags: sflags,
                activity,
                act_weight,
                events,
                bbmin,
                bbmax,
                anims: anims_grid,
                group_size,
                param_index,
                param_start,
                param_end,
                pose_keys,
                weights,
                autolayers,
                cycle_pose,
                fade_in,
                fade_out,
                entry_node,
                exit_node,
                node_flags,
                next_seq,
                keyvalues,
                source: 0,
            });
        }

        let mut textures = Vec::with_capacity(num_textures.max(0) as usize);
        for i in 0..num_textures.max(0) as usize {
            let base = texture_index as usize + i * 64;
            let off = Reader::at(d, base).i32()?;
            textures.push(cstr_at(d, base + off as usize));
        }
        let mut texture_dirs = Vec::with_capacity(num_cd_textures.max(0) as usize);
        for i in 0..num_cd_textures.max(0) as usize {
            let off = Reader::at(d, cd_texture_index as usize + i * 4).i32()?;
            texture_dirs.push(cstr_at(d, off as usize));
        }
        let mut skins = Vec::with_capacity(num_skin_families.max(0) as usize);
        let mut sr = Reader::at(d, skin_index as usize);
        for _ in 0..num_skin_families.max(0) {
            let mut fam = Vec::with_capacity(num_skin_ref.max(0) as usize);
            for _ in 0..num_skin_ref.max(0) {
                fam.push(sr.i16()?);
            }
            skins.push(fam);
        }

        let mut body_parts = Vec::with_capacity(num_body_parts.max(0) as usize);
        for i in 0..num_body_parts.max(0) as usize {
            let bp = body_part_index as usize + i * 16;
            let mut b = Reader::at(d, bp);
            let bp_name = cstr_at(d, bp + b.i32()? as usize);
            let num_models = b.i32()?;
            let base = b.i32()?;
            let model_index = b.i32()?;
            let mut models = Vec::with_capacity(num_models.max(0) as usize);
            for m in 0..num_models.max(0) as usize {
                let mb = bp + model_index as usize + m * 148;
                let mut mr = Reader::at(d, mb);
                let mname = mr.fixed_str(64)?;
                let _type = mr.i32()?;
                let bounding_radius = mr.f32()?;
                let num_meshes = mr.i32()?;
                let mesh_index = mr.i32()?;
                let num_vertices = mr.i32()?;
                let vertex_index = mr.i32()?;
                let mut meshes = Vec::with_capacity(num_meshes.max(0) as usize);
                for k in 0..num_meshes.max(0) as usize {
                    let mut me = Reader::at(d, mb + mesh_index as usize + k * 116);
                    let material = me.i32()?;
                    let _model_index = me.i32()?;
                    let nv = me.i32()?;
                    let vo = me.i32()?;
                    me.skip(20)?;
                    let center = me.vec3()?;
                    meshes.push(Mesh { material, num_vertices: nv, vertex_offset: vo, center });
                }
                models.push(Model {
                    name: mname,
                    bounding_radius,
                    num_vertices,
                    vertex_index: vertex_index / 48,
                    meshes,
                });
            }
            body_parts.push(BodyPart { name: bp_name, base, models });
        }

        let mut attachments = Vec::with_capacity(num_attachments.max(0) as usize);
        for i in 0..num_attachments.max(0) as usize {
            let base = attachment_index as usize + i * 92;
            let mut a = Reader::at(d, base);
            let aname = cstr_at(d, base + a.i32()? as usize);
            let _flags = a.u32()?;
            let bone = a.i32()?;
            let local = mat3x4(&mut a)?;
            attachments.push(Attachment { name: aname, bone, local });
        }

        let mut pose_parameters = Vec::with_capacity(num_pose_params.max(0) as usize);
        for i in 0..num_pose_params.max(0) as usize {
            let base = pose_param_index as usize + i * 20;
            let mut p = Reader::at(d, base);
            let pname = cstr_at(d, base + p.i32()? as usize);
            pose_parameters.push(PoseParameter {
                name: pname,
                flags: p.i32()?,
                start: p.f32()?,
                end: p.f32()?,
                loop_range: p.f32()?,
            });
        }

        let mut include_models = Vec::new();
        for i in 0..num_include_models.max(0) as usize {
            let base = include_model_index as usize + i * 8;
            let mut ir = Reader::at(d, base + 4);
            let name_off = ir.i32()?;
            include_models.push(cstr_at(d, base + name_off as usize));
        }
        let mut anim_blocks = Vec::new();
        for i in 0..num_anim_blocks.max(0) as usize {
            let mut ar = Reader::at(d, anim_block_index as usize + i * 8);
            anim_blocks.push((ar.i32()?, ar.i32()?));
        }

        let surface_prop = cstr_at(d, surface_prop_index as usize);
        let keyvalues = if keyvalue_size > 0 {
            String::from_utf8_lossy(
                d.get(keyvalue_index as usize..(keyvalue_index + keyvalue_size) as usize).unwrap_or(&[]),
            )
            .trim_end_matches('\0')
            .to_owned()
        } else {
            String::new()
        };
        let anim_block_name = if anim_block_name_index > 0 { cstr_at(d, anim_block_name_index as usize) } else { String::new() };

        Ok(Self {
            version,
            checksum,
            name,
            eye_position,
            illum_position,
            hull_min,
            hull_max,
            view_bbmin,
            view_bbmax,
            flags,
            bones,
            hitboxes,
            anims,
            sequences,
            textures,
            texture_dirs,
            skins,
            body_parts,
            attachments,
            pose_parameters,
            surface_prop,
            keyvalues,
            mass,
            contents,
            include_models,
            anim_block_name,
            anim_blocks,
            data,
        })
    }

    pub fn find_sequence(&self, name: &str) -> Option<usize> {
        self.sequences.iter().position(|s| s.label.eq_ignore_ascii_case(name))
    }

    pub fn find_sequence_by_activity(&self, activity: &str) -> Option<usize> {
        self.sequences.iter().position(|s| s.activity_name.eq_ignore_ascii_case(activity))
    }

    pub fn find_bone(&self, name: &str) -> Option<usize> {
        self.bones.iter().position(|b| b.name.eq_ignore_ascii_case(name))
    }

    pub fn find_attachment(&self, name: &str) -> Option<usize> {
        self.attachments.iter().position(|a| a.name.eq_ignore_ascii_case(name))
    }

    pub fn find_pose_parameter(&self, name: &str) -> Option<usize> {
        self.pose_parameters.iter().position(|p| p.name.eq_ignore_ascii_case(name))
    }
}

/// Unpacks a Quaternion48 (16/16/15 bit components with a w sign bit).
pub fn quat48(b: &[u8]) -> Quat {
    let x = u16::from_le_bytes([b[0], b[1]]) as i32;
    let y = u16::from_le_bytes([b[2], b[3]]) as i32;
    let zw = u16::from_le_bytes([b[4], b[5]]) as i32;
    let z = zw & 0x7FFF;
    let wneg = zw >> 15 != 0;
    let fx = (x - 32768) as f32 / 32768.0;
    let fy = (y - 32768) as f32 / 32768.0;
    let fz = (z - 16384) as f32 / 16384.0;
    let mut w = (1.0 - fx * fx - fy * fy - fz * fz).max(0.0).sqrt();
    if wneg {
        w = -w;
    }
    Quat::from_xyzw(fx, fy, fz, w)
}

/// Unpacks a Quaternion64 (21/21/21 bit components with a w sign bit).
pub fn quat64(b: &[u8]) -> Quat {
    let v = u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
    let x = (v & 0x1F_FFFF) as i64;
    let y = ((v >> 21) & 0x1F_FFFF) as i64;
    let z = ((v >> 42) & 0x1F_FFFF) as i64;
    let wneg = (v >> 63) != 0;
    let fx = (x - 1_048_576) as f32 / 1_048_576.5;
    let fy = (y - 1_048_576) as f32 / 1_048_576.5;
    let fz = (z - 1_048_576) as f32 / 1_048_576.5;
    let mut w = (1.0 - fx * fx - fy * fy - fz * fz).max(0.0).sqrt();
    if wneg {
        w = -w;
    }
    Quat::from_xyzw(fx, fy, fz, w)
}

/// 48-bit "smallest three" quaternion: three 15-bit components (offset
/// 16384, 23168 per unit). The top bits of the first two words give the
/// (x, y, z, w) index of the first stored component; the other two follow
/// it, wrapping round, and the one left out is rebuilt from the unit
/// length, negated when the third word's top bit is set.
pub fn quat48s(b: &[u8]) -> Quat {
    let word = |i: usize| u16::from_le_bytes([b[i * 2], b[i * 2 + 1]]);
    let (a, bb, c) = (word(0), word(1), word(2));
    let first = (((a >> 15) << 1) | (bb >> 15)) as usize;
    let comp = |w: u16| ((w & 0x7FFF) as f32 - 16384.0) / 23168.0;
    let mut q = [0f32; 4];
    q[first] = comp(a);
    q[(first + 1) & 3] = comp(bb);
    q[(first + 2) & 3] = comp(c);
    let rest = q[first].powi(2) + q[(first + 1) & 3].powi(2) + q[(first + 2) & 3].powi(2);
    q[(first + 3) & 3] = (1.0 - rest).max(0.0).sqrt() * if c >> 15 != 0 { -1.0 } else { 1.0 };
    Quat::from_xyzw(q[0], q[1], q[2], q[3])
}

pub fn vector48(b: &[u8]) -> Vec3 {
    let h = |o: usize| crate::vtf::f16_to_f32(u16::from_le_bytes([b[o], b[o + 1]]));
    Vec3::new(h(0), h(2), h(4))
}

/// Source radian euler (x, y, z) to quaternion, same convention as entity angles.
pub fn euler_to_quat(e: Vec3) -> Quat {
    Quat::from_rotation_z(e.z) * Quat::from_rotation_y(e.y) * Quat::from_rotation_x(e.x)
}

/// Reads one channel of RLE-compressed animation values at `frame`.
fn anim_value(d: &[u8], start: usize, frame: i32) -> f32 {
    let mut p = start;
    let mut k = frame;
    // Each run header is {valid: u8, total: u8} followed by `valid` shorts.
    loop {
        let (Some(&valid), Some(&total)) = (d.get(p), d.get(p + 1)) else { return 0.0 };
        if total == 0 {
            return 0.0;
        }
        if (total as i32) > k {
            let idx = if (valid as i32) > k { k + 1 } else { valid as i32 };
            let o = p + idx as usize * 2;
            return d.get(o..o + 2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32).unwrap_or(0.0);
        }
        k -= total as i32;
        p += (valid as usize + 1) * 2;
    }
}

impl Mdl {
    /// Samples local bone transforms (position, rotation) of an animation at a
    /// frame (fractional frames are interpolated). `block_data` provides the
    /// contents of external animation blocks (`.ani`) when needed.
    pub fn sample_anim(
        &self,
        anim: usize,
        frame: f32,
        block_data: Option<&[u8]>,
        out: &mut [(Vec3, Quat)],
    ) -> bool {
        let Some(desc) = self.anims.get(anim) else { return false };
        let max_frame = (desc.num_frames - 1).max(0);
        let f0 = (frame.floor() as i32).clamp(0, max_frame);
        let f1 = (f0 + 1).min(max_frame);
        let t = (frame - f0 as f32).clamp(0.0, 1.0);
        let a = self.sample_frame(desc, f0, block_data);
        if t <= 0.0 || f0 == f1 {
            for (i, v) in a.into_iter().enumerate().take(out.len()) {
                out[i] = v;
            }
            return true;
        }
        let b = self.sample_frame(desc, f1, block_data);
        for i in 0..out.len().min(a.len()) {
            out[i] = (a[i].0.lerp(b[i].0, t), a[i].1.slerp(b[i].1, t));
        }
        true
    }

    fn sample_frame(&self, desc: &AnimDesc, frame: i32, block_data: Option<&[u8]>) -> Vec<(Vec3, Quat)> {
        let delta = desc.flags & seq_flags::DELTA != 0;
        let mut out: Vec<(Vec3, Quat)> = self
            .bones
            .iter()
            .map(|b| if delta { (Vec3::ZERO, Quat::IDENTITY) } else { (b.pos, b.quat) })
            .collect();

        let (mut block, mut index, mut local_frame) = (desc.anim_block, desc.anim_index, frame);
        if desc.section_frames > 0 && !desc.sections.is_empty() {
            let s = ((frame / desc.section_frames) as usize).min(desc.sections.len() - 1);
            block = desc.sections[s].anim_block;
            index = desc.sections[s].anim_index;
            local_frame = frame - s as i32 * desc.section_frames;
        }
        let (data, mut p): (&[u8], usize) = if block == 0 {
            (&self.data, desc.offset + index as usize)
        } else {
            let Some(bd) = block_data else { return out };
            let Some(&(start, _end)) = self.anim_blocks.get(block as usize) else { return out };
            (bd, start as usize + index as usize)
        };
        if index == 0 && block == 0 {
            return out;
        }
        if desc.flags & seq_flags::FRAMEANIM != 0 {
            self.sample_frame_anim(data, p, local_frame, &mut out);
            return out;
        }
        for _ in 0..self.bones.len() + 1 {
            let (Some(&bone), Some(&flags)) = (data.get(p), data.get(p + 1)) else { break };
            let next = data.get(p + 2..p + 4).map(|b| i16::from_le_bytes([b[0], b[1]])).unwrap_or(0);
            let bone = bone as usize;
            if bone < self.bones.len() {
                let b = &self.bones[bone];
                let mut q = 4usize;
                let rot = if flags & anim_flags::RAWROT != 0 {
                    let r = data.get(p + q..p + q + 6).map(quat48);
                    q += 6;
                    r
                } else if flags & anim_flags::RAWROT2 != 0 {
                    let r = data.get(p + q..p + q + 8).map(quat64);
                    q += 8;
                    r
                } else if flags & anim_flags::ANIMROT != 0 {
                    let vp = p + q;
                    q += 6;
                    let mut e = Vec3::ZERO;
                    for axis in 0..3 {
                        let off = data.get(vp + axis * 2..vp + axis * 2 + 2).map(|b| i16::from_le_bytes([b[0], b[1]])).unwrap_or(0);
                        if off > 0 {
                            e[axis] = anim_value(data, vp + off as usize, local_frame) * b.rot_scale[axis];
                        }
                    }
                    if flags & anim_flags::DELTA == 0 {
                        e += b.rot;
                    }
                    Some(euler_to_quat(e))
                } else {
                    None
                };
                let pos = if flags & anim_flags::RAWPOS != 0 {
                    data.get(p + q..p + q + 6).map(vector48)
                } else if flags & anim_flags::ANIMPOS != 0 {
                    let vp = p + q;
                    let mut v = Vec3::ZERO;
                    for axis in 0..3 {
                        let off = data.get(vp + axis * 2..vp + axis * 2 + 2).map(|b| i16::from_le_bytes([b[0], b[1]])).unwrap_or(0);
                        if off > 0 {
                            v[axis] = anim_value(data, vp + off as usize, local_frame) * b.pos_scale[axis];
                        }
                    }
                    if flags & anim_flags::DELTA == 0 {
                        v += b.pos;
                    }
                    Some(v)
                } else {
                    None
                };
                if let Some(r) = rot {
                    out[bone].1 = r.normalize();
                }
                if let Some(v) = pos {
                    out[bone].0 = v;
                }
            }
            if next == 0 {
                break;
            }
            p = (p as isize + next as isize) as usize;
        }
        out
    }

    /// A frame of a whole-frame animation at `base`: a header (constants
    /// offset, frame offset, frame length, 12 unused bytes), then one flag
    /// byte per bone. Each bone takes its rotation and then its position
    /// from the constants or from the frame's data, in bone order.
    fn sample_frame_anim(&self, data: &[u8], base: usize, frame: i32, out: &mut [(Vec3, Quat)]) {
        use frame_flags::*;
        let int = |at: usize| data.get(at..at + 4).map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]])).unwrap_or(0);
        let float = |at: usize| data.get(at..at + 4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).unwrap_or(0.0);
        let vector = |at: usize| Vec3::new(float(at), float(at + 4), float(at + 8));
        let mut constants = base + int(base).max(0) as usize;
        let mut frame_data = base + int(base + 4).max(0) as usize + frame.max(0) as usize * int(base + 8).max(0) as usize;
        // Reads `len` bytes from a stream and moves it on.
        let take = |at: &mut usize, len: usize| {
            let b = data.get(*at..*at + len);
            *at += len;
            b
        };
        for (bone, slot) in out.iter_mut().enumerate().take(self.bones.len()) {
            let Some(&flags) = data.get(base + 24 + bone) else { break };
            if flags & CONST_ROT != 0 {
                slot.1 = take(&mut constants, 6).map_or(slot.1, |b| quat48s(b).normalize());
            }
            if flags & RAW_ROT != 0 {
                slot.1 = take(&mut constants, 6).map_or(slot.1, |b| quat48(b).normalize());
            }
            if flags & ANIM_ROT != 0 {
                slot.1 = take(&mut frame_data, 6).map_or(slot.1, |b| quat48s(b).normalize());
            }
            if flags & ANIM_RAW_ROT != 0 {
                slot.1 = take(&mut frame_data, 6).map_or(slot.1, |b| quat48(b).normalize());
            }
            if flags & CONST_POS != 0 {
                slot.0 = vector(constants);
                constants += 12;
            }
            if flags & RAW_POS != 0 {
                slot.0 = take(&mut constants, 6).map_or(slot.0, vector48);
            }
            if flags & ANIM_POS != 0 {
                slot.0 = vector(frame_data);
                frame_data += 12;
            }
            if flags & ANIM_RAW_POS != 0 {
                slot.0 = take(&mut frame_data, 6).map_or(slot.0, vector48);
            }
        }
    }
}

/// Per-bone flags of whole-frame animations.
pub mod frame_flags {
    /// Position: a half-float vector in the constants.
    pub const RAW_POS: u8 = 0x01;
    /// Rotation: a 48-bit quaternion in the constants.
    pub const RAW_ROT: u8 = 0x02;
    /// Position: a half-float vector in each frame.
    pub const ANIM_RAW_POS: u8 = 0x04;
    /// Rotation: a 48-bit quaternion in each frame.
    pub const ANIM_RAW_ROT: u8 = 0x08;
    /// Position: a float vector in each frame.
    pub const ANIM_POS: u8 = 0x10;
    /// Position: a float vector in the constants.
    pub const CONST_POS: u8 = 0x20;
    /// Rotation: a 48-bit smallest-three quaternion in the constants.
    pub const CONST_ROT: u8 = 0x40;
    /// Rotation: a 48-bit smallest-three quaternion in each frame.
    pub const ANIM_ROT: u8 = 0x80;
}
