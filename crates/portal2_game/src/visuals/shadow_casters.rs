//! What the shadow maps show, and when that changes.
//!
//! The projected textures' maps (`visuals/projected.rs`) and the overhaul's
//! (`visuals/fx/shadows.rs`) are drawn by cameras into whose views only
//! opaque world and model meshes draw (every other shader discards in
//! shadow views). A map drawn once therefore stays right until one of those
//! within its view moves, animates, appears, disappears, changes shape or
//! changes which of its pixels draw (its material's alpha test,
//! translucency, culling or portal clip plane), or until the renderer
//! finishes a pipeline it had to leave meshes out of a pass for.
//! [`ShadowCasters`] notes all of that each frame, so that the maps can be
//! redrawn exactly then and kept otherwise.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use bevy::camera::primitives::{Aabb, Frustum, Sphere};
use bevy::camera::visibility::{NoFrustumCulling, RenderLayers, VisibilitySystems};
use bevy::math::Affine3A;
use bevy::mesh::skinning::SkinnedMesh;
use bevy::prelude::*;
use bevy::render::render_resource::PipelineCache;
use bevy::render::{Render, RenderApp, RenderSystems};
use portal2_render::source_material::{ModelMaterial, WorldMaterial};

pub struct ShadowCastersPlugin;

/// Notes the frame's changes; the shadow cameras are aimed after it.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ShadowCastersSet;

impl Plugin for ShadowCastersPlugin {
    fn build(&self, app: &mut App) {
        let pending = PipelinesPending::default();
        app.init_resource::<ShadowCasters>().insert_resource(pending.clone()).add_systems(
            PostUpdate,
            track_casters.in_set(ShadowCastersSet).after(VisibilitySystems::VisibilityPropagate).after(VisibilitySystems::CalculateBounds),
        );
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app.insert_resource(pending).add_systems(Render, note_pending_pipelines.in_set(RenderSystems::Cleanup));
        }
    }
}

/// Reach added around a skinned model's joints, whose vertices lie a little
/// beyond them.
const SKIN_MARGIN: f32 = 48.0;

/// How far a written transform may differ from the drawn one and still
/// count as the same.
const SAME_EPSILON: f32 = 1e-4;

/// Meshes that draw into the shadow maps (opaque world and model meshes in
/// the shadow views' layer) as last seen, and the bounds of what changed
/// among them this frame. Changes are told by value: many models have
/// their transforms or poses written again every frame without moving.
#[derive(Resource, Default)]
pub struct ShadowCasters {
    casters: HashMap<Entity, Caster>,
    poses: HashMap<Entity, Vec<Affine3A>>,
    changed: Vec<CasterBounds>,
    /// A pass since last frame may have left meshes out, their pipelines
    /// still compiling.
    settling: bool,
}

impl ShadowCasters {
    /// Whether a map drawn before this frame may come out differently now,
    /// given which places show in it.
    pub fn stale(&self, shows: impl Fn(&CasterBounds) -> bool) -> bool {
        self.settling || self.changed.iter().any(shows)
    }
}

/// Where a caster changed: a sphere around what of it changed, and the box
/// the shadow views cull the caster by (none for one they never cull).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct CasterBounds {
    pub center: Vec3,
    pub radius: f32,
    pub culled_by: Option<(Aabb, Affine3A)>,
}

impl CasterBounds {
    /// Whether the change can show in a view with this frustum. Views cull
    /// meshes as Bevy does, by the frustum's sides and near plane (meshes
    /// past the far plane still draw).
    pub fn in_frustum(&self, frustum: &Frustum) -> bool {
        frustum.intersects_sphere(&Sphere { center: self.center.into(), radius: self.radius }, false)
            && self.culled_by.is_none_or(|(aabb, transform)| frustum.intersects_obb(&aabb, &transform, true, false))
    }
}

/// Set by the render world after a frame with pipelines still compiling:
/// the meshes using them were left out of its passes.
#[derive(Resource, Clone, Default)]
struct PipelinesPending(Arc<AtomicBool>);

fn note_pending_pipelines(cache: Res<PipelineCache>, pending: Res<PipelinesPending>) {
    if cache.waiting_pipelines().next().is_some() {
        pending.0.store(true, Ordering::Relaxed);
    }
}

