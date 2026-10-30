//! The sky. Faces that show the sky are not drawn; behind them sits the 2D
//! sky box, the six `skybox/<skyname><side>` materials drawn as quads around
//! the view the way the engine lays them out (sides rt, lf, bk, ft, up and
//! dn face +X, -X, +Y, -Y, +Z and -Z). Maps with a `sky_camera` also have a
//! 3D sky box: the part of the map in the camera's BSP area, drawn first from
//! `sky origin + eye / scale` with the camera's own fog and the 2D sky
//! behind it, after which the main view draws over it.

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::{NoFrustumCulling, RenderLayers, VisibilitySystems};
use bevy::camera::{Exposure, Hdr};
use bevy::mesh::{Indices, MeshVertexBufferLayoutRef, PrimitiveTopology};
use bevy::pbr::{DistanceFog, FogFalloff, MaterialPipeline, MaterialPipelineKey};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError};
use bevy::render::view::Tonemapping;
use bevy::shader::ShaderRef;
use portal2_core::layers::{SKY2D_LAYER, SKYBOX_LAYER};
use source_formats::bsp::Bsp;

use super::{CurrentMap, MapLoaded, MapObject};
use portal2_core::sets::LoadMapSet;
use portal2_core::vfs::GameFs;
use portal2_core::view::MainCamera;
use portal2_render::materials::load_vmt;
use portal2_render::textures::TextureCache;

pub const SKY_SHADER: &str = "embedded://portal2_render/shaders/sky.wesl";

/// Side names in the order the engine loads them.
const SIDES: [&str; 6] = ["rt", "bk", "lf", "ft", "up", "dn"];
/// Which loaded side each drawn quad uses.
const SIDE_ORDER: [usize; 6] = [0, 2, 1, 3, 4, 5];
/// Per quad, where each world axis comes from in (s, t, 1): `k` is axis
/// `k - 1`, negative values negate it.
const ST_TO_VEC: [[i32; 3]; 6] = [[3, -1, 2], [-3, 1, 2], [1, 3, 2], [-1, -3, 2], [-2, -1, 3], [2, -1, -3]];

const FLAG_TEXTURE: u32 = 1;
const FLAG_NOFOG: u32 = 2;

#[derive(Clone, Copy, Debug, Default, ShaderType)]
pub struct SkyParams {
    pub color: Vec4,
    pub flags: u32,
}

#[derive(Asset, TypePath, AsBindGroup, Clone, Debug)]
pub struct SkyMaterial {
    #[uniform(0)]
    pub params: SkyParams,
    #[texture(1)]
    #[sampler(2)]
    pub base: Option<Handle<Image>>,
}

impl Material for SkyMaterial {
    fn vertex_shader() -> ShaderRef {
        SKY_SHADER.into()
    }

    fn fragment_shader() -> ShaderRef {
        SKY_SHADER.into()
    }

    fn enable_prepass() -> bool {
        false
    }

    fn enable_shadows() -> bool {
        false
    }

    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.primitive.cull_mode = None;
        if let Some(depth) = descriptor.depth_stencil.as_mut() {
            depth.depth_write_enabled = Some(false);
        }
        Ok(())
    }
}

/// The map's 3D sky box (`sky_camera`).
#[derive(Resource, Clone, Copy, Debug)]
pub struct Skybox3d {
    pub origin: Vec3,
    pub scale: f32,
    pub area: u16,
    pub fog: Option<SkyFog>,
}

#[derive(Clone, Copy, Debug)]
pub struct SkyFog {
    /// sRGB 0-255.
    pub color: Vec3,
    pub color2: Vec3,
    pub dir: Vec3,
    pub blend: bool,
    pub start: f32,
    pub end: f32,
    pub max_density: f32,
}

impl Skybox3d {
    /// The first `sky_camera` of a map, if any.
    pub fn from_bsp(bsp: &Bsp) -> Option<Self> {
        let e = bsp.entities.iter().find(|e| e.classname() == "sky_camera")?;
        let origin = e.origin();
        let leaf = bsp.leafs.get(bsp.find_leaf(origin))?;
        let scale = e.get_f32("scale").filter(|s| *s > 0.0).unwrap_or(16.0);
        let fog = (e.get_i32("fogenable").unwrap_or(0) != 0).then(|| SkyFog {
            color: e.get_vec3("fogcolor").unwrap_or(Vec3::splat(255.0)),
            color2: e.get_vec3("fogcolor2").unwrap_or(Vec3::splat(255.0)),
            dir: e.get_vec3("fogdir").unwrap_or(Vec3::X).normalize_or(Vec3::X),
            blend: e.get_i32("fogblend").unwrap_or(0) != 0,
            start: e.get_f32("fogstart").unwrap_or(500.0),
            end: e.get_f32("fogend").unwrap_or(2000.0),
            max_density: e.get_f32("fogmaxdensity").unwrap_or(1.0),
        });
        Some(Self { origin, scale, area: leaf.area, fog })
    }
}

