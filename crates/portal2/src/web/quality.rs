//! Dynamic quality for the browser build.
//!
//! The browser runs the whole game on one thread and shares the GPU and its
//! memory with every other tab and program, and a page has no idea how
//! strong the machine is. The resolution-sized render targets are what hurt
//! most: the main view (multisampled), four portal views with their own
//! colour and depth buffers, and the overhaul's screen passes. At 2560 x 1440
//! on an 8 GB card that already holds the desktop and a browser they no
//! longer fit, the driver starts paging, and frames take ten times longer.
//!
//! So the canvas renders at most `r_pixel_budget` megapixels (the page
//! draws it at a smaller CSS size and scales it up, which also keeps
//! high-density displays from rendering every device pixel), and a governor
//! watches the frame rate the page
//! measures and steps through quality levels: portal views at a lower
//! resolution first, then a lower render scale, one level of portals seen in
//! portals, no multisampling, and finally without screen-space reflections
//! and volumetric light. It steps down quickly when frames are slow because
//! of the GPU, and back up by probing: a better level is kept unless frames
//! get slower, which also climbs back out of lowered resolutions when the
//! CPU turns out to be the limit (where only fewer portal views help). A
//! level that proved too much is retried later and later.
//! `r_dynamic_quality 0` hands the settings back to the console:
//! `r_render_scale`, `r_portal_scale`, `r_portal_depth` and `r_msaa`.

use bevy::camera::RenderTarget;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use portal2_core::console::Console;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    /// The page's frame timings over the last frames: `[median frame
    /// interval, 90th percentile interval, median CPU time, frames]` in ms.
    #[wasm_bindgen(js_namespace = p2r)]
    fn perf() -> js_sys::Float64Array;
    /// Device pixels the page covers at full resolution.
    #[wasm_bindgen(js_namespace = p2r, js_name = viewportPixels)]
    fn viewport_pixels() -> f64;
    /// Renders the canvas at this fraction of the page's resolution.
    #[wasm_bindgen(js_namespace = p2r, js_name = setRenderScale)]
    fn set_render_scale(scale: f64);
}

/// One quality level: render scale (on top of the pixel budget), portal view
/// scale, portal levels, multisampling, and whether the costliest overhaul
/// passes run.
#[derive(Clone, Copy, Debug)]
struct Level {
    render: f32,
    portal: f32,
    depth: i32,
    msaa: u32,
    heavy_fx: bool,
}

const LEVELS: &[Level] = &[
    Level { render: 1.0, portal: 1.0, depth: 2, msaa: 4, heavy_fx: true },
    Level { render: 1.0, portal: 0.75, depth: 2, msaa: 4, heavy_fx: true },
    Level { render: 0.9, portal: 0.6, depth: 2, msaa: 4, heavy_fx: true },
    Level { render: 0.8, portal: 0.5, depth: 2, msaa: 4, heavy_fx: true },
    Level { render: 0.7, portal: 0.5, depth: 1, msaa: 4, heavy_fx: true },
    Level { render: 0.6, portal: 0.5, depth: 1, msaa: 1, heavy_fx: false },
    Level { render: 0.5, portal: 0.4, depth: 1, msaa: 1, heavy_fx: false },
];

/// The frame interval aimed for (60 frames a second).
const TARGET_MS: f64 = 1000.0 / 60.0;
/// Seconds between looks at the frame timings.
const CHECK_EVERY: f64 = 0.5;
/// Seconds of steady frames before trying a better level.
const STEADY_FOR: f64 = 5.0;
/// Seconds after a map loads before frames count: new materials compile
/// their pipelines as they first come into view, and textures upload.
const AFTER_LOAD: f64 = 8.0;
/// Seconds a change gets before it is judged (render targets reallocate).
const SETTLE: f64 = 1.5;

#[derive(Resource)]
struct Governor {
    level: usize,
    /// Portals seen in portals left out (two views of the world fewer)
    /// because the CPU, not the GPU, is the limit.
    cpu_relief: bool,
    next_check: f64,
    last_change: f64,
    slow_checks: u32,
    /// Since when frames have been either steady at the target or limited
    /// by the CPU (where a better level costs nothing): time to probe.
    calm_since: Option<f64>,
    /// While trying something better: the level and relief it came from
    /// and the frame interval there.
    probe: Option<(usize, bool, f64)>,
    /// No probing before this time; doubles with each failed probe.
    no_probe_until: f64,
    probe_backoff: f64,
    map_generation: u64,
}

