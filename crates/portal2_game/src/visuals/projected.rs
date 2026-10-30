//! `env_projectedtexture`: lights that project a texture (a cookie such as
//! `effects/flashlight001`) through a frustum onto the world and models,
//! optionally with shadows. They are the main light of most test chambers.
//!
//! The lighting follows the engine's flashlight. Its colour is `lightcolor`
//! rgb / 255 times alpha / 255, times `brightnessscale` and the flashlight
//! scale (`r_flashlightbrightness`, 0.25, times 2.5 when blending is
//! sRGB-correct, as on every current card). Only one shines at a time:
//! turning one on, or one starting on, turns all the others off. Light
//! falls off as 100 / distance (no constant or quadratic term), fades out
//! between 60% and 100%
//! of the far distance and is scaled by N·L and the cookie. In shadow a
//! surface keeps `r_flashlightshadowatten` (0.35) of the light, less where
//! the light is strong. The colour follows changes of `lightcolor` at
//! `colortransitiontime` × 255 per second.
//!
//! Every world and model material binds the same three things: a storage
//! buffer with the lights in view, an array of every cookie texture the game
//! has and four shadow maps. A shadowed light gets a camera that draws its
//! view into one of the shadow maps; world and model shaders recognise those
//! views by their exponential fog and write the distance to the light
//! instead of shading.

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::RenderLayers;
use bevy::camera::{CameraProjection, Exposure, Hdr, RenderTarget};
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::pbr::{DistanceFog, FogFalloff};
use bevy::prelude::*;
use bevy::render::render_resource::{
    Extent3d, TextureDimension, TextureFormat, TextureUsages, TextureViewDescriptor, TextureViewDimension,
};
use bevy::render::storage::ShaderBuffer;
use bevy::render::view::Tonemapping;
use serde::{Deserialize, Serialize};
use source_formats::vtf::Vtf;

use crate::entities::SpawnHandlers;
use crate::entities::io::{InputCall, InputHandlers};
use portal2_core::coords::camera_rotation;
use portal2_core::entity::{EntityRegistry, MapEntity};
use portal2_core::vfs::GameFs;
use portal2_core::view::MainCamera;

pub const MAX_LIGHTS: usize = 8;
pub const MAX_SHADOWS: usize = 4;
/// `r_flashlightdepthres`.
const SHADOW_SIZE: u32 = 1024;
const COOKIE_SIZE: u32 = 512;
/// Flashlight brightness is `r_flashlightbrightness` (0.25; the first map
/// raises it for the container ride) × 2.5 for sRGB-correct blending.
const SRGB_BLEND_SCALE: f32 = 2.5;
/// `r_flashlightshadowatten`.
const SHADOW_ATTEN: f32 = 0.35;
/// The engine's attenuation for projected textures: constant, linear, quadratic.
const ATTENUATION: Vec3 = Vec3::new(0.0, 100.0, 0.0);
/// Floats per light in the storage buffer (a matrix and four vectors).
const LIGHT_FLOATS: usize = 32;

/// Handles every world and model material binds.
#[derive(Resource, Clone)]
pub struct ProjectedGpu {
    pub lights: Handle<ShaderBuffer>,
    pub cookies: Handle<Image>,
    pub shadows: [Handle<Image>; MAX_SHADOWS],
    /// Cookie names (`effects/flashlight001`, lower case) by array layer.
    cookie_names: Vec<String>,
}

impl ProjectedGpu {
    fn cookie_layer(&self, name: &str) -> usize {
        let name = name.trim_end_matches(".vtf").replace('\\', "/").to_ascii_lowercase();
        self.cookie_names.iter().position(|n| *n == name).unwrap_or(0)
    }
}

