//! Custom materials that shade like Source's world and model shaders.

use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::pbr::{MaterialPipeline, MaterialPipelineKey, MeshPipelineKey};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, Face, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError,
};
use bevy::shader::ShaderRef;

pub const WORLD_SHADER: &str = "embedded://portal2/render/shaders/world.wesl";
pub const MODEL_SHADER: &str = "embedded://portal2/render/shaders/model.wesl";

pub mod world_flags {
    pub const BASE2: u32 = 1;
    pub const BUMP: u32 = 2;
    pub const SSBUMP: u32 = 4;
    pub const ENVMAP: u32 = 8;
    pub const ENVMAPMASK: u32 = 16;
    pub const BASEALPHA_ENVMAPMASK: u32 = 32;
    pub const NORMALALPHA_ENVMAPMASK: u32 = 64;
    pub const SELFILLUM: u32 = 128;
    pub const ALPHATEST: u32 = 256;
    pub const UNLIT: u32 = 512;
    pub const DETAIL: u32 = 1024;
    pub const LIGHTMAPPED: u32 = 2048;
    pub const TRANSLUCENT: u32 = 4096;
    pub const VERTEX_BLEND: u32 = 8192;
}

pub mod model_flags {
    pub const BUMP: u32 = 1;
    pub const ENVMAP: u32 = 2;
    pub const ENVMAPMASK: u32 = 4;
    pub const BASEALPHA_ENVMAPMASK: u32 = 8;
    pub const NORMALALPHA_ENVMAPMASK: u32 = 16;
    pub const SELFILLUM: u32 = 32;
    pub const ALPHATEST: u32 = 64;
    pub const UNLIT: u32 = 128;
    pub const PHONG: u32 = 256;
    pub const BAKED: u32 = 512;
    pub const TRANSLUCENT: u32 = 1024;
    pub const BASEMAPALPHA_PHONGMASK: u32 = 2048;
    pub const VERTEX_COLOR: u32 = 4096;
}

/// Pipeline state that cannot live in uniforms.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SourceMaterialKey {
    /// 0 = none, 1 = back, 2 = front.
    pub cull: u8,
    pub depth_bias: i32,
}

fn cull_code(cull: Option<Face>) -> u8 {
    match cull {
        None => 0,
        Some(Face::Back) => 1,
        Some(Face::Front) => 2,
    }
}

fn apply_key(descriptor: &mut RenderPipelineDescriptor, key: SourceMaterialKey, mesh_key: MeshPipelineKey) {
    let invert = mesh_key.contains(MeshPipelineKey::INVERT_CULLING);
    descriptor.primitive.cull_mode = match (key.cull, invert) {
        (1, false) | (2, true) => Some(Face::Back),
        (2, false) | (1, true) => Some(Face::Front),
        _ => None,
    };
    if let Some(depth_stencil) = descriptor.depth_stencil.as_mut() {
        depth_stencil.bias.constant = key.depth_bias;
    }
}

#[derive(Clone, Copy, Debug, Default, ShaderType)]
pub struct WorldParams {
    pub color: Vec4,
    pub envmap_tint: Vec4,
    pub selfillum_tint: Vec4,
    pub detail: Vec4,
    pub flags: u32,
    pub alpha_ref: f32,
    pub lightmap_scale: f32,
    pub envmap_contrast: f32,
    pub envmap_saturation: f32,
    /// Blend of reflections toward the local lightmap brightness (`$envmaplightscale`).
    pub envmap_light_scale: f32,
    pub _pad1: f32,
    pub _pad2: f32,
}

#[derive(Asset, TypePath, AsBindGroup, Clone, Debug)]
#[bind_group_data(SourceMaterialKey)]
pub struct WorldMaterial {
    #[uniform(0)]
    pub params: WorldParams,
    #[texture(1)]
    #[sampler(2)]
    pub base: Option<Handle<Image>>,
    #[texture(3)]
    #[sampler(4)]
    pub base2: Option<Handle<Image>>,
    #[texture(5)]
    #[sampler(6)]
    pub bump: Option<Handle<Image>>,
    #[texture(7)]
    #[sampler(8)]
    pub lightmap: Option<Handle<Image>>,
    #[texture(9, dimension = "cube")]
    #[sampler(10)]
    pub envmap: Option<Handle<Image>>,
    #[texture(11)]
    #[sampler(12)]
    pub envmask: Option<Handle<Image>>,
    #[texture(13)]
    #[sampler(14)]
    pub detail: Option<Handle<Image>>,
    pub alpha_mode: AlphaMode,
    pub cull: Option<Face>,
    pub depth_bias: i32,
}

