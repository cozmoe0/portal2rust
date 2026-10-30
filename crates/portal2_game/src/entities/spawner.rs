//! Helpers that attach render geometry (brush models, studio models) to
//! entities from exclusive (world-access) code.

use std::sync::Arc;

use bevy::ecs::system::SystemState;
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;

use super::MapEntity;
use portal2_core::vfs::GameFs;
use portal2_render::materials::{MaterialCache, MaterialContext};
use portal2_render::models::lighting;
use portal2_render::models::lighting::ModelLighting;
use portal2_render::models::{LoadedModel, ModelCache, SpawnModel, StudioAnimator, spawn_model_parts};
use portal2_render::source_material::{ModelMaterial, RefractMaterial};
use portal2_render::textures::TextureCache;
use portal2_world::{CurrentMap, is_invisible_brush_class, spawn_model_meshes};

/// Adds brush geometry for `model "*N"` entities whose class is drawn.
pub fn attach_brush_model(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e) else { return };
    let Some(model) = me.brush_model() else { return };
    if model == 0 || is_invisible_brush_class(&me.classname) {
        return;
    }
    if me.get_i32("rendermode") == Some(10) || me.get_bool("disablerender") {
        return;
    }
    let Some(map) = world.get_resource::<CurrentMap>().cloned() else { return };
    if model >= map.model_meshes.len() {
        return;
    }
    let mut state: SystemState<Commands> = SystemState::new(world);
    let mut commands = state.get_mut(world).expect("render resources available");
    spawn_model_meshes(&mut commands, &map, model, e);
    state.apply(world);
}

#[derive(Clone, Debug)]
pub struct ModelOptions {
    pub skin: usize,
    pub body: i32,
    pub animated: bool,
    /// Lighting sample point; defaults to the entity origin.
    pub light_origin: Option<Vec3>,
}

impl Default for ModelOptions {
    fn default() -> Self {
        Self { skin: 0, body: 0, animated: true, light_origin: None }
    }
}

type ModelParams<'w, 's> = (
    Commands<'w, 's>,
    Res<'w, GameFs>,
    Res<'w, CurrentMap>,
    ResMut<'w, ModelCache>,
    ResMut<'w, MaterialCache>,
    ResMut<'w, TextureCache>,
    ResMut<'w, Assets<Image>>,
    ResMut<'w, Assets<Mesh>>,
    ResMut<'w, Assets<ModelMaterial>>,
    ResMut<'w, Assets<RefractMaterial>>,
    ResMut<'w, Assets<SkinnedMeshInverseBindposes>>,
);

/// Loads `path` and spawns its meshes under `e` with dynamic lighting.
/// Animated models also get a [`StudioAnimator`].
pub fn attach_studio_model(world: &mut World, e: Entity, path: &str, opts: ModelOptions) -> Option<Arc<LoadedModel>> {
    let placement = super::world_transform(world, e);
    let lighting_origin = world.get::<MapEntity>(e).and_then(|m| m.get("lightingorigin")).filter(|n| !n.is_empty()).map(str::to_owned);
    let mut state: SystemState<ModelParams> = SystemState::new(world);
    let (mut commands, fs, map, mut cache, mut mats, mut textures, mut images, mut meshes, mut model_mats, mut refract_mats, mut bindposes) =
        state.get_mut(world).expect("render resources available");
    let model = cache.get(path, &fs, &mut meshes, &mut bindposes)?;
    // Dynamic models are lit at their illumination point (or the entity
    // `lightingorigin` names, found once the map has spawned).
    let illum = model.studio.mdl.illum_position;
    let light_point = opts.light_origin.unwrap_or(placement.transform_point(illum));
    let light: ModelLighting = lighting::compute(&map.bsp, light_point);
    let envmap = lighting::nearest_cubemap(&map.bsp, &map.name, light_point, &fs, &mut textures, &mut images);
    let mut ctx = MaterialContext { fs: &fs, textures: &mut textures, images: &mut images };
    let joints = spawn_model_parts(
        &mut commands,
        e,
        SpawnModel { model: &model, skin: opts.skin, body: opts.body, vhv: None, animated: opts.animated, lighting: Some(light), envmap },
        &fs,
        &mut ctx,
        &mut mats,
        &mut meshes,
        &mut model_mats,
        &mut refract_mats,
    );
    if opts.animated {
        commands.entity(e).insert(StudioAnimator::new(model.clone(), joints));
    }
    commands.entity(e).insert(LitModel { lighting_origin, fixed: opts.light_origin, illum, last: Some(light_point), generation: 0 });
    state.apply(world);
    Some(model)
}

/// A model lit by the light cache: its lighting is worked out again when
/// its lighting point moves or a light style changes, as the engine does
/// for moving models.
#[derive(Component, Clone, Debug)]
pub struct LitModel {
    /// Entity whose origin is the lighting point (`lightingorigin`).
    lighting_origin: Option<String>,
    fixed: Option<Vec3>,
    /// Illumination point in model space.
    illum: Vec3,
    last: Option<Vec3>,
    generation: u32,
}

/// Lighting points move at least this far before models are lit again.
const RELIGHT_DISTANCE: f32 = 1.0;

