//! Models partly through a portal are drawn in two pieces, as the game's
//! ghost renderables draw them: the model itself, cut off at the portal it
//! is in, and a copy carried through to the linked portal and cut off at
//! that one, so the part already through comes out of the other side (and
//! shows in the view through the first). A cube pushed into a portal or
//! carried through it, or the player's body stepping in, then passes
//! through without the part behind the portal vanishing until the whole
//! model is moved across.

use std::collections::HashMap;

use bevy::camera::visibility::{DynamicSkinnedMeshBounds, NoFrustumCulling, RenderLayers};
use bevy::math::Affine3A;
use bevy::mesh::skinning::SkinnedMesh;
use bevy::prelude::*;

use super::{CLIP_BEHIND, Portal, Portals, linked_frames};
use crate::physics::portal_travel::PortalTraveler;
use crate::player::Player;
use crate::player::body::PlayerBody;
use portal2_render::models::StudioAnimator;
use portal2_render::source_material::{ModelMaterial, ModelParams};
use portal2_world::collision::Collider as TraceCollider;
use portal2_world::collision::portal::{HALF_HEIGHT, HALF_WIDTH, PortalFrame, transfer};
use portal2_world::{MapLoaded, MapObject};

pub struct GhostsPlugin;

impl Plugin for GhostsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Ghosts>().add_systems(
            PostUpdate,
            update_ghosts
                .after(portal2_render::models::animation::animate_models)
                .after(crate::player::body::pose_body)
                .before(TransformSystems::Propagate),
        );
    }
}

/// How far a box must reach behind a portal's plane to count as going
/// through when its centre is not over the opening (a cube resting on the
/// floor beside a floor portal reaches the 1/32 unit the portal stands out
/// from the floor).
const MIN_DEPTH_OFF_CENTRE: f32 = 1.0;

/// How far a model may stand out from its collision box (a cube's bevels
/// do): held against the wall at a portal's rim, its edge reaches past the
/// portal's plane and would be cut off in the view through the other one.
const MODEL_MARGIN: f32 = 2.0;

/// Unit directions of the ambient cube faces (+X, -X, +Y, -Y, +Z, -Z).
const CUBE_AXES: [Vec3; 6] = [Vec3::X, Vec3::NEG_X, Vec3::Y, Vec3::NEG_Y, Vec3::Z, Vec3::NEG_Z];

/// Marks the entities making up the copies, kept apart from the models.
#[derive(Component)]
struct GhostPart;

/// A model's copy at the far side of the portal it is partly through.
struct Ghost {
    /// The portal the model is in.
    slot: usize,
    root: Entity,
    /// The model's meshes, the copy's, and the copy's material.
    meshes: Vec<(Entity, Entity, Handle<ModelMaterial>)>,
    /// The model's joints and the copy's.
    joints: Vec<(Entity, Entity)>,
}

/// Copies by the model they copy.
#[derive(Resource, Default)]
struct Ghosts(HashMap<Entity, Ghost>);

type Parts<'w, 's> = Query<
    'w,
    's,
    (
        &'static Mesh3d,
        &'static MeshMaterial3d<ModelMaterial>,
        &'static Transform,
        Option<&'static RenderLayers>,
        Has<NoFrustumCulling>,
        Option<&'static SkinnedMesh>,
    ),
    Without<GhostPart>,
>;

type Sources<'w, 's> = Query<
    'w,
    's,
    (&'static Transform, Option<&'static Visibility>, Option<&'static Children>, Option<&'static StudioAnimator>),
    Without<GhostPart>,
>;

type Joints<'w, 's> = Query<'w, 's, (&'static Transform, &'static ChildOf), Without<GhostPart>>;

