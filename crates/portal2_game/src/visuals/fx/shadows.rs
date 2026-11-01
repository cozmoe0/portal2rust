//! Shadow maps for the overhaul's strongest lights (the portals first), so
//! a portal's glow stops at the cube or pillar in front of it.
//!
//! They work like the projected textures' maps (`visuals/projected.rs`): a
//! camera per map draws its view into a distance texture, which the world
//! and model shaders fill with the distance to the light. Every extra view
//! costs the render thread a full pass over the scene, so a map is only
//! redrawn when its light moves or takes the slot, and otherwise every
//! twelfth frame (staggered between maps) to follow moving props.
//!
//! Ultra keeps every map exact instead: only opaque world and model meshes
//! draw into them (every other shader discards in shadow views), so a map
//! is redrawn the frame anything of those within its light's reach moves,
//! animates, appears, disappears, changes shape or changes which of its
//! pixels draw (its material's alpha test, translucency, culling or portal
//! clip plane), and is kept otherwise. A portal's light over a still scene
//! then costs no extra passes.

use std::collections::{HashMap, HashSet};

use bevy::camera::primitives::Aabb;
use bevy::camera::visibility::{RenderLayers, VisibilitySystems};
use bevy::camera::{Exposure, Hdr, RenderTarget};
use bevy::math::Affine3A;
use bevy::mesh::skinning::SkinnedMesh;
use bevy::prelude::*;
use bevy::render::view::Tonemapping;
use portal2_render::fx::{FX, HEADER_FLOATS, LIGHT_FLOATS, MAX_SHADOWS};
use portal2_render::source_material::{ModelMaterial, WorldMaterial};

use super::lights::{FxActiveLights, FxLightShape};
use super::material::{FxBuffer, FxFillSet};
use super::{FxPreset, FxSettings, features};
use crate::visuals::projected::{SHADOW_CLEAR, SHADOW_SIZE, shadow_fog};
use portal2_core::coords::camera_rotation;

/// The shadow views' field of view: wide, since a portal lights a whole
/// hemisphere; what falls outside is left unshadowed.
const FOV_DEGREES: f32 = 130.0;
const NEAR: f32 = 1.0;
/// Past every overhaul light's range.
const FAR: f32 = 1000.0;
/// Where the buffer's shadow matrices and shadow info start.
const MATRICES: usize = 32;
const SHADOW_INFO: usize = 112;

/// Draws one of the overhaul's shadow maps.
#[derive(Component)]
pub struct FxShadowCamera(pub usize);

/// What each shadow map shows and when it was last drawn.
#[derive(Resource, Default)]
struct ShadowSlots {
    slots: [Option<SlotState>; MAX_SHADOWS],
    frame: u32,
}

#[derive(Clone, Copy, PartialEq)]
struct SlotState {
    position: Vec3,
    rotation: Quat,
    age: u32,
}

pub struct FxShadowsPlugin;

impl Plugin for FxShadowsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ShadowSlots>().init_resource::<ShadowCasters>().add_systems(Startup, spawn_cameras).add_systems(
            PostUpdate,
            (
                track_casters.after(VisibilitySystems::VisibilityPropagate).after(VisibilitySystems::CalculateBounds),
                assign.in_set(FxFillSet).after(super::lights::gather),
            )
                .chain(),
        );
    }
}

fn spawn_cameras(mut commands: Commands) {
    let Some(handles) = FX.get() else { return };
    for (slot, image) in handles.shadows.iter().enumerate() {
        commands.spawn((
            Name::new(format!("overhaul shadow {slot}")),
            FxShadowCamera(slot),
            Camera3d::default(),
            Camera {
                order: -40 + slot as isize,
                is_active: false,
                clear_color: ClearColorConfig::Custom(Color::linear_rgb(SHADOW_CLEAR, SHADOW_CLEAR, SHADOW_CLEAR)),
                ..default()
            },
            RenderTarget::Image(image.clone().into()),
            Hdr,
            Tonemapping::None,
            bevy::render::view::DebandDither::Disabled,
            Exposure::default(),
            Msaa::Off,
            Projection::Perspective(PerspectiveProjection {
                fov: FOV_DEGREES.to_radians(),
                aspect_ratio: 1.0,
                near: NEAR,
                far: FAR,
                ..default()
            }),
            shadow_fog(),
            RenderLayers::layer(0),
            Transform::default(),
        ));
    }
}

