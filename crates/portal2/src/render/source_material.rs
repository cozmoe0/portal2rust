//! Custom materials that shade like Source's world and model shaders.

use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::pbr::{MaterialPipeline, MaterialPipelineKey, MeshPipelineKey};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, Face, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError,
};
use bevy::render::storage::ShaderBuffer;
use bevy::shader::ShaderRef;

pub const WORLD_SHADER: &str = "embedded://portal2/render/shaders/world.wesl";
pub const MODEL_SHADER: &str = "embedded://portal2/render/shaders/model.wesl";
pub const REFRACT_SHADER: &str = "embedded://portal2/render/shaders/refract.wesl";

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
    /// `$bumpmap2`: the second texture's bump map, blended like the base.
    pub const BUMP2: u32 = 16384;
    /// `$blendmodulatetexture` sharpens the vertex blend.
    pub const BLEND_MODULATE: u32 = 32768;
    /// `$additive`: drawn with premultiplied blending, so the shader puts
    /// alpha into the colour and writes zero alpha.
    pub const ADDITIVE: u32 = 65536;
    /// The `Water` shader: water fog colour (lit by the lightmap) with,
    /// for goo, the flowing `$basetexture` on top and a dim reflection.
    pub const WATER: u32 = 131072;
    /// The water has a `$basetexture` (goo); `base2` holds its flow map.
    pub const WATER_BASE: u32 = 262144;
    /// `SolidEnergy` with a `$flowmap` (emancipation fields): the base
    /// texture's alpha, scrolled along the flow map in two phases, times
    /// `$flow_color` and masked by `$flowbounds` (`detail`).
    pub const SOLID_ENERGY: u32 = 524288;
    /// Paint (gels) drawn over the surface from the map's paint map (see
    /// docs/re/paint.md); set on paintable materials of paint maps.
    pub const PAINT: u32 = 1 << 20;
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
    /// Phong exponent (red) and albedo tint mask (green) from a texture.
    pub const EXPONENT_TEXTURE: u32 = 8192;
    /// Specular takes on the base colour where the exponent texture's green is set.
    pub const ALBEDO_TINT: u32 = 16384;
    /// Diffuse from local lights uses half-Lambert, (N·L / 2 + 1/2)².
    pub const HALF_LAMBERT: u32 = 32768;
    /// Diffuse from local lights is looked up in a light warp texture.
    pub const LIGHTWARP: u32 = 65536;
    /// Self-illumination mask from its own texture instead of base alpha.
    pub const SELFILLUM_MASK: u32 = 131072;
    pub const BASEMAPLUMINANCE_PHONGMASK: u32 = 262144;
    /// `$invertphongmask`: phong reflections use one minus their mask.
    pub const INVERT_PHONG_ENVMAPMASK: u32 = 524288;
    /// `$additive` (see `world_flags::ADDITIVE`).
    pub const ADDITIVE: u32 = 1 << 20;
}

pub mod refract_flags {
    /// `$localrefract`: refract a parallax lookup into the base texture
    /// instead of the scene behind the surface.
    pub const LOCAL: u32 = 1;
    pub const NORMALMAP: u32 = 2;
    pub const ENVMAP: u32 = 4;
    /// Four-tap blur of the scene behind (`$bluramount` of 1 or more).
    pub const BLUR: u32 = 8;
    /// `$refracttinttexture` multiplies the tint (times two).
    pub const TINT_TEXTURE: u32 = 16;
    /// `$additive` (see `world_flags::ADDITIVE`).
    pub const ADDITIVE: u32 = 32;
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
        if key.depth_bias != 0 {
            // Decals and overlays also get the game's slope-scaled decal
            // bias (`mat_slopescaledepthbias_decal`, 2 toward the viewer).
            depth_stencil.bias.slope_scale = DECAL_SLOPE_BIAS;
        }
    }
}

/// The projected-texture lights, cookies and shadow maps every world and
/// model material binds (see render/projected.rs). Set once at startup;
/// materials built before that (tests) bind nothing usable.
#[derive(Clone, Debug)]
pub struct ProjectedHandles {
    pub lights: Handle<ShaderBuffer>,
    pub cookies: Handle<Image>,
    pub shadows: [Handle<Image>; 4],
}

pub static PROJECTED: std::sync::OnceLock<ProjectedHandles> = std::sync::OnceLock::new();

