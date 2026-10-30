//! Post-processing of the finished 3D view, on the interface camera after
//! the auto-exposure histogram and before the interface (so it covers the
//! scene, the portals' views and the viewmodel, and never the menus).
//!
//! Bloom: the view is prefiltered (brightness above 1, the display's white,
//! with a soft knee) into a half-size mip chain, downsampled with a 13-tap
//! filter and added back up with a tent filter. Then the tone curve: values
//! up to its knee stay exactly as the game draws them and brighter ones roll
//! off toward the white point instead of clipping, after a light grade
//! (cooler shadows, warmer highlights, a touch of contrast and saturation).
//! A half step of noise in display space hides banding. Each part follows
//! its feature (`BLOOM`, `GRADE`); with neither nothing runs.

use bevy::core_pipeline::schedule::Core3d;
use bevy::core_pipeline::{Core3dSystems, FullscreenShader};
use bevy::prelude::*;
use bevy::render::render_resource::binding_types::{sampler, texture_2d, uniform_buffer};
use bevy::render::render_resource::*;
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery};
use bevy::render::texture::TextureCache;
use bevy::render::view::ViewTarget;
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};
use portal2_ui::vgui::render::{VguiComposite, VguiDrawSet};

use super::{FxGradeSet, FxSceneSet, FxSettings, features};
use crate::visuals::tonemap::ExposureMeasureSet;

pub const POST_SHADER: &str = "embedded://portal2_render/shaders/fx_post.wesl";

const FORMAT: TextureFormat = TextureFormat::Rgba16Float;
const BLOOM_MIPS: u32 = 6;
/// Bloom starts at the display's white.
const BLOOM_THRESHOLD: f32 = 1.0;
const BLOOM_KNEE: f32 = 0.5;
/// The tone curve's knee and white point (in exposed scene values).
pub const TONE_KNEE: f32 = 0.85;
pub const TONE_WHITE: f32 = 5.0;

/// The tone curve (as `fx_post.wesl`'s `shoulder`).
pub fn shoulder(x: f32, knee: f32, white: f32) -> f32 {
    if x <= knee {
        return x;
    }
    let t = (x - knee) / (1.0 - knee);
    let tw = ((white - knee) / (1.0 - knee)).max(1e-3);
    let g = t * (1.0 + t / (tw * tw)) / (1.0 + t);
    (knee + (1.0 - knee) * g).min(1.0)
}

#[derive(Clone, Copy, Debug, Default, ShaderType)]
struct FxPostUniform {
    bloom: Vec4,
    shadow_tint: Vec4,
    highlight_tint: Vec4,
    tone: Vec4,
    flags: Vec4,
}

/// `rgb` scaled to luminance 1.
fn unit_luminance(rgb: Vec3) -> Vec3 {
    rgb / rgb.dot(Vec3::new(0.2126, 0.7152, 0.0722))
}

impl FxPostUniform {
    fn from_settings(s: &FxSettings, frame: u32) -> Self {
        Self {
            bloom: Vec4::new(BLOOM_THRESHOLD, BLOOM_KNEE, s.bloom_intensity, 1.0 / BLOOM_MIPS as f32),
            shadow_tint: unit_luminance(Vec3::new(0.94, 0.985, 1.08)).extend(1.0),
            highlight_tint: unit_luminance(Vec3::new(1.07, 1.0, 0.9)).extend(1.0),
            tone: Vec4::new(TONE_KNEE, TONE_WHITE, 1.06, 1.04),
            flags: Vec4::new(
                if s.has(features::BLOOM) { 1.0 } else { 0.0 },
                if s.has(features::GRADE) { 1.0 } else { 0.0 },
                (frame % 64) as f32,
                0.0,
            ),
        }
    }
}

pub struct PostFxPlugin;