#[derive(Clone, Copy, PartialEq)]
struct Caster {
    transform: Affine3A,
    center: Vec3,
    radius: f32,
    culled_by: Option<(Aabb, Affine3A)>,
    mesh: AssetId<Mesh>,
    material: CasterMaterial,
    look: Option<Look>,
}

impl Caster {
    fn same(&self, other: &Caster) -> bool {
        self.mesh == other.mesh
            && self.material == other.material
            && self.look == other.look
            && self.transform.abs_diff_eq(other.transform, SAME_EPSILON)
            && self.center.abs_diff_eq(other.center, SAME_EPSILON)
            && (self.radius - other.radius).abs() <= SAME_EPSILON
    }

    fn bounds(&self) -> CasterBounds {
        CasterBounds { center: self.center, radius: self.radius, culled_by: self.culled_by }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum CasterMaterial {
    World(AssetId<WorldMaterial>),
    Model(AssetId<ModelMaterial>),
}

/// What of a material shows in a shadow map: which of its pixels draw
/// (translucency, the alpha test and what it tests, the portal clip plane,
/// culling) and their depth bias.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Look {
    flags: u32,
    alpha: f32,
    alpha_ref: f32,
    clip_plane: Vec4,
    textures: [Option<AssetId<Image>>; 3],
    alpha_mode: AlphaMode,
    cull: Option<bevy::render::render_resource::Face>,
    depth_bias: i32,
}

impl CasterMaterial {
    fn look(&self, world: &Assets<WorldMaterial>, model: &Assets<ModelMaterial>) -> Option<Look> {
        match self {
            CasterMaterial::World(id) => world.get(*id).map(|m| Look {
                flags: m.params.flags,
                alpha: m.params.color.w,
                alpha_ref: m.params.alpha_ref,
                clip_plane: Vec4::ZERO,
                textures: [m.base.as_ref().map(|h| h.id()), m.base2.as_ref().map(|h| h.id()), m.blend_modulate.as_ref().map(|h| h.id())],
                alpha_mode: m.alpha_mode,
                cull: m.cull,
                depth_bias: m.depth_bias,
            }),
            CasterMaterial::Model(id) => model.get(*id).map(|m| Look {
                flags: m.params.flags,
                alpha: m.params.color.w,
                alpha_ref: m.params.alpha_ref,
                clip_plane: m.params.clip_plane,
                textures: [m.base.as_ref().map(|h| h.id()), None, None],
                alpha_mode: m.alpha_mode,
                cull: m.cull,
                depth_bias: m.depth_bias,
            }),
        }
    }
}

/// A mesh's world bounding sphere.
fn sphere(gt: &GlobalTransform, aabb: Option<&Aabb>) -> (Vec3, f32) {
    match aabb {
        Some(b) => (gt.transform_point(b.center.into()), gt.radius_vec3a(b.half_extents)),
        None => (gt.translation(), SKIN_MARGIN),
    }
}

/// The box a mesh is culled by, unless it never is.
fn culled_by(gt: &GlobalTransform, aabb: Option<&Aabb>, no_culling: bool) -> Option<(Aabb, Affine3A)> {
    aabb.filter(|_| !no_culling).map(|b| (*b, gt.affine()))
}

/// A sphere around a skinned model's joints.
fn pose_sphere(pose: &[Affine3A]) -> Option<(Vec3, f32)> {
    let mut points = pose.iter().map(|a| Vec3::from(a.translation));
    let first = points.next()?;
    let (lo, hi) = points.fold((first, first), |(lo, hi), p| (lo.min(p), hi.max(p)));
    Some(((lo + hi) * 0.5, (hi - lo).length() * 0.5 + SKIN_MARGIN))
}

fn in_shadow_layer(layers: Option<&RenderLayers>) -> bool {
    layers.is_none_or(|l| l.intersects(&RenderLayers::layer(0)))
}

type CasterFilter = (With<Mesh3d>, Or<(With<MeshMaterial3d<WorldMaterial>>, With<MeshMaterial3d<ModelMaterial>>)>);

/// Notes where shadow casters moved, animated, appeared, disappeared,
/// changed shape or changed what of them draws since last frame (their old
/// and new places both), and whether the renderer has been leaving meshes
/// out.
#[allow(clippy::too_many_arguments)]
fn track_casters(
    mut casters: ResMut<ShadowCasters>,
    changed: Query<
        (
            Entity,
            &GlobalTransform,
            Option<&Aabb>,
            Option<&RenderLayers>,
            &InheritedVisibility,
            &Mesh3d,
            Option<&MeshMaterial3d<WorldMaterial>>,
            Option<&MeshMaterial3d<ModelMaterial>>,
            Has<NoFrustumCulling>,
        ),
        (
            CasterFilter,
            Or<(
                Changed<GlobalTransform>,
                Changed<InheritedVisibility>,
                Changed<Mesh3d>,
                Changed<Aabb>,
                Changed<RenderLayers>,
                Changed<MeshMaterial3d<WorldMaterial>>,
                Changed<MeshMaterial3d<ModelMaterial>>,
                Changed<NoFrustumCulling>,
            )>,
        ),
    >,
    skinned: Query<
        (Entity, &SkinnedMesh, &GlobalTransform, Option<&Aabb>, Option<&RenderLayers>, &InheritedVisibility, Has<NoFrustumCulling>),
        CasterFilter,
    >,
    joints: Query<Ref<GlobalTransform>>,
    mut removed: RemovedComponents<Mesh3d>,
    (mut mesh_events, mut world_events, mut model_events): (
        MessageReader<AssetEvent<Mesh>>,
        MessageReader<AssetEvent<WorldMaterial>>,
        MessageReader<AssetEvent<ModelMaterial>>,
    ),
    (world_materials, model_materials): (Res<Assets<WorldMaterial>>, Res<Assets<ModelMaterial>>),
    pending: Option<Res<PipelinesPending>>,
) {
    let state = &mut *casters;
    state.changed.clear();
    state.settling = pending.is_some_and(|p| p.0.swap(false, Ordering::Relaxed));
    for e in removed.read() {
        if let Some(old) = state.casters.remove(&e) {
            state.changed.push(old.bounds());
        }
        state.poses.remove(&e);
    }
    for (e, gt, aabb, layers, visible, mesh, world, model, no_culling) in &changed {
        let material = match (world, model) {
            (Some(w), _) => CasterMaterial::World(w.id()),
            (_, Some(m)) => CasterMaterial::Model(m.id()),
            _ => continue,
        };
        let old = state.casters.get(&e).copied();
        let new = (visible.get() && in_shadow_layer(layers)).then(|| {
            let (center, radius) = sphere(gt, aabb);
            let look = material.look(&world_materials, &model_materials);
            Caster { transform: gt.affine(), center, radius, culled_by: culled_by(gt, aabb, no_culling), mesh: mesh.id(), material, look }
        });
        if let (Some(a), Some(b)) = (old, new)
            && a.same(&b)
        {
            continue;
        }
        if let Some(a) = old {
            state.changed.push(a.bounds());
            state.casters.remove(&e);
        }
        if let Some(b) = new {
            state.changed.push(b.bounds());
            state.casters.insert(e, b);
        }
    }
    // Reshaped meshes, and materials whose drawn pixels changed (most edits,
    // such as relighting a model, leave them as they were).
    let meshes: HashSet<AssetId<Mesh>> =
        mesh_events.read().filter_map(|e| if let AssetEvent::Modified { id } = e { Some(*id) } else { None }).collect();
    let mut materials: HashSet<CasterMaterial> = world_events
        .read()
        .filter_map(|e| if let AssetEvent::Modified { id } = e { Some(CasterMaterial::World(*id)) } else { None })
        .collect();
    materials.extend(
        model_events.read().filter_map(|e| if let AssetEvent::Modified { id } = e { Some(CasterMaterial::Model(*id)) } else { None }),
    );
    if !meshes.is_empty() || !materials.is_empty() {
        for c in state.casters.values_mut() {
            if meshes.contains(&c.mesh) {
                state.changed.push(c.bounds());
            }
            if materials.contains(&c.material) {
                let look = c.material.look(&world_materials, &model_materials);
                if look != c.look {
                    c.look = look;
                    state.changed.push(c.bounds());
                }
            }
        }
    }
    for (e, skin, gt, aabb, layers, visible, no_culling) in &skinned {
        if !visible.get() || !in_shadow_layer(layers) || !skin.joints.iter().any(|j| joints.get(*j).is_ok_and(|t| t.is_changed())) {
            continue;
        }
        let pose: Vec<Affine3A> = skin.joints.iter().map(|j| joints.get(*j).map_or(Affine3A::IDENTITY, |t| t.affine())).collect();
        let old = state.poses.get(&e);
        if old.is_some_and(|old| old.len() == pose.len() && old.iter().zip(&pose).all(|(a, b)| a.abs_diff_eq(*b, SAME_EPSILON))) {
            continue;
        }
        // The vertices may stray from the box, but the box decides whether
        // they draw at all (a change of box is noted above).
        let culled_by = culled_by(gt, aabb, no_culling);
        let spheres = [Some(sphere(gt, aabb)), old.and_then(|o| pose_sphere(o)), pose_sphere(&pose)];
        state.changed.extend(spheres.into_iter().flatten().map(|(center, radius)| CasterBounds { center, radius, culled_by }));
        state.poses.insert(e, pose);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use portal2_render::source_material::world_flags;

    /// An app tracking shadow casters, without a renderer.
    pub(crate) fn app() -> App {
        let mut app = App::new();
        app.init_resource::<ShadowCasters>()
            .insert_resource(Assets::<WorldMaterial>::default())
            .insert_resource(Assets::<ModelMaterial>::default())
            .add_message::<AssetEvent<Mesh>>()
            .add_message::<AssetEvent<WorldMaterial>>()
            .add_message::<AssetEvent<ModelMaterial>>()
            .add_systems(Update, track_casters.in_set(ShadowCastersSet));
        app
    }

    /// Spawns a world mesh at `at` with these half extents.
    pub(crate) fn spawn_caster(app: &mut App, at: Vec3, half: Vec3) -> Entity {
        let material = app.world_mut().resource_mut::<Assets<WorldMaterial>>().add(WorldMaterial::default());
        let bounds = Aabb::from_min_max(-half, half);
        app.world_mut()
            .spawn((Mesh3d(Handle::default()), MeshMaterial3d(material), GlobalTransform::from_translation(at), InheritedVisibility::VISIBLE, bounds))
            .id()
    }

    fn changed(app: &App) -> usize {
        app.world().resource::<ShadowCasters>().changed.len()
    }

    /// A caster counts as changed when it moves or what of it draws
    /// changes, not when its transform or material is written again with
    /// what it had.
    #[test]
    fn casters_change_by_value() {
        let mut app = app();
        let at = GlobalTransform::from_translation(Vec3::new(100.0, 0.0, 0.0));
        let e = spawn_caster(&mut app, at.translation(), Vec3::splat(8.0));
        app.update();
        assert_eq!(changed(&app), 1, "a new caster");
        app.update();
        assert_eq!(changed(&app), 0);
        *app.world_mut().get_mut::<GlobalTransform>(e).unwrap() = at;
        app.update();
        assert_eq!(changed(&app), 0, "written again in place");
        *app.world_mut().get_mut::<GlobalTransform>(e).unwrap() = GlobalTransform::from_translation(Vec3::new(140.0, 0.0, 0.0));
        app.update();
        assert_eq!(changed(&app), 2, "its old and new places");

        let id = app.world().get::<MeshMaterial3d<WorldMaterial>>(e).unwrap().id();
        app.world_mut().resource_mut::<Assets<WorldMaterial>>().get_mut_untracked(id).unwrap().params.lightmap_scale = 2.0;
        app.world_mut().write_message(AssetEvent::Modified { id });
        app.update();
        assert_eq!(changed(&app), 0, "an edit that draws the same pixels");
        app.world_mut().resource_mut::<Assets<WorldMaterial>>().get_mut_untracked(id).unwrap().params.flags |= world_flags::ALPHATEST;
        app.world_mut().write_message(AssetEvent::Modified { id });
        app.update();
        assert_eq!(changed(&app), 1, "alpha tested now");

        app.world_mut().despawn(e);
        app.update();
        assert_eq!(changed(&app), 1, "gone from its place");
    }

    /// A frame with pipelines still compiling makes every map stale once.
    #[test]
    fn pending_pipelines_settle() {
        let mut app = app();
        let pending = PipelinesPending::default();
        app.insert_resource(pending.clone());
        app.update();
        assert!(!app.world().resource::<ShadowCasters>().stale(|_| false));
        pending.0.store(true, Ordering::Relaxed);
        app.update();
        assert!(app.world().resource::<ShadowCasters>().stale(|_| false));
        app.update();
        assert!(!app.world().resource::<ShadowCasters>().stale(|_| false));
    }
}