/// The camera that draws the 3D sky box.
#[derive(Component)]
pub struct SkyboxCamera;

/// Mesh entities already sorted into the 3D sky box or the main world.
#[derive(Component)]
pub struct SkyChecked;

pub struct SkyPlugin;

impl Plugin for SkyPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<SkyMaterial>::default())
            .add_systems(PreUpdate, spawn_sky.after(LoadMapSet))
            .add_systems(PostUpdate, update_sky_cameras.after(crate::camera::sync_camera_transform).before(TransformSystems::Propagate))
            .add_systems(PostUpdate, sort_into_skybox.after(VisibilitySystems::CalculateBounds).before(VisibilitySystems::CheckVisibility));
    }
}

/// One quad per side, holding directions (the shader centres them on the view).
fn side_mesh(side: usize) -> Mesh {
    const DIST: f32 = 1.0;
    let corners = [(-1.0f32, -1.0f32), (-1.0, 1.0), (1.0, 1.0), (1.0, -1.0)];
    let mut positions = Vec::with_capacity(4);
    let mut uvs = Vec::with_capacity(4);
    for (s, t) in corners {
        let b = [s * DIST, t * DIST, DIST];
        let axis = |k: i32| if k > 0 { b[(k - 1) as usize] } else { -b[(-k - 1) as usize] };
        let m = ST_TO_VEC[side];
        positions.push([axis(m[0]), axis(m[1]), axis(m[2])]);
        uvs.push([(s + 1.0) * 0.5, 1.0 - (t + 1.0) * 0.5]);
    }
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
        .with_inserted_indices(Indices::U16(vec![0, 1, 2, 0, 2, 3]))
}

fn sky_material(fs: &GameFs, textures: &mut TextureCache, images: &mut Assets<Image>, name: &str) -> Option<SkyMaterial> {
    let vmt = load_vmt(fs, &format!("materials/skybox/{name}.vmt"))?;
    let mut flags = 0;
    let base = vmt
        .texture_path("$hdrbasetexture")
        .or_else(|| vmt.texture_path("$basetexture"))
        .and_then(|path| textures.load(fs, images, &path, true))
        .map(|t| t.handle);
    if base.is_some() {
        flags |= FLAG_TEXTURE;
    }
    if vmt.get_bool("$nofog") {
        flags |= FLAG_NOFOG;
    }
    // `$color` may be 0-1 or 0-255 ("{r g b}"), in gamma space.
    let raw = vmt.get("$color").unwrap_or("[1 1 1]");
    let mut c = vmt.get_vec3("$color").unwrap_or(Vec3::ONE);
    if raw.trim_start().starts_with('{') {
        c /= 255.0;
    }
    let linear = Color::srgb(c.x, c.y, c.z).to_linear();
    Some(SkyMaterial { params: SkyParams { color: Vec4::new(linear.red, linear.green, linear.blue, 1.0), flags }, base })
}