/// Frames between redraws of a shadow map whose light has not moved (a
/// moved light is redrawn at once), following props moving in the light;
/// none on ultra, which redraws exactly when what the map shows changes.
fn refresh_interval(preset: FxPreset) -> Option<u32> {
    (preset != FxPreset::Ultra).then_some(12)
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
struct ShadowCasters {
    casters: HashMap<Entity, Caster>,
    poses: HashMap<Entity, Vec<Affine3A>>,
    changed: Vec<(Vec3, f32)>,
}

#[derive(Clone, Copy, PartialEq)]
struct Caster {
    transform: Affine3A,
    center: Vec3,
    radius: f32,
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
/// and new places both).
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
            )>,
        ),
    >,
    skinned: Query<(Entity, &SkinnedMesh, &GlobalTransform, Option<&Aabb>, Option<&RenderLayers>, &InheritedVisibility), CasterFilter>,
    joints: Query<Ref<GlobalTransform>>,
    mut removed: RemovedComponents<Mesh3d>,
    (mut mesh_events, mut world_events, mut model_events): (
        MessageReader<AssetEvent<Mesh>>,
        MessageReader<AssetEvent<WorldMaterial>>,
        MessageReader<AssetEvent<ModelMaterial>>,
    ),
    (world_materials, model_materials): (Res<Assets<WorldMaterial>>, Res<Assets<ModelMaterial>>),
) {
    let state = &mut *casters;
    state.changed.clear();
    for e in removed.read() {
        if let Some(old) = state.casters.remove(&e) {
            state.changed.push((old.center, old.radius));
        }
        state.poses.remove(&e);
    }
    for (e, gt, aabb, layers, visible, mesh, world, model) in &changed {
        let material = match (world, model) {
            (Some(w), _) => CasterMaterial::World(w.id()),
            (_, Some(m)) => CasterMaterial::Model(m.id()),
            _ => continue,
        };
        let old = state.casters.get(&e).copied();
        let new = (visible.get() && in_shadow_layer(layers)).then(|| {
            let (center, radius) = sphere(gt, aabb);
            let look = material.look(&world_materials, &model_materials);
            Caster { transform: gt.affine(), center, radius, mesh: mesh.id(), material, look }
        });
        if let (Some(a), Some(b)) = (old, new)
            && a.same(&b)
        {
            continue;
        }
        if let Some(a) = old {
            state.changed.push((a.center, a.radius));
            state.casters.remove(&e);
        }
        if let Some(b) = new {
            state.changed.push((b.center, b.radius));
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
                state.changed.push((c.center, c.radius));
            }
            if materials.contains(&c.material) {
                let look = c.material.look(&world_materials, &model_materials);
                if look != c.look {
                    c.look = look;
                    state.changed.push((c.center, c.radius));
                }
            }
        }
    }
    for (e, skin, gt, aabb, layers, visible) in &skinned {
        if !visible.get() || !in_shadow_layer(layers) || !skin.joints.iter().any(|j| joints.get(*j).is_ok_and(|t| t.is_changed())) {
            continue;
        }
        let pose: Vec<Affine3A> = skin.joints.iter().map(|j| joints.get(*j).map_or(Affine3A::IDENTITY, |t| t.affine())).collect();
        let old = state.poses.get(&e);
        if old.is_some_and(|old| old.len() == pose.len() && old.iter().zip(&pose).all(|(a, b)| a.abs_diff_eq(*b, SAME_EPSILON))) {
            continue;
        }
        state.changed.push(sphere(gt, aabb));
        state.changed.extend(old.and_then(|o| pose_sphere(o)));
        state.changed.extend(pose_sphere(&pose));
        state.poses.insert(e, pose);
    }
}

/// Whether a change in a sphere shows in a light's shadow map: within the
/// light's reach and not behind it.
fn shows_in(light: Vec3, forward: Vec3, reach: f32, (center, radius): (Vec3, f32)) -> bool {
    let d = center - light;
    d.length_squared() < (reach + radius) * (reach + radius) && d.dot(forward) > -radius
}

