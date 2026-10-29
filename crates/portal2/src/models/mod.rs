//! Studio models on the GPU: mesh building, caching, static (baked) and
//! animated (skinned) instances, and per-instance lighting.

pub mod animation;
pub mod lighting;

use std::collections::HashMap;
use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::DynamicSkinnedMeshBounds;
use bevy::mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes};
use bevy::mesh::{Indices, PrimitiveTopology, VertexAttributeValues};
use bevy::prelude::*;
use source_formats::studio::{StudioMesh, StudioModel};
use source_formats::vhv::Vhv;

use crate::render::materials::{MaterialCache, MaterialContext};
use crate::render::source_material::{ModelMaterial, RefractMaterial, model_flags};
use portal2_core::vfs::GameFs;

pub use animation::StudioAnimator;

pub struct ModelsPlugin;

impl Plugin for ModelsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ModelCache>().add_plugins(animation::AnimationPlugin);
    }
}

/// A parsed model plus GPU meshes in bind pose (for skinning) and baked into
/// the first frame of its default sequence (for static use).
pub struct LoadedModel {
    pub path: String,
    pub studio: Arc<StudioModel>,
    pub skinned_meshes: Vec<Handle<Mesh>>,
    pub posed_meshes: Vec<Handle<Mesh>>,
    pub inverse_bindposes: Handle<SkinnedMeshInverseBindposes>,
    /// Model-space bone matrices of the default pose.
    pub default_pose: Vec<Mat4>,
}

impl LoadedModel {
    pub fn bone_count(&self) -> usize {
        self.studio.mdl.bones.len()
    }
}

#[derive(Resource, Default)]
pub struct ModelCache {
    map: HashMap<String, Option<Arc<LoadedModel>>>,
}

impl ModelCache {
    pub fn get(
        &mut self,
        path: &str,
        fs: &GameFs,
        meshes: &mut Assets<Mesh>,
        bindposes: &mut Assets<SkinnedMeshInverseBindposes>,
    ) -> Option<Arc<LoadedModel>> {
        let key = source_formats::text::normalize_path(path);
        if let Some(m) = self.map.get(&key) {
            return m.clone();
        }
        let loaded = match StudioModel::load(fs, &key) {
            Ok(studio) => Some(Arc::new(build_loaded_model(key.clone(), Arc::new(studio), meshes, bindposes))),
            Err(e) => {
                warn!("model {key}: {e}");
                None
            }
        };
        self.map.insert(key, loaded.clone());
        loaded
    }

    pub fn clear(&mut self) {
        self.map.clear();
    }
}

fn build_loaded_model(
    path: String,
    studio: Arc<StudioModel>,
    meshes: &mut Assets<Mesh>,
    bindposes: &mut Assets<SkinnedMeshInverseBindposes>,
) -> LoadedModel {
    let bones = &studio.mdl.bones;
    let inverse: Vec<Mat4> = bones.iter().map(|b| b.pose_to_bone).collect();
    let inverse_bindposes = bindposes.add(SkinnedMeshInverseBindposes::from(inverse));

    // Default pose: first frame of the first sequence with the autoplay
    // layers on top (falls back to bind pose).
    let mut local = studio.bind_pose();
    if studio.sequence_count() > 0 {
        studio.sample_sequence(0, 0.0, &mut local);
        let settings = studio.default_pose_settings();
        for &(seq, _) in &studio.autoplay {
            studio.accumulate_sequence(&mut local, seq, 0.0, 1.0, &settings);
        }
    }
    let default_pose = studio.bone_matrices(&local);
    let skin_mats: Vec<Mat4> = default_pose.iter().zip(bones).map(|(w, b)| *w * b.pose_to_bone).collect();

    let mut skinned_meshes = Vec::with_capacity(studio.meshes.len());
    let mut posed_meshes = Vec::with_capacity(studio.meshes.len());
    for m in &studio.meshes {
        skinned_meshes.push(meshes.add(studio_mesh(m, None)));
        posed_meshes.push(meshes.add(studio_mesh(m, Some(&skin_mats))));
    }
    LoadedModel { path, studio, skinned_meshes, posed_meshes, inverse_bindposes, default_pose }
}

