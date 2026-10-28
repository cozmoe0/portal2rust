//! `vgui_movie_display`: in-world screens playing a Bink movie, such as the
//! ring of panels around the elevator shafts.
//!
//! A screen is a `width` × `height` rectangle with its lower left corner at
//! the entity's origin, running along the entity's +Y and +Z axes and
//! facing +X (the engine's VGUI screen layout). Screens start disabled;
//! `SetMovie` names the movie and `Enable` shows it. A screen with
//! `forcedslave` shows the movie of its group's master (the screen of the
//! same `groupname` that is not a slave), all in step; with custom UVs
//! (`SetUseCustomUVs`, `SetUMin`/`SetUMax`/`SetVMin`/`SetVMax`) a screen
//! shows only that part of the frame, which is how the elevator scripts
//! spread one movie over many panels. Movies loop when `looping` is set.

use std::collections::HashMap;

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology, VertexAttributeValues};
use bevy::prelude::*;
use bevy::render::render_resource::Face;
use serde::{Deserialize, Serialize};

use crate::entities::io::{InputCall, InputHandlers};
use crate::entities::{MapEntity, SpawnHandlers};
use crate::render::source_material::{WorldMaterial, world_flags};
use crate::vfs::GameFs;
use crate::video::{VideoId, Videos};
use crate::vgui::Surface;

#[derive(Component, Clone, Debug, Serialize, Deserialize)]
pub struct MovieDisplay {
    pub enabled: bool,
    pub movie: String,
    pub looping: bool,
    pub group: Option<String>,
    pub slave: bool,
    pub custom_uvs: bool,
    /// u min, u max, v min, v max.
    pub uv: [f32; 4],
    #[serde(skip)]
    pub quad: Option<Entity>,
    #[serde(skip)]
    pub material: Handle<WorldMaterial>,
    /// What the quad currently shows: (movie key, uv) when drawn.
    #[serde(skip)]
    shown: Option<(String, [f32; 4])>,
}

impl crate::save::Restore for MovieDisplay {
    fn keep(&mut self, live: Self) {
        self.quad = live.quad;
        self.material = live.material;
    }
}

pub fn register_save(r: &mut crate::save::SaveRegistry) {
    r.component::<MovieDisplay>("movie_display");
}

/// Movies being decoded for screens, by path.
#[derive(Resource, Default)]
struct ScreenMovies {
    open: HashMap<String, VideoId>,
}

pub struct MoviePlugin;

impl Plugin for MoviePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ScreenMovies>().add_systems(PostUpdate, update_displays);
        let world = app.world_mut();
        world.resource_mut::<SpawnHandlers>().register("vgui_movie_display", spawn_display);
        world.resource_mut::<InputHandlers>().register("vgui_movie_display", display_input);
    }
}

fn movie_path(name: &str) -> String {
    let p = name.trim().replace('\\', "/").to_ascii_lowercase();
    if p.ends_with(".bik") { p } else { format!("{p}.bik") }
}

/// The screen's quad, `width` along +Y and `height` along +Z from the origin.
fn screen_mesh(width: f32, height: f32) -> Mesh {
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD)
        .with_inserted_attribute(
            Mesh::ATTRIBUTE_POSITION,
            vec![[0.0, 0.0, 0.0], [0.0, width, 0.0], [0.0, width, height], [0.0, 0.0, height]],
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[1.0, 0.0, 0.0]; 4])
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs([0.0, 1.0, 0.0, 1.0]))
        .with_inserted_indices(Indices::U16(vec![0, 1, 2, 0, 2, 3]))
}

/// Corner texture coordinates for a uv range (v runs down from the top edge).
fn uvs(r: [f32; 4]) -> Vec<[f32; 2]> {
    vec![[r[0], r[3]], [r[1], r[3]], [r[1], r[2]], [r[0], r[2]]]
}

fn spawn_display(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e).cloned() else { return };
    let width = me.get_f32("width").unwrap_or(256.0).max(1.0);
    let height = me.get_f32("height").unwrap_or(128.0).max(1.0);
    let material = world.resource_mut::<Assets<WorldMaterial>>().add(WorldMaterial {
        params: crate::render::source_material::WorldParams {
            color: Vec4::ONE,
            flags: world_flags::UNLIT,
            lightmap_scale: 1.0,
            envmap_saturation: 1.0,
            ..default()
        },
        cull: Some(Face::Back),
        ..default()
    });
    let mesh = world.resource_mut::<Assets<Mesh>>().add(screen_mesh(width, height));
    let quad = world
        .spawn((Name::new("movie screen"), Mesh3d(mesh), MeshMaterial3d(material.clone()), Transform::IDENTITY, Visibility::Hidden))
        .id();
    world.entity_mut(e).add_child(quad);
    let movie = me.get("moviefilename").unwrap_or("").trim().to_owned();
    world.entity_mut(e).insert(MovieDisplay {
        enabled: false,
        movie,
        looping: me.get_bool("looping"),
        group: me.get("groupname").filter(|g| !g.is_empty()).map(|g| g.to_ascii_lowercase()),
        slave: me.get_bool("forcedslave"),
        custom_uvs: false,
        uv: [0.0, 1.0, 0.0, 1.0],
        quad: Some(quad),
        material,
        shown: None,
    });
}

