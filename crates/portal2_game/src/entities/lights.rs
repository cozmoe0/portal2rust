//! Switchable lights (`light`, `light_spot` with a name): the map compiler
//! gave each its own light style (`style`, 32 and up), and the light drives
//! that style's pattern. It starts dark (`a`) with spawn flag 1, else with
//! its `pattern` or normal (`m`). `TurnOn` restores the pattern (or `m`),
//! `TurnOff` darkens it, `Toggle` swaps, `SetPattern` sets a new pattern and
//! `FadeToPattern` walks one letter every 0.1 s from the current pattern's
//! first letter to the new one's before switching to it.

use bevy::prelude::*;

use super::io::{InputCall, InputHandlers};
use super::{MapEntity, SpawnHandlers};
use portal2_core::clock::GameClock;
use portal2_world::lightstyles::LightStyles;

const SF_START_OFF: u32 = 1;
/// Styles below this are the engine's fixed patterns.
const FIRST_SWITCHABLE: i32 = 32;

pub fn register(app: &mut App) {
    app.add_systems(PreUpdate, drive_styles.after(portal2_core::sets::EntitySpawnSet));
    let world = app.world_mut();
    for class in ["light", "light_spot"] {
        world.resource_mut::<SpawnHandlers>().register(class, spawn_light);
        world.resource_mut::<InputHandlers>().register(class, light_input);
    }
}

#[derive(Component, Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Light {
    style: usize,
    pattern: Option<String>,
    off: bool,
    /// A fade in progress: current and target letters and the next step.
    fade: Option<(u8, u8, f64)>,
}

impl portal2_core::save::Restore for Light {}

pub fn register_save(r: &mut portal2_core::save::SaveRegistry) {
    r.component::<Light>("light");
}

impl Light {
    /// The pattern the style shows now.
    fn current(&self) -> String {
        match self.fade {
            Some((c, _, _)) => (c as char).to_string(),
            None if self.off => "a".to_owned(),
            None => self.pattern.clone().unwrap_or_else(|| "m".to_owned()),
        }
    }
}

fn spawn_light(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e) else { return };
    let style = me.get_i32("style").unwrap_or(0);
    if style < FIRST_SWITCHABLE {
        return;
    }
    let light = Light {
        style: style as usize,
        pattern: me.get("pattern").filter(|p| !p.is_empty()).map(str::to_owned),
        off: me.has_flag(SF_START_OFF),
        fade: None,
    };
    world.resource_mut::<LightStyles>().set(light.style, &light.current());
    world.entity_mut(e).insert(light);
}

fn light_input(world: &mut World, call: &InputCall) -> bool {
    let now = world.resource::<GameClock>().curtime;
    let Some(mut light) = world.get_mut::<Light>(call.target) else {
        // Unswitchable lights accept their inputs without effect.
        return matches!(call.input.as_str(), "turnon" | "turnoff" | "toggle" | "setpattern" | "fadetopattern");
    };
    match call.input.as_str() {
        "turnon" => {
            light.off = false;
            light.fade = None;
        }
        "turnoff" => {
            light.off = true;
            light.fade = None;
        }
        "toggle" => {
            light.off = !light.off;
            light.fade = None;
        }
        "setpattern" => {
            light.pattern = Some(call.param.clone()).filter(|p| !p.is_empty());
            light.off = false;
            light.fade = None;
        }
        "fadetopattern" => {
            let from = light.pattern.as_deref().and_then(|p| p.bytes().next()).unwrap_or(b'm');
            let to = call.param.bytes().next().unwrap_or(b'm');
            light.pattern = Some(call.param.clone()).filter(|p| !p.is_empty());
            light.off = false;
            light.fade = Some((from, to, now));
        }
        _ => return false,
    }
    let (style, pattern) = (light.style, light.current());
    world.resource_mut::<LightStyles>().set(style, &pattern);
    true
}

/// Steps fades and keeps every light's style showing its pattern (also
/// after a saved game is loaded).
fn drive_styles(clock: Res<GameClock>, mut lights: Query<&mut Light>, mut styles: ResMut<LightStyles>) {
    for mut light in &mut lights {
        if let Some((cur, to, next)) = light.fade
            && clock.due(next)
        {
            light.fade = if cur == to {
                None
            } else {
                let step = if cur < to { cur + 1 } else { cur - 1 };
                Some((step, to, next + 0.1))
            };
        }
        styles.set(light.style, &light.current());
    }
}
