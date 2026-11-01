//! Texture frame switching: `env_texturetoggle` shows another frame of its
//! target brushes' or overlays' base textures (the floor and wall indicator
//! lights), and `prop_indicator_panel` drives its indicator lights on Check
//! / Uncheck. Named overlays are reached through their
//! `info_overlay_accessor`, which also takes the frame inputs itself.

use std::collections::HashMap;

use bevy::prelude::*;

use super::io::{EventQueue, EventTarget, InputCall, InputHandlers};
use super::{MapEntity, SpawnHandlers};
use portal2_core::vfs::GameFs;
use portal2_render::materials::MaterialCache;
use portal2_render::source_material::WorldMaterial;
use portal2_render::textures::TextureCache;
use portal2_world::OverlayMesh;

pub fn register(app: &mut App) {
    app.init_resource::<FrameMaterials>();
    let world = app.world_mut();
    {
        let mut s = world.resource_mut::<SpawnHandlers>();
        s.register("env_texturetoggle", |_, _| {});
        s.register("prop_indicator_panel", |_, _| {});
    }
    let mut h = world.resource_mut::<InputHandlers>();
    h.register("env_texturetoggle", toggle_input);
    h.register("info_overlay_accessor", toggle_input);
    h.register("prop_indicator_panel", panel_input);
}

/// Current frame per toggle, and materials made for each (material, frame).
#[derive(Resource, Default)]
struct FrameMaterials {
    made: HashMap<(AssetId<WorldMaterial>, u32), Handle<WorldMaterial>>,
    /// Original material of each toggled mesh.
    original: HashMap<Entity, Handle<WorldMaterial>>,
    index: HashMap<Entity, u32>,
}

fn toggle_input(world: &mut World, call: &InputCall) -> bool {
    let current = world.resource::<FrameMaterials>().index.get(&call.target).copied().unwrap_or(0);
    let frame = match call.input.as_str() {
        "settextureindex" => source_formats::kv::parse_i32_lenient(&call.param).max(0) as u32,
        "incrementtextureindex" => current + 1,
        _ => return false,
    };
    world.resource_mut::<FrameMaterials>().index.insert(call.target, frame);
    if world.get::<MapEntity>(call.target).is_some_and(|m| m.classname == "info_overlay_accessor") {
        set_frame(world, call.target, frame);
        return true;
    }
    let Some(target) = world.get::<MapEntity>(call.target).and_then(|m| m.get("target").map(str::to_ascii_lowercase)) else {
        return true;
    };
    for e in super::io::resolve_targets(world, &target, call.activator, Some(call.target)) {
        set_frame(world, e, frame);
    }
    true
}

/// Shows `frame` of the base texture on every brush mesh of `e`, or on the
/// overlay an accessor `e` names.
fn set_frame(world: &mut World, e: Entity, frame: u32) {
    let overlay = world.get::<MapEntity>(e).filter(|m| m.classname == "info_overlay_accessor").map(|m| m.get_i32("overlayid"));
    let meshes: Vec<(Entity, Option<String>)> = match overlay {
        Some(id) => {
            let mut q = world.query::<(Entity, &OverlayMesh)>();
            q.iter(world).filter(|(_, o)| Some(o.id) == id).map(|(m, o)| (m, o.base_texture.clone())).collect()
        }
        None => world
            .get::<Children>(e)
            .map(|c| c.iter().filter(|c| world.get::<MeshMaterial3d<WorldMaterial>>(*c).is_some()).map(|c| (c, None)).collect())
            .unwrap_or_default(),
    };
    for (mesh, base_texture) in meshes {
        let Some(current) = world.get::<MeshMaterial3d<WorldMaterial>>(mesh).map(|m| m.0.clone()) else { continue };
        let original = world.resource_mut::<FrameMaterials>().original.entry(mesh).or_insert(current).clone();
        let key = (original.id(), frame);
        let made = world.resource::<FrameMaterials>().made.get(&key).cloned();
        let handle = match made {
            Some(h) => h,
            None => {
                let Some(h) = make_frame_material(world, &original, frame, base_texture) else { continue };
                world.resource_mut::<FrameMaterials>().made.insert(key, h.clone());
                h
            }
        };
        world.entity_mut(mesh).insert(MeshMaterial3d(handle));
    }
}

/// A copy of `original` showing `frame` of its base texture (`base_texture`
/// when the caller knows it: overlays use copies the cache does not list).
fn make_frame_material(
    world: &mut World,
    original: &Handle<WorldMaterial>,
    frame: u32,
    base_texture: Option<String>,
) -> Option<Handle<WorldMaterial>> {
    if frame == 0 {
        return Some(original.clone());
    }
    let path = match base_texture {
        Some(p) => p,
        None => {
            let cache = world.resource::<MaterialCache>();
            let info = cache.world_by_handle(original)?;
            info.surface.vmt.as_ref()?.texture_path("$basetexture")?
        }
    };
    let fs = world.resource::<GameFs>().clone();
    let image = world.resource_scope(|world, mut textures: Mut<TextureCache>| {
        let mut images = world.resource_mut::<Assets<Image>>();
        textures.load_frame(&fs, &mut images, &path, true, frame)
    })?;
    let mut mats = world.resource_mut::<Assets<WorldMaterial>>();
    let mut m = mats.get(original)?.clone();
    m.base = Some(image);
    Some(mats.add(m))
}

/// Check / Uncheck light the panel's indicator strips (frame 1 / 0).
fn panel_input(world: &mut World, call: &InputCall) -> bool {
    let frame = match call.input.as_str() {
        "check" => 1,
        "uncheck" => 0,
        "start" | "stop" | "reset" => return true,
        _ => return false,
    };
    let Some(lights) = world.get::<MapEntity>(call.target).and_then(|m| m.get("indicatorlights").map(str::to_ascii_lowercase)) else {
        return true;
    };
    if !lights.is_empty() {
        world.resource_mut::<EventQueue>().push(
            0.0,
            EventTarget::Name(lights),
            "settextureindex",
            &frame.to_string(),
            call.activator,
            Some(call.target),
        );
    }
    true
}
