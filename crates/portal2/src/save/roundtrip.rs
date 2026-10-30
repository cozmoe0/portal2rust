//! `--save-test <maps>`: for each map, starts it, plays `--smoke-frames`
//! frames, saves, loads that save and saves again as soon as it is restored
//! (before anything runs), then reports every difference between the two
//! saves to `re/out/savetest.txt`. A complete save/restore round trip has
//! none.

use std::fmt::Write as _;

use bevy::prelude::*;
use ciborium::Value;

use super::{PendingRestore, SaveFile, capture, load_named, write_file};
use portal2_core::args::Args;
use portal2_world::{CurrentMap, LoadMap, MapLoadType};

const FILE: &str = "p2r_roundtrip";

pub struct RoundTripPlugin;

impl Plugin for RoundTripPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup).add_systems(Update, run).add_systems(PreUpdate, check_restored.after(super::finish_restore));
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Step {
    Start,
    Loading,
    Playing(u32),
    Restoring,
}

#[derive(Resource)]
struct RoundTrip {
    maps: Vec<String>,
    index: usize,
    step: Step,
    first: Option<SaveFile>,
    report: String,
    failures: usize,
}

fn setup(mut commands: Commands, args: Res<Args>, fs: Res<portal2_core::vfs::GameFs>) {
    let Some(spec) = &args.save_test else { return };
    let maps = crate::smoke::smoke_maps(spec, &fs);
    info!("save round trip of {} maps", maps.len());
    commands.insert_resource(RoundTrip { maps, index: 0, step: Step::Start, first: None, report: String::new(), failures: 0 });
}

fn run(world: &mut World) {
    let Some(mut rt) = world.remove_resource::<RoundTrip>() else { return };
    let frames = world.resource::<Args>().smoke_frames;
    if rt.index >= rt.maps.len() {
        let _ = std::fs::create_dir_all("re/out");
        let summary = format!("{} maps, {} with differences\n", rt.maps.len(), rt.failures);
        rt.report.push_str(&summary);
        let _ = std::fs::write("re/out/savetest.txt", &rt.report);
        info!("save round trip: {summary}");
        world.write_message(AppExit::Success);
        return;
    }
    let name = rt.maps[rt.index].clone();
    match rt.step {
        Step::Start => {
            world.write_message(LoadMap::new(name, MapLoadType::NewGame));
            rt.step = Step::Loading;
        }
        Step::Loading => {
            if world.get_resource::<CurrentMap>().is_some_and(|m| m.name == name) {
                rt.step = Step::Playing(frames);
            }
        }
        Step::Playing(0) => match capture(world, "round trip") {
            Ok(file) => match write_file(FILE, &file) {
                Ok(_) => {
                    rt.first = Some(file);
                    load_named(world, FILE);
                    rt.step = Step::Restoring;
                }
                Err(e) => fail(&mut rt, &name, &format!("write failed: {e}")),
            },
            Err(e) => fail(&mut rt, &name, &format!("save failed: {e}")),
        },
        Step::Playing(n) => rt.step = Step::Playing(n - 1),
        Step::Restoring => {}
    }
    world.insert_resource(rt);
}

fn fail(rt: &mut RoundTrip, map: &str, why: &str) {
    let _ = writeln!(rt.report, "{map}: {why}");
    rt.failures += 1;
    rt.index += 1;
    rt.step = Step::Start;
}

/// Saves again the moment the restore finished and compares.
fn check_restored(world: &mut World) {
    let waiting = world.get_resource::<RoundTrip>().is_some_and(|r| r.step == Step::Restoring);
    if !waiting || world.contains_resource::<PendingRestore>() {
        return;
    }
    let mut rt = world.remove_resource::<RoundTrip>().unwrap();
    let name = rt.maps[rt.index].clone();
    let first = rt.first.take().unwrap();
    match capture(world, "round trip") {
        Ok(second) => {
            let diffs = compare(&first, &second);
            info!("round trip {name}: {} entities, {} differences", first.entities.len(), diffs.len());
            let _ = writeln!(rt.report, "{name}: {} entities, {} differences", first.entities.len(), diffs.len());
            for d in diffs.iter().take(400) {
                let _ = writeln!(rt.report, "  {d}");
            }
            if !diffs.is_empty() {
                rt.failures += 1;
            }
        }
        Err(e) => {
            let _ = writeln!(rt.report, "{name}: second save failed: {e}");
            rt.failures += 1;
        }
    }
    rt.index += 1;
    rt.step = Step::Start;
    world.insert_resource(rt);
}

/// CBOR value with map entries sorted, so hash map order does not count.
fn normalized(bytes: &[u8]) -> Value {
    fn norm(v: Value) -> Value {
        match v {
            Value::Map(entries) => {
                let mut e: Vec<(Value, Value)> = entries.into_iter().map(|(k, v)| (norm(k), norm(v))).collect();
                e.sort_by_key(|(k, _)| format!("{k:?}"));
                Value::Map(e)
            }
            Value::Array(a) => Value::Array(a.into_iter().map(norm).collect()),
            other => other,
        }
    }
    norm(ciborium::from_reader(bytes).unwrap_or(Value::Null))
}

