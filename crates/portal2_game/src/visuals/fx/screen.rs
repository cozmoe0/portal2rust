//! Screen-space passes on the main view, which need its depth.
//!
//! The window's cameras share one multisampled colour texture, and each
//! later camera (the viewmodel, the interface) resolves it over the main
//! texture again, so whatever the main camera wrote into the resolved
//! texture would be lost. These passes therefore draw into the main view's
//! multisampled attachment itself with blending: ambient occlusion
//! multiplies the scene right after the opaque pass, before glass, beams,
//! particles and the portals' openings are drawn over it.
//!
//! The main camera is marked [`FxMainView`] (its depth texture made
//! sampleable); portal views never run these passes.

use bevy::core_pipeline::core_3d::main_opaque_pass_3d;
use bevy::core_pipeline::schedule::Core3d;
use bevy::core_pipeline::{Core3dSystems, FullscreenShader};
use bevy::pbr::main_transmissive_pass_3d;
use bevy::prelude::*;
use bevy::render::extract_component::{ExtractComponent, ExtractComponentPlugin};
use bevy::render::render_resource::binding_types::{texture_2d, texture_depth_2d_multisampled, uniform_buffer};
use bevy::render::render_resource::*;
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery};
use bevy::render::texture::TextureCache;
use bevy::render::view::{ViewDepthStencilTexture, ViewTarget, ViewUniform, ViewUniformOffset, ViewUniforms};
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};

use super::{FxSettings, features};
use portal2_core::view::MainCamera;

pub const AO_SHADER: &str = "embedded://portal2_render/shaders/fx_ao.wesl";

/// Samples per pixel and reach of the occlusion.
const AO_SAMPLES: f32 = 12.0;
const AO_RADIUS: f32 = 20.0;
const AO_FADE_DISTANCE: f32 = 2500.0;
/// The window cameras' multisampling (`camera.rs`).
const MAIN_SAMPLES: u32 = 4;

/// The player's view, which runs the overhaul's screen-space passes.
#[derive(Component, Clone, Copy, Default, ExtractComponent)]
#[extract_app(RenderApp)]
pub struct FxMainView;

pub struct FxScreenPlugin;

impl Plugin for FxScreenPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ExtractComponentPlugin::<FxMainView>::default()).add_systems(PostUpdate, mark_main_view);
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
        render_app
            .init_resource::<AoUniformBuffer>()
            .add_systems(RenderStartup, init_pipelines)
            .add_systems(Render, prepare.in_set(RenderSystems::PrepareResources))
            .add_systems(
                Core3d,
                ambient_occlusion.after(main_opaque_pass_3d).before(main_transmissive_pass_3d).in_set(Core3dSystems::MainPass),
            );
    }
}

/// Marks the main camera and lets its depth be sampled.
fn mark_main_view(mut commands: Commands, mut cameras: Query<(Entity, &mut Camera3d, Has<FxMainView>), With<MainCamera>>) {
    for (e, mut camera, marked) in &mut cameras {
        let usage: TextureUsages = camera.depth_texture_usages.into();
        if !usage.contains(TextureUsages::TEXTURE_BINDING) {
            camera.depth_texture_usages = (usage | TextureUsages::TEXTURE_BINDING).into();
        }
        if !marked {
            commands.entity(e).insert(FxMainView);
        }
    }
}

#[derive(Clone, Copy, Debug, Default, ShaderType)]
struct AoUniform {
    params: Vec4,
}

#[derive(Resource, Default)]
struct AoUniformBuffer(UniformBuffer<AoUniform>);

#[derive(Resource)]
struct ScreenPipelines {
    raw_layout: BindGroupLayoutDescriptor,
    apply_layout: BindGroupLayoutDescriptor,
    raw: CachedRenderPipelineId,
    apply: CachedRenderPipelineId,
}

/// The main view's raw occlusion, before the blur.
#[derive(Component)]
struct AoTexture(TextureView);