fn update_ghosts(
    mut commands: Commands,
    mut ghosts: ResMut<Ghosts>,
    mut loaded: MessageReader<MapLoaded>,
    portals: Res<Portals>,
    portal_query: Query<&Portal>,
    props: Query<(Entity, &TraceCollider, &Transform), (With<PortalTraveler>, Without<GhostPart>)>,
    bodies: Query<(Entity, &Transform), (With<PlayerBody>, Without<GhostPart>)>,
    players: Query<&Player>,
    sources: Sources,
    parts: Parts,
    joints: Joints,
    mut copies: Query<(&mut Transform, Option<&mut Visibility>), With<GhostPart>>,
    mut materials: ResMut<Assets<ModelMaterial>>,
) {
    if loaded.read().last().is_some() {
        // The copies went with the old map's objects.
        ghosts.0.clear();
    }
    let frames = linked_frames(&portals, &portal_query);

    // Which models are partly through which portal.
    let mut wanted: Vec<(Entity, usize)> = Vec::new();
    if let Some(frames) = &frames {
        for (e, col, t) in &props {
            let centre = t.transform_point((col.shape.mins + col.shape.maxs) * 0.5);
            let half = (col.shape.maxs - col.shape.mins) * 0.5 * t.scale;
            if let Some(slot) = straddled(frames, centre, t.rotation, half) {
                wanted.push((e, slot));
            }
        }
        if let (Ok((e, t)), Ok(p)) = (bodies.single(), players.single()) {
            let (mins, maxs) = p.state.hull();
            if let Some(slot) = straddled(frames, t.translation + (mins + maxs) * 0.5, Quat::IDENTITY, (maxs - mins) * 0.5) {
                wanted.push((e, slot));
            }
        }
    }

    // Copies no longer needed, or of models whose meshes changed (a new
    // skin or model), go.
    let stale: Vec<Entity> = ghosts
        .0
        .iter()
        .filter(|(src, g)| {
            !wanted.contains(&(**src, g.slot))
                || sources.get(**src).is_ok_and(|(_, _, children, _)| !g.meshes.iter().map(|m| m.0).eq(model_meshes(children, &parts)))
        })
        .map(|(src, _)| *src)
        .collect();
    for src in stale {
        if let Some(g) = ghosts.0.remove(&src) {
            debug!("{src} no longer through portal {}", g.slot);
            drop_ghost(&mut commands, g, &parts, &mut materials);
        }
    }
    for &(src, slot) in &wanted {
        if !ghosts.0.contains_key(&src)
            && let Some(g) = spawn_ghost(&mut commands, src, slot, &sources, &parts, &joints, &mut materials)
        {
            debug!("{src} partly through portal {slot}");
            ghosts.0.insert(src, g);
        }
    }

    let Some(frames) = frames else { return };
    for (src, g) in &ghosts.0 {
        let Ok((t, visibility, _, _)) = sources.get(*src) else { continue };
        let (from, to) = (frames[g.slot], frames[1 - g.slot]);
        let m = transfer(&from, &to);
        if let Ok((mut ghost_t, ghost_visibility)) = copies.get_mut(g.root) {
            ghost_t.set_if_neq(Transform::from_matrix((m * t.compute_affine()).into()));
            if let Some(mut v) = ghost_visibility {
                v.set_if_neq(visibility.copied().unwrap_or(Visibility::Inherited));
            }
        }
        for &(joint, copy) in &g.joints {
            if let (Ok((t, _)), Ok((mut copy_t, _))) = (joints.get(joint), copies.get_mut(copy)) {
                copy_t.set_if_neq(*t);
            }
        }
        let (keep_near, keep_far) = (clip_plane(&from), clip_plane(&to));
        for (mesh, copy, copy_material) in &g.meshes {
            let Ok((_, material, t, ..)) = parts.get(*mesh) else { continue };
            if let Ok((mut copy_t, _)) = copies.get_mut(*copy) {
                copy_t.set_if_neq(*t);
            }
            let Some(params) = materials.get(&material.0).map(|m| m.params) else { continue };
            if params.clip_plane != keep_near
                && let Some(mut m) = materials.get_mut(&material.0)
            {
                m.params.clip_plane = keep_near;
            }
            let want = carried_params(&params, &m, keep_far);
            if materials.get(copy_material).is_some_and(|c| c.params != want)
                && let Some(mut c) = materials.get_mut(copy_material)
            {
                c.params = want;
            }
        }
    }
}

