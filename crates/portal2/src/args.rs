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
        }
    }
}

fn parse_vec3(s: &str) -> Option<Vec3> {
    let v: Vec<f32> = s.split([',', ' ']).filter(|p| !p.is_empty()).filter_map(|p| p.parse().ok()).collect();
    (v.len() == 3).then(|| Vec3::new(v[0], v[1], v[2]))
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
                "--io-debug" => args.io_debug = true,
                "--no-static-props" => args.no_static_props = true,
                "--no-entity-models" => args.no_entity_models = true,
                "--probe" => args.probe = true,
                "--autopilot" => args.autopilot = it.next(),
                "--portalgun" => args.portalgun = it.next(),
                "--no-portals" => args.no_portals = true,
                "--spawn" => args.spawn.extend(it.next()),
                "--fire" => args.fire.extend(it.next()),
                "--strip-flags" => args.strip_flags = it.next().and_then(|s| s.parse().ok()).unwrap_or(0),
                "--width" => args.width = it.next().and_then(|s| s.parse().ok()).unwrap_or(args.width),
                "--height" => args.height = it.next().and_then(|s| s.parse().ok()).unwrap_or(args.height),
                "--exposure" => args.exposure = it.next().and_then(|s| s.parse().ok()).unwrap_or(args.exposure),
                other if !other.starts_with('-') => args.map = other.to_owned(),
                other => warn!("unknown argument {other}"),
            }
        }
        args
    }
}