#[derive(Component, Clone, Debug, Serialize, Deserialize)]
pub struct ProjectedTexture {
    pub on: bool,
    pub fov: f32,
    pub near: f32,
    pub far: f32,
    pub shadows: bool,
    pub light_world: bool,
    pub light_only_target: bool,
    pub brightness_scale: f32,
    /// `lightcolor` as set (0-255, alpha is brightness).
    pub color: Vec4,
    /// The colour being shown, moving toward `color`.
    pub current: Option<Vec4>,
    pub transition_rate: f32,
    pub cookie: String,
    pub target: Option<String>,
}

impl portal2_core::save::Restore for ProjectedTexture {}

pub fn register_save(r: &mut portal2_core::save::SaveRegistry) {
    r.component::<ProjectedTexture>("projected_texture");
}

/// Draws one shadow map.
#[derive(Component)]
pub struct ShadowCamera(pub usize);

pub struct ProjectedPlugin;

impl Plugin for ProjectedPlugin {
    fn build(&self, app: &mut App) {
        let gpu = {
            let world = app.world_mut();
            let fs = world.resource::<GameFs>().clone();
            let (cookies, cookie_names) = cookie_array(&fs);
            let mut images = world.resource_mut::<Assets<Image>>();
            let cookies = images.add(cookies);
            let shadows = std::array::from_fn(|_| images.add(shadow_image()));
            let lights = world
                .resource_mut::<Assets<ShaderBuffer>>()
                .add(ShaderBuffer::new(vec![0f32; 4 + MAX_LIGHTS * LIGHT_FLOATS], RenderAssetUsages::default()));
            ProjectedGpu { lights, cookies, shadows, cookie_names }
        };
        let _ = portal2_render::source_material::PROJECTED.set(portal2_render::source_material::ProjectedHandles {
            lights: gpu.lights.clone(),
            cookies: gpu.cookies.clone(),
            shadows: gpu.shadows.clone(),
        });
        app.insert_resource(gpu)
            .add_systems(Startup, (spawn_shadow_cameras, load_shader_module))
            .add_systems(PostUpdate, update_lights.after(TransformSystems::Propagate).after(portal2_world::camera::sync_camera_transform));
        let world = app.world_mut();
        world.resource_mut::<SpawnHandlers>().register("env_projectedtexture", spawn_projected);
        world.resource_mut::<InputHandlers>().register("env_projectedtexture", projected_input);
    }
}

/// Keeps the shared WESL module loaded so the shaders importing it resolve.
#[derive(Resource)]
#[allow(dead_code)]
struct ProjectedShaderModule(Handle<Shader>);

fn load_shader_module(mut commands: Commands, assets: Res<AssetServer>) {
    commands.insert_resource(ProjectedShaderModule(assets.load("embedded://portal2_render/shaders/projected.wesl")));
}