/// The portal a box (centre, turn, half size) is partly through: it
/// reaches both sides of the portal's plane, over the opening. Over the
/// opening itself the model around the box counts.
fn straddled(frames: &[PortalFrame; 2], centre: Vec3, rot: Quat, half: Vec3) -> Option<usize> {
    let axes = Mat3::from_quat(rot);
    let extent =
        |dir: Vec3| dir.dot(axes.x_axis).abs() * half.x + dir.dot(axes.y_axis).abs() * half.y + dir.dot(axes.z_axis).abs() * half.z;
    (0..2).find(|&slot| {
        let f = &frames[slot];
        let l = f.to_local(centre);
        let (ex, ey, ez) = (extent(f.forward), extent(f.left()), extent(f.up));
        let depth = ex - l.x;
        let over = l.y.abs() < HALF_WIDTH && l.z.abs() < HALF_HEIGHT;
        let near = l.y.abs() < HALF_WIDTH + ey + MODEL_MARGIN && l.z.abs() < HALF_HEIGHT + ez + MODEL_MARGIN;
        l.x + ex > 0.0 && near && if over { depth > -MODEL_MARGIN } else { depth > MIN_DEPTH_OFF_CENTRE }
    })
}

/// Keeps what lies in front of a portal's plane, and the sliver behind it
/// that its quad hides (see [`CLIP_BEHIND`]).
fn clip_plane(frame: &PortalFrame) -> Vec4 {
    frame.forward.extend(CLIP_BEHIND - frame.plane_dist())
}

/// The meshes of a model drawn with model materials, in child order.
fn model_meshes<'a>(children: Option<&'a Children>, parts: &'a Parts) -> impl Iterator<Item = Entity> + 'a {
    children.into_iter().flatten().copied().filter(|c| parts.contains(*c))
}

/// Builds the copy of `src`: its joints (parents before children, as the
/// model's bones come) and its meshes, each with a material of its own.
fn spawn_ghost(
    commands: &mut Commands,
    src: Entity,
    slot: usize,
    sources: &Sources,
    parts: &Parts,
    joints: &Joints,
    materials: &mut Assets<ModelMaterial>,
) -> Option<Ghost> {
    let (_, _, children, animator) = sources.get(src).ok()?;
    let meshes: Vec<Entity> = model_meshes(children, parts).collect();
    if meshes.is_empty() {
        return None;
    }
    let root = commands.spawn((Name::new("portal ghost"), GhostPart, MapObject, Transform::default(), Visibility::Hidden)).id();
    let mut copied = HashMap::from([(src, root)]);
    let mut joint_pairs = Vec::new();
    for &joint in animator.map(|a| a.joints.as_slice()).unwrap_or_default() {
        let Ok((t, parent)) = joints.get(joint) else { continue };
        let Some(&parent) = copied.get(&parent.parent()) else { continue };
        let copy = commands.spawn((GhostPart, *t, ChildOf(parent))).id();
        copied.insert(joint, copy);
        joint_pairs.push((joint, copy));
    }
    let mut mesh_triples = Vec::new();
    for mesh in meshes {
        let Ok((mesh3d, material, t, layers, no_culling, skinned)) = parts.get(mesh) else { continue };
        let Some(m) = materials.get(&material.0).cloned() else { continue };
        let handle = materials.add(m);
        let mut copy = commands.spawn((GhostPart, Mesh3d(mesh3d.0.clone()), MeshMaterial3d(handle.clone()), *t, ChildOf(root)));
        if let Some(layers) = layers {
            copy.insert(layers.clone());
        }
        if no_culling {
            copy.insert(NoFrustumCulling);
        }
        if let Some(s) = skinned {
            let joints = s.joints.iter().map(|j| copied.get(j).copied().unwrap_or(root)).collect();
            copy.insert((SkinnedMesh { inverse_bindposes: s.inverse_bindposes.clone(), joints }, DynamicSkinnedMeshBounds));
        }
        mesh_triples.push((mesh, copy.id(), handle));
    }
    Some(Ghost { slot, root, meshes: mesh_triples, joints: joint_pairs })
}

/// Removes a copy and draws its model whole again.
fn drop_ghost(commands: &mut Commands, g: Ghost, parts: &Parts, materials: &mut Assets<ModelMaterial>) {
    for (mesh, _, _) in &g.meshes {
        if let Ok((_, material, ..)) = parts.get(*mesh)
            && materials.get(&material.0).is_some_and(|m| m.params.clip_plane != Vec4::ZERO)
            && let Some(mut m) = materials.get_mut(&material.0)
        {
            m.params.clip_plane = Vec4::ZERO;
        }
    }
    if let Ok(mut root) = commands.get_entity(g.root) {
        root.try_despawn();
    }
}

