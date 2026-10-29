//! `point_spotlight` and `beam_spotlight`: a visible beam with a halo.
//!
//! The beam is `sprites/glow_test02` (a cone of light, its origin at the
//! top) on a quad `SpotlightLength` long and `SpotlightWidth` wide hanging
//! from the light along its forward axis, turned about that axis to face the
//! view; it is added to the scene and fades as the view looks along it, as
//! the material's `LampBeam` proxy does. The halo is
//! `sprites/light_glow03` drawn as a world glow (visibility-tested, over
//! everything) `HaloScale` across, brightest when the view looks into the
//! light. `beam_spotlight` turns at `maxspeed` degrees per second about its
//! local Z axis (X or Y with spawn flags 16 / 32, reversed with 8) while its
//! rotation is on (spawn flag 4, `Start` / `Stop`). Spawn flag 1 starts the
//! light on. Colours are `rendercolor` × `HDRColorScale`.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, MeshVertexBufferLayoutRef, PrimitiveTopology};
use bevy::pbr::{MaterialPipeline, MaterialPipelineKey};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError};
use bevy::shader::ShaderRef;
use serde::{Deserialize, Serialize};

use super::sprites::{SourceSprite, SpriteMaterial, SpriteParams, load_sprite_source, sprite_quad};
use crate::entities::SpawnHandlers;
use crate::entities::io::{InputCall, InputHandlers};
use crate::render::textures::TextureCache;
use portal2_core::entity::MapEntity;
use portal2_core::vfs::GameFs;
use portal2_core::view::MainCamera;

pub const BEAM_SHADER: &str = "embedded://portal2/render/shaders/beam.wesl";
const BEAM_MATERIAL: &str = "sprites/glow_test02.vmt";
const HALO_MATERIAL: &str = "sprites/light_glow03.vmt";
/// The engine's widest beam.
const MAX_WIDTH: f32 = 102.3;
const DEFAULT_HALO_SCALE: f32 = 60.0;

#[derive(Clone, Copy, Debug, Default, ShaderType)]
pub struct BeamParams {
    pub color: Vec4,
    pub shape: Vec4,
}

#[derive(Asset, TypePath, AsBindGroup, Clone, Debug)]
pub struct BeamMaterial {
    #[uniform(0)]
    pub params: BeamParams,
    #[texture(1)]
    #[sampler(2)]
    pub base: Option<Handle<Image>>,
}

impl Material for BeamMaterial {
    fn vertex_shader() -> ShaderRef {
        BEAM_SHADER.into()
    }

    fn fragment_shader() -> ShaderRef {
        BEAM_SHADER.into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Add
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

#[derive(Component, Clone, Debug, Serialize, Deserialize)]
pub struct Spotlight {
    pub on: bool,
    pub rotating: bool,
    /// Local rotation axis and speed (degrees per second, signed).
    pub axis: Vec3,
    pub speed: f32,
    #[serde(skip)]
    pub beam: Option<Entity>,
    #[serde(skip)]
    pub halo: Option<Entity>,
}

impl portal2_core::save::Restore for Spotlight {
    fn keep(&mut self, live: Self) {
        self.beam = live.beam;
        self.halo = live.halo;
    }
}

pub fn register_save(r: &mut portal2_core::save::SaveRegistry) {
    r.component::<Spotlight>("spotlight");
}

pub struct SpotlightPlugin;

impl Plugin for SpotlightPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<BeamMaterial>::default())
            .add_systems(Update, rotate_spotlights)
            .add_systems(PostUpdate, update_spotlights.after(TransformSystems::Propagate));
        let world = app.world_mut();
        for class in ["point_spotlight", "beam_spotlight"] {
            world.resource_mut::<SpawnHandlers>().register(class, spawn_spotlight);
            world.resource_mut::<InputHandlers>().register(class, spotlight_input);
        }
    }
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn beam_quad() -> Mesh {
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, vec![[0.0, -1.0, 0.0], [0.0, 1.0, 0.0], [1.0, 1.0, 0.0], [1.0, -1.0, 0.0]])
        .with_inserted_indices(Indices::U16(vec![0, 1, 2, 0, 2, 3]))
}

