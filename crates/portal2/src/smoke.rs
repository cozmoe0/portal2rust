//! `--smoke <maps>`: loads each map, runs it for a while and reports script
//! errors, inputs nothing handled and entity classes without behaviour.
//! `<maps>` is `sp` (every single-player map), or a comma separated list.
//! The report goes to `re/out/smoke.txt`.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use bevy::prelude::*;

use crate::entities::SpawnHandlers;
use crate::entities::io::InputHandlers;
use crate::entities::io::IoStats;
use crate::world::{CurrentMap, LoadMap, MapLoadType};
use portal2_core::args::Args;
use portal2_core::entity::MapEntity;
use portal2_core::vfs::GameFs;

pub struct SmokePlugin;

impl Plugin for SmokePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup).add_systems(Update, run);
    }
}

#[derive(Default)]
struct MapReport {
    script_errors: Vec<String>,
    unhandled: BTreeMap<(String, String), u32>,
    unknown_classes: BTreeMap<String, u32>,
}

#[derive(Resource)]
struct Smoke {
    maps: Vec<String>,
    index: usize,
    frames_left: u32,
    loading: bool,
    log_start: usize,
    reports: Vec<(String, MapReport)>,
}

/// The maps a `--smoke` spec names.
pub fn smoke_maps(spec: &str, fs: &GameFs) -> Vec<String> {
    if spec == "sp" {
        return fs
            .list("maps", Some("bsp"), false)
            .into_iter()
            .filter_map(|p| p.strip_prefix("maps/").and_then(|n| n.strip_suffix(".bsp")).map(str::to_owned))
            .filter(|n| n.starts_with("sp_"))
            .collect();
    }
    spec.split(',').map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty()).collect()
}

fn setup(mut commands: Commands, args: Res<Args>, fs: Res<GameFs>) {
    let Some(spec) = &args.smoke else { return };
    let maps = smoke_maps(spec, &fs);
    info!("smoke test of {} maps", maps.len());
    commands.insert_resource(Smoke { maps, index: 0, frames_left: 0, loading: false, log_start: 0, reports: Vec::new() });
}

fn run(world: &mut World) {
    let Some(mut smoke) = world.remove_resource::<Smoke>() else { return };
    let frames = world.resource::<Args>().smoke_frames;
    loop {
        if smoke.index >= smoke.maps.len() {
            write_report(&smoke);
            world.write_message(AppExit::Success);
            return;
        }
        let name = smoke.maps[smoke.index].clone();
        let loaded = world.get_resource::<CurrentMap>().is_some_and(|m| m.name == name);
        if !smoke.loading && !loaded {
            world.write_message(LoadMap::new(name, MapLoadType::NewGame));
            smoke.loading = true;
            break;
        }
        if smoke.loading {
            if !loaded {
                break;
            }
            smoke.loading = false;
            smoke.frames_left = frames;
            smoke.log_start = world.resource::<portal2_core::console::Console>().log.len();
            world.resource_mut::<IoStats>().unhandled.clear();
            break;
        }
        if smoke.frames_left > 0 {
            smoke.frames_left -= 1;
            break;
        }
        let report = collect(world, smoke.log_start);
        info!("smoke {name}: {} script errors, {} unhandled inputs", report.script_errors.len(), report.unhandled.len());
        smoke.reports.push((name, report));
        smoke.index += 1;
    }
    world.insert_resource(smoke);
}

fn collect(world: &mut World, log_start: usize) -> MapReport {
    let mut report = MapReport::default();
    let log = &world.resource::<portal2_core::console::Console>().log;
    let start = log_start.min(log.len());
    for (i, line) in log[start..].iter().enumerate() {
        if line.starts_with("AN ERROR HAS OCCURED") || line.starts_with("Script compile error") {
            let at = log.get(start + i + 2).cloned().unwrap_or_default();
            report.script_errors.push(format!("{line} {at}"));
        }
    }
    report.unhandled = world.resource::<IoStats>().unhandled.iter().map(|(k, v)| (k.clone(), *v)).collect();
    let mut q = world.query::<&MapEntity>();
    let classes: Vec<String> = q.iter(world).map(|m| m.classname.clone()).collect();
    let spawn = world.resource::<SpawnHandlers>();
    let inputs = world.resource::<InputHandlers>();
    for class in classes {
        if !spawn.handles(&class) && !inputs.handles(&class) {
            *report.unknown_classes.entry(class).or_default() += 1;
        }
    }
    report
}

fn write_report(smoke: &Smoke) {
    let mut out = String::new();
    let mut total_errors = 0;
    let mut all_unhandled: BTreeMap<(String, String), u32> = BTreeMap::new();
    let mut all_unknown: BTreeMap<String, u32> = BTreeMap::new();
    for (name, r) in &smoke.reports {
        total_errors += r.script_errors.len();
        let _ = writeln!(out, "== {name}: {} script errors", r.script_errors.len());
        for e in &r.script_errors {
            let _ = writeln!(out, "   {e}");
        }
        for (k, v) in &r.unhandled {
            *all_unhandled.entry(k.clone()).or_default() += v;
        }
        for (k, v) in &r.unknown_classes {
            *all_unknown.entry(k.clone()).or_default() += v;
        }
    }
    let _ = writeln!(out, "\n== unhandled inputs (class, input): count");
    for ((class, input), n) in &all_unhandled {
        let _ = writeln!(out, "   {class}.{input}: {n}");
    }
    let _ = writeln!(out, "\n== classes without behaviour: entities");
    for (class, n) in &all_unknown {
        let _ = writeln!(out, "   {class}: {n}");
    }
    let _ = writeln!(out, "\n{} maps, {total_errors} script errors", smoke.reports.len());
    let _ = std::fs::create_dir_all("re/out");
    let _ = std::fs::write("re/out/smoke.txt", &out);
    info!("smoke report written to re/out/smoke.txt ({total_errors} script errors)");
}
