//! Command line options.

use bevy::prelude::*;

#[derive(Resource, Clone, Debug)]
pub struct Args {
    /// Map to load on startup (without `maps/` and `.bsp`).
    pub map: String,
    /// Save a screenshot to this path after `frames` frames, then exit.
    pub screenshot: Option<String>,
    pub frames: u32,
    /// Override the start position / view angles (Source units / degrees).
    pub pos: Option<Vec3>,
    pub angles: Option<Vec3>,
    /// Free-fly camera instead of the player controller.
    pub noclip: bool,
    pub width: u32,
    pub height: u32,
    /// Linear exposure multiplier applied to the HDR scene.
    pub exposure: f32,
    /// Ignore lightmaps and draw every material unlit (debugging).
    pub fullbright: bool,
    /// No audio output (`--nosound`, also implied by the batch modes).
    pub nosound: bool,
    /// `--snd-record <file.wav>`: write the first `snd_seconds` of the mix.
    pub snd_record: Option<String>,
    pub snd_seconds: f32,
    /// Bitmask of world material flags to clear (A/B debugging of shader features).
    pub strip_flags: u32,
    /// Log every output firing (`--io-debug`).
    pub io_debug: bool,
    /// Debug switches: skip static props / entity models.
    pub no_static_props: bool,
    pub no_entity_models: bool,
    /// Report what the view ray hits (`--probe`).
    pub probe: bool,
    /// Scripted input for movement tests (`--autopilot f2,j,w1`).
    pub autopilot: Option<String>,
    /// Portal gun override: none, blue or both.
    pub portalgun: Option<String>,
    /// Disable portal rendering (debugging).
    pub no_portals: bool,
    /// Debug entity spawns after map load: `class@x,y,z[@key=value;...]`
    /// (repeatable).
    pub spawn: Vec<String>,
    /// Debug inputs sent to every entity of a class after map load:
    /// `class.input[=param][@delay]` (repeatable).
    pub fire: Vec<String>,
    /// Source-style `+command args` from the command line, run by the
    /// console's `stuffcmds` once the first map has loaded.
    pub commands: Vec<String>,
    /// Batch screenshots of listed views (`--shots <file>`), written to
    /// `shots_out`, waiting `shot_settle` frames at each view.
    pub shots: Option<String>,
    pub shots_out: String,
    pub shot_settle: u32,
    /// `--smoke <maps>`: run each map briefly and report problems.
    pub smoke: Option<String>,
    /// `--save-test <maps>`: save/load round trip of each map (see `save::roundtrip`).
    pub save_test: Option<String>,
    /// `--save-info <name>`: print a summary of a saved game and quit.
    pub save_info: Option<String>,
    pub smoke_frames: u32,
    /// `--chain <map>`: take each level's exit from `map` on through the
    /// campaign (`chain_count` maps, `chain_speed` times real time).
    pub chain: Option<String>,
    pub chain_count: u32,
    pub chain_speed: f32,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            map: "sp_a2_laser_intro".to_owned(),
            screenshot: None,
            frames: 240,
            pos: None,
            angles: None,
            noclip: false,
            width: 1600,
            height: 900,
            exposure: 3.0,
            fullbright: false,
            nosound: false,
            snd_record: None,
            snd_seconds: 10.0,
            strip_flags: 0,
            io_debug: false,
            no_static_props: false,
            no_entity_models: false,
            probe: false,
            autopilot: None,
            portalgun: None,
            no_portals: false,
            spawn: Vec::new(),
            fire: Vec::new(),
            commands: Vec::new(),
            shots: None,
            shots_out: "re/out/ours".to_owned(),
            shot_settle: 120,
            smoke: None,
            save_test: None,
            save_info: None,
            smoke_frames: 300,
            chain: None,
            chain_count: 100,
            chain_speed: 8.0,
        }
    }
}

fn parse_vec3(s: &str) -> Option<Vec3> {
    let v: Vec<f32> = s.split([',', ' ']).filter(|p| !p.is_empty()).filter_map(|p| p.parse().ok()).collect();
    (v.len() == 3).then(|| Vec3::new(v[0], v[1], v[2]))
}

