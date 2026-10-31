//! Screen-space reflections (ultra): puddles and very glossy surfaces show
//! the scene around them, portals and beams included.
//!
//! The FX world shader marks how mirror-like each opaque pixel is in the
//! scene's alpha. After the main view's transparent pass the scene is
//! copied, reflected rays are marched against the depth, and what they
//! find is added into the multisampled scene (see `screen.rs` for why),
//! the pass also putting alpha back to 1. Rays that leave the screen fade
//! out, leaving the probe reflection the material already drew.

use bevy::core_pipeline::core_3d::main_transparent_pass_3d;
use bevy::core_pipeline::schedule::Core3d;
use bevy::core_pipeline::{Core3dSystems, FullscreenShader};
use bevy::prelude::*;
use bevy::render::render_resource::binding_types::{sampler, texture_2d, texture_depth_2d_multisampled, uniform_buffer};
use bevy::render::render_resource::*;
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery};
use bevy::render::texture::TextureCache;
use bevy::render::view::{ViewDepthStencilTexture, ViewTarget, ViewUniform, ViewUniformOffset, ViewUniforms};
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};

use super::screen::FxMainView;
use super::{FxSettings, features};

pub const SSR_SHADER: &str = "embedded://portal2_render/shaders/fx_ssr.wesl";

const REACH: f32 = 1400.0;
const STEPS: f32 = 40.0;
const STRENGTH: f32 = 1.0;
const MAIN_SAMPLES: u32 = 4;

/// Runs after the transparent pass and before the volumetric light.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct FxReflectionSet;

pub struct FxSsrPlugin;

impl Plugin for FxSsrPlugin {
    fn build(&self, app: &mut App) {
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
        render_app
            .init_resource::<SsrUniformBuffer>()
            .add_systems(RenderStartup, init_pipelines)
            .add_systems(Render, prepare.in_set(RenderSystems::PrepareResources))
            .add_systems(Core3d, reflections.after(main_transparent_pass_3d).in_set(Core3dSystems::MainPass).in_set(FxReflectionSet));
    }
}

#[derive(Clone, Copy, Debug, Default, ShaderType)]
struct SsrUniform {
    params: Vec4,
}

#[derive(Resource, Default)]
struct SsrUniformBuffer(UniformBuffer<SsrUniform>);

#[derive(Resource)]
struct SsrPipelines {
    layout: BindGroupLayoutDescriptor,
    sampler: Sampler,
    copy: CachedRenderPipelineId,
    reflect: CachedRenderPipelineId,
}

#[derive(Component)]
struct SsrTexture(TextureView);

fn init_pipelines(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
    fullscreen: Res<FullscreenShader>,
) {
    let layout = BindGroupLayoutDescriptor::new(
        "fx_ssr",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_depth_2d_multisampled(),
                uniform_buffer::<ViewUniform>(true),
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                uniform_buffer::<SsrUniform>(false),
            ),
        ),
    );
    let shader: Handle<Shader> = asset_server.load(SSR_SHADER);
    let copy = cache.queue_render_pipeline(RenderPipelineDescriptor {
        label: Some("fx_ssr_copy".into()),
        layout: vec![layout.clone()],
        vertex: fullscreen.to_vertex_state(),
        fragment: Some(FragmentState {
            shader: shader.clone(),
            entry_point: Some("copy".into()),
            targets: vec![Some(ColorTargetState { format: TextureFormat::Rgba16Float, blend: None, write_mask: ColorWrites::ALL })],
            ..default()
        }),
        ..default()
    });
    // Reflections are added; alpha goes back to 1.
    let add = BlendState {
        color: BlendComponent { src_factor: BlendFactor::One, dst_factor: BlendFactor::One, operation: BlendOperation::Add },
        alpha: BlendComponent { src_factor: BlendFactor::One, dst_factor: BlendFactor::Zero, operation: BlendOperation::Add },
    };
    let reflect = cache.queue_render_pipeline(RenderPipelineDescriptor {
        label: Some("fx_ssr".into()),
        layout: vec![layout.clone()],
        vertex: fullscreen.to_vertex_state(),
        fragment: Some(FragmentState {
            shader,
            entry_point: Some("reflection".into()),
            targets: vec![Some(ColorTargetState { format: TextureFormat::Rgba16Float, blend: Some(add), write_mask: ColorWrites::ALL })],
            ..default()
        }),
        multisample: MultisampleState { count: MAIN_SAMPLES, ..default() },
        ..default()
    });
    let sampler = device.create_sampler(&SamplerDescriptor {
        mag_filter: FilterMode::Linear,
        min_filter: FilterMode::Linear,
        address_mode_u: AddressMode::ClampToEdge,
        address_mode_v: AddressMode::ClampToEdge,
        ..default()
    });
    commands.insert_resource(SsrPipelines { layout, sampler, copy, reflect });
}