/// The material of a model's copy carried through the portal transfer `m`:
/// lit as the model is (its lights and ambient cube turned along with it)
/// and cut off at `clip`.
fn carried_params(model: &ModelParams, m: &Affine3A, clip: Vec4) -> ModelParams {
    let mut p = *model;
    let turn = Mat3::from(m.matrix3);
    let back = turn.transpose();
    for (face, axis) in p.ambient.iter_mut().zip(CUBE_AXES) {
        *face = ambient_cube(&model.ambient, back * axis);
    }
    for light in &mut p.lights {
        light.position = m.transform_point3(light.position.truncate()).extend(light.position.w);
        light.direction = (turn * light.direction.truncate()).extend(light.direction.w);
    }
    p.clip_plane = clip;
    p
}

/// Light from an ambient cube along `n` (the model shader's weighting).
fn ambient_cube(cube: &[Vec4; 6], n: Vec3) -> Vec4 {
    let sq = n * n;
    let face = |first: usize, negative: bool| cube[first + negative as usize];
    face(0, n.x < 0.0) * sq.x + face(2, n.y < 0.0) * sq.y + face(4, n.z < 0.0) * sq.z
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wall() -> [PortalFrame; 2] {
        [
            PortalFrame { center: Vec3::ZERO, forward: Vec3::X, up: Vec3::Z },
            PortalFrame { center: Vec3::new(0.0, 500.0, 0.0), forward: Vec3::NEG_Y, up: Vec3::Z },
        ]
    }

    #[test]
    fn a_cube_half_in_a_portal_is_through_it() {
        assert_eq!(straddled(&wall(), Vec3::new(4.0, 0.0, 0.0), Quat::IDENTITY, Vec3::splat(10.0)), Some(0));
        // Clear of the plane, or beside the opening against the wall.
        assert_eq!(straddled(&wall(), Vec3::new(12.5, 0.0, 0.0), Quat::IDENTITY, Vec3::splat(10.0)), None);
        assert_eq!(straddled(&wall(), Vec3::new(11.0, 0.0, 0.0), Quat::IDENTITY, Vec3::splat(10.0)), Some(0));
        assert_eq!(straddled(&wall(), Vec3::new(9.97, 0.0, 0.0), Quat::IDENTITY, Vec3::splat(10.0)), Some(0));
        assert_eq!(straddled(&wall(), Vec3::new(9.97, HALF_WIDTH + 5.0, 0.0), Quat::IDENTITY, Vec3::splat(10.0)), None);
    }

    #[test]
    fn the_copy_is_lit_as_the_model() {
        let [from, to] = wall();
        let m = transfer(&from, &to);
        // Light from +X only; a light 10 units out in front of the entrance.
        let mut model = ModelParams { ambient: [Vec4::ONE, Vec4::ZERO, Vec4::ZERO, Vec4::ZERO, Vec4::ZERO, Vec4::ZERO], ..default() };
        model.lights[0].position = Vec3::new(10.0, 0.0, 0.0).extend(0.0);
        let copy = carried_params(&model, &m, clip_plane(&to));
        // The entrance's +X comes out of the exit along -Y, and the light
        // lies behind the exit (10 units in front of the entrance).
        let out = m.transform_vector3(Vec3::X);
        assert!(out.abs_diff_eq(Vec3::Y, 1e-5));
        assert!(copy.ambient[2].abs_diff_eq(Vec4::ONE, 1e-5));
        assert!(copy.lights[0].position.truncate().abs_diff_eq(Vec3::new(0.0, 510.0, 0.0), 1e-3));
        // The copy keeps what is in front of the exit, and the sliver behind.
        let c = copy.clip_plane;
        let keep = |p: Vec3| c.truncate().dot(p) + c.w >= 0.0;
        assert!(keep(Vec3::new(0.0, 499.7, 0.0)) && !keep(Vec3::new(0.0, 500.6, 0.0)));
    }
}
