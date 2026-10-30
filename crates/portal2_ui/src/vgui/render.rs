//! Draws the frame's interface geometry over the 3D view.
//!
//! The game's interface blends in linear light: its colours and textures
//! are gamma values, converted to linear, blended into the sRGB back buffer
//! and encoded again (a white bar at alpha 15 over a dark scene comes out
//! as bright as linear blending predicts, not gamma blending). The last
//! camera on the window (`VguiComposite`) draws the batches straight into
//! the linear HDR view texture after the scene; the shader decodes texels
//! and vertex colours from sRGB.

use bevy::core_pipeline::schedule::Core3d;
use bevy::core_pipeline::{Core3dSystems, FullscreenShader};
use bevy::mesh::VertexBufferLayout;
use bevy::prelude::*;
use bevy::render::extract_component::{ExtractComponent, ExtractComponentPlugin};
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::binding_types::{sampler, texture_2d};
use bevy::render::render_resource::*;
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery};
use bevy::render::texture::GpuImage;
use bevy::render::view::ViewTarget;
use bevy::render::{Extract, ExtractSchedule, Render, RenderApp, RenderStartup, RenderSystems};

pub const SHADER: &str = "embedded://portal2_ui/vgui/shaders/vgui.wesl";

/// Interface vertex: clip-space position, texture coordinate, RGBA colour.
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct VguiVertex {
    pub pos: [f32; 2],
    pub uv: [f32; 2],
    pub color: [u8; 4],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Blend {
    /// `SrcAlpha, InvSrcAlpha`.
    Alpha,
    /// `SrcAlpha, One` (`$additive` materials and additive fonts).
    Additive,
    /// `DstColor, Zero`: multiplies what is already drawn.
    Modulate,
}

#[derive(Clone, Debug)]
pub struct Batch {
    pub image: AssetId<Image>,
    pub blend: Blend,
    pub start: u32,
    pub count: u32,
}

/// This frame's interface geometry, built by `Surface::finish`.
#[derive(Resource, Default, Clone)]
pub struct VguiFrame {
    pub vertices: Vec<VguiVertex>,
    pub batches: Vec<Batch>,
    /// Replace the scene with the paused-game backdrop (grey and blurred)
    /// before drawing.
    pub backdrop: bool,
}

/// The pass that draws the interface over the finished 3D view.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct VguiDrawSet;

/// Marks the camera that composites the interface (the last on the window).
#[derive(Component, Clone, Copy, Default, ExtractComponent)]
#[extract_app(RenderApp)]
pub struct VguiComposite;

pub struct VguiRenderPlugin;

impl Plugin for VguiRenderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<VguiFrame>().add_plugins(ExtractComponentPlugin::<VguiComposite>::default());
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
        render_app
            .init_resource::<ExtractedFrame>()
            .init_resource::<VguiBuffers>()
            .add_systems(RenderStartup, init_pipelines)
            .add_systems(ExtractSchedule, extract_frame)
            .add_systems(
                Render,
                (prepare_buffers.in_set(RenderSystems::PrepareResources), prepare_bind_groups.in_set(RenderSystems::PrepareBindGroups)),
            )
            .add_systems(Core3d, draw_interface.in_set(Core3dSystems::PostProcess).in_set(VguiDrawSet));
    }
}

#[derive(Resource, Default)]
struct ExtractedFrame(VguiFrame);

fn extract_frame(mut out: ResMut<ExtractedFrame>, frame: Extract<Res<VguiFrame>>) {
    out.0.vertices.clear();
    out.0.vertices.extend_from_slice(&frame.vertices);
    out.0.batches.clear();
    out.0.batches.extend_from_slice(&frame.batches);
    out.0.backdrop = frame.backdrop;
}

#[derive(Resource)]
struct VguiPipelines {
    layout: BindGroupLayoutDescriptor,
    sampler: Sampler,
    backdrop: CachedRenderPipelineId,
    alpha: CachedRenderPipelineId,
    additive: CachedRenderPipelineId,
    modulate: CachedRenderPipelineId,
}