/// Every `materials/effects/flashlight*` texture, resampled into one array
/// (layer 0 is the default cookie).
fn cookie_array(fs: &GameFs) -> (Image, Vec<String>) {
    let mut names: Vec<String> = fs
        .list("materials/effects", Some("vtf"), false)
        .into_iter()
        .filter_map(|p| {
            let p = p.replace('\\', "/").to_ascii_lowercase();
            let file = p.rsplit('/').next()?.trim_end_matches(".vtf").to_owned();
            file.starts_with("flashlight").then(|| format!("effects/{file}"))
        })
        .collect();
    names.sort();
    names.dedup();
    if let Some(i) = names.iter().position(|n| n == "effects/flashlight001") {
        let default = names.remove(i);
        names.insert(0, default);
    }
    let size = COOKIE_SIZE as usize;
    let mut data = Vec::with_capacity(names.len().max(1) * size * size * 4);
    let mut kept = Vec::new();
    for name in &names {
        let Some(vtf) = fs.read(&format!("materials/{name}.vtf")).and_then(|b| Vtf::parse(&b).ok()) else { continue };
        let (w, h) = (vtf.width as usize, vtf.height as usize);
        let src = vtf.decode_rgba8(0, 0, 0);
        if w == 0 || h == 0 || src.len() < w * h * 4 {
            continue;
        }
        // Bilinear resample to the array's size.
        for y in 0..size {
            let fy = ((y as f32 + 0.5) * h as f32 / size as f32 - 0.5).clamp(0.0, (h - 1) as f32);
            let (y0, ty) = (fy.floor() as usize, fy.fract());
            let y1 = (y0 + 1).min(h - 1);
            for x in 0..size {
                let fx = ((x as f32 + 0.5) * w as f32 / size as f32 - 0.5).clamp(0.0, (w - 1) as f32);
                let (x0, tx) = (fx.floor() as usize, fx.fract());
                let x1 = (x0 + 1).min(w - 1);
                for c in 0..4 {
                    let p = |xx: usize, yy: usize| src[(yy * w + xx) * 4 + c] as f32;
                    let top = p(x0, y0) * (1.0 - tx) + p(x1, y0) * tx;
                    let bottom = p(x0, y1) * (1.0 - tx) + p(x1, y1) * tx;
                    data.push((top * (1.0 - ty) + bottom * ty).round() as u8);
                }
            }
        }
        kept.push(name.clone());
    }
    if kept.is_empty() {
        data = vec![255; size * size * 4];
        kept.push("effects/flashlight001".into());
    }
    let mut image = Image::new(
        Extent3d { width: COOKIE_SIZE, height: COOKIE_SIZE, depth_or_array_layers: kept.len() as u32 },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.texture_view_descriptor = Some(TextureViewDescriptor { dimension: Some(TextureViewDimension::D2Array), ..default() });
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::ClampToEdge,
        address_mode_v: ImageAddressMode::ClampToEdge,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        ..default()
    });
    (image, kept)
}