/// Builds a Bevy mesh. With `pose`, vertices are skinned on the CPU into
/// model space and no joint attributes are emitted.
fn studio_mesh(m: &StudioMesh, pose: Option<&[Mat4]>) -> Mesh {
    let n = m.vertices.len();
    let mut positions = Vec::with_capacity(n);
    let mut normals = Vec::with_capacity(n);
    let mut uvs = Vec::with_capacity(n);
    let mut tangents = Vec::with_capacity(n);
    let mut joints = Vec::with_capacity(n);
    let mut weights = Vec::with_capacity(n);
    for (v, t) in m.vertices.iter().zip(&m.tangents) {
        let count = (v.num_bones as usize).clamp(1, 3);
        let mut w = [0f32; 4];
        let mut j = [0u16; 4];
        let mut total = 0.0;
        for k in 0..count {
            w[k] = v.weights[k];
            j[k] = v.bones[k] as u16;
            total += v.weights[k];
        }
        if total <= 0.0 {
            w = [1.0, 0.0, 0.0, 0.0];
        } else {
            for x in w.iter_mut() {
                *x /= total;
            }
        }
        match pose {
            Some(mats) => {
                let mut p = Vec3::ZERO;
                let mut nrm = Vec3::ZERO;
                let mut tan = Vec3::ZERO;
                for k in 0..count.max(1) {
                    let mat = mats.get(j[k] as usize).copied().unwrap_or(Mat4::IDENTITY);
                    p += mat.transform_point3(v.pos) * w[k];
                    nrm += mat.transform_vector3(v.normal) * w[k];
                    tan += mat.transform_vector3(t.truncate()) * w[k];
                }
                positions.push(p.to_array());
                normals.push(nrm.normalize_or(Vec3::Z).to_array());
                tangents.push(tan.normalize_or(Vec3::X).extend(t.w).to_array());
            }
            None => {
                positions.push(v.pos.to_array());
                normals.push(v.normal.to_array());
                tangents.push(t.to_array());
                joints.push(j);
                weights.push(w);
            }
        }
        uvs.push(v.uv.to_array());
    }
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
        .with_inserted_attribute(Mesh::ATTRIBUTE_TANGENT, tangents)
        .with_inserted_indices(Indices::U32(m.indices.clone()));
    if pose.is_none() {
        mesh.insert_attribute(Mesh::ATTRIBUTE_JOINT_INDEX, VertexAttributeValues::Uint16x4(joints));
        mesh.insert_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT, weights);
        // Per-joint bounds, so culling follows the animated pose rather than
        // the reference pose (sequences may move geometry far from it).
        if let Err(e) = mesh.generate_skinned_mesh_bounds() {
            warn!("skinned mesh bounds: {e}");
        }
    }
    mesh
}

/// Copies a posed mesh and attaches baked per-vertex lighting from a VHV mesh.
pub fn with_baked_lighting(base: &Mesh, studio_mesh: &StudioMesh, colors: &[Vec3]) -> Mesh {
    let mut mesh = base.clone();
    let mut per_vertex = vec![[0.0f32, 0.0, 0.0, 1.0]; studio_mesh.vertices.len()];
    for (hw, &mdl_vertex) in studio_mesh.hardware_vertex_ids.iter().enumerate() {
        if let (Some(c), Some(slot)) = (colors.get(hw), per_vertex.get_mut(mdl_vertex as usize)) {
            *slot = [c.x, c.y, c.z, 1.0];
        }
    }
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, per_vertex);
    mesh
}

/// Which model in each body part is visible for a `body` value.
pub fn body_selection(studio: &StudioModel, body: i32) -> Vec<usize> {
    studio
        .mdl
        .body_parts
        .iter()
        .map(|bp| {
            let n = bp.models.len().max(1) as i32;
            let base = bp.base.max(1);
            ((body / base) % n).max(0) as usize
        })
        .collect()
}

/// Resolved material per skin reference slot for a skin family.
pub fn skin_materials(studio: &StudioModel, fs: &GameFs, skin: usize) -> Vec<Option<String>> {
    let mdl = &studio.mdl;
    let family = mdl.skins.get(skin).or_else(|| mdl.skins.first());
    let count = mdl.skins.first().map(|f| f.len()).unwrap_or(mdl.textures.len()).max(mdl.textures.len());
    (0..count)
        .map(|slot| {
            let tex = family.and_then(|f| f.get(slot)).map(|&t| t.max(0) as usize).unwrap_or(slot);
            studio.resolve_material(fs, tex)
        })
        .collect()
}

pub struct SpawnModel<'a> {
    pub model: &'a Arc<LoadedModel>,
    pub skin: usize,
    pub body: i32,
    /// Baked lighting (static props) or `None` for dynamic lighting.
    pub vhv: Option<&'a Vhv>,
    pub animated: bool,
    pub lighting: Option<lighting::ModelLighting>,
    /// Cube map for materials using `env_cubemap` (the baked sample nearest
    /// the model's lighting origin).
    pub envmap: Option<Handle<Image>>,
}