impl Default for WorldMaterial {
    fn default() -> Self {
        Self {
            params: WorldParams {
                color: Vec4::ONE,
                envmap_tint: Vec4::ONE,
                selfillum_tint: Vec4::ONE,
                detail: Vec4::new(4.0, 1.0, 0.0, 0.0),
                lightmap_scale: 1.0,
                envmap_saturation: 1.0,
                alpha_ref: 0.5,
                ..default()
            },
            base: None,
            base2: None,
            bump: None,
            lightmap: None,
            envmap: None,
            envmask: None,
            detail: None,
            alpha_mode: AlphaMode::Opaque,
            cull: Some(Face::Back),
            depth_bias: 0,
        }
    }
}

impl From<&WorldMaterial> for SourceMaterialKey {
    fn from(m: &WorldMaterial) -> Self {
        Self { cull: cull_code(m.cull), depth_bias: m.depth_bias }
    }
}

impl Material for WorldMaterial {
    fn fragment_shader() -> ShaderRef {
        WORLD_SHADER.into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        self.alpha_mode
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
        key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        apply_key(descriptor, key.bind_group_data, key.mesh_key);
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, ShaderType)]
pub struct ModelLight {
    pub position: Vec4,
    pub color: Vec4,
    pub direction: Vec4,
    pub attenuation: Vec4,
}

#[derive(Clone, Copy, Debug, Default, ShaderType)]
pub struct ModelParams {
    pub color: Vec4,
    pub envmap_tint: Vec4,
    pub selfillum_tint: Vec4,
    pub phong: Vec4,
    pub ambient: [Vec4; 6],
    pub lights: [ModelLight; 4],
    pub flags: u32,
    pub alpha_ref: f32,
    pub num_lights: u32,
    pub _pad: f32,
}

#[derive(Asset, TypePath, AsBindGroup, Clone, Debug)]
#[bind_group_data(SourceMaterialKey)]
pub struct ModelMaterial {
    #[uniform(0)]
    pub params: ModelParams,
    #[texture(1)]
    #[sampler(2)]
    pub base: Option<Handle<Image>>,
    #[texture(3)]
    #[sampler(4)]
    pub bump: Option<Handle<Image>>,
    #[texture(5, dimension = "cube")]
    #[sampler(6)]
    pub envmap: Option<Handle<Image>>,
    #[texture(7)]
    #[sampler(8)]
    pub envmask: Option<Handle<Image>>,
    pub alpha_mode: AlphaMode,
    pub cull: Option<Face>,
    pub depth_bias: i32,
}

impl Default for ModelMaterial {
    fn default() -> Self {
        Self {
            params: ModelParams {
                color: Vec4::ONE,
                envmap_tint: Vec4::ONE,
                selfillum_tint: Vec4::ONE,
                phong: Vec4::new(5.0, 1.0, 0.0, 1.0),
                ambient: [Vec4::splat(0.2); 6],
                alpha_ref: 0.5,
                ..default()
            },
            base: None,
            bump: None,
            envmap: None,
            envmask: None,
            alpha_mode: AlphaMode::Opaque,
            cull: Some(Face::Back),
            depth_bias: 0,
        }
    }
}

impl From<&ModelMaterial> for SourceMaterialKey {
    fn from(m: &ModelMaterial) -> Self {
        Self { cull: cull_code(m.cull), depth_bias: m.depth_bias }
    }
}

impl Material for ModelMaterial {
    fn fragment_shader() -> ShaderRef {
        MODEL_SHADER.into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        self.alpha_mode
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
        key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        apply_key(descriptor, key.bind_group_data, key.mesh_key);
        Ok(())
    }
}