fn init_pipelines(mut commands: Commands, asset_server: Res<AssetServer>, cache: Res<PipelineCache>, fullscreen: Res<FullscreenShader>) {
    let raw_layout = BindGroupLayoutDescriptor::new(
        "fx_ao_raw",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (texture_depth_2d_multisampled(), uniform_buffer::<ViewUniform>(true), uniform_buffer::<AoUniform>(false)),
        ),
    );
    let apply_layout = BindGroupLayoutDescriptor::new(
        "fx_ao_apply",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_depth_2d_multisampled(),
                uniform_buffer::<ViewUniform>(true),
                uniform_buffer::<AoUniform>(false),
                texture_2d(TextureSampleType::Float { filterable: false }),
            ),
        ),
    );
    let shader: Handle<Shader> = asset_server.load(AO_SHADER);
    let raw = cache.queue_render_pipeline(RenderPipelineDescriptor {
        label: Some("fx_ao_raw".into()),
        layout: vec![raw_layout.clone()],
        vertex: fullscreen.to_vertex_state(),
        fragment: Some(FragmentState {
            shader: shader.clone(),
            entry_point: Some("ao_raw".into()),
            targets: vec![Some(ColorTargetState { format: TextureFormat::R16Float, blend: None, write_mask: ColorWrites::ALL })],
            ..default()
        }),
        ..default()
    });
    // Multiplies the scene: colour = src * dst, alpha kept.
    let multiply = BlendState {
        color: BlendComponent { src_factor: BlendFactor::Dst, dst_factor: BlendFactor::Zero, operation: BlendOperation::Add },
        alpha: BlendComponent { src_factor: BlendFactor::Zero, dst_factor: BlendFactor::One, operation: BlendOperation::Add },
    };
    let apply = cache.queue_render_pipeline(RenderPipelineDescriptor {
        label: Some("fx_ao_apply".into()),
        layout: vec![apply_layout.clone()],
        vertex: fullscreen.to_vertex_state(),
        fragment: Some(FragmentState {
            shader,
            entry_point: Some("ao_apply".into()),
            targets: vec![Some(ColorTargetState {
                format: TextureFormat::Rgba16Float,
                blend: Some(multiply),
                write_mask: ColorWrites::ALL,
            })],
            ..default()
        }),
        multisample: MultisampleState { count: MAIN_SAMPLES, ..default() },
        ..default()
    });
    commands.insert_resource(ScreenPipelines { raw_layout, apply_layout, raw, apply });
}

fn prepare(
    mut commands: Commands,
    settings: Option<Res<FxSettings>>,
    mut uniform: ResMut<AoUniformBuffer>,
    mut textures: ResMut<TextureCache>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    views: Query<(Entity, &ViewTarget), With<FxMainView>>,
) {
    let Some(settings) = settings else { return };
    if !settings.has(features::AO) {
        return;
    }
    uniform.0.set(AoUniform { params: Vec4::new(AO_RADIUS, settings.ao_strength, AO_FADE_DISTANCE, AO_SAMPLES) });
    uniform.0.write_buffer(&device, &queue);
    for (entity, target) in &views {
        let size = target.main_texture().size();
        let texture = textures.get(
            &device,
            TextureDescriptor {
                label: Some("fx_ao_raw"),
                size: Extent3d { width: size.width, height: size.height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format: TextureFormat::R16Float,
                usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
        );
        commands.entity(entity).insert(AoTexture(texture.default_view.clone()));
    }
}

fn ambient_occlusion(
    view: ViewQuery<(&ViewTarget, &ViewDepthStencilTexture, &ViewUniformOffset, &Msaa, Option<&AoTexture>), With<FxMainView>>,
    settings: Option<Res<FxSettings>>,
    pipelines: Option<Res<ScreenPipelines>>,
    uniform: Res<AoUniformBuffer>,
    view_uniforms: Res<ViewUniforms>,
    cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
    mut ctx: RenderContext,
) {
    let (Some(settings), Some(pipelines)) = (settings, pipelines) else { return };
    if !settings.has(features::AO) {
        return;
    }
    let (target, depth, offset, msaa, raw) = view.into_inner();
    let Some(raw) = raw else { return };
    if msaa.samples() != MAIN_SAMPLES {
        return;
    }
    let (Some(raw_pipeline), Some(apply_pipeline), Some(ao), Some(views)) = (
        cache.get_render_pipeline(pipelines.raw),
        cache.get_render_pipeline(pipelines.apply),
        uniform.0.binding(),
        view_uniforms.uniforms.binding(),
    ) else {
        return;
    };
    let depth_view = depth.texture().create_view(&TextureViewDescriptor { aspect: TextureAspect::DepthOnly, ..default() });
    let raw_group = device.create_bind_group(
        "fx_ao_raw",
        &cache.get_bind_group_layout(&pipelines.raw_layout),
        &BindGroupEntries::sequential((&depth_view, views.clone(), ao.clone())),
    );
    let apply_group = device.create_bind_group(
        "fx_ao_apply",
        &cache.get_bind_group_layout(&pipelines.apply_layout),
        &BindGroupEntries::sequential((&depth_view, views, ao, &raw.0)),
    );
    let encoder = ctx.command_encoder();
    {
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("fx_ao_raw"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: &raw.0,
                depth_slice: None,
                resolve_target: None,
                ops: Operations { load: LoadOp::Clear(Default::default()), store: StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(raw_pipeline);
        pass.set_bind_group(0, &raw_group, &[offset.offset]);
        pass.draw(0..3, 0..1);
    }
    let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
        label: Some("fx_ao_apply"),
        color_attachments: &[Some(target.get_color_attachment())],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_pipeline(apply_pipeline);
    pass.set_bind_group(0, &apply_group, &[offset.offset]);
    pass.draw(0..3, 0..1);
}