/// Gives the strongest shadow-casting lights a map, writes their matrices
/// and slots into the buffer, and points (and wakes) the cameras.
fn assign(
    settings: Res<FxSettings>,
    active: Res<FxActiveLights>,
    mut buffer: ResMut<FxBuffer>,
    mut state: ResMut<ShadowSlots>,
    casters: Res<ShadowCasters>,
    mut cameras: Query<(&FxShadowCamera, &mut Camera, &mut Transform, &mut GlobalTransform)>,
) {
    let slots = if settings.has(features::SHADOWS) && settings.has(features::LIGHTS) { settings.shadow_slots.min(MAX_SHADOWS) } else { 0 };
    state.frame = state.frame.wrapping_add(1);
    let frame = state.frame;
    let interval = refresh_interval(settings.preset);
    let tan_half = (FOV_DEGREES.to_radians() * 0.5).tan();
    // The strongest shadow-casting lights, with their index in the buffer.
    let mut lights: Vec<(usize, Vec3, Quat, f32)> = Vec::new();
    for (i, light) in active.0.iter().enumerate() {
        if lights.len() >= slots {
            break;
        }
        if !light.shadow {
            continue;
        }
        let (forward, up) = match light.shape {
            FxLightShape::Ellipse { normal, up, .. } => (normal, up),
            FxLightShape::Spot { axis, .. } => (axis, if axis.z.abs() > 0.9 { Vec3::X } else { Vec3::Z }),
            _ => continue,
        };
        let left = up.cross(forward).normalize_or_zero();
        let up = forward.cross(left);
        let rotation = Quat::from_mat3(&Mat3::from_cols(forward, left, up));
        lights.push((i, light.position, rotation, light.range.min(FAR)));
    }
    // A light keeps the map it had (lights of equal score, such as the two
    // portals', trade places as they flicker); the rest take free ones.
    let same = |s: &SlotState, position: Vec3, rotation: Quat| {
        s.position.distance_squared(position) <= 0.01 && s.rotation.angle_between(rotation) <= 1e-3
    };
    let mut wanted: [Option<(Vec3, Quat, f32)>; MAX_SHADOWS] = [None; MAX_SHADOWS];
    let mut placed = vec![false; lights.len()];
    for (n, &(i, position, rotation, reach)) in lights.iter().enumerate() {
        let kept = (0..slots).find(|&k| wanted[k].is_none() && state.slots[k].is_some_and(|s| same(&s, position, rotation)));
        if let Some(k) = kept {
            wanted[k] = Some((position, rotation, reach));
            buffer.data[HEADER_FLOATS + i * LIGHT_FLOATS + 14] = k as f32;
            placed[n] = true;
        }
    }
    for (n, &(i, position, rotation, reach)) in lights.iter().enumerate() {
        if placed[n] {
            continue;
        }
        let Some(k) =
            (0..slots).find(|&k| wanted[k].is_none() && !lights.iter().any(|l| state.slots[k].is_some_and(|s| same(&s, l.1, l.2))))
        else {
            continue;
        };
        wanted[k] = Some((position, rotation, reach));
        buffer.data[HEADER_FLOATS + i * LIGHT_FLOATS + 14] = k as f32;
    }
    buffer.data[SHADOW_INFO..SHADOW_INFO + 4].copy_from_slice(&[SHADOW_SIZE as f32, 0.0, tan_half, 0.0]);
    for (cam, mut camera, mut transform, mut global) in &mut cameras {
        let slot = cam.0;
        let Some((position, rotation, reach)) = wanted.get(slot).copied().flatten() else {
            state.slots[slot] = None;
            if camera.is_active {
                camera.is_active = false;
            }
            continue;
        };
        let moved = state.slots[slot].is_none_or(|s| !same(&s, position, rotation));
        let age = state.slots[slot].map_or(0, |s| s.age + 1);
        let forward = rotation * Vec3::X;
        let touched = casters.changed.iter().any(|c| shows_in(position, forward, reach, *c));
        let due = match interval {
            // Ultra: exactly when what the map shows changes.
            None => moved || touched,
            Some(interval) => moved || age >= interval || (frame + slot as u32 * 2).is_multiple_of(interval),
        };
        let t = Transform { translation: position, rotation: camera_rotation(rotation), ..default() };
        let p = PerspectiveProjection { fov: FOV_DEGREES.to_radians(), aspect_ratio: 1.0, near: NEAR, far: FAR, ..default() };
        let clip_from_world = bevy::camera::CameraProjection::get_clip_from_view(&p) * t.to_matrix().inverse();
        let m = MATRICES + slot * 16;
        buffer.data[m..m + 16].copy_from_slice(&clip_from_world.to_cols_array());
        if due {
            // Set the global transform too: propagation has run this frame.
            *transform = t;
            *global = GlobalTransform::from(t);
            camera.is_active = true;
            state.slots[slot] = Some(SlotState { position, rotation, age: 0 });
        } else {
            if camera.is_active {
                camera.is_active = false;
            }
            if let Some(s) = state.slots[slot].as_mut() {
                s.age = age;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use portal2_render::source_material::world_flags;

    fn app() -> App {
        let mut app = App::new();
        app.init_resource::<ShadowCasters>()
            .insert_resource(Assets::<WorldMaterial>::default())
            .insert_resource(Assets::<ModelMaterial>::default())
            .add_message::<AssetEvent<Mesh>>()
            .add_message::<AssetEvent<WorldMaterial>>()
            .add_message::<AssetEvent<ModelMaterial>>()
            .add_systems(Update, track_casters);
        app
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
        let material = app.world_mut().resource_mut::<Assets<WorldMaterial>>().add(WorldMaterial::default());
        let at = GlobalTransform::from_translation(Vec3::new(100.0, 0.0, 0.0));
        let bounds = Aabb::from_min_max(Vec3::splat(-8.0), Vec3::splat(8.0));
        let e = app
            .world_mut()
            .spawn((Mesh3d(Handle::default()), MeshMaterial3d(material.clone()), at, InheritedVisibility::VISIBLE, bounds))
            .id();
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

        let id = material.id();
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

    #[test]
    fn changes_show_within_reach_and_in_front() {
        let light = Vec3::ZERO;
        assert!(shows_in(light, Vec3::X, 420.0, (Vec3::new(400.0, 0.0, 0.0), 30.0)));
        assert!(!shows_in(light, Vec3::X, 420.0, (Vec3::new(500.0, 0.0, 0.0), 30.0)));
        assert!(!shows_in(light, Vec3::X, 420.0, (Vec3::new(-100.0, 0.0, 0.0), 30.0)), "behind the light");
        assert!(shows_in(light, Vec3::X, 420.0, (Vec3::new(-10.0, 0.0, 0.0), 30.0)), "reaching past its plane");
    }
}
