//! `--shots <file>`: visit a list of views and save a screenshot of each,
//! for side-by-side comparison with captures from the original game
//! (`re/tools/capture.py` reads the same list, `re/tools/imgdiff.py`
//! compares the results).
//!
//! Each line is `name map x y z pitch yaw roll [commands]`, the position
//! being the player's feet as with `setpos`; `#` starts a comment. The player
//! is put in noclip at each view, with the viewmodel and HUD hidden; anything
//! after the angles is run as console commands at that view (for example
//! `r_drawviewmodel 1` to show the first-person weapon). Each view waits
//! `--shot-settle` frames, or `--shot-settle-time` seconds, before its
//! screenshot; `--shot-map-settle` waits that many seconds after each map
//! load first (as `capture.py --map-settle` does in the game).
//!
//! `--bench <file>` visits the same kind of list but measures frame times
//! for `--bench-seconds` at each view instead of taking a screenshot, and
//! appends one line per view to `--bench-out` (`re/out/bench.txt`).

use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::Instant;

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};

use crate::player::{Player, PlayerInput};
use crate::world::{CurrentMap, LoadMap, MapLoadType};
use portal2_core::args::Args;
use portal2_core::entity::EntityRegistry;

#[derive(Clone, Debug, PartialEq)]
pub struct Shot {
    pub name: String,
    pub map: String,
    pub pos: Vec3,
    pub angles: Vec3,
    /// Console commands run at this view after the defaults.
    pub commands: String,
}

pub fn parse_shots(text: &str) -> Vec<Shot> {
    text.lines()
        .filter_map(|line| {
            let line = line.split('#').next().unwrap_or("").trim();
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 8 {
                return None;
            }
            let n: Vec<f32> = parts[2..8].iter().filter_map(|s| s.parse().ok()).collect();
            (n.len() == 6).then(|| Shot {
                name: parts[0].to_owned(),
                map: parts[1].to_ascii_lowercase(),
                pos: Vec3::new(n[0], n[1], n[2]),
                angles: Vec3::new(n[3], n[4], n[5]),
                commands: parts[8..].join(" "),
            })
        })
        .collect()
}

/// Starts on the first view's map rather than loading the default one.
pub fn start_on_first_shot(args: &mut Args) {
    let list = args.shots.as_deref().or(args.bench.as_deref());
    if let Some(first) = list.and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| parse_shots(&t).into_iter().next()) {
        args.map = first.map;
    }
}

pub struct ShotsPlugin;

impl Plugin for ShotsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup).add_systems(Update, run_shots);
    }
}

#[derive(Clone, Copy, Debug)]
enum Phase {
    Next,
    Loading,
    Settling(u32),
    Saving(u32),
    /// `--bench`: collecting frame times since the instant.
    Measuring(Instant),
}

/// Frame times collected at the current view in `--bench` mode.
struct Bench {
    seconds: f32,
    out: PathBuf,
    samples: Vec<f32>,
}

impl Bench {
    /// Appends `name map frames mean p50 p95 p99 max fps` for the view.
    fn report(&mut self, shot: &Shot) {
        let mut ms: Vec<f32> = self.samples.drain(..).map(|s| s * 1000.0).collect();
        if ms.is_empty() {
            return;
        }
        ms.sort_by(f32::total_cmp);
        let mean = ms.iter().sum::<f32>() / ms.len() as f32;
        let pct = |p: f32| ms[((ms.len() - 1) as f32 * p).round() as usize];
        let mut line = String::new();
        let _ = write!(
            line,
            "{}	{}	frames={}	mean={mean:.2}ms	p50={:.2}ms	p95={:.2}ms	p99={:.2}ms	max={:.2}ms	fps={:.0}",
            shot.name,
            shot.map,
            ms.len(),
            pct(0.5),
            pct(0.95),
            pct(0.99),
            ms[ms.len() - 1],
            1000.0 / mean
        );
        info!("bench {line}");
        if let Some(dir) = self.out.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let old = std::fs::read_to_string(&self.out).unwrap_or_default();
        let _ = std::fs::write(
            &self.out,
            old + &line
                + "
",
        );
    }
}

#[derive(Resource)]
struct ShotRunner {
    shots: Vec<Shot>,
    index: usize,
    phase: Phase,
    settle: u32,
    settle_time: Option<f32>,
    /// When the current view started settling.
    settle_started: Option<std::time::Instant>,
    map_settle: f32,
    /// When the current map finished loading.
    loaded_at: Option<std::time::Instant>,
    out_dir: PathBuf,
    map_identity: usize,
    bench: Option<Bench>,
}

fn setup(mut commands: Commands, args: Res<Args>) {
    let Some(path) = args.shots.as_ref().or(args.bench.as_ref()) else { return };
    let shots = std::fs::read_to_string(path).map(|t| parse_shots(&t)).unwrap_or_default();
    info!("{} shots from {path}", shots.len());
    let out_dir = PathBuf::from(&args.shots_out);
    let _ = std::fs::create_dir_all(&out_dir);
    commands.insert_resource(ShotRunner {
        shots,
        index: 0,
        phase: Phase::Next,
        settle: args.shot_settle,
        settle_time: args.shot_settle_time,
        settle_started: None,
        map_settle: args.shot_map_settle,
        loaded_at: None,
        out_dir,
        map_identity: 0,
        bench: args.bench.is_some().then(|| Bench {
            seconds: args.bench_seconds,
            out: PathBuf::from(&args.bench_out),
            samples: Vec::new(),
        }),
    });
}

