//! The game's dynamic tone mapping ("auto exposure"), with the settings
//! `env_tonemap_controller` changes.
//!
//! Each frame the finished 3D view is measured into a luminance histogram
//! (sixteen buckets even in gamma space, edges (i / 16)^2.5, plus one for
//! pixels at 1 or above, which are left out of the sums). The engine's newer
//! algorithm then finds the luminance above which `percent bright pixels`
//! (`mat_tonemap_percent_bright_pixels`, 2; maps set 5) of the pixels lie,
//! interpolating inside its bucket, and asks for the scale that puts it at
//! `percent target` (`mat_tonemap_percent_target`, 60%); it scales at
//! least enough to bring the median up to `mat_tonemap_min_avglum` (3%).
//! Those factors multiply the current scale. The last ten targets, clamped
//! to the auto-exposure range (`mat_autoexposure_min` / `_max`, 0.5 / 2;
//! test chambers set 0.5 / 3), are averaged with weights |i - 5| (oldest
//! first), and the scale moves toward the result at the tone map rate
//! (`mat_hdr_manual_tonemap_rate` 1, doubled for this algorithm; maps set
//! 0.25) times the frame time, at most 1/64 per frame, faster when
//! darkening (`mat_accelerate_adjust_exposure_down`, 3). The scale is the
//! view's exposure.
//!
//! The histogram is measured on the GPU with a compute pass over the view
//! (every other pixel) and read back a frame or two later.

use bevy::core_pipeline::Core3dSystems;
use bevy::core_pipeline::schedule::Core3d;
use bevy::prelude::*;
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::gpu_readback::{Readback, ReadbackComplete};
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::binding_types::{storage_buffer, texture_2d};
use bevy::render::render_resource::*;
use bevy::render::renderer::{RenderContext, RenderDevice, ViewQuery};
use bevy::render::storage::{GpuShaderBuffer, ShaderBuffer};
use bevy::render::view::ViewTarget;
use bevy::render::{RenderApp, RenderStartup};
use serde::{Deserialize, Serialize};

use crate::entities::SpawnHandlers;
use crate::entities::io::{InputCall, InputHandlers};
use portal2_core::view::MainCamera;
use portal2_ui::vgui::render::VguiComposite;

pub const HISTOGRAM_SHADER: &str = "embedded://portal2_render/shaders/tonemap_histogram.wesl";

const BUCKETS: usize = 17;
/// The scale's per-frame step limit with the newer algorithm.
const MAX_STEP: f32 = 1.0 / 64.0;
const HISTORY: usize = 10;

/// Settings `env_tonemap_controller` can change (cvar defaults otherwise).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TonemapSettings {
    pub min: f32,
    pub max: f32,
    pub rate: f32,
    pub percent_target: f32,
    pub percent_bright_pixels: f32,
    pub min_avg_lum: f32,
}

impl Default for TonemapSettings {
    fn default() -> Self {
        Self { min: 0.5, max: 2.0, rate: 1.0, percent_target: 60.0, percent_bright_pixels: 2.0, min_avg_lum: 3.0 }
    }
}

#[derive(Resource, Clone, Debug, Serialize, Deserialize)]
pub struct Tonemap {
    pub settings: TonemapSettings,
    /// The current tone map scale (the view's exposure).
    pub scale: f32,
    /// Recent clamped targets, oldest first.
    history: Vec<f32>,
    /// The filtered target the scale moves toward.
    target: f32,
    /// A fresh histogram from the GPU.
    #[serde(skip)]
    histogram: Option<[u32; BUCKETS]>,
    /// `mat_force_tonemap_scale` / `--exposure`: a fixed scale (0 = dynamic).
    #[serde(skip)]
    pub forced: f32,
}

impl Default for Tonemap {
    fn default() -> Self {
        // Starts at the usual test chamber maximum, where dark rooms settle.
        Self { settings: TonemapSettings::default(), scale: 3.0, history: Vec::new(), target: 3.0, histogram: None, forced: 0.0 }
    }
}

impl Tonemap {
    /// Back to the start of a map: the cvar defaults and a fresh history.
    pub fn reset(&mut self) {
        self.settings = TonemapSettings::default();
        self.history.clear();
    }
}

impl portal2_core::save::Restore for Tonemap {}

pub fn register_save(r: &mut portal2_core::save::SaveRegistry) {
    r.resource::<Tonemap>("tonemap");
}

#[derive(Resource, Clone, ExtractResource)]
#[extract_app(RenderApp)]
struct HistogramBuffer(Handle<ShaderBuffer>);

/// The histogram measurement on the interface camera; the visual overhaul
/// orders its scene passes before it and its tone curve after it.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ExposureMeasureSet;

pub struct TonemapPlugin;