fn projected_handles() -> (Handle<ShaderBuffer>, Option<Handle<Image>>, [Option<Handle<Image>>; 4]) {
    match PROJECTED.get() {
        Some(p) => (p.lights.clone(), Some(p.cookies.clone()), p.shadows.clone().map(Some)),
        None => (Handle::default(), None, [None, None, None, None]),
    }
}

/// Constant depth bias of decal materials and overlays, in steps of the
/// depth buffer's precision at the surface (reverse Z: toward the viewer).
pub const DECAL_DEPTH_BIAS: i32 = 64;
const DECAL_SLOPE_BIAS: f32 = 2.0;

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
    #[texture(15)]
    #[sampler(16)]
    pub bump2: Option<Handle<Image>>,
    #[texture(17)]
    #[sampler(18)]
    pub blend_modulate: Option<Handle<Image>>,
    /// The map's paint map, laid out like the lightmap atlas.
    #[texture(19)]
    #[sampler(20)]
    pub paint: Option<Handle<Image>>,
    #[texture(21)]
    #[sampler(22)]
    pub paint_splat: Option<Handle<Image>>,
    #[texture(23)]
    #[sampler(24)]
    pub paint_bubble_layout: Option<Handle<Image>>,
    #[texture(25)]
    #[sampler(26)]
    pub paint_bubble: Option<Handle<Image>>,
    #[texture(27, dimension = "cube")]
    #[sampler(28)]
    pub paint_envmap: Option<Handle<Image>>,
    #[storage(30, read_only)]
    pub projected_lights: Handle<ShaderBuffer>,
    #[texture(31, dimension = "2d_array")]
    #[sampler(32)]
    pub cookies: Option<Handle<Image>>,
    #[texture(33)]
    pub shadow0: Option<Handle<Image>>,
    #[texture(34)]
    pub shadow1: Option<Handle<Image>>,
    #[texture(35)]
    pub shadow2: Option<Handle<Image>>,
    #[texture(36)]
    pub shadow3: Option<Handle<Image>>,
    pub alpha_mode: AlphaMode,
    pub cull: Option<Face>,
    pub depth_bias: i32,
}