fn map_identity(world: &World) -> usize {
    world.get_resource::<CurrentMap>().map(|m| std::sync::Arc::as_ptr(&m.bsp) as usize).unwrap_or(0)
}

/// Puts the player at the shot's view and keeps them there.
fn hold_view(world: &mut World, shot: &Shot) {
    let Some(e) = world.resource::<EntityRegistry>().player else { return };
    if let Some(mut p) = world.get_mut::<Player>(e) {
        p.noclip = true;
        p.state.origin = shot.pos;
        p.previous_origin = shot.pos;
        p.state.velocity = Vec3::ZERO;
        p.state.ducked = false;
        p.state.duck_fraction = 0.0;
    }
    world.resource_mut::<PlayerInput>().input.angles = shot.angles;
}

fn run_shots(world: &mut World) {
    let Some(runner) = world.get_resource::<ShotRunner>() else { return };
    let (index, phase) = (runner.index, runner.phase);
    let Some(shot) = runner.shots.get(index).cloned() else {
        info!("shots done");
        world.remove_resource::<ShotRunner>();
        world.write_message(AppExit::Success);
        return;
    };
    let identity = map_identity(world);
    let next_phase = match phase {
        Phase::Next => {
            let loaded = world.get_resource::<CurrentMap>().is_some_and(|m| m.name == shot.map);
            let runner = world.resource::<ShotRunner>();
            let settled = runner.loaded_at.is_none_or(|t| t.elapsed().as_secs_f32() >= runner.map_settle);
            if loaded && !settled {
                hold_view(world, &shot);
                Phase::Next
            } else if loaded {
                portal2_core::console::execute_now(world, "sv_cheats 1; r_drawviewmodel 0; cl_drawhud 0; crosshair 0");
                if !shot.commands.is_empty() {
                    portal2_core::console::execute_now(world, &shot.commands);
                }
                hold_view(world, &shot);
                let mut runner = world.resource_mut::<ShotRunner>();
                runner.settle_started = Some(std::time::Instant::now());
                Phase::Settling(if runner.settle_time.is_some() { u32::MAX } else { runner.settle })
            } else {
                world.write_message(LoadMap::new(shot.map.clone(), MapLoadType::NewGame));
                world.resource_mut::<ShotRunner>().map_identity = identity;
                Phase::Loading
            }
        }
        Phase::Loading => {
            let old = world.resource::<ShotRunner>().map_identity;
            if identity != 0 && identity != old && world.resource::<EntityRegistry>().player.is_some() {
                world.resource_mut::<ShotRunner>().loaded_at = Some(std::time::Instant::now());
                Phase::Next
            } else {
                Phase::Loading
            }
        }
        Phase::Settling(0) if world.resource::<ShotRunner>().bench.is_some() => {
            info!("bench {}", shot.name);
            Phase::Measuring(Instant::now())
        }
        Phase::Measuring(start) => {
            hold_view(world, &shot);
            let frame = world.resource::<Time<Real>>().delta_secs();
            let mut runner = world.resource_mut::<ShotRunner>();
            let bench = runner.bench.as_mut().expect("measuring only in bench mode");
            bench.samples.push(frame);
            if start.elapsed().as_secs_f32() >= bench.seconds {
                bench.report(&shot);
                Phase::Saving(0)
            } else {
                Phase::Measuring(start)
            }
        }
        Phase::Settling(0) => {
            let path = world.resource::<ShotRunner>().out_dir.join(format!("{}.png", shot.name));
            info!("shot {} -> {}", shot.name, path.display());
            world.spawn(Screenshot::primary_window()).observe(save_to_disk(path));
            Phase::Saving(15)
        }
        Phase::Settling(n) => {
            hold_view(world, &shot);
            let runner = world.resource::<ShotRunner>();
            match (runner.settle_time, runner.settle_started) {
                (Some(t), Some(start)) if start.elapsed().as_secs_f32() >= t => Phase::Settling(0),
                (Some(_), _) => Phase::Settling(n),
                _ => Phase::Settling(n - 1),
            }
        }
        Phase::Saving(0) => {
            world.resource_mut::<ShotRunner>().index += 1;
            Phase::Next
        }
        Phase::Saving(n) => Phase::Saving(n - 1),
    };
    world.resource_mut::<ShotRunner>().phase = next_phase;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_shot_lines() {
        let shots = parse_shots(
            "# comment\nstart sp_a1_intro3 1 2 3 10 90 0 # trailing\nbad line\ngun sp_a1_intro3 1 2 3 0 0 0 r_drawviewmodel 1\n",
        );
        assert_eq!(
            shots,
            vec![
                Shot {
                    name: "start".into(),
                    map: "sp_a1_intro3".into(),
                    pos: Vec3::new(1.0, 2.0, 3.0),
                    angles: Vec3::new(10.0, 90.0, 0.0),
                    commands: String::new(),
                },
                Shot {
                    name: "gun".into(),
                    map: "sp_a1_intro3".into(),
                    pos: Vec3::new(1.0, 2.0, 3.0),
                    angles: Vec3::ZERO,
                    commands: "r_drawviewmodel 1".into(),
                },
            ]
        );
    }
}