impl Plugin for PostFxPlugin {
    fn build(&self, app: &mut App) {
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
        render_app
            .init_resource::<PostUniformBuffer>()
            .configure_sets(
                Core3d,
                (
                    FxSceneSet.in_set(Core3dSystems::PostProcess).before(ExposureMeasureSet),
                    FxGradeSet.in_set(Core3dSystems::PostProcess).after(ExposureMeasureSet).before(VguiDrawSet),
                ),
            )
            .add_systems(RenderStartup, init_pipelines)
            .add_systems(Render, prepare.in_set(RenderSystems::PrepareResources))
            .add_systems(Core3d, draw.in_set(FxGradeSet));
    }
}

#[derive(Resource)]
struct PostPipelines {
    layout: BindGroupLayoutDescriptor,
    sampler: Sampler,
    prefilter: CachedRenderPipelineId,
    down: CachedRenderPipelineId,
    up: CachedRenderPipelineId,
    composite: CachedRenderPipelineId,
}

#[derive(Resource, Default)]
struct PostUniformBuffer {
    buffer: UniformBuffer<FxPostUniform>,
    frame: u32,
}

/// The bloom mip chain of a compositing view, one view per mip.
#[derive(Component)]
struct BloomTexture {
    mips: Vec<TextureView>,
}

fn init_pipelines(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
    fullscreen: Res<FullscreenShader>,
) {
    let layout = BindGroupLayoutDescriptor::new(
        "fx_post",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_2d(TextureSampleType::Float { filterable: true }),
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                uniform_buffer::<FxPostUniform>(false),
            ),
        ),
    );
    let shader: Handle<Shader> = asset_server.load(POST_SHADER);
    let pipeline = |label: &'static str, entry: &'static str, blend: Option<BlendState>| {
        cache.queue_render_pipeline(RenderPipelineDescriptor {
            label: Some(label.into()),
            layout: vec![layout.clone()],
            vertex: fullscreen.to_vertex_state(),
            fragment: Some(FragmentState {
                shader: shader.clone(),
                entry_point: Some(entry.into()),
                targets: vec![Some(ColorTargetState { format: FORMAT, blend, write_mask: ColorWrites::ALL })],
                ..default()
            }),
            ..default()
        })
    };
    let additive = BlendState {
        color: BlendComponent { src_factor: BlendFactor::One, dst_factor: BlendFactor::One, operation: BlendOperation::Add },
        alpha: BlendComponent { src_factor: BlendFactor::Zero, dst_factor: BlendFactor::One, operation: BlendOperation::Add },
    };
    let pipelines = PostPipelines {
        prefilter: pipeline("fx_bloom_prefilter", "bloom_prefilter", None),
        down: pipeline("fx_bloom_down", "bloom_down", None),
        up: pipeline("fx_bloom_up", "bloom_up", Some(additive)),
        composite: pipeline("fx_composite", "composite", None),
        sampler: device.create_sampler(&SamplerDescriptor {
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            address_mode_u: AddressMode::ClampToEdge,
            address_mode_v: AddressMode::ClampToEdge,
            ..default()
        }),
        layout,
    };
    commands.insert_resource(pipelines);
}

fn prepare(
    mut commands: Commands,
    settings: Option<Res<FxSettings>>,
    mut uniform: ResMut<PostUniformBuffer>,
    mut textures: ResMut<TextureCache>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    views: Query<(Entity, &ViewTarget), With<VguiComposite>>,
) {
    let Some(settings) = settings else { return };
    if !settings.has(features::BLOOM | features::GRADE) {
        return;
    }
    uniform.frame = uniform.frame.wrapping_add(1);
    let value = FxPostUniform::from_settings(&settings, uniform.frame);
    uniform.buffer.set(value);
    uniform.buffer.write_buffer(&device, &queue);
    if !settings.has(features::BLOOM) {
        return;
    }
    for (entity, target) in &views {
        let size = target.main_texture().size();
        let (w, h) = ((size.width / 2).max(1), (size.height / 2).max(1));
        let mips = BLOOM_MIPS.min(32 - w.min(h).leading_zeros()).max(1);
        let texture = textures.get(
            &device,
            TextureDescriptor {
                label: Some("fx_bloom"),
                size: Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: mips,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format: FORMAT,
                usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
        );
        let mips = (0..mips)
            .map(|m| {
                texture.texture.create_view(&TextureViewDescriptor {
                    label: Some("fx_bloom_mip"),
                    base_mip_level: m,
                    mip_level_count: Some(1),
                    ..default()
                })
            })
            .collect();
        commands.entity(entity).insert(BloomTexture { mips });
    }
}

/// One fullscreen triangle into `target`, cleared first or drawn over.
fn fullscreen_pass(
    encoder: &mut CommandEncoder,
    label: &'static str,
    target: &TextureView,
    clear: bool,
    pipeline: &RenderPipeline,
    group: &BindGroup,
) {
    let load = if clear { LoadOp::Clear(Default::default()) } else { LoadOp::Load };
    let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: Operations { load, store: StoreOp::Store },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, group, &[]);
    pass.draw(0..3, 0..1);
}