/// A command-line word that continues the previous `+command`: anything
/// not starting with `-`, or a negative number.
fn is_value(s: &str) -> bool {
    !s.starts_with('-') || s[1..].starts_with(|c: char| c.is_ascii_digit() || c == '.')
}

/// Re-quotes a word containing spaces so the console sees one argument.
fn quote(s: &str) -> String {
    if s.contains(char::is_whitespace) { format!("\"{s}\"") } else { s.to_owned() }
}

impl Args {
    pub fn from_env() -> Self {
        let mut args = Args::default();
        let mut it = std::env::args().skip(1);
        while let Some(a) = it.next() {
            match a.as_str() {
                "--map" | "+map" => args.map = it.next().unwrap_or_default(),
                "--screenshot" => args.screenshot = it.next(),
                "--frames" => args.frames = it.next().and_then(|s| s.parse().ok()).unwrap_or(args.frames),
                "--pos" => args.pos = it.next().as_deref().and_then(parse_vec3),
                "--ang" | "--angles" => args.angles = it.next().as_deref().and_then(parse_vec3),
                "--noclip" => args.noclip = true,
                "--fullbright" => args.fullbright = true,
                "--nosound" | "-nosound" => args.nosound = true,
                "--snd-record" => args.snd_record = it.next(),
                "--snd-seconds" => args.snd_seconds = it.next().and_then(|s| s.parse().ok()).unwrap_or(10.0),
                "--io-debug" => args.io_debug = true,
                "--no-static-props" => args.no_static_props = true,
                "--no-entity-models" => args.no_entity_models = true,
                "--probe" => args.probe = true,
                "--autopilot" => args.autopilot = it.next(),
                "--portalgun" => args.portalgun = it.next(),
                "--no-portals" => args.no_portals = true,
                "--shots" => args.shots = it.next(),
                "--smoke" => args.smoke = it.next(),
                "--save-test" => args.save_test = it.next(),
                "--save-info" => args.save_info = it.next(),
                "--smoke-frames" => args.smoke_frames = it.next().and_then(|s| s.parse().ok()).unwrap_or(args.smoke_frames),
                "--chain" => args.chain = it.next(),
                "--chain-count" => args.chain_count = it.next().and_then(|s| s.parse().ok()).unwrap_or(args.chain_count),
                "--chain-speed" => args.chain_speed = it.next().and_then(|s| s.parse().ok()).unwrap_or(args.chain_speed),
                "--shots-out" => args.shots_out = it.next().unwrap_or(args.shots_out),
                "--shot-settle" => args.shot_settle = it.next().and_then(|s| s.parse().ok()).unwrap_or(args.shot_settle),
                "--spawn" => args.spawn.extend(it.next()),
                "--fire" => args.fire.extend(it.next()),
                "--strip-flags" => args.strip_flags = it.next().and_then(|s| s.parse().ok()).unwrap_or(0),
                "--width" => args.width = it.next().and_then(|s| s.parse().ok()).unwrap_or(args.width),
                "--height" => args.height = it.next().and_then(|s| s.parse().ok()).unwrap_or(args.height),
                "--exposure" => args.exposure = it.next().and_then(|s| s.parse().ok()).unwrap_or(args.exposure),
                other if other.starts_with('+') => args.commands.push(other[1..].to_owned()),
                other if is_value(other) && !args.commands.is_empty() => {
                    let last = args.commands.last_mut().expect("checked non-empty");
                    last.push(' ');
                    last.push_str(&quote(other));
                }
                other if !other.starts_with('-') => args.map = other.to_owned(),
                other => warn!("unknown argument {other}"),
            }
        }
        // Start on the first shot's map rather than loading the default one.
        if let Some(first) = args.shots.as_deref().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| crate::shots::parse_shots(&t).into_iter().next()) {
            args.map = first.map;
        }
        if let Some(first) = &args.chain {
            args.map = first.clone();
        }
        args
    }
}