fn spawn_spotlight(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e).cloned() else { return };
    let beam_spotlight = me.classname == "beam_spotlight";
    let flags = me.spawnflags();
    let on = flags & 1 != 0;
    let length = me.get_f32("spotlightlength").unwrap_or(500.0).max(1.0);
    let width = me.get_f32("spotlightwidth").unwrap_or(50.0).clamp(1.0, MAX_WIDTH);
    let halo_scale = me.get_f32("haloscale").filter(|s| *s > 0.0).unwrap_or(DEFAULT_HALO_SCALE);
    let c = me
        .get("rendercolor")
        .map(|s| {
            let v: Vec<f32> = s.split_whitespace().filter_map(|p| p.parse().ok()).collect();
            if v.len() >= 3 { Vec3::new(v[0], v[1], v[2]) } else { Vec3::splat(255.0) }
        })
        .unwrap_or(Vec3::splat(255.0))
        / 255.0;
    let color = Vec3::new(srgb_to_linear(c.x), srgb_to_linear(c.y), srgb_to_linear(c.z)) * me.get_f32("hdrcolorscale").unwrap_or(1.0);

    let fs = world.resource::<GameFs>().clone();
    let (beam_tex, halo) = world.resource_scope(|world, mut textures: Mut<TextureCache>| {
        let mut images = world.resource_mut::<Assets<Image>>();
        (
            load_sprite_source(&fs, &mut textures, &mut images, BEAM_MATERIAL),
            load_sprite_source(&fs, &mut textures, &mut images, HALO_MATERIAL),
        )
    });
    let beam_material = world.resource_mut::<Assets<BeamMaterial>>().add(BeamMaterial {
        params: BeamParams { color: color.extend(1.0), shape: Vec4::new(length, width * 0.5, 0.0, 0.0) },
        base: beam_tex.base,
    });
    let halo_material = world.resource_mut::<Assets<SpriteMaterial>>().add(SpriteMaterial {
        params: SpriteParams {
            color: color.extend(1.0),
            shape: Vec4::new(halo_scale * 0.5, halo_scale * 0.5, 0.0, 9.0),
            state: Vec4::new(0.0, 1.0, 0.0, 0.0),
        },
        base: halo.base,
        alpha_mode: AlphaMode::Add,
        no_depth_test: true,
    });
    let (beam_mesh, halo_mesh) = {
        let mut meshes = world.resource_mut::<Assets<Mesh>>();
        (meshes.add(beam_quad()), meshes.add(sprite_quad()))
    };
    let visibility = if on { Visibility::Inherited } else { Visibility::Hidden };
    let beam = world
        .spawn((
            Name::new("spotlight beam"),
            Mesh3d(beam_mesh),
            MeshMaterial3d(beam_material),
            Transform::IDENTITY,
            bevy::camera::visibility::NoFrustumCulling,
            visibility,
        ))
        .id();
    let halo = world
        .spawn((
            Name::new("spotlight halo"),
            Mesh3d(halo_mesh),
            MeshMaterial3d(halo_material.clone()),
            Transform::IDENTITY,
            bevy::camera::visibility::NoFrustumCulling,
            visibility,
        ))
        .id();
    world.entity_mut(halo).insert(SourceSprite { on, mode: 9, proxy_size: 4.0, visible: 0.0, material: halo_material, quad: Some(halo) });
    world.entity_mut(e).add_children(&[beam, halo]);
    let axis = if flags & 16 != 0 {
        Vec3::X
    } else if flags & 32 != 0 {
        Vec3::Y
    } else {
        Vec3::Z
    };
    let speed = me.get_f32("maxspeed").unwrap_or(100.0) * if flags & 8 != 0 { -1.0 } else { 1.0 };
    world.entity_mut(e).insert(Spotlight {
        on,
        rotating: beam_spotlight && flags & 4 != 0,
        axis,
        speed: if beam_spotlight { speed } else { 0.0 },
        beam: Some(beam),
        halo: Some(halo),
    });
}

fn spotlight_input(world: &mut World, call: &InputCall) -> bool {
    let Some(mut s) = world.get_mut::<Spotlight>(call.target) else { return false };
    match call.input.as_str() {
        "lighton" | "turnon" => s.on = true,
        "lightoff" | "turnoff" => s.on = false,
        "lighttoggle" | "toggle" => s.on = !s.on,
        "start" => s.rotating = true,
        "stop" => s.rotating = false,
        "reverse" => s.speed = -s.speed,
        "setspeed" => s.speed = source_formats::kv::parse_f32_lenient(&call.param) * s.speed.signum(),
        // Dynamic light from the beam's end is not drawn.
        "forcerelink" => {}
        _ => return false,
    }
    let (on, beam, halo) = (s.on, s.beam, s.halo);
    for child in [beam, halo].into_iter().flatten() {
        if let Some(mut v) = world.get_mut::<Visibility>(child) {
            *v = if on { Visibility::Inherited } else { Visibility::Hidden };
        }
        if let Some(mut sprite) = world.get_mut::<SourceSprite>(child) {
            sprite.on = on;
        }
    }
    true
}

fn rotate_spotlights(time: Res<Time>, mut lights: Query<(&Spotlight, &mut Transform)>) {
    let dt = time.delta_secs();
    for (s, mut t) in &mut lights {
        if s.rotating && s.speed != 0.0 {
            let axis = t.rotation * s.axis;
            t.rotate(Quat::from_axis_angle(axis, (s.speed * dt).to_radians()));
        }
    }
}

/// Halos are brightest seen from in front of the light and gone from behind.
fn update_spotlights(
    main: Query<&GlobalTransform, With<MainCamera>>,
    lights: Query<(&GlobalTransform, &Spotlight)>,
    halos: Query<&MeshMaterial3d<SpriteMaterial>>,
    mut materials: ResMut<Assets<SpriteMaterial>>,
) {
    let Ok(eye) = main.single() else { return };
    let eye = eye.translation();
    for (gt, s) in &lights {
        if !s.on {
            continue;
        }
        let Some(halo) = s.halo.and_then(|h| halos.get(h).ok()) else { continue };
        let forward: Vec3 = gt.rotation() * Vec3::X;
        let to_eye = (eye - gt.translation()).normalize_or_zero();
        let fade = forward.dot(to_eye).clamp(0.0, 1.0);
        if let Some(mut m) = materials.get_mut(&halo.0)
            && (m.params.state.y - fade).abs() > 0.01
        {
            m.params.state.y = fade;
        }
    }
}
