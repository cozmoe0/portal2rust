//! Atmosphere for the overhaul: a faint distance haze in the materials and
//! light scattered in the air.
//!
//! The haze takes the map's mean ambient light (the leaf ambient samples
//! vrad baked) as its colour, so a cold test chamber and a warm industrial
//! void each get their own, and it applies in every view, portal views
//! included.
//!
//! The scattering pass marches the main view's rays up to the depth buffer
//! at half resolution: the projected textures (the chambers' sun and
//! floodlights, shadowed where their light really falls), the overhaul's
//! volumetric lights (fixture spotlights, portals) and a little ambient
//! haze, so huge dark spaces read as air rather than flat black. It is
//! blended into the main view's multisampled scene after the transparent
//! pass (see `screen.rs` for why), before the viewmodel is drawn.

use bevy::core_pipeline::core_3d::main_transparent_pass_3d;
use bevy::core_pipeline::schedule::Core3d;
use bevy::core_pipeline::{Core3dSystems, FullscreenShader};
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::binding_types::{
    sampler, storage_buffer_read_only_sized, texture_2d, texture_2d_array, texture_depth_2d_multisampled, uniform_buffer,
};
use bevy::render::render_resource::*;
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery};
use bevy::render::storage::GpuShaderBuffer;
use bevy::render::texture::{GpuImage, TextureCache};
use bevy::render::view::{ViewDepthStencilTexture, ViewTarget, ViewUniform, ViewUniformOffset, ViewUniforms};
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};
use portal2_render::fx::FX;
use portal2_render::source_material::PROJECTED;

use super::material::{FxBuffer, FxFillSet};
use super::screen::FxMainView;
use super::{FxSettings, features};
use portal2_world::CurrentMap;

pub const VOLUMETRIC_SHADER: &str = "embedded://portal2_render/shaders/fx_volumetric.wesl";

/// Where the haze starts and how thick it can get.
const HAZE_START: f32 = 256.0;
const HAZE_MAX: f32 = 0.35;
/// The medium: in-scattering and extinction per unit, forward scattering.
const SCATTERING: f32 = 0.003;
const EXTINCTION: f32 = 0.00008;
const ANISOTROPY: f32 = 0.35;
/// How far the march goes into empty space, and how much ambient haze it adds.
const MAX_DISTANCE: f32 = 3500.0;
const AMBIENT_SCATTER: f32 = 0.0085;
const MAIN_SAMPLES: u32 = 4;

/// The map's haze colour (its mean baked ambient light).
#[derive(Resource, Default)]
struct Haze {
    color: Vec3,
}

pub struct FxAtmospherePlugin;

impl Plugin for FxAtmospherePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Haze>()
            .add_systems(PreUpdate, map_haze.after(portal2_core::sets::LoadMapSet))
            .add_systems(PostUpdate, fill_haze.in_set(FxFillSet));
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
        render_app
            .init_resource::<VolumeUniformBuffer>()
            .add_systems(RenderStartup, init_pipelines)
            .add_systems(Render, prepare.in_set(RenderSystems::PrepareResources))
            .add_systems(
                Core3d,
                volumetrics.after(main_transparent_pass_3d).after(super::ssr::FxReflectionSet).in_set(Core3dSystems::MainPass),
            );
    }
}

/// The mean of a map's ambient samples (all six faces).
pub fn mean_ambient(samples: &[source_formats::bsp::LeafAmbientSample]) -> Vec3 {
    if samples.is_empty() {
        return Vec3::splat(0.02);
    }
    let sum: Vec3 = samples.iter().map(|s| s.cube.iter().copied().sum::<Vec3>() / 6.0).sum();
    sum / samples.len() as f32
}

fn map_haze(loaded: Option<Res<portal2_core::LoadedMap>>, mut seen: Local<u64>, map: Option<Res<CurrentMap>>, mut haze: ResMut<Haze>) {
    let generation = loaded.map_or(0, |l| l.generation);
    if generation == *seen {
        return;
    }
    *seen = generation;
    let Some(map) = map else { return };
    // Kept within a sensible range: some maps bake almost no ambient light,
    // a few are lit by huge skies.
    let c = mean_ambient(&map.bsp.leaf_ambient_samples);
    let luma = c.dot(Vec3::new(0.299, 0.587, 0.114)).max(1e-4);
    haze.color = c * (luma.clamp(0.01, 0.25) / luma);
}

fn fill_haze(settings: Res<FxSettings>, haze: Res<Haze>, mut buffer: ResMut<FxBuffer>) {
    let h = buffer.header_mut();
    let density = if settings.has(features::FOG) { settings.haze_density / 1000.0 } else { 0.0 };
    h[8..12].copy_from_slice(&[haze.color.x, haze.color.y, haze.color.z, density]);
    h[12..16].copy_from_slice(&[HAZE_START, HAZE_MAX, 0.0, 0.0]);
}

