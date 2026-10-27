//! Player footsteps. Every step plays the `stepleft`/`stepright` sound of
//! the surface property under the player's feet (from the brush material's
//! `$surfaceprop`, resolved through `scripts/surfaceproperties*.txt` with
//! `base` inheritance), alternating sides, on `CHAN_BODY` at the feet.
//! Steps come every 400 ms below run speed (220) and every 300 ms above,
//! 100 ms slower while ducked; the volume is 0.2 walking, 0.5 running and
//! ×0.65 ducked. No steps below 10 units/s or off the ground.

use std::collections::HashMap;

use bevy::prelude::*;
use source_formats::kv;

use super::Emit;
use crate::collision::Physics;
use crate::entities::EntityRegistry;
use crate::player::Player;
use crate::vfs::GameFs;
use crate::world::CurrentMap;

const RUN_SPEED: f32 = 220.0;
const MIN_SPEED: f32 = 10.0;

#[derive(Clone, Default, Debug)]
pub struct SurfaceSounds {
    pub step_left: Option<String>,
    pub step_right: Option<String>,
    pub impact_hard: Option<String>,
    pub impact_soft: Option<String>,
}

#[derive(Default)]
pub struct SurfaceProps {
    raw: HashMap<String, HashMap<String, String>>,
    resolved: HashMap<String, SurfaceSounds>,
}

impl SurfaceProps {
    pub fn load(fs: &source_formats::fs::GameFileSystem) -> Self {
        let mut s = Self::default();
        let Some(manifest) = fs.read_string("scripts/surfaceproperties_manifest.txt").and_then(|t| kv::parse(&t)) else { return s };
        for file in manifest.get_all("file").filter_map(|f| f.as_str()) {
            let Some(text) = fs.read_string(file) else { continue };
            for root in kv::parse_document(&text).roots {
                let entry = s.raw.entry(root.key.to_ascii_lowercase()).or_default();
                for c in root.children() {
                    if let Some(v) = c.as_str() {
                        entry.insert(c.key.to_ascii_lowercase(), v.to_owned());
                    }
                }
            }
        }
        s
    }

    fn lookup(&self, name: &str, key: &str, depth: u32) -> Option<String> {
        let props = self.raw.get(name)?;
        if let Some(v) = props.get(key) {
            return Some(v.clone());
        }
        if depth > 8 {
            return None;
        }
        props.get("base").and_then(|b| self.lookup(&b.to_ascii_lowercase(), key, depth + 1))
    }

    pub fn sounds(&mut self, name: &str) -> SurfaceSounds {
        let name = if self.raw.contains_key(name) { name.to_owned() } else { "default".to_owned() };
        if let Some(s) = self.resolved.get(&name) {
            return s.clone();
        }
        let s = SurfaceSounds {
            step_left: self.lookup(&name, "stepleft", 0),
            step_right: self.lookup(&name, "stepright", 0),
            impact_hard: self.lookup(&name, "impacthard", 0),
            impact_soft: self.lookup(&name, "impactsoft", 0),
        };
        self.resolved.insert(name, s.clone());
        s
    }
}

#[derive(Default)]
pub struct FootstepState {
    step_time: f32,
    left: bool,
    props: Option<SurfaceProps>,
    /// Material name to its `$surfaceprop`.
    materials: HashMap<String, String>,
}

/// The surface property of a BSP texinfo.
fn texinfo_surface(world: &World, fs: &GameFs, state: &mut FootstepState, texinfo: i32) -> String {
    let Some(map) = world.get_resource::<CurrentMap>() else { return "default".into() };
    let name = usize::try_from(texinfo)
        .ok()
        .and_then(|t| map.bsp.texinfo.get(t))
        .and_then(|t| usize::try_from(t.texdata).ok())
        .and_then(|d| map.bsp.texdata.get(d))
        .and_then(|d| usize::try_from(d.name_index).ok())
        .and_then(|n| map.bsp.texdata_strings.get(n))
        .map(|s| s.to_ascii_lowercase());
    let Some(name) = name else { return "default".into() };
    if let Some(p) = state.materials.get(&name) {
        return p.clone();
    }
    let prop = crate::render::materials::load_vmt(fs, &format!("materials/{name}.vmt"))
        .and_then(|v| v.get("$surfaceprop").map(str::to_ascii_lowercase))
        .unwrap_or_else(|| "default".into());
    state.materials.insert(name, prop.clone());
    prop
}

pub fn player_footsteps(world: &mut World, mut state: Local<FootstepState>) {
    let Some(fs) = world.get_resource::<GameFs>().cloned() else { return };
    let Some(player) = world.resource::<EntityRegistry>().player else { return };
    let Some(p) = world.get::<Player>(player) else { return };
    let (s, noclip) = (p.state, p.noclip);
    let dt_ms = world.resource::<Time>().delta_secs() * 1000.0;
    if state.step_time > 0.0 {
        state.step_time = (state.step_time - dt_ms).max(0.0);
    }
    let speed = s.velocity.length();
    let ground_speed = s.velocity.truncate().length();
    if noclip || state.step_time > 0.0 || speed < MIN_SPEED || !s.on_ground || ground_speed <= 0.0001 {
        return;
    }
    let walking = speed < RUN_SPEED;
    state.step_time = if walking { 400.0 } else { 300.0 } + if s.ducked { 100.0 } else { 0.0 };
    let mut volume = if walking { 0.2 } else { 0.5 };
    if s.ducked {
        volume *= 0.65;
    }
    let surface = {
        let mut sys: bevy::ecs::system::SystemState<Physics> = bevy::ecs::system::SystemState::new(world);
        let Ok(physics) = sys.get(world) else { return };
        let t = physics.trace(s.origin + Vec3::Z, s.origin - Vec3::Z * 8.0, Vec3::ZERO, Vec3::ZERO, crate::collision::SOLID, &[player]);
        t.hit().then_some(t.surface)
    };
    let prop = match surface {
        Some(t) if t >= 0 => texinfo_surface(world, &fs, &mut state, t),
        _ => "default".into(),
    };
    if state.props.is_none() {
        state.props = Some(SurfaceProps::load(&fs.0));
    }
    let sounds = state.props.as_mut().unwrap().sounds(&prop);
    state.left = !state.left;
    let Some(name) = (if state.left { sounds.step_left } else { sounds.step_right }) else { return };
    super::emit(world, &name, Emit { entity: Some(player), origin: Some(s.origin), volume: Some(volume), slot: Some(4), ..default() });
}