/// The HDR view texture the scene is in.
const TARGET_FORMAT: TextureFormat = TextureFormat::Rgba16Float;

fn init_pipelines(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
    fullscreen: Res<FullscreenShader>,
) {
    let layout = BindGroupLayoutDescriptor::new(
        "vgui_texture",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (texture_2d(TextureSampleType::Float { filterable: true }), sampler(SamplerBindingType::Filtering)),
        ),
    );
    let shader: Handle<Shader> = asset_server.load(SHADER);
    let pass = |label: &'static str, color: BlendComponent| {
        cache.queue_render_pipeline(RenderPipelineDescriptor {
            label: Some(label.into()),
            layout: vec![layout.clone()],
            vertex: VertexState {
                shader: shader.clone(),
                entry_point: Some("ui_vertex".into()),
                buffers: vec![VertexBufferLayout::from_vertex_formats(
                    VertexStepMode::Vertex,
                    [VertexFormat::Float32x2, VertexFormat::Float32x2, VertexFormat::Unorm8x4],
                )],
                ..default()
            },
            fragment: Some(FragmentState {
                shader: shader.clone(),
                entry_point: Some("ui_fragment".into()),
                targets: vec![Some(ColorTargetState {
                    format: TARGET_FORMAT,
                    blend: Some(BlendState {
                        color,
                        alpha: BlendComponent {
                            src_factor: BlendFactor::Zero,
                            dst_factor: BlendFactor::One,
                            operation: BlendOperation::Add,
                        },
                    }),
                    write_mask: ColorWrites::ALL,
                })],
                ..default()
            }),
            ..default()
        })
    };
    let add = BlendOperation::Add;
    let alpha =
        pass("vgui_alpha", BlendComponent { src_factor: BlendFactor::SrcAlpha, dst_factor: BlendFactor::OneMinusSrcAlpha, operation: add });
    let additive =
        pass("vgui_additive", BlendComponent { src_factor: BlendFactor::SrcAlpha, dst_factor: BlendFactor::One, operation: add });
    let modulate = pass("vgui_modulate", BlendComponent { src_factor: BlendFactor::Dst, dst_factor: BlendFactor::Zero, operation: add });
    let backdrop = cache.queue_render_pipeline(RenderPipelineDescriptor {
        label: Some("vgui_backdrop".into()),
        layout: vec![layout.clone()],
        vertex: fullscreen.to_vertex_state(),
        fragment: Some(FragmentState {
            shader: shader.clone(),
            entry_point: Some("backdrop".into()),
            targets: vec![Some(ColorTargetState { format: TARGET_FORMAT, blend: None, write_mask: ColorWrites::ALL })],
            ..default()
        }),
        ..default()
    });
    let sampler = device.create_sampler(&SamplerDescriptor { mag_filter: FilterMode::Linear, min_filter: FilterMode::Linear, ..default() });
    commands.insert_resource(VguiPipelines { layout, sampler, backdrop, alpha, additive, modulate });
}

#[derive(Resource)]
struct VguiBuffers {
    vertices: RawBufferVec<VguiVertex>,
}

impl Default for VguiBuffers {
    fn default() -> Self {
        let mut vertices = RawBufferVec::new(BufferUsages::VERTEX);
        vertices.set_label(Some("vgui_vertices"));
        Self { vertices }
    }
}

fn prepare_buffers(frame: Res<ExtractedFrame>, mut buffers: ResMut<VguiBuffers>, device: Res<RenderDevice>, queue: Res<RenderQueue>) {
    buffers.vertices.clear();
    if frame.0.batches.is_empty() {
        return;
    }
    buffers.vertices.values_mut().extend_from_slice(&frame.0.vertices);
    buffers.vertices.write_buffer(&device, &queue);
}

/// One bind group per batch for this frame (`None` while an image loads).
#[derive(Resource, Default)]
struct FrameBindGroups {
    images: Vec<Option<BindGroup>>,
}

/// The scene textures of a compositing view, for the backdrop pass.
#[derive(Component)]
struct ViewBindGroups {
    main_a: (TextureViewId, BindGroup),
    main_b: (TextureViewId, BindGroup),
}