fn spawn_sky(
    mut loaded: MessageReader<MapLoaded>,
    map: Option<Res<CurrentMap>>,
    fs: Res<GameFs>,
    mut textures: ResMut<TextureCache>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<SkyMaterial>>,
    mut commands: Commands,
) {
    if loaded.read().last().is_none() {
        return;
    }
    let Some(map) = map else { return };
    let skyname =
        map.bsp.entities.iter().find(|e| e.classname() == "worldspawn").and_then(|e| e.get("skyname")).unwrap_or("sky_urb01").to_owned();
    let mut load_all = |name: &str| -> Option<Vec<SkyMaterial>> {
        SIDES.iter().map(|side| sky_material(&fs, &mut textures, &mut images, &format!("{name}{side}"))).collect()
    };
    let Some(sides) = load_all(&skyname).or_else(|| {
        debug!("Unable to load sky {skyname}");
        load_all("sky_urb01")
    }) else {
        return;
    };
    let handles: Vec<Handle<SkyMaterial>> = sides.into_iter().map(|m| materials.add(m)).collect();
    for (quad, &side) in SIDE_ORDER.iter().enumerate() {
        commands.spawn((
            Name::new(format!("sky {}", SIDES[side])),
            MapObject,
            Mesh3d(meshes.add(side_mesh(quad))),
            MeshMaterial3d(handles[side].clone()),
            Transform::IDENTITY,
            NoFrustumCulling,
            RenderLayers::layer(SKY2D_LAYER),
        ));
    }
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// The sky camera's fog; with blending the colour follows how much the
/// view faces `fogdir`.
fn sky_fog(sky: &Skybox3d, forward: Vec3) -> DistanceFog {
    let Some(f) = sky.fog else {
        return DistanceFog {
            color: Color::NONE,
            directional_light_color: Color::NONE,
            directional_light_exponent: 0.0,
            falloff: FogFalloff::Linear { start: 1.0e9, end: 2.0e9 },
        };
    };
    let gamma = if f.blend { f.color2.lerp(f.color, (forward.dot(f.dir) + 1.0) * 0.5) } else { f.color } / 255.0;
    let (start, end) = (f.start / sky.scale, f.end / sky.scale);
    DistanceFog {
        color: Color::linear_rgb(srgb_to_linear(gamma.x), srgb_to_linear(gamma.y), srgb_to_linear(gamma.z)),
        directional_light_color: Color::NONE,
        directional_light_exponent: f.max_density,
        // The sky box is drawn at 1 / scale, and so is its fog.
        falloff: FogFalloff::Linear { start, end: end.max(start + 1.0) },
    }
}

/// Keeps the 3D sky box camera on the main view and switches the main
/// view between drawing the 2D sky itself and drawing over the 3D sky box.
fn update_sky_cameras(
    mut commands: Commands,
    sky: Option<Res<Skybox3d>>,
    mut main: Query<(&Transform, &Projection, &Exposure, &mut Camera, &mut RenderLayers), (With<MainCamera>, Without<SkyboxCamera>)>,
    mut cameras: Query<
        (Entity, &mut Transform, &mut Projection, &mut Exposure, &mut DistanceFog),
        (With<SkyboxCamera>, Without<MainCamera>),
    >,
) {
    let Ok((main_t, main_p, main_e, mut main_cam, mut main_layers)) = main.single_mut() else { return };
    let mut layers = vec![0, portal2_core::layers::PORTAL_LAYER];
    if sky.is_none() {
        layers.push(SKY2D_LAYER);
    }
    let want = RenderLayers::from_layers(&layers);
    if *main_layers != want {
        *main_layers = want;
    }
    let keep = matches!(main_cam.clear_color, ClearColorConfig::None);
    if keep != sky.is_some() {
        main_cam.clear_color = if sky.is_some() { ClearColorConfig::None } else { ClearColorConfig::Default };
    }
    let Some(sky) = sky else {
        for (e, ..) in &cameras {
            commands.entity(e).despawn();
        }
        return;
    };
    let mut projection = main_p.clone();
    if let Projection::Perspective(p) = &mut projection {
        p.near = (p.near / sky.scale).max(0.05);
    }
    let translation = sky.origin + main_t.translation / sky.scale;
    let fog = sky_fog(&sky, main_t.forward().into());
    let mut found = false;
    for (_, mut t, mut p, mut e, mut f) in &mut cameras {
        found = true;
        t.translation = translation;
        t.rotation = main_t.rotation;
        *p = projection.clone();
        *e = *main_e;
        *f = fog.clone();
    }
    if found {
        return;
    }
    commands.spawn((
        Name::new("3D sky box camera"),
        SkyboxCamera,
        MapObject,
        Camera3d::default(),
        Camera { order: -1, ..default() },
        Hdr,
        projection,
        *main_e,
        Tonemapping::None,
        bevy::render::view::DebandDither::Disabled,
        Msaa::Sample4,
        fog,
        RenderLayers::from_layers(&[SKYBOX_LAYER, SKY2D_LAYER]),
        Transform { translation, rotation: main_t.rotation, ..default() },
    ));
}

/// Meshes of entities and props standing in the sky box's area move to its layer.
fn sort_into_skybox(
    mut commands: Commands,
    sky: Option<Res<Skybox3d>>,
    map: Option<Res<CurrentMap>>,
    meshes: Query<
        (Entity, &GlobalTransform, Option<&bevy::camera::primitives::Aabb>, Has<NoFrustumCulling>),
        (With<Mesh3d>, Without<RenderLayers>, Without<SkyChecked>),
    >,
) {
    let (Some(sky), Some(map)) = (sky, map) else { return };
    for (e, gt, aabb, unbounded) in &meshes {
        // Meshes get their bounds a frame after spawning; ones drawn
        // without bounds (sprites) go by their origin.
        let centre = match aabb {
            Some(aabb) => gt.transform_point(Vec3::from(aabb.center)),
            None if unbounded => gt.translation(),
            None => continue,
        };
        let leaf = map.bsp.find_leaf(centre);
        let mut ec = commands.entity(e);
        ec.insert(SkyChecked);
        if map.bsp.leafs.get(leaf).is_some_and(|l| l.area == sky.area) {
            ec.insert(RenderLayers::layer(SKYBOX_LAYER));
        }
    }
}