#[derive(Clone, Copy, Debug, Default, ShaderType)]
struct VolumeUniform {
    params: Vec4,
    medium: Vec4,
}

#[derive(Resource, Default)]
struct VolumeUniformBuffer {
    buffer: UniformBuffer<VolumeUniform>,
    time: f32,
}

#[derive(Resource)]
struct VolumePipelines {
    march_layout: BindGroupLayoutDescriptor,
    composite_layout: BindGroupLayoutDescriptor,
    march: CachedRenderPipelineId,
    composite: CachedRenderPipelineId,
}

#[derive(Component)]
struct VolumeTexture(TextureView);

fn init_pipelines(mut commands: Commands, asset_server: Res<AssetServer>, cache: Res<PipelineCache>, fullscreen: Res<FullscreenShader>) {
    let shadow = || texture_2d(TextureSampleType::Float { filterable: false });
    let march_layout = BindGroupLayoutDescriptor::new(
        "fx_volume_march",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_depth_2d_multisampled(),
                uniform_buffer::<ViewUniform>(true),
                uniform_buffer::<VolumeUniform>(false),
                storage_buffer_read_only_sized(false, None),
                storage_buffer_read_only_sized(false, None),
                texture_2d_array(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                shadow(),
                shadow(),
                shadow(),
                shadow(),
                shadow(),
                shadow(),
                shadow(),
                shadow(),
            ),
        ),
    );
    let composite_layout = BindGroupLayoutDescriptor::new(
        "fx_volume_composite",
        &BindGroupLayoutEntries::with_indices(
            ShaderStages::FRAGMENT,
            (
                (0, texture_depth_2d_multisampled()),
                (1, uniform_buffer::<ViewUniform>(true)),
                (15, texture_2d(TextureSampleType::Float { filterable: false })),
            ),
        ),
    );
    let shader: Handle<Shader> = asset_server.load(VOLUMETRIC_SHADER);
    let march = cache.queue_render_pipeline(RenderPipelineDescriptor {
        label: Some("fx_volume_march".into()),
        layout: vec![march_layout.clone()],
        vertex: fullscreen.to_vertex_state(),
        fragment: Some(FragmentState {
            shader: shader.clone(),
            entry_point: Some("march".into()),
            targets: vec![Some(ColorTargetState { format: TextureFormat::Rgba16Float, blend: None, write_mask: ColorWrites::ALL })],
            ..default()
        }),
        ..default()
    });
    // In-scattered light plus the scene times the transmittance.
    let over = BlendState {
        color: BlendComponent { src_factor: BlendFactor::One, dst_factor: BlendFactor::SrcAlpha, operation: BlendOperation::Add },
        alpha: BlendComponent { src_factor: BlendFactor::Zero, dst_factor: BlendFactor::One, operation: BlendOperation::Add },
    };
    let composite = cache.queue_render_pipeline(RenderPipelineDescriptor {
        label: Some("fx_volume_composite".into()),
        layout: vec![composite_layout.clone()],
        vertex: fullscreen.to_vertex_state(),
        fragment: Some(FragmentState {
            shader,
            entry_point: Some("composite".into()),
            targets: vec![Some(ColorTargetState { format: TextureFormat::Rgba16Float, blend: Some(over), write_mask: ColorWrites::ALL })],
            ..default()
        }),
        multisample: MultisampleState { count: MAIN_SAMPLES, ..default() },
        ..default()
    });
    commands.insert_resource(VolumePipelines { march_layout, composite_layout, march, composite });
}