pub fn relight_models(
    map: Option<Res<CurrentMap>>,
    styles: Res<portal2_world::lightstyles::LightStyles>,
    registry: Res<super::EntityRegistry>,
    mut models: Query<(Entity, &mut LitModel, &GlobalTransform)>,
    placements: Query<&GlobalTransform>,
    children: Query<&Children>,
    mesh_materials: Query<&MeshMaterial3d<ModelMaterial>>,
    mut materials: ResMut<Assets<ModelMaterial>>,
) {
    let Some(map) = map else { return };
    for (e, mut lit, gt) in &mut models {
        let point = match (lit.fixed, &lit.lighting_origin) {
            (Some(p), _) => p,
            (None, Some(name)) => match registry.first_named(name).and_then(|o| placements.get(o).ok()) {
                Some(o) => o.translation(),
                None => gt.transform_point(lit.illum),
            },
            (None, None) => gt.transform_point(lit.illum),
        };
        let moved = lit.last.is_none_or(|l| l.distance(point) > RELIGHT_DISTANCE);
        if !moved && lit.generation == styles.generation {
            continue;
        }
        lit.last = Some(point);
        lit.generation = styles.generation;
        let light = lighting::compute_styled(&map.bsp, point, &|s| styles.value(s));
        for c in children.iter_descendants(e) {
            if let Ok(h) = mesh_materials.get(c)
                && let Some(mut m) = materials.get_mut(&h.0)
            {
                light.apply(&mut m.params);
            }
        }
    }
}

/// Removes previously attached render children (used by SetModel / skin changes).
pub fn clear_render_children(world: &mut World, e: Entity) {
    let children: Vec<Entity> = world.get::<Children>(e).map(|c| c.iter().collect()).unwrap_or_default();
    for c in children {
        if world.get::<MapEntity>(c).is_none() {
            world.entity_mut(c).despawn();
        }
    }
    world.entity_mut(e).remove::<StudioAnimator>();
}

/// Cache of studio model collision shapes (model space).
#[derive(Resource, Default)]
pub struct ShapeCache {
    map: std::collections::HashMap<(String, i32), Option<Arc<portal2_world::collision::Shape>>>,
    /// Bone-following collision per model: bone index and shape in bone space.
    followers: std::collections::HashMap<String, Option<Arc<Vec<(usize, Arc<portal2_world::collision::Shape>)>>>>,
}

/// Gives an animated model collision that follows its bones, as the
/// engine's dynamic props, floor buttons and chamber doors do when their
/// model lists `bone_followers` or has more than one collision solid: one
/// piece per bone, riding on that bone's joint. Returns whether it did.
pub fn attach_bone_followers(world: &mut World, e: Entity) -> bool {
    use portal2_world::collision::{BoneFollower, Collider, Shape};
    let Some(me) = world.get::<MapEntity>(e) else { return false };
    let Some(path) = me.studio_model() else { return false };
    let joints = match world.get::<StudioAnimator>(e) {
        Some(a) if !a.joints.is_empty() => a.joints.clone(),
        _ => return false,
    };
    let key = source_formats::text::normalize_path(&path);
    let cached = world.get_resource_or_init::<ShapeCache>().followers.get(&key).cloned();
    let solids = match cached {
        Some(s) => s,
        None => {
            let fs = world.resource::<GameFs>().clone();
            let s = portal2_world::collision::build::studio_bone_solids(&fs, &key)
                .map(|v| Arc::new(v.into_iter().map(|(bone, vols)| (bone, Arc::new(Shape::new(vols)))).collect::<Vec<_>>()));
            world.resource_mut::<ShapeCache>().followers.insert(key, s.clone());
            s
        }
    };
    let Some(solids) = solids else { return false };
    for (bone, shape) in solids.iter() {
        let Some(&joint) = joints.get(*bone) else { continue };
        let follower = world
            .spawn((
                Name::new("bone follower"),
                BoneFollower { owner: e },
                Collider { shape: shape.clone(), enabled: true, solid: true },
                Transform::IDENTITY,
                Visibility::Inherited,
            ))
            .id();
        world.entity_mut(joint).add_child(follower);
    }
    world.entity_mut(e).remove::<Collider>();
    true
}

/// Attaches collision to brush entities (solid or touch-only) and to solid
/// studio-model entities.
pub fn attach_collider(world: &mut World, e: Entity) {
    use portal2_world::collision::{Collider, CollisionWorld, brush_class_is_solid};
    let Some(me) = world.get::<MapEntity>(e).cloned() else { return };
    if let Some(model) = me.brush_model() {
        let Some(shape) = world.resource::<CollisionWorld>().brush_models.get(model).cloned() else { return };
        if shape.volumes.is_empty() {
            return;
        }
        // func_brush "solidity": 0 = solid while enabled, 1 = never, 2 = always.
        let solidity = (me.classname == "func_brush").then(|| me.get_i32("solidity").unwrap_or(0));
        let solid = brush_class_is_solid(&me.classname) && solidity != Some(1);
        let enabled = solidity == Some(2) || world.get::<super::movers::Toggleable>(e).is_none_or(|t| t.enabled);
        world.entity_mut(e).insert(Collider { shape, enabled, solid });
        return;
    }
    if !(me.classname.starts_with("prop_") || me.classname.starts_with("npc_")) || me.get_i32("solid") == Some(0) {
        return;
    }
    let Some(path) = me.studio_model() else { return };
    // Props default to physics collision.
    let solid = me.get_i32("solid").unwrap_or(6);
    if solid == 6 && me.classname.starts_with("prop_") && attach_bone_followers(world, e) {
        return;
    }
    let key = (source_formats::text::normalize_path(&path), solid);
    let cached = world.get_resource_or_init::<ShapeCache>().map.get(&key).cloned();
    let shape = match cached {
        Some(s) => s,
        None => {
            let fs = world.resource::<GameFs>().clone();
            let s = portal2_world::collision::build::studio_volumes(&fs, &key.0, solid)
                .map(|v| Arc::new(portal2_world::collision::Shape::new(v)));
            world.resource_mut::<ShapeCache>().map.insert(key, s.clone());
            s
        }
    };
    if let Some(shape) = shape {
        world.entity_mut(e).insert(Collider { shape, enabled: true, solid: true });
    }
}
