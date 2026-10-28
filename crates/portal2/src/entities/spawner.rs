//! Helpers that attach render geometry (brush models, studio models) to
//! entities from exclusive (world-access) code.

use std::sync::Arc;

use bevy::ecs::system::SystemState;
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;

use super::MapEntity;
use crate::models::lighting::{self, ModelLighting};
use crate::models::{LoadedModel, ModelCache, SpawnModel, StudioAnimator, spawn_model_parts};
use crate::render::materials::{MaterialCache, MaterialContext};
use crate::render::source_material::{ModelMaterial, RefractMaterial};
use crate::render::textures::TextureCache;
use crate::vfs::GameFs;
use crate::world::{CurrentMap, is_invisible_brush_class, spawn_model_meshes};

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
    let origin = super::world_transform(world, e).translation();
    let mut state: SystemState<ModelParams> = SystemState::new(world);
    let (mut commands, fs, map, mut cache, mut mats, mut textures, mut images, mut meshes, mut model_mats, mut refract_mats, mut bindposes) =
        state.get_mut(world).expect("render resources available");
    let model = cache.get(path, &fs, &mut meshes, &mut bindposes)?;
    let light_point = opts.light_origin.unwrap_or(origin + Vec3::Z * 4.0);
    let light: ModelLighting = lighting::compute(&map.bsp, light_point);
    let envmap = lighting::nearest_cubemap(&map.bsp, &map.name, light_point, &fs, &mut textures, &mut images);
    let mut ctx = MaterialContext { fs: &fs, textures: &mut textures, images: &mut images };
    let joints = spawn_model_parts(
        &mut commands,
        e,
        SpawnModel {
            model: &model,
            skin: opts.skin,
            body: opts.body,
            vhv: None,
            animated: opts.animated,
            lighting: Some(light),
            envmap,
        },
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
    state.apply(world);
    Some(model)
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
    map: std::collections::HashMap<(String, i32), Option<Arc<crate::collision::Shape>>>,
}

/// Attaches collision to brush entities (solid or touch-only) and to solid
/// studio-model entities.
pub fn attach_collider(world: &mut World, e: Entity) {
    use crate::collision::{Collider, CollisionWorld, brush_class_is_solid};
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
    let key = (source_formats::text::normalize_path(&path), solid);
    let cached = world.get_resource_or_init::<ShapeCache>().map.get(&key).cloned();
    let shape = match cached {
        Some(s) => s,
        None => {
            let fs = world.resource::<GameFs>().clone();
            let s = crate::collision::build::studio_volumes(&fs, &key.0, solid).map(|v| Arc::new(crate::collision::Shape::new(v)));
            world.resource_mut::<ShapeCache>().map.insert(key, s.clone());
            s
        }
    };
    if let Some(shape) = shape {
        world.entity_mut(e).insert(Collider { shape, enabled: true, solid: true });
    }
}
