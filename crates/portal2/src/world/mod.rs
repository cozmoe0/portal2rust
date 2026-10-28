//! Map loading: BSP geometry, lightmaps, brush entities and static props.

pub mod bsp_mesh;

use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::image::ImageSampler;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use source_formats::bsp::{Bsp, Entity as MapEntity};
use source_formats::zip::ZipArchive;

use crate::args::Args;
use crate::render::materials::{MaterialCache, MaterialContext};
use crate::render::source_material::{WorldMaterial, world_flags};
use crate::render::textures::TextureCache;
use crate::vfs::GameFs;
use bsp_mesh::{MeshBatch, build_world_geometry};

/// How a map is being entered; `logic_auto` fires the matching output.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MapLoadType {
    #[default]
    NewGame,
    LoadGame,
    Transition,
    Background,
}

/// The loaded map; shared with gameplay systems.
#[derive(Resource, Clone)]
pub struct CurrentMap {
    pub name: String,
    pub load_type: MapLoadType,
    pub bsp: Arc<Bsp>,
    pub lightmap: Handle<Image>,
    /// Render meshes per brush model, in model order.
    pub model_meshes: Arc<Vec<Vec<(Handle<Mesh>, Handle<WorldMaterial>)>>>,
}

/// Marks entities that belong to the current map and get despawned on unload.
#[derive(Component)]
pub struct MapObject;

/// Where the player starts (Source coordinates, eye at origin + 64 by default).
#[derive(Resource, Clone, Copy, Debug)]
pub struct PlayerStart {
    pub origin: Vec3,
    pub angles: Vec3,
}

#[derive(Message, Clone, Debug)]
pub struct LoadMap {
    pub name: String,
    pub kind: MapLoadType,
}

impl LoadMap {
    pub fn new(name: impl Into<String>, kind: MapLoadType) -> Self {
        Self { name: name.into(), kind }
    }
}

/// With the menus running, a map load waits two frames so the loading
/// screen is on screen while the (blocking) load runs.
#[derive(Resource, Default)]
pub struct PendingLoad {
    pub enabled: bool,
    pub request: Option<LoadMap>,
    pub frames: u32,
    /// Whether a map was loaded when the request came (a level change
    /// rather than a load from the menu).
    pub in_game: bool,
}

/// Sent after a map's geometry has been built so other plugins can populate it.
#[derive(Message, Clone, Debug)]
pub struct MapLoaded(pub String);

pub struct WorldPlugin;

impl Plugin for WorldPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<LoadMap>()
            .add_message::<MapLoaded>()
            .init_resource::<PendingLoad>()
            .add_systems(Startup, request_initial_map)
            .add_systems(PreUpdate, load_map_system.in_set(LoadMapSet));
    }
}

fn request_initial_map(args: Res<Args>, mut writer: MessageWriter<LoadMap>) {
    if !args.map.is_empty() {
        writer.write(LoadMap::new(args.map.clone(), MapLoadType::NewGame));
    }
}

/// Map loading; entity spawning runs after it.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct LoadMapSet;

/// Brush entity classes whose geometry is never drawn.
pub fn is_invisible_brush_class(class: &str) -> bool {
    (class.starts_with("trigger_") && class != "trigger_portal_cleanser")
        || matches!(
            class,
            "func_areaportal"
                | "func_areaportalwindow"
                | "func_occluder"
                | "func_clip_vphysics"
                | "func_noportal_volume"
                | "func_portal_bumper"
                | "func_portal_detector"
                | "func_portal_orientation"
                | "func_placement_clip"
                | "func_precipitation"
                | "func_vehicleclip"
                | "env_bubbles"
                | "func_buyzone"
                | "func_dustcloud"
                | "func_smokevolume"
        )
}