fn prepare(
    mut commands: Commands,
    settings: Option<Res<FxSettings>>,
    mut uniform: ResMut<SsrUniformBuffer>,
    mut textures: ResMut<TextureCache>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    views: Query<(Entity, &ViewTarget), With<FxMainView>>,
) {
    let Some(settings) = settings else { return };
    if !settings.has(features::SSR) {
        return;
    }
    uniform.0.set(SsrUniform { params: Vec4::new(REACH, STEPS, STRENGTH, 0.0) });
    uniform.0.write_buffer(&device, &queue);
    for (entity, target) in &views {
        let size = target.main_texture().size();
        let texture = textures.get(
            &device,
            TextureDescriptor {
                label: Some("fx_ssr_scene"),
                size: Extent3d { width: size.width, height: size.height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format: TextureFormat::Rgba16Float,
                usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
        );
        commands.entity(entity).insert(SsrTexture(texture.default_view.clone()));
    }
}

fn reflections(
    view: ViewQuery<(&ViewTarget, &ViewDepthStencilTexture, &ViewUniformOffset, &Msaa, Option<&SsrTexture>), With<FxMainView>>,
    settings: Option<Res<FxSettings>>,
    pipelines: Option<Res<SsrPipelines>>,
    uniform: Res<SsrUniformBuffer>,
    view_uniforms: Res<ViewUniforms>,
    cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
    mut ctx: RenderContext,
) {
    let (Some(settings), Some(pipelines)) = (settings, pipelines) else { return };
    if !settings.has(features::SSR) {
        return;
    }
    let (target, depth, offset, msaa, scene) = view.into_inner();
    let Some(scene) = scene else { return };
    if msaa.samples() != MAIN_SAMPLES {
        return;
    }
    let (Some(copy), Some(reflect), Some(params), Some(views)) = (
        cache.get_render_pipeline(pipelines.copy),
        cache.get_render_pipeline(pipelines.reflect),
        uniform.0.binding(),
        view_uniforms.uniforms.binding(),
    ) else {
        return;
    };
    let layout = cache.get_bind_group_layout(&pipelines.layout);
    let depth_view = depth.texture().create_view(&TextureViewDescriptor { aspect: TextureAspect::DepthOnly, ..default() });
    let group = |color: &TextureView| {
        device.create_bind_group(
            "fx_ssr",
            &layout,
            &BindGroupEntries::sequential((&depth_view, views.clone(), color, &pipelines.sampler, params.clone())),
        )
    };
    let copy_group = group(target.main_texture_view());
    let reflect_group = group(&scene.0);
    let encoder = ctx.command_encoder();
    {
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("fx_ssr_copy"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: &scene.0,
                depth_slice: None,
                resolve_target: None,
                ops: Operations { load: LoadOp::Clear(Default::default()), store: StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(copy);
        pass.set_bind_group(0, &copy_group, &[offset.offset]);
        pass.draw(0..3, 0..1);
    }
    let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
        label: Some("fx_ssr"),
        color_attachments: &[Some(target.get_color_attachment())],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_pipeline(reflect);
    pass.set_bind_group(0, &reflect_group, &[offset.offset]);
    pass.draw(0..3, 0..1);
}