fn compare(a: &SaveFile, b: &SaveFile) -> Vec<String> {
    let mut out = Vec::new();
    if a.tick != b.tick {
        out.push(format!("clock tick {} vs {}", a.tick, b.tick));
    }
    fn by_id(f: &SaveFile) -> std::collections::BTreeMap<u32, &super::SavedEntity> {
        f.entities.iter().map(|e| (e.id, e)).collect()
    }
    let (ea, eb) = (by_id(a), by_id(b));
    for (id, x) in &ea {
        let Some(y) = eb.get(id) else {
            out.push(format!("{} {id} ({}) missing after load", x.classname, x.targetname.as_deref().unwrap_or("")));
            continue;
        };
        let who = format!("{} {id} ({})", x.classname, x.targetname.as_deref().unwrap_or(""));
        if x.keyvalues != y.keyvalues {
            out.push(format!("{who}: keyvalues differ"));
        }
        if x.outputs.len() != y.outputs.len() {
            out.push(format!("{who}: outputs {} vs {}", x.outputs.len(), y.outputs.len()));
        }
        if x.transform.translation.distance(y.transform.translation) > 0.01
            || x.transform.rotation.dot(y.transform.rotation).abs() < 0.99999
        {
            out.push(format!(
                "{who}: placement {:?} {:?} vs {:?} {:?}",
                x.transform.translation, x.transform.rotation, y.transform.translation, y.transform.rotation
            ));
        }
        if x.parent != y.parent {
            out.push(format!("{who}: parent {:?} vs {:?}", x.parent, y.parent));
        }
        if x.hidden != y.hidden {
            out.push(format!("{who}: hidden {} vs {}", x.hidden, y.hidden));
        }
        for (name, bytes) in &x.components {
            match y.components.iter().find(|(n, _)| n == name) {
                None => out.push(format!("{who}: {name} missing after load")),
                Some((_, other)) if normalized(bytes) != normalized(other) => {
                    out.push(format!("{who}: {name} {:?} vs {:?}", short(bytes), short(other)))
                }
                _ => {}
            }
        }
        for (name, _) in &y.components {
            if !x.components.iter().any(|(n, _)| n == name) {
                out.push(format!("{who}: {name} appeared after load"));
            }
        }
    }
    for (id, y) in &eb {
        if !ea.contains_key(id) {
            out.push(format!("{} {id} ({}) appeared after load", y.classname, y.targetname.as_deref().unwrap_or("")));
        }
    }
    for (name, bytes) in &a.resources {
        match b.resources.iter().find(|(n, _)| n == name) {
            None => out.push(format!("resource {name} missing after load")),
            Some((_, other)) if name == "event_queue" && queue_events(bytes) != queue_events(other) => {
                out.push(format!("resource {name} differs: {:?} vs {:?}", queue_events(bytes), queue_events(other)))
            }
            Some((_, other)) if name != "event_queue" && normalized(bytes) != normalized(other) => {
                out.push(format!("resource {name} differs"))
            }
            _ => {}
        }
    }
    match (&a.scripts, &b.scripts) {
        (Some(x), Some(y)) if x != y => out.push(format!("script VM differs ({} vs {} bytes)", x.len(), y.len())),
        (Some(_), None) => out.push("script VM missing after load".into()),
        _ => {}
    }
    match (&a.player, &b.player) {
        (Some(x), Some(y)) => {
            if x.state.origin.distance(y.state.origin) > 0.01 || x.angles.distance(y.angles) > 0.01 {
                out.push(format!("player {:?} {:?} vs {:?} {:?}", x.state.origin, x.angles, y.state.origin, y.angles));
            }
        }
        (Some(_), None) => out.push("player missing after load".into()),
        _ => {}
    }
    out
}

fn short(bytes: &[u8]) -> String {
    let s = format!("{:?}", normalized(bytes));
    if s.len() > 160 { format!("{}...", &s[..160]) } else { s }
}

/// Pending events as (time, input, parameter), without the `logic_auto`
/// re-arm that loading adds on purpose.
fn queue_events(bytes: &[u8]) -> Vec<String> {
    let Value::Map(top) = normalized(bytes) else { return Vec::new() };
    let Some((_, Value::Array(events))) = top.into_iter().find(|(k, _)| k.as_text() == Some("events")) else { return Vec::new() };
    let field = |ev: &Value, name: &str| match ev {
        Value::Map(m) => m.iter().find(|(k, _)| k.as_text() == Some(name)).map(|(_, v)| format!("{v:?}")).unwrap_or_default(),
        _ => String::new(),
    };
    events
        .iter()
        .filter(|ev| !field(ev, "input").contains("__autothink"))
        .map(|ev| format!("{} {} {} {}", field(ev, "time"), field(ev, "target"), field(ev, "input"), field(ev, "param")))
        .collect()
}