fn shadow_image() -> Image {
    let mut image = Image::new_fill(
        Extent3d { width: SHADOW_SIZE, height: SHADOW_SIZE, depth_or_array_layers: 1 },
        TextureDimension::D2,
        &[0u8; 8],
        TextureFormat::Rgba16Float,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.texture_descriptor.usage = TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST | TextureUsages::RENDER_ATTACHMENT;
    image
}

/// The fog shadow views carry: exponential falloff marks them for the shaders.
fn shadow_fog() -> DistanceFog {
    DistanceFog {
        color: Color::NONE,
        directional_light_color: Color::NONE,
        directional_light_exponent: 0.0,
        falloff: FogFalloff::Exponential { density: 0.0 },
    }
}

/// Distances the shadow maps start out with (beyond any light).
const SHADOW_CLEAR: f32 = 60000.0;

fn spawn_shadow_cameras(mut commands: Commands, gpu: Res<ProjectedGpu>) {
    for (slot, image) in gpu.shadows.iter().enumerate() {
        commands.spawn((
            Name::new(format!("projected texture shadow {slot}")),
            ShadowCamera(slot),
            Camera3d::default(),
            Camera {
                order: -30 + slot as isize,
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
            Projection::Perspective(PerspectiveProjection { fov: 1.0, aspect_ratio: 1.0, near: 4.0, far: 1000.0, ..default() }),
            shadow_fog(),
            RenderLayers::layer(0),
            Transform::default(),
        ));
    }
}

fn parse_color(s: &str) -> Vec4 {
    let v: Vec<f32> = s.split_whitespace().filter_map(|p| p.parse().ok()).collect();
    match v.as_slice() {
        [r, g, b, a, ..] => Vec4::new(*r, *g, *b, *a),
        [r, g, b] => Vec4::new(*r, *g, *b, 255.0),
        _ => Vec4::splat(255.0),
    }
}

fn spawn_projected(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e) else { return };
    let light = ProjectedTexture {
        // Spawn flag 1 starts the light on.
        on: me.has_flag(1),
        fov: me.get_f32("lightfov").unwrap_or(90.0).clamp(1.0, 179.0),
        near: me.get_f32("nearz").unwrap_or(4.0).max(0.1),
        far: me.get_f32("farz").unwrap_or(750.0).max(1.0),
        shadows: me.get("enableshadows").is_none_or(|v| v.trim() != "0"),
        light_world: me.get("lightworld").is_none_or(|v| v.trim() != "0"),
        light_only_target: me.get_bool("lightonlytarget"),
        brightness_scale: me.get_f32("brightnessscale").unwrap_or(1.0),
        color: me.get("lightcolor").map(parse_color).unwrap_or(Vec4::splat(255.0)),
        current: None,
        transition_rate: me.get_f32("colortransitiontime").unwrap_or(0.5),
        cookie: me.get("texturename").unwrap_or("effects/flashlight001").to_owned(),
        target: me.get("target").filter(|t| !t.is_empty()).map(str::to_owned),
    };
    let on = light.on;
    world.entity_mut(e).insert(light);
    if on {
        turn_off_others(world, e);
    }
}

/// Only one projected texture shines at a time: turning one on (or one
/// starting on) turns every other one off.
fn turn_off_others(world: &mut World, keep: Entity) {
    let mut lights = world.query::<(Entity, &mut ProjectedTexture)>();
    for (e, mut l) in lights.iter_mut(world) {
        if e != keep {
            l.on = false;
        }
    }
}

fn projected_input(world: &mut World, call: &InputCall) -> bool {
    if call.input == "turnon" && world.get::<ProjectedTexture>(call.target).is_some() {
        turn_off_others(world, call.target);
    }
    let Some(mut l) = world.get_mut::<ProjectedTexture>(call.target) else { return false };
    let f = source_formats::kv::parse_f32_lenient(&call.param);
    match call.input.as_str() {
        "turnon" => l.on = true,
        "turnoff" => l.on = false,
        "fov" | "setfov" => l.fov = f.clamp(1.0, 179.0),
        "enableshadows" => l.shadows = f != 0.0,
        "lightcolor" => l.color = parse_color(&call.param),
        "spotlighttexture" => l.cookie = call.param.trim().to_owned(),
        "setnearz" => l.near = f.max(0.1),
        "setfarz" => l.far = f.max(1.0),
        "lightworld" => l.light_world = f != 0.0,
        "lightonlytarget" => l.light_only_target = f != 0.0,
        "target" => l.target = Some(call.param.trim().to_owned()).filter(|t| !t.is_empty()),
        "setbrightness" | "setbrightnessscale" => l.brightness_scale = f,
        // Light style, ambient light and update rate do not change the result here.
        "alwaysupdateon" | "alwaysupdateoff" | "ambient" | "cameraspace" | "setlightstyle" | "setpattern" => {}
        _ => return false,
    }
    true
}

struct ActiveLight {
    position: Vec3,
    rotation: Quat,
    light: ProjectedTexture,
    color: Vec3,
    distance: f32,
}

/// Collects the lights that are on, fills the storage buffer and points the
/// shadow cameras.
fn update_lights(
    time: Res<Time>,
    gpu: Res<ProjectedGpu>,
    registry: Option<Res<EntityRegistry>>,
    mut buffers: ResMut<Assets<ShaderBuffer>>,
    mut lights: Query<(&GlobalTransform, &mut ProjectedTexture)>,
    transforms: Query<&GlobalTransform>,
    main: Query<&GlobalTransform, With<MainCamera>>,
    mut cameras: Query<(&ShadowCamera, &mut Camera, &mut Transform, &mut Projection), Without<ProjectedTexture>>,
    mut last_count: Local<usize>,
    console: Option<Res<portal2_core::console::Console>>,
) {
    let eye = main.single().map(|t| t.translation()).unwrap_or_default();
    let flashlight_scale = console.map_or(0.25, |c| c.f32("r_flashlightbrightness")) * SRGB_BLEND_SCALE;
    let dt = time.delta_secs();
    let mut active = Vec::new();
    for (gt, mut light) in &mut lights {
        // Colour transitions: at most rate × 255 per second per channel.
        let target = light.color;
        let current = match light.current {
            None => target,
            Some(c) => {
                let step = light.transition_rate * dt * 255.0;
                c + (target - c).clamp(Vec4::splat(-step), Vec4::splat(step))
            }
        };
        light.current = Some(current);
        if !light.on || !light.light_world || light.light_only_target {
            continue;
        }
        let position = gt.translation();
        let mut rotation = gt.rotation();
        if let (Some(name), Some(registry)) = (&light.target, &registry)
            && let Some(&t) = registry.by_name.get(&name.to_ascii_lowercase()).and_then(|l| l.first())
            && let Ok(tt) = transforms.get(t)
        {
            let dir = (tt.translation() - position).normalize_or_zero();
            if dir != Vec3::ZERO {
                rotation = Quat::from_rotation_arc(Vec3::X, dir);
            }
        }
        let brightness = current.w / 255.0 * light.brightness_scale * flashlight_scale;
        let color = current.truncate() / 255.0 * brightness;
        if color.max_element() <= 0.0 {
            continue;
        }
        let distance = position.distance(eye);
        active.push(ActiveLight { position, rotation, light: light.clone(), color, distance });
    }
    active.sort_by(|a, b| a.distance.total_cmp(&b.distance));
    active.truncate(MAX_LIGHTS);
    if active.len() != *last_count {
        *last_count = active.len();
        debug!("{} projected textures lit: {:?}", active.len(), active.iter().map(|a| (a.position, a.color)).collect::<Vec<_>>());
    }

    let mut data = vec![0f32; 4 + MAX_LIGHTS * LIGHT_FLOATS];
    data[0] = active.len() as f32;
    let mut shadow_slots: Vec<(usize, Transform, PerspectiveProjection)> = Vec::new();
    for (i, a) in active.iter().enumerate() {
        let fov = a.light.fov.to_radians();
        let projection = PerspectiveProjection { fov, aspect_ratio: 1.0, near: a.light.near, far: a.light.far, ..default() };
        let transform = Transform { translation: a.position, rotation: camera_rotation(a.rotation), ..default() };
        let clip_from_world = projection.get_clip_from_view() * transform.to_matrix().inverse();
        let slot = (a.light.shadows && shadow_slots.len() < MAX_SHADOWS).then(|| {
            shadow_slots.push((i, transform, projection.clone()));
            shadow_slots.len() - 1
        });
        let o = 4 + i * LIGHT_FLOATS;
        data[o..o + 16].copy_from_slice(&clip_from_world.to_cols_array());
        data[o + 16..o + 20].copy_from_slice(&[a.position.x, a.position.y, a.position.z, a.light.far]);
        data[o + 20..o + 24].copy_from_slice(&[a.color.x, a.color.y, a.color.z, SHADOW_ATTEN]);
        data[o + 24..o + 28].copy_from_slice(&[ATTENUATION.x, ATTENUATION.y, ATTENUATION.z, (fov * 0.5).tan()]);
        let layer = gpu.cookie_layer(&a.light.cookie) as f32;
        data[o + 28..o + 32].copy_from_slice(&[layer, slot.map_or(-1.0, |s| s as f32), 0.0, 0.0]);
    }
    if let Some(mut buffer) = buffers.get_mut(&gpu.lights) {
        *buffer = ShaderBuffer::new(data, RenderAssetUsages::default());
    }
    for (cam, mut camera, mut t, mut p) in &mut cameras {
        match shadow_slots.get(cam.0) {
            Some((_, transform, projection)) => {
                camera.is_active = true;
                *t = *transform;
                *p = Projection::Perspective(projection.clone());
            }
            None => camera.is_active = false,
        }
    }
}