fn prepare(
    mut commands: Commands,
    time: Res<Time>,
    settings: Option<Res<FxSettings>>,
    mut uniform: ResMut<VolumeUniformBuffer>,
    mut textures: ResMut<TextureCache>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    views: Query<(Entity, &ViewTarget), With<FxMainView>>,
) {
    let Some(settings) = settings else { return };
    if !settings.has(features::VOLUMETRICS) {
        return;
    }
    uniform.time += time.delta_secs();
    let noise = if settings.has(features::PROCEDURAL) { 1.0 } else { 0.0 };
    let value = VolumeUniform {
        params: Vec4::new(
            settings.volumetric_steps.max(1) as f32,
            SCATTERING * settings.volumetric_density,
            MAX_DISTANCE,
            if settings.has(features::FOG) { AMBIENT_SCATTER } else { 0.0 },
        ),
        medium: Vec4::new(EXTINCTION * settings.volumetric_density, ANISOTROPY, uniform.time, noise),
    };
    uniform.buffer.set(value);
    uniform.buffer.write_buffer(&device, &queue);
    for (entity, target) in &views {
        let size = target.main_texture().size();
        let texture = textures.get(
            &device,
            TextureDescriptor {
                label: Some("fx_volume"),
                size: Extent3d { width: size.width.div_ceil(2), height: size.height.div_ceil(2), depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format: TextureFormat::Rgba16Float,
                usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
        );
        commands.entity(entity).insert(VolumeTexture(texture.default_view.clone()));
    }
}

fn volumetrics(
    view: ViewQuery<(&ViewTarget, &ViewDepthStencilTexture, &ViewUniformOffset, &Msaa, Option<&VolumeTexture>), With<FxMainView>>,
    settings: Option<Res<FxSettings>>,
    pipelines: Option<Res<VolumePipelines>>,
    uniform: Res<VolumeUniformBuffer>,
    view_uniforms: Res<ViewUniforms>,
    buffers: Res<RenderAssets<GpuShaderBuffer>>,
    images: Res<RenderAssets<GpuImage>>,
    cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
    mut ctx: RenderContext,
) {
    let (Some(settings), Some(pipelines), Some(fx), Some(projected)) = (settings, pipelines, FX.get(), PROJECTED.get()) else {
        return;
    };
    if !settings.has(features::VOLUMETRICS) {
        return;
    }
    let (target, depth, offset, msaa, volume) = view.into_inner();
    let Some(volume) = volume else { return };
    if msaa.samples() != MAIN_SAMPLES {
        return;
    }
    let (Some(march), Some(composite), Some(params), Some(views)) = (
        cache.get_render_pipeline(pipelines.march),
        cache.get_render_pipeline(pipelines.composite),
        uniform.buffer.binding(),
        view_uniforms.uniforms.binding(),
    ) else {
        return;
    };
    let (Some(fx_buffer), Some(projected_buffer), Some(cookies)) =
        (buffers.get(&fx.buffer), buffers.get(&projected.lights), images.get(&projected.cookies))
    else {
        return;
    };
    let pshadows: Option<Vec<&GpuImage>> = projected.shadows.iter().map(|h| images.get(h)).collect();
    let fshadows: Option<Vec<&GpuImage>> = fx.shadows.iter().map(|h| images.get(h)).collect();
    let (Some(p), Some(f)) = (pshadows, fshadows) else { return };
    let depth_view = depth.texture().create_view(&TextureViewDescriptor { aspect: TextureAspect::DepthOnly, ..default() });
    let march_group = device.create_bind_group(
        "fx_volume_march",
        &cache.get_bind_group_layout(&pipelines.march_layout),
        &BindGroupEntries::sequential((
            &depth_view,
            views.clone(),
            params,
            fx_buffer.buffer.as_entire_binding(),
            projected_buffer.buffer.as_entire_binding(),
            &cookies.texture_view,
            &cookies.sampler,
            &p[0].texture_view,
            &p[1].texture_view,
            &p[2].texture_view,
            &p[3].texture_view,
            &f[0].texture_view,
            &f[1].texture_view,
            &f[2].texture_view,
            &f[3].texture_view,
        )),
    );
    let composite_group = device.create_bind_group(
        "fx_volume_composite",
        &cache.get_bind_group_layout(&pipelines.composite_layout),
        &BindGroupEntries::with_indices(((0, &depth_view), (1, views), (15, &volume.0))),
    );
    let encoder = ctx.command_encoder();
    {
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("fx_volume_march"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: &volume.0,
                depth_slice: None,
                resolve_target: None,
                ops: Operations { load: LoadOp::Clear(Default::default()), store: StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(march);
        pass.set_bind_group(0, &march_group, &[offset.offset]);
        pass.draw(0..3, 0..1);
    }
    let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
        label: Some("fx_volume_composite"),
        color_attachments: &[Some(target.get_color_attachment())],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_pipeline(composite);
    pass.set_bind_group(0, &composite_group, &[offset.offset]);
    pass.draw(0..3, 0..1);
}

#[cfg(test)]
mod tests {
    use super::*;
    use source_formats::bsp::LeafAmbientSample;

    #[test]
    fn haze_colour_is_the_mean_ambient() {
        let warm = LeafAmbientSample { cube: [Vec3::new(0.2, 0.1, 0.05); 6], pos: [0; 3] };
        let cold = LeafAmbientSample { cube: [Vec3::new(0.0, 0.1, 0.15); 6], pos: [0; 3] };
        let mean = mean_ambient(&[warm, cold]);
        assert!((mean - Vec3::new(0.1, 0.1, 0.1)).length() < 1e-5);
        assert_eq!(mean_ambient(&[]), Vec3::splat(0.02));
    }
}