impl Default for Governor {
    fn default() -> Self {
        Self {
            level: 0,
            cpu_relief: false,
            next_check: 0.0,
            last_change: 0.0,
            slow_checks: 0,
            calm_since: None,
            probe: None,
            no_probe_until: 0.0,
            probe_backoff: 20.0,
            map_generation: 0,
        }
    }
}

pub struct QualityPlugin;

impl Plugin for QualityPlugin {
    fn build(&self, app: &mut App) {
        {
            let mut console = app.world_mut().resource_mut::<Console>();
            let archive = portal2_core::console::cvars::flags::ARCHIVE;
            console.register_cvar("r_dynamic_quality", "1", archive);
            console.register_cvar("r_render_scale", "1", archive);
            console.register_cvar("r_pixel_budget", "2.1", archive);
            console.register_cvar("r_msaa", "4", 0);
            console.register_command("r_quality", print_quality);
        }
        app.init_resource::<Governor>().add_systems(Last, (govern, apply_render_scale, apply_msaa).chain());
    }
}

fn print_quality(world: &mut World, _args: &portal2_core::console::CommandArgs) {
    let (level, relief) = {
        let g = world.resource::<Governor>();
        (g.level, g.cpu_relief)
    };
    let window =
        world.query_filtered::<&Window, With<PrimaryWindow>>().iter(world).next().map(|w| (w.physical_width(), w.physical_height()));
    let line = format!(
        "dynamic quality level {level} of {}: {:?}{}, canvas {:?}",
        LEVELS.len() - 1,
        LEVELS[level],
        if relief { " (one portal level for the CPU)" } else { "" },
        window
    );
    world.resource_mut::<Console>().print(line);
}

/// Sets the console variables a level stands for.
fn apply_level(console: &mut Console, level: Level, cpu_relief: bool) {
    let _ = console.set("r_portal_scale", &level.portal.to_string());
    let _ = console.set("r_portal_depth", &if cpu_relief { 1 } else { level.depth }.to_string());
    let _ = console.set("r_msaa", &level.msaa.to_string());
    let heavy = if level.heavy_fx { "-1" } else { "0" };
    let _ = console.set("r_fx_ssr", heavy);
    let _ = console.set("r_fx_volumetrics", heavy);
}