/// Spawns render meshes for a model under `root`. Returns joint entities
/// (bone order) when the model is animated.
pub fn spawn_model_parts(
    commands: &mut Commands,
    root: Entity,
    spec: SpawnModel,
    fs: &GameFs,
    ctx: &mut MaterialContext,
    material_cache: &mut MaterialCache,
    meshes: &mut Assets<Mesh>,
    model_materials: &mut Assets<ModelMaterial>,
    refract_materials: &mut Assets<RefractMaterial>,
) -> Vec<Entity> {
    let model = spec.model;
    let studio = &model.studio;
    let bodies = body_selection(studio, spec.body);
    let skins = skin_materials(studio, fs, spec.skin);

    let mut joints = Vec::new();
    if spec.animated {
        let bones = &studio.mdl.bones;
        joints.reserve(bones.len());
        for (i, b) in bones.iter().enumerate() {
            let parent = if b.parent >= 0 { joints[b.parent as usize] } else { root };
            let local =
                if b.parent >= 0 { model.default_pose[b.parent as usize].inverse() * model.default_pose[i] } else { model.default_pose[i] };
            let (_, rot, pos) = local.to_scale_rotation_translation();
            let j = commands.spawn((Name::new(b.name.clone()), Transform::from_translation(pos).with_rotation(rot))).id();
            commands.entity(parent).add_child(j);
            joints.push(j);
        }
    }

    for (mi, m) in studio.meshes.iter().enumerate() {
        if bodies.get(m.body_part).copied().unwrap_or(0) != m.model {
            continue;
        }
        let material_path = skins.get(m.material).cloned().flatten();
        let baked = spec.vhv.is_some();
        let refract = material_path.as_deref().and_then(|p| material_cache.refract(p, ctx, refract_materials));
        let info = match (&refract, &material_path) {
            (Some(_), _) => None,
            (None, Some(p)) => Some(material_cache.model(p, baked, ctx, model_materials)),
            (None, None) => Some(material_cache.model("materials/__missing.vmt", baked, ctx, model_materials)),
        };
        if info.as_ref().is_some_and(|i| i.surface.shader.contains("eyeball") || i.surface.shader == "eyerefract") {
            continue;
        }
        let mesh_handle = if spec.animated {
            model.skinned_meshes[mi].clone()
        } else if let Some(colors) = spec.vhv.and_then(|v| v.meshes.get(m.vhv_mesh_index)) {
            let base = meshes.get(&model.posed_meshes[mi]).cloned();
            match base {
                Some(base) => meshes.add(with_baked_lighting(&base, m, colors)),
                None => model.posed_meshes[mi].clone(),
            }
        } else {
            model.posed_meshes[mi].clone()
        };
        let mut e = match (info, refract) {
            (Some(info), _) => {
                let envmap = spec.envmap.as_ref().filter(|_| info.env_cubemap);
                let material = match (&spec.lighting, envmap) {
                    (Some(l), _) => {
                        let mut mat = model_materials.get(&info.handle).cloned().unwrap_or_default();
                        l.apply(&mut mat.params);
                        if let Some(env) = envmap {
                            mat.envmap = Some(env.clone());
                            mat.params.flags |= model_flags::ENVMAP;
                        }
                        model_materials.add(mat)
                    }
                    // Baked lighting lives in the mesh, so props near the same
                    // probe can share one material.
                    (None, Some(env)) => material_cache.model_with_cubemap(&info.handle, env, model_materials),
                    (None, None) => info.handle.clone(),
                };
                commands.spawn((Mesh3d(mesh_handle), MeshMaterial3d(material), Transform::IDENTITY))
            }
            (None, Some(refract)) => {
                // Refract is unlit; only an `env_cubemap` needs the instance's probe.
                let material = match spec.envmap.as_ref().filter(|_| refract.env_cubemap) {
                    Some(env) => material_cache.refract_with_cubemap(&refract.handle, env, refract_materials),
                    None => refract.handle.clone(),
                };
                commands.spawn((Mesh3d(mesh_handle), MeshMaterial3d(material), Transform::IDENTITY))
            }
            (None, None) => continue,
        };
        if spec.animated {
            e.insert((
                SkinnedMesh { inverse_bindposes: model.inverse_bindposes.clone(), joints: joints.clone() },
                DynamicSkinnedMeshBounds,
            ));
        }
        let id = e.id();
        commands.entity(root).add_child(id);
    }
    joints
}