fn prepare_bind_groups(
    mut commands: Commands,
    frame: Res<ExtractedFrame>,
    pipelines: Option<Res<VguiPipelines>>,
    cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
    images: Res<RenderAssets<GpuImage>>,
    views: Query<(Entity, &ViewTarget), With<VguiComposite>>,
) {
    let Some(pipelines) = pipelines else { return };
    let layout = cache.get_bind_group_layout(&pipelines.layout);
    for (entity, target) in &views {
        let make = |view: &TextureView| {
            device.create_bind_group("vgui_backdrop", &layout, &BindGroupEntries::sequential((view, &pipelines.sampler)))
        };
        let (a, b) = (target.main_texture_view(), target.main_texture_other_view());
        commands.entity(entity).insert(ViewBindGroups { main_a: (a.id(), make(a)), main_b: (b.id(), make(b)) });
    }
    let mut groups = FrameBindGroups::default();
    let mut by_image: Vec<(AssetId<Image>, Option<BindGroup>)> = Vec::new();
    for batch in &frame.0.batches {
        let group = match by_image.iter().find(|(id, _)| *id == batch.image) {
            Some((_, g)) => g.clone(),
            None => {
                let g = images.get(batch.image).map(|gpu| {
                    device.create_bind_group("vgui_bind_group", &layout, &BindGroupEntries::sequential((&gpu.texture_view, &gpu.sampler)))
                });
                by_image.push((batch.image, g.clone()));
                g
            }
        };
        groups.images.push(group);
    }
    commands.insert_resource(groups);
}

fn draw_interface(
    view: ViewQuery<(&ViewTarget, &ViewBindGroups), With<VguiComposite>>,
    frame: Res<ExtractedFrame>,
    groups: Option<Res<FrameBindGroups>>,
    buffers: Res<VguiBuffers>,
    pipelines: Option<Res<VguiPipelines>>,
    cache: Res<PipelineCache>,
    mut ctx: RenderContext,
) {
    let (Some(pipelines), Some(groups)) = (pipelines, groups) else { return };
    if frame.0.batches.is_empty() || groups.images.len() != frame.0.batches.len() {
        return;
    }
    let (Some(alpha), Some(additive), Some(modulate), Some(backdrop), Some(vertex_buffer)) = (
        cache.get_render_pipeline(pipelines.alpha),
        cache.get_render_pipeline(pipelines.additive),
        cache.get_render_pipeline(pipelines.modulate),
        cache.get_render_pipeline(pipelines.backdrop),
        buffers.vertices.buffer(),
    ) else {
        return;
    };
    let (target, view_groups) = view.into_inner();
    let encoder = ctx.command_encoder();
    if frame.0.backdrop {
        let post = target.post_process_write();
        let group = if view_groups.main_a.0 == post.source.id() { &view_groups.main_a.1 } else { &view_groups.main_b.1 };
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("vgui_backdrop"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: post.destination,
                depth_slice: None,
                resolve_target: None,
                ops: Operations::default(),
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(backdrop);
        pass.set_bind_group(0, group, &[]);
        pass.draw(0..3, 0..1);
    }
    let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
        label: Some("vgui_interface"),
        color_attachments: &[Some(RenderPassColorAttachment {
            view: target.main_texture_view(),
            depth_slice: None,
            resolve_target: None,
            ops: Operations { load: LoadOp::Load, store: StoreOp::Store },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_vertex_buffer(0, *vertex_buffer.slice(..));
    let mut current = None;
    for (batch, group) in frame.0.batches.iter().zip(&groups.images) {
        let Some(group) = group else { continue };
        if current != Some(batch.blend) {
            pass.set_pipeline(match batch.blend {
                Blend::Alpha => alpha,
                Blend::Additive => additive,
                Blend::Modulate => modulate,
            });
            current = Some(batch.blend);
        }
        pass.set_bind_group(0, group, &[]);
        pass.draw(batch.start..batch.start + batch.count, 0..1);
    }
}