fn govern(
    time: Res<Time<Real>>,
    mut gov: ResMut<Governor>,
    mut console: ResMut<Console>,
    pending: Option<Res<portal2_world::PendingLoad>>,
    loaded: Res<portal2_core::LoadedMap>,
) {
    let now = time.elapsed_secs_f64();
    if !console.bool("r_dynamic_quality") {
        return;
    }
    // Loading screens and the frames right after a load say nothing about
    // the game's speed.
    if pending.as_ref().is_some_and(|p| p.request.is_some()) || loaded.generation != gov.map_generation {
        gov.map_generation = loaded.generation;
        gov.last_change = now + AFTER_LOAD - SETTLE;
        gov.slow_checks = 0;
        gov.calm_since = None;
        return;
    }
    if now < gov.next_check || now - gov.last_change < SETTLE {
        return;
    }
    gov.next_check = now + CHECK_EVERY;
    let stats = perf().to_vec();
    let &[median, p90, cpu, frames] = stats.as_slice() else { return };
    if frames < 20.0 {
        return;
    }
    let level = gov.level;
    let slow = median > TARGET_MS * 1.12;
    // The frame waits on the GPU (or its memory) rather than on our code.
    let gpu_bound = cpu < median * 0.8;

    // Something better on trial: kept unless frames got slower than before
    // (by more than the noise) and below the target.
    if let Some((from, relief, before)) = gov.probe
        && now - gov.last_change >= SETTLE + 1.0
    {
        if median > (before * 1.08).max(TARGET_MS * 1.12) {
            gov.probe = None;
            gov.no_probe_until = now + gov.probe_backoff;
            gov.probe_backoff = (gov.probe_backoff * 2.0).min(600.0);
            change(&mut gov, &mut console, from, relief, now);
            return;
        }
        if now - gov.last_change >= STEADY_FOR {
            gov.probe = None;
            gov.probe_backoff = 20.0;
        }
    }

    if slow && gpu_bound {
        gov.calm_since = None;
        gov.slow_checks += 1;
        if gov.probe.is_some() {
            return;
        }
        // Much too slow for long (memory paging): two levels at once.
        let much = median > TARGET_MS * 2.0;
        let sustained = if much { 4 } else { 2 };
        if gov.slow_checks >= sustained && level + 1 < LEVELS.len() {
            let step = if much { 2 } else { 1 };
            let relief = gov.cpu_relief;
            change(&mut gov, &mut console, (level + step).min(LEVELS.len() - 1), relief, now);
        }
        return;
    }
    gov.slow_checks = 0;
    if slow && !gov.cpu_relief && LEVELS[level].depth > 1 && gov.probe.is_none() {
        // Limited by the CPU: resolution does not matter, but portals seen
        // in portals cost two more views of the world.
        change(&mut gov, &mut console, level, true, now);
        return;
    }
    // Steady at the target, or limited by the CPU so that the GPU has room:
    // after a while try something better.
    let steady = median <= TARGET_MS * 1.04 && p90 <= TARGET_MS * 1.2;
    if !(steady || slow) {
        gov.calm_since = None;
        return;
    }
    let since = *gov.calm_since.get_or_insert(now);
    if now - since < STEADY_FOR || now < gov.no_probe_until || gov.probe.is_some() {
        return;
    }
    let relief = gov.cpu_relief;
    if level > 0 {
        gov.probe = Some((level, relief, median));
        change(&mut gov, &mut console, level - 1, relief, now);
    } else if relief && steady {
        // Room to spare: portals in portals again.
        gov.probe = Some((level, relief, median));
        change(&mut gov, &mut console, level, false, now);
    }
}

fn change(gov: &mut Governor, console: &mut Console, level: usize, cpu_relief: bool, now: f64) {
    if level == gov.level && cpu_relief == gov.cpu_relief {
        return;
    }
    info!(
        "dynamic quality: level {} -> {level} ({:?}){}",
        gov.level,
        LEVELS[level],
        if cpu_relief { ", one portal level for the CPU" } else { "" }
    );
    gov.level = level;
    gov.cpu_relief = cpu_relief;
    gov.last_change = now;
    gov.slow_checks = 0;
    gov.calm_since = None;
    apply_level(console, LEVELS[level], cpu_relief);
}

/// The canvas renders at `r_render_scale` times the level's scale, and at
/// most `r_pixel_budget` megapixels, in steps of 5 % (each change
/// reallocates every render target).
fn apply_render_scale(console: Res<Console>, gov: Res<Governor>, mut last: Local<f64>) {
    let pixels = viewport_pixels();
    if pixels < 1.0 {
        return;
    }
    let manual = console.f32("r_render_scale") as f64;
    let manual = if manual > 0.0 { manual.clamp(0.25, 1.0) } else { 1.0 };
    let level = if console.bool("r_dynamic_quality") { LEVELS[gov.level].render as f64 } else { 1.0 };
    let budget = console.f32("r_pixel_budget") as f64 * 1e6;
    let budget_scale = if budget > 0.0 { (budget / pixels).sqrt().min(1.0) } else { 1.0 };
    let scale = ((manual * level).min(budget_scale) * 20.0).floor().max(5.0) / 20.0;
    if scale != *last {
        info!("render scale {scale} ({:.2} of {:.2} megapixels)", pixels * scale * scale / 1e6, pixels / 1e6);
        *last = scale;
        set_render_scale(scale);
    }
}

/// `r_msaa` (4 or 1) on the cameras drawing into the window; the overhaul's
/// screen passes only run with 4.
fn apply_msaa(console: Res<Console>, mut cameras: Query<(&mut Msaa, &RenderTarget), With<Camera3d>>) {
    let want = if console.i32("r_msaa") >= 4 { Msaa::Sample4 } else { Msaa::Off };
    for (mut msaa, target) in &mut cameras {
        if matches!(target, RenderTarget::Window(_)) && *msaa != want {
            *msaa = want;
        }
    }
}