impl Default for WorldMaterial {
    fn default() -> Self {
        let (projected_lights, cookies, [shadow0, shadow1, shadow2, shadow3]) = projected_handles();
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
            bump2: None,
            blend_modulate: None,
            paint: None,
            paint_splat: None,
            paint_bubble_layout: None,
            paint_bubble: None,
            paint_envmap: None,
            projected_lights,
            cookies,
            shadow0,
            shadow1,
            shadow2,
            shadow3,
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
    /// xyz: position, w: kind (0 point, 1 spot, 2 directional).
    pub position: Vec4,
    /// rgb: intensity (linear), w: spot cone falloff exponent.
    pub color: Vec4,
    /// xyz: direction (spot / directional), w: cosine of the inner cone.
    pub direction: Vec4,
    /// xyz: constant, linear and quadratic attenuation, w: cosine of the outer cone.
    pub attenuation: Vec4,
}

#[derive(Clone, Copy, Debug, Default, ShaderType)]
pub struct ModelParams {
    pub color: Vec4,
    pub envmap_tint: Vec4,
    pub selfillum_tint: Vec4,
    /// x: exponent (0 = from the exponent texture), y: boost.
    pub phong: Vec4,
    /// xyz: Fresnel ranges (facing, middle, edge-on), w: how much the
    /// ranges also scale cube map reflections (`$envmapfresnel`).
    pub fresnel: Vec4,
    /// rgb: `$phongtint` (white when unset).
    pub phong_tint: Vec4,
    /// rgb: `$envmapcontrast` (0 = reflection as is, 1 = squared); not used by phong.
    pub envmap_contrast: Vec4,
    /// rgb: `$envmapsaturation` (0 = grey, 1 = as is); not used by phong.
    pub envmap_saturation: Vec4,
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
    #[texture(9)]
    #[sampler(10)]
    pub exponent: Option<Handle<Image>>,
    #[texture(11)]
    #[sampler(12)]
    pub selfillum_mask: Option<Handle<Image>>,
    #[texture(13)]
    #[sampler(14)]
    pub lightwarp: Option<Handle<Image>>,
    #[storage(30, read_only)]
    pub projected_lights: Handle<ShaderBuffer>,
    #[texture(31, dimension = "2d_array")]
    #[sampler(32)]
    pub cookies: Option<Handle<Image>>,
    #[texture(33)]
    pub shadow0: Option<Handle<Image>>,
    #[texture(34)]
    pub shadow1: Option<Handle<Image>>,
    #[texture(35)]
    pub shadow2: Option<Handle<Image>>,
    #[texture(36)]
    pub shadow3: Option<Handle<Image>>,
    pub alpha_mode: AlphaMode,
    pub cull: Option<Face>,
    pub depth_bias: i32,
}

impl Default for ModelMaterial {
    fn default() -> Self {
        let (projected_lights, cookies, [shadow0, shadow1, shadow2, shadow3]) = projected_handles();
        Self {
            params: ModelParams {
                color: Vec4::ONE,
                envmap_tint: Vec4::ONE,
                selfillum_tint: Vec4::ONE,
                phong: Vec4::new(5.0, 1.0, 0.0, 0.0),
                fresnel: Vec4::new(0.0, 0.5, 1.0, 0.0),
                phong_tint: Vec4::ONE,
                envmap_saturation: Vec4::ONE,
                ambient: [Vec4::splat(0.2); 6],
                alpha_ref: 0.5,
                ..default()
            },
            base: None,
            bump: None,
            envmap: None,
            envmask: None,
            exponent: None,
            selfillum_mask: None,
            lightwarp: None,
            projected_lights,
            cookies,
            shadow0,
            shadow1,
            shadow2,
            shadow3,
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

#[derive(Clone, Copy, Debug, Default, ShaderType)]
pub struct RefractParams {
    /// rgb: `$refracttint`, already converted to linear.
    pub refract_tint: Vec4,
    /// rgb: `$envmaptint`, already converted to linear.
    pub envmap_tint: Vec4,
    /// rgb: `$envmapcontrast` (0 = as is, 1 = squared).
    pub envmap_contrast: Vec4,
    /// rgb: `$envmapsaturation` (0 = grey, 1 = as is).
    pub envmap_saturation: Vec4,
    /// x: `$refractamount`, y: `$localrefractdepth`, z: base texture height
    /// over width as a whole number (scales the parallax offset in u).
    pub refract: Vec4,
    pub flags: u32,
    pub _pad1: f32,
    pub _pad2: f32,
    pub _pad3: f32,
}

/// Source's `Refract` shader on studio models: the scene behind the surface
/// (Bevy's copy of the opaque pass), or with `$localrefract` the base
/// texture, offset by the normal map and tinted, plus a cube map reflection.
#[derive(Asset, TypePath, AsBindGroup, Clone, Debug)]
#[bind_group_data(SourceMaterialKey)]
pub struct RefractMaterial {
    #[uniform(0)]
    pub params: RefractParams,
    #[texture(1)]
    #[sampler(2)]
    pub base: Option<Handle<Image>>,
    #[texture(3)]
    #[sampler(4)]
    pub normal: Option<Handle<Image>>,
    #[texture(5, dimension = "cube")]
    #[sampler(6)]
    pub envmap: Option<Handle<Image>>,
    #[texture(7)]
    #[sampler(8)]
    pub tint: Option<Handle<Image>>,
    pub alpha_mode: AlphaMode,
    pub cull: Option<Face>,
}

impl Default for RefractMaterial {
    fn default() -> Self {
        Self {
            params: RefractParams {
                refract_tint: Vec4::ONE,
                envmap_tint: Vec4::ONE,
                envmap_saturation: Vec4::ONE,
                refract: Vec4::new(2.0, 0.05, 1.0, 0.0),
                ..default()
            },
            base: None,
            normal: None,
            envmap: None,
            tint: None,
            alpha_mode: AlphaMode::Opaque,
            cull: Some(Face::Back),
        }
    }
}

impl From<&RefractMaterial> for SourceMaterialKey {
    fn from(m: &RefractMaterial) -> Self {
        Self { cull: cull_code(m.cull), depth_bias: 0 }
    }
}

impl Material for RefractMaterial {
    fn fragment_shader() -> ShaderRef {
        REFRACT_SHADER.into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        self.alpha_mode
    }

    /// Everything but local refraction samples the scene drawn before it,
    /// which puts it in the transmissive pass after a copy of the opaque one.
    fn reads_view_transmission_texture(&self) -> bool {
        self.params.flags & refract_flags::LOCAL == 0
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