fn display_input(world: &mut World, call: &InputCall) -> bool {
    let Some(mut d) = world.get_mut::<MovieDisplay>(call.target) else { return false };
    let f = source_formats::kv::parse_f32_lenient(&call.param);
    match call.input.as_str() {
        "enable" => d.enabled = true,
        "disable" => d.enabled = false,
        "setmovie" => d.movie = call.param.trim().to_owned(),
        "setusecustomuvs" => d.custom_uvs = f != 0.0,
        "setumin" => d.uv[0] = f,
        "setumax" => d.uv[1] = f,
        "setvmin" => d.uv[2] = f,
        "setvmax" => d.uv[3] = f,
        "takeoverasmaster" => d.slave = false,
        "setdisplaytext" => {}
        _ => return false,
    }
    true
}

/// Opens the movies enabled screens need, closes the rest and points each
/// screen's quad at its movie.
#[allow(clippy::too_many_arguments)]
fn update_displays(
    fs: Res<GameFs>,
    mut movies: ResMut<ScreenMovies>,
    mut videos: ResMut<Videos>,
    mut surface: ResMut<Surface>,
    mut displays: Query<&mut MovieDisplay>,
    mut visibility: Query<&mut Visibility>,
    quads: Query<&Mesh3d>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<WorldMaterial>>,
) {
    // Each group's master decides the movie and whether it loops.
    let mut masters: HashMap<String, (String, bool)> = HashMap::new();
    for d in &displays {
        if let Some(g) = &d.group
            && !d.slave
        {
            masters.insert(g.clone(), (d.movie.clone(), d.looping));
        }
    }
    let mut wanted: HashMap<String, bool> = HashMap::new();
    let mut plan: Vec<(Option<String>, bool)> = Vec::new();
    for d in &displays {
        let (movie, looping) = match (&d.group, d.slave) {
            (Some(g), true) => masters.get(g).cloned().unwrap_or((d.movie.clone(), d.looping)),
            _ => (d.movie.clone(), d.looping),
        };
        let path = movie_path(&movie);
        let usable = d.enabled && !movie.trim().is_empty() && !path.ends_with("/.bik") && path != ".bik";
        if usable {
            wanted.insert(path.clone(), looping);
        }
        plan.push((usable.then_some(path), looping));
    }
    // Close movies no screen shows any more, open the new ones.
    let stale: Vec<String> = movies.open.keys().filter(|k| !wanted.contains_key(*k)).cloned().collect();
    for k in stale {
        if let Some(id) = movies.open.remove(&k) {
            videos.close(id);
        }
    }
    for (path, looping) in &wanted {
        if !movies.open.contains_key(path)
            && let Some(id) = videos.open_with(&fs, &mut surface, path, *looping, false)
        {
            movies.open.insert(path.clone(), id);
        }
    }
    for (mut d, (path, _)) in displays.iter_mut().zip(plan) {
        let image = path.as_ref().and_then(|p| movies.open.get(p)).and_then(|id| videos.get(*id)).map(|v| v.image.id);
        let uv = if d.custom_uvs { d.uv } else { [0.0, 1.0, 0.0, 1.0] };
        let want = match (&path, image) {
            (Some(p), Some(_)) => Some((p.clone(), uv)),
            _ => None,
        };
        if d.shown == want {
            continue;
        }
        let Some(quad) = d.quad else { continue };
        if let Ok(mut v) = visibility.get_mut(quad) {
            *v = if want.is_some() { Visibility::Inherited } else { Visibility::Hidden };
        }
        if let (Some(_), Some(bevy::asset::AssetId::Uuid { uuid })) = (&want, image) {
            if let Some(mut m) = materials.get_mut(&d.material) {
                m.base = Some(Handle::from(uuid));
            }
            if let Ok(mesh) = quads.get(quad)
                && let Some(mut mesh) = meshes.get_mut(&mesh.0)
            {
                mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, VertexAttributeValues::Float32x2(uvs(uv)));
            }
        }
        d.shown = want;
    }
}