fn draw(
    view: ViewQuery<(&ViewTarget, Option<&BloomTexture>), With<VguiComposite>>,
    settings: Option<Res<FxSettings>>,
    pipelines: Option<Res<PostPipelines>>,
    uniform: Res<PostUniformBuffer>,
    cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
    mut ctx: RenderContext,
) {
    let (Some(settings), Some(pipelines)) = (settings, pipelines) else { return };
    if !settings.has(features::BLOOM | features::GRADE) {
        return;
    }
    let (Some(prefilter), Some(down), Some(up), Some(composite), Some(uniform)) = (
        cache.get_render_pipeline(pipelines.prefilter),
        cache.get_render_pipeline(pipelines.down),
        cache.get_render_pipeline(pipelines.up),
        cache.get_render_pipeline(pipelines.composite),
        uniform.buffer.binding(),
    ) else {
        return;
    };
    let layout = cache.get_bind_group_layout(&pipelines.layout);
    let (target, bloom) = view.into_inner();
    let group = |src: &TextureView, aux: &TextureView| {
        device.create_bind_group("fx_post", &layout, &BindGroupEntries::sequential((src, aux, &pipelines.sampler, uniform.clone())))
    };
    let encoder = ctx.command_encoder();
    let bloom = bloom.filter(|_| settings.has(features::BLOOM));
    if let Some(bloom) = bloom {
        let scene = target.main_texture_view();
        fullscreen_pass(encoder, "fx_bloom_prefilter", &bloom.mips[0], true, prefilter, &group(scene, scene));
        for m in 1..bloom.mips.len() {
            fullscreen_pass(encoder, "fx_bloom_down", &bloom.mips[m], true, down, &group(&bloom.mips[m - 1], &bloom.mips[m - 1]));
        }
        for m in (0..bloom.mips.len().saturating_sub(1)).rev() {
            fullscreen_pass(encoder, "fx_bloom_up", &bloom.mips[m], false, up, &group(&bloom.mips[m + 1], &bloom.mips[m + 1]));
        }
    }
    let post = target.post_process_write();
    let aux = bloom.map_or(post.source, |b| &b.mips[0]);
    fullscreen_pass(encoder, "fx_composite", post.destination, true, composite, &group(post.source, aux));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tone_curve_keeps_the_low_range() {
        // Exactly the game's picture below the knee.
        for i in 0..=85 {
            let x = i as f32 / 100.0;
            assert_eq!(shoulder(x, TONE_KNEE, TONE_WHITE), x);
        }
        // Above it: continuous, rising, never past 1, white at the white point.
        let mut last = shoulder(TONE_KNEE, TONE_KNEE, TONE_WHITE);
        for i in 1..400 {
            let x = TONE_KNEE + i as f32 * 0.02;
            let y = shoulder(x, TONE_KNEE, TONE_WHITE);
            assert!(y >= last && y <= 1.0, "{x} -> {y}");
            last = y;
        }
        assert!((shoulder(TONE_KNEE + 1e-4, TONE_KNEE, TONE_WHITE) - TONE_KNEE).abs() < 1e-3);
        assert!((shoulder(TONE_WHITE, TONE_KNEE, TONE_WHITE) - 1.0).abs() < 1e-4);
    }
}