impl Plugin for TonemapPlugin {
    fn build(&self, app: &mut App) {
        let buffer = app
            .world_mut()
            .resource_mut::<Assets<ShaderBuffer>>()
            .add(ShaderBuffer::new(vec![0u32; BUCKETS], bevy::asset::RenderAssetUsages::default()));
        app.insert_resource(HistogramBuffer(buffer))
            .init_resource::<Tonemap>()
            .add_plugins(ExtractResourcePlugin::<HistogramBuffer>::default())
            .add_systems(Startup, start_readback)
            .add_systems(PostUpdate, update_scale.before(portal2_world::camera::sync_camera_transform))
            .add_systems(PreUpdate, reset_on_load.after(portal2_core::sets::LoadMapSet));
        let world = app.world_mut();
        world.resource_mut::<SpawnHandlers>().register("env_tonemap_controller", |_, _| {});
        world.resource_mut::<InputHandlers>().register("env_tonemap_controller", tonemap_input);
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
        render_app.add_systems(RenderStartup, init_pipeline).add_systems(
            Core3d,
            measure.in_set(Core3dSystems::PostProcess).in_set(ExposureMeasureSet).before(portal2_ui::vgui::render::VguiDrawSet),
        );
    }
}

fn start_readback(mut commands: Commands, buffer: Res<HistogramBuffer>) {
    commands.spawn((Name::new("tonemap histogram readback"), Readback::buffer(buffer.0.clone()))).observe(
        |event: On<ReadbackComplete>, mut tonemap: ResMut<Tonemap>| {
            let data: Vec<u32> = event.to_shader_type();
            if data.len() >= BUCKETS && data.iter().any(|&c| c > 0) {
                let mut h = [0u32; BUCKETS];
                h.copy_from_slice(&data[..BUCKETS]);
                tonemap.histogram = Some(h);
            }
        },
    );
}

fn reset_on_load(mut loaded: MessageReader<portal2_world::MapLoaded>, mut tonemap: ResMut<Tonemap>) {
    if loaded.read().last().is_some() {
        tonemap.reset();
    }
}

fn tonemap_input(world: &mut World, call: &InputCall) -> bool {
    let f = source_formats::kv::parse_f32_lenient(&call.param);
    let mut t = world.resource_mut::<Tonemap>();
    let s = &mut t.settings;
    match call.input.as_str() {
        "setautoexposuremin" => s.min = f,
        "setautoexposuremax" => s.max = f,
        "usedefaultautoexposure" => {
            let d = TonemapSettings::default();
            (s.min, s.max) = (d.min, d.max);
        }
        "settonemaprate" => s.rate = f,
        "settonemappercenttarget" => s.percent_target = f,
        "settonemappercentbrightpixels" => s.percent_bright_pixels = f,
        "settonemapminavglum" => s.min_avg_lum = f,
        // Bloom is not drawn.
        "setbloomscale" | "setbloomscalerange" | "setbloomexponent" | "setbloomsaturation" | "usedefaultbloomscale" => {}
        _ => return false,
    }
    true
}

/// Bucket edges in linear luminance.
fn bucket_range(i: usize) -> (f32, f32) {
    if i >= 16 {
        return (1.0, 100000.0);
    }
    ((i as f32 / 16.0).powf(2.5), ((i + 1) as f32 / 16.0).powf(2.5))
}

/// The luminance above which `percent` of the counted pixels lie, or -1;
/// snaps to `snap` / 100 when that falls in the same bucket.
pub fn percent_bright_location(h: &[u32; BUCKETS], percent: f32, snap: f32) -> f32 {
    let total: u32 = h[..16].iter().sum();
    if total == 0 {
        return -1.0;
    }
    let mut above = 0.0;
    let mut widths = 0.0;
    for i in (0..16).rev() {
        let share = h[i] as f32 / total as f32;
        let (lo, hi) = bucket_range(i);
        let width = hi - lo;
        let needed = percent * 0.01 - above;
        if share >= needed {
            if snap >= 0.0 && snap * 0.01 >= lo && hi >= snap * 0.01 {
                return snap * 0.01;
            }
            let at = 1.0 - (needed / share.max(1e-9) * width + widths);
            return at.clamp(lo, hi);
        }
        above += share;
        widths += width;
    }
    -1.0
}

/// The newer algorithm's target scale for a histogram measured at `current`.
pub fn target_scale(h: &[u32; BUCKETS], s: &TonemapSettings, current: f32) -> f32 {
    let mut at = percent_bright_location(h, s.percent_bright_pixels, s.percent_target);
    if at < 0.0 {
        at = s.percent_target * 0.01;
    }
    let mut factor = s.percent_target * 0.01 / at.max(0.0001);
    let median = percent_bright_location(h, 50.0, -1.0);
    if median > 0.0 {
        factor = factor.max(s.min_avg_lum * 0.01 / median);
    }
    (current * factor).max(0.001)
}

