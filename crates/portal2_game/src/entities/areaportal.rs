//! `func_areaportalwindow`: the window takes over the brush named by its
//! `target` (the game's window steals the model and stops the brush drawing
//! itself) and draws it blended by the view's distance: transparent at
//! `TranslucencyLimit` up to `FadeStartDist`, opaque from `FadeDist`
//! (`docs/re/world_visuals.md`, "Area portal windows"). Test chamber doors
//! close their window (fades 0 and 0, so the black window brush shows
//! behind the closed door) and open it again (10000 and 10000, never drawn)
//! as they open.

use bevy::prelude::*;

use super::io::{InputCall, InputHandlers};
use super::{EntityRegistry, MapEntity, SpawnHandlers};
use portal2_core::entity::BrushBounds;
use portal2_core::view::MainCamera;
use portal2_render::source_material::{WorldMaterial, world_flags};

pub fn register(app: &mut App) {
    app.add_systems(PostUpdate, fade_windows.after(portal2_world::camera::sync_camera_transform).before(TransformSystems::Propagate));
    let world = app.world_mut();
    world.resource_mut::<SpawnHandlers>().register("func_areaportalwindow", spawn_window);
    world.resource_mut::<InputHandlers>().register("func_areaportalwindow", window_input);
}

#[derive(Component, Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AreaPortalWindow {
    pub fade_start: f32,
    pub fade_end: f32,
    pub translucency_limit: f32,
    /// The taken-over brush and its meshes with their own materials.
    #[serde(skip)]
    target: Option<Entity>,
    #[serde(skip)]
    meshes: Vec<(Entity, Handle<WorldMaterial>)>,
    /// See-through copies of the materials, made when first needed.
    #[serde(skip)]
    faded: Vec<Handle<WorldMaterial>>,
}

impl portal2_core::save::Restore for AreaPortalWindow {
    fn keep(&mut self, live: Self) {
        self.target = live.target;
        self.meshes = live.meshes;
        self.faded = live.faded;
    }
}

pub fn register_save(r: &mut portal2_core::save::SaveRegistry) {
    r.component::<AreaPortalWindow>("area_portal_window");
}

fn spawn_window(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e) else { return };
    let window = AreaPortalWindow {
        fade_start: me.get_f32("fadestartdist").unwrap_or(128.0),
        fade_end: me.get_f32("fadedist").unwrap_or(512.0),
        translucency_limit: me.get_f32("translucencylimit").unwrap_or(0.2),
        target: None,
        meshes: Vec::new(),
        faded: Vec::new(),
    };
    world.entity_mut(e).insert(window);
}

fn window_input(world: &mut World, call: &InputCall) -> bool {
    let Some(mut w) = world.get_mut::<AreaPortalWindow>(call.target) else { return false };
    let value = source_formats::kv::parse_f32_lenient(&call.param);
    match call.input.as_str() {
        "setfadestartdistance" => w.fade_start = value,
        "setfadeenddistance" => w.fade_end = value,
        _ => return false,
    }
    true
}

/// Sets the fade distances of the window a test chamber door names.
pub fn set_window_fades(world: &mut World, name: &str, start: f32, end: f32) {
    let windows: Vec<Entity> = world.resource::<EntityRegistry>().by_name.get(&name.to_ascii_lowercase()).cloned().unwrap_or_default();
    for e in windows {
        if let Some(mut w) = world.get_mut::<AreaPortalWindow>(e) {
            w.fade_start = start;
            w.fade_end = end;
        }
    }
}

/// How opaque the window is from `distance` (the game's distance blend; the
/// field-of-view zoom factor is 1 without zoom).
fn distance_blend(w: &AreaPortalWindow, distance: f32) -> f32 {
    let limit = w.translucency_limit;
    if w.fade_start == w.fade_end {
        return if distance - w.fade_end < 0.0 { limit } else { 1.0 };
    }
    let t = ((distance - w.fade_start) / (w.fade_end - w.fade_start)).clamp(0.0, 1.0);
    (1.0 - limit) * t + limit
}

fn fade_windows(
    registry: Res<EntityRegistry>,
    camera: Query<&Transform, With<MainCamera>>,
    mut windows: Query<(&MapEntity, &mut AreaPortalWindow)>,
    brushes: Query<(&GlobalTransform, &BrushBounds, Option<&Children>)>,
    mut meshes: Query<(&mut Visibility, &mut MeshMaterial3d<WorldMaterial>)>,
    mut materials: ResMut<Assets<WorldMaterial>>,
) {
    let Ok(eye) = camera.single().map(|t| t.translation) else { return };
    for (me, mut w) in &mut windows {
        if w.target.is_none() {
            let Some(target) = me.get("target").and_then(|t| registry.first_named(t)) else { continue };
            let Ok((_, _, children)) = brushes.get(target) else { continue };
            let found: Vec<(Entity, Handle<WorldMaterial>)> =
                children.into_iter().flatten().filter_map(|c| meshes.get(*c).ok().map(|(_, m)| (*c, m.0.clone()))).collect();
            if found.is_empty() {
                continue;
            }
            w.target = Some(target);
            w.meshes = found;
        }
        let Some(Ok((gt, bounds, _))) = w.target.map(|t| brushes.get(t)) else { continue };
        // Distance to the brush's box (zero inside it).
        let local = gt.affine().inverse().transform_point3(eye);
        let distance = (local - local.clamp(bounds.mins, bounds.maxs)).length();
        let blend = distance_blend(&w, distance);
        if blend > 0.0 && blend < 1.0 && w.faded.len() != w.meshes.len() {
            w.faded = w
                .meshes
                .iter()
                .map(|(_, m)| {
                    let mut copy = materials.get(m).cloned().unwrap_or_default();
                    copy.alpha_mode = AlphaMode::Blend;
                    copy.params.flags &= !world_flags::ALPHATEST;
                    materials.add(copy)
                })
                .collect();
        }
        let w = &*w;
        for (i, (mesh, original)) in w.meshes.iter().enumerate() {
            let Ok((mut vis, mut mat)) = meshes.get_mut(*mesh) else { continue };
            let want_vis = if blend > 0.0 { Visibility::Inherited } else { Visibility::Hidden };
            if *vis != want_vis {
                *vis = want_vis;
            }
            let want = if blend < 1.0 { w.faded.get(i).unwrap_or(original) } else { original };
            if mat.0 != *want {
                mat.0 = want.clone();
            }
            if blend > 0.0
                && blend < 1.0
                && let Some(mut m) = w.faded.get(i).and_then(|h| materials.get_mut(h))
                && m.params.color.w != blend
            {
                m.params.color.w = blend;
            }
        }
    }
}