#[allow(clippy::too_many_arguments)]
fn load_map_system(
    mut commands: Commands,
    mut requests: MessageReader<LoadMap>,
    mut loaded: MessageWriter<MapLoaded>,
    fs: Res<GameFs>,
    args: Res<Args>,
    mut textures: ResMut<TextureCache>,
    mut material_cache: ResMut<MaterialCache>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut world_materials: ResMut<Assets<WorldMaterial>>,
    old: Query<Entity, With<MapObject>>,
    mut pending: ResMut<PendingLoad>,
    current: Option<Res<CurrentMap>>,
) {
    if let Some(req) = requests.read().last().cloned() {
        if pending.enabled {
            pending.in_game = current.is_some();
            pending.request = Some(req);
            pending.frames = 0;
            return;
        }
        pending.request = Some(req);
        pending.frames = u32::MAX;
    }
    if pending.request.is_none() {
        return;
    }
    if pending.frames < 2 {
        pending.frames += 1;
        return;
    }
    let Some(LoadMap { name, kind }) = pending.request.take() else { return };
    let started = std::time::Instant::now();
    // Children go with their parents; the ones already gone are skipped.
    for e in &old {
        commands.entity(e).try_despawn();
    }
    let Some(bytes) = fs.read(&format!("maps/{name}.bsp")) else {
        error!("map {name} not found");
        return;
    };
    let bsp = match Bsp::parse(&bytes) {
        Ok(b) => Arc::new(b),
        Err(e) => {
            error!("failed to parse {name}: {e}");
            return;
        }
    };
    match ZipArchive::new(bsp.pakfile.clone()) {
        Ok(zip) => fs.mount_map_pakfile(zip),
        Err(e) => warn!("map pakfile unreadable: {e}"),
    }
    textures.clear();
    material_cache.clear();

    let geometry = build_world_geometry(&bsp);
    let lightmap = images.add(lightmap_image(&geometry.atlas));
    material_cache.lightmap = Some(lightmap.clone());

    let mut ctx = MaterialContext { fs: &fs, textures: &mut textures, images: &mut images };
    let mut model_meshes = Vec::with_capacity(geometry.models.len());
    for model in geometry.models {
        let mut list = Vec::new();
        for batch in model.batches {
            let info = material_cache.world(&batch.texdata_name, &mut ctx, &mut world_materials);
            if (args.fullbright || args.strip_flags != 0)
                && let Some(mut m) = world_materials.get_mut(&info.handle)
            {
                if args.fullbright {
                    m.params.flags |= world_flags::UNLIT;
                }
                m.params.flags &= !args.strip_flags;
            }
            let mesh = meshes.add(batch_to_mesh(batch));
            list.push((mesh, info.handle.clone()));
        }
        model_meshes.push(list);
    }

    let map = CurrentMap {
        name: name.clone(),
        load_type: kind,
        bsp: bsp.clone(),
        lightmap: lightmap.clone(),
        model_meshes: Arc::new(model_meshes),
    };

    // World geometry (model 0).
    let world = commands.spawn((Name::new("worldspawn"), MapObject, Transform::IDENTITY, Visibility::Visible)).id();
    spawn_model_meshes(&mut commands, &map, 0, world);

    let start = find_player_start(&bsp.entities);
    info!(
        "loaded {name}: {} faces, {} entities, lightmap {}x{} in {:?}",
        bsp.faces.len(),
        bsp.entities.len(),
        geometry.atlas.width,
        geometry.atlas.height,
        started.elapsed()
    );
    commands.insert_resource(start);
    commands.insert_resource(map);
    loaded.write(MapLoaded(name));
}

/// `disconnect`: leaves the map; nothing of it stays in the world.
pub fn unload_map(world: &mut World) {
    let objects: Vec<Entity> = world.query_filtered::<Entity, With<MapObject>>().iter(world).collect();
    for e in objects {
        if let Ok(e) = world.get_entity_mut(e) {
            e.despawn();
        }
    }
    let player = world.get_resource::<crate::entities::EntityRegistry>().and_then(|r| r.player);
    if let Some(p) = player
        && let Ok(e) = world.get_entity_mut(p)
    {
        e.despawn();
    }
    if let Some(mut r) = world.get_resource_mut::<crate::entities::EntityRegistry>() {
        r.player = None;
    }
    world.remove_resource::<CurrentMap>();
    world.remove_resource::<PlayerStart>();
}

pub fn spawn_model_meshes(commands: &mut Commands, map: &CurrentMap, model: usize, parent: Entity) {
    for (mesh, material) in &map.model_meshes[model] {
        let id = commands.spawn((Mesh3d(mesh.clone()), MeshMaterial3d(material.clone()), Transform::IDENTITY)).id();
        commands.entity(parent).add_child(id);
    }
}

fn find_player_start(entities: &[MapEntity]) -> PlayerStart {
    let e = entities
        .iter()
        .find(|e| e.classname() == "info_player_start")
        .or_else(|| entities.iter().find(|e| e.classname().starts_with("info_player")));
    match e {
        Some(e) => PlayerStart { origin: e.origin(), angles: e.angles() },
        None => PlayerStart { origin: Vec3::ZERO, angles: Vec3::ZERO },
    }
}

fn lightmap_image(atlas: &bsp_mesh::LightmapAtlas) -> Image {
    let mut data = Vec::with_capacity(atlas.texels.len() * 8);
    for t in &atlas.texels {
        for c in [t[0], t[1], t[2], 1.0] {
            data.extend_from_slice(&half::f16::from_f32(c).to_le_bytes());
        }
    }
    let mut img = Image::new(
        Extent3d { width: atlas.width, height: atlas.height, depth_or_array_layers: 1 },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba16Float,
        RenderAssetUsages::RENDER_WORLD,
    );
    img.sampler = ImageSampler::linear();
    img
}

fn batch_to_mesh(batch: MeshBatch) -> Mesh {
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, batch.positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, batch.normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_TANGENT, batch.tangents)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, batch.uv0)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_1, batch.uv1)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, batch.colors)
        .with_inserted_indices(Indices::U32(batch.indices))
}