fn update_scale(
    time: Res<Time>,
    console: Res<portal2_core::console::Console>,
    args: Res<portal2_core::args::Args>,
    mut tonemap: ResMut<Tonemap>,
    mut main: Query<&mut bevy::camera::Exposure, With<MainCamera>>,
) {
    let t = &mut *tonemap;
    let forced = console.f32("mat_force_tonemap_scale");
    t.forced = if forced > 0.0 {
        forced
    } else if args.fixed_exposure {
        args.exposure
    } else {
        0.0
    };
    if t.forced > 0.0 {
        t.scale = t.forced;
        t.target = t.forced;
        t.history.clear();
    } else if let Some(h) = t.histogram.take() {
        let s = t.settings;
        let target = target_scale(&h, &s, t.scale).clamp(s.min.min(s.max), s.max.max(s.min)).max(0.001);
        if t.history.len() == HISTORY {
            t.history.remove(0);
        }
        t.history.push(target);
        t.target = if t.history.len() == HISTORY {
            let weights = (0..HISTORY).map(|i| (i as f32 - 5.0).abs());
            let sum: f32 = weights.clone().sum();
            let avg = t.history.iter().zip(weights).map(|(v, w)| v * w).sum::<f32>() / sum;
            avg.clamp(s.min.min(s.max), s.max.max(s.min))
        } else {
            target
        };
        let mut rate = s.rate * 2.0;
        if rate == 0.0 {
            t.scale = t.target;
        } else {
            if t.scale > t.target {
                let fast = console.f32("mat_accelerate_adjust_exposure_down") * rate;
                rate = ((t.scale - t.target) * (fast - rate) * (2.0 / 3.0) + rate).min(fast);
            }
            let step = (rate * time.delta_secs()).clamp(0.0, MAX_STEP);
            t.scale += (t.target - t.scale) * step;
        }
    }
    let ev100 = -(1.2 * t.scale.max(0.001)).log2();
    for mut e in &mut main {
        if (e.ev100 - ev100).abs() > 1e-5 {
            e.ev100 = ev100;
        }
    }
}

#[derive(Resource)]
struct HistogramPipeline {
    layout: BindGroupLayoutDescriptor,
    pipeline: CachedComputePipelineId,
}

fn init_pipeline(mut commands: Commands, asset_server: Res<AssetServer>, cache: Res<PipelineCache>) {
    let layout = BindGroupLayoutDescriptor::new(
        "tonemap_histogram",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (texture_2d(TextureSampleType::Float { filterable: false }), storage_buffer::<Vec<u32>>(false)),
        ),
    );
    let shader: Handle<Shader> = asset_server.load(HISTOGRAM_SHADER);
    let pipeline = cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("tonemap_histogram".into()),
        layout: vec![layout.clone()],
        shader,
        entry_point: Some("histogram".into()),
        ..default()
    });
    commands.insert_resource(HistogramPipeline { layout, pipeline });
}

/// Measures the composited 3D view (before the interface is drawn over it).
fn measure(
    view: ViewQuery<&ViewTarget, With<VguiComposite>>,
    pipeline: Option<Res<HistogramPipeline>>,
    buffer: Option<Res<HistogramBuffer>>,
    buffers: Res<RenderAssets<GpuShaderBuffer>>,
    cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
    mut ctx: RenderContext,
) {
    let (Some(pipeline), Some(buffer)) = (pipeline, buffer) else { return };
    let (Some(compute), Some(gpu)) = (cache.get_compute_pipeline(pipeline.pipeline), buffers.get(&buffer.0)) else {
        return;
    };
    let target = view.into_inner();
    let texture = target.main_texture();
    let size = texture.size();
    let bind_group = device.create_bind_group(
        "tonemap_histogram",
        &cache.get_bind_group_layout(&pipeline.layout),
        &BindGroupEntries::sequential((target.main_texture_view(), gpu.buffer.as_entire_binding())),
    );
    let encoder = ctx.command_encoder();
    encoder.clear_buffer(&gpu.buffer, 0, None);
    let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor { label: Some("tonemap_histogram"), timestamp_writes: None });
    pass.set_pipeline(compute);
    pass.set_bind_group(0, &bind_group, &[]);
    pass.dispatch_workgroups(size.width.div_ceil(32), size.height.div_ceil(32), 1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bright_pixels_location() {
        // Everything in one bucket: the 2% point lies near its top.
        let mut h = [0u32; BUCKETS];
        h[8] = 1000;
        let (lo, hi) = bucket_range(8);
        let at = percent_bright_location(&h, 2.0, -1.0);
        assert!(at > lo && at <= hi, "{at} not in {lo}..{hi}");
        // A dark frame asks for a brighter scale, a bright one for a darker one.
        let s = TonemapSettings::default();
        let mut dark = [0u32; BUCKETS];
        dark[2] = 1000;
        assert!(target_scale(&dark, &s, 1.0) > 1.0);
        let mut bright = [0u32; BUCKETS];
        bright[15] = 1000;
        assert!(target_scale(&bright, &s, 1.0) < 1.0);
    }
}
