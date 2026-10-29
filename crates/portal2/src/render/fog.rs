//! Distance fog from `env_fog_controller`.
//!
//! The player's view uses the controller named by its `SetFogController`
//! input, else the first one flagged master, else the first one. Its fog is
//! handed to every 3D camera as Bevy's `DistanceFog` with linear falloff, and
//! our shaders read that uniform to fog by view depth the way Source does:
//! `min(max density, (depth - start) / (end - start))`, clamped to 0..1.
//!
//! The uniform has no slot for the maximum density, so it travels in
//! `directional_light_exponent` (our shaders do not use directional fog
//! light).

use bevy::pbr::{DistanceFog, FogFalloff};
use bevy::prelude::*;

use crate::entities::SpawnHandlers;
use crate::entities::detectors::PlayerFog;
use crate::entities::io::{InputCall, InputHandlers};
use portal2_core::clock::GameClock;
use portal2_core::entity::{EntityRegistry, MapEntity};

pub struct FogPlugin;

impl Plugin for FogPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostUpdate, apply_fog);
        let world = app.world_mut();
        world.resource_mut::<SpawnHandlers>().register("env_fog_controller", spawn_controller);
        world.resource_mut::<InputHandlers>().register("env_fog_controller", controller_input);
    }
}

/// One fog setting, with colours as 0-255 gamma values like the keyvalues.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct FogValues {
    color: Vec3,
    color2: Vec3,
    start: f32,
    end: f32,
}

impl FogValues {
    fn lerp(self, to: Self, t: f32) -> Self {
        Self {
            color: self.color.lerp(to.color, t),
            color2: self.color2.lerp(to.color2, t),
            start: self.start + (to.start - self.start) * t,
            end: self.end + (to.end - self.end) * t,
        }
    }
}

#[derive(Component, Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct FogController {
    enabled: bool,
    /// Blend toward the secondary colour by view direction (`fogblend`).
    blend: bool,
    /// Primary fog direction (world space).
    dir: Vec3,
    max_density: f32,
    /// HDR multiplier of the fog colour.
    hdr_scale: f32,
    values: FogValues,
    /// Targets set by the `...LerpTo` inputs.
    lerp_to: FogValues,
    lerp_time: f32,
    /// Transition in progress: from, start time.
    transition: Option<(FogValues, f64)>,
}

fn color255(me: &MapEntity, key: &str, default: Vec3) -> Vec3 {
    me.get(key).map(parse_color).unwrap_or(default)
}

fn parse_color(s: &str) -> Vec3 {
    let v: Vec<f32> = s.split_whitespace().filter_map(|p| p.parse().ok()).collect();
    match v.as_slice() {
        [r, g, b, ..] => Vec3::new(*r, *g, *b),
        _ => Vec3::splat(255.0),
    }
}

fn spawn_controller(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e) else { return };
    let values = FogValues {
        color: color255(me, "fogcolor", Vec3::splat(255.0)),
        color2: color255(me, "fogcolor2", Vec3::splat(255.0)),
        start: me.get_f32("fogstart").unwrap_or(500.0),
        end: me.get_f32("fogend").unwrap_or(2000.0),
    };
    let dir = if me.get_bool("use_angles") {
        portal2_core::coords::angles_forward(me.angles())
    } else {
        me.get_vec3("fogdir").unwrap_or(Vec3::X)
    };
    let controller = FogController {
        enabled: me.get_bool("fogenable"),
        blend: me.get_bool("fogblend"),
        dir: dir.normalize_or(Vec3::X),
        max_density: me.get_f32("fogmaxdensity").unwrap_or(1.0).clamp(0.0, 1.0),
        hdr_scale: me.get_f32("hdrcolorscale").unwrap_or(1.0),
        values,
        lerp_to: values,
        lerp_time: me.get_f32("foglerptime").unwrap_or(0.0),
        transition: None,
    };
    world.entity_mut(e).insert(controller);
}

fn controller_input(world: &mut World, call: &InputCall) -> bool {
    let now = world.resource::<GameClock>().curtime;
    let Some(mut c) = world.get_mut::<FogController>(call.target) else { return false };
    let f = source_formats::kv::parse_f32_lenient(&call.param);
    match call.input.as_str() {
        "turnon" => c.enabled = true,
        "turnoff" => c.enabled = false,
        "setstartdist" => c.values.start = f,
        "setenddist" => c.values.end = f,
        "setcolor" => c.values.color = parse_color(&call.param),
        "setcolorsecondary" => c.values.color2 = parse_color(&call.param),
        "setmaxdensity" => c.max_density = f.clamp(0.0, 1.0),
        "setfarz" => {}
        "setangles" => {
            let a: Vec<f32> = call.param.split_whitespace().filter_map(|p| p.parse().ok()).collect();
            if let [p, y, r, ..] = a.as_slice() {
                c.dir = portal2_core::coords::angles_forward(Vec3::new(*p, *y, *r));
            }
        }
        "setcolorlerpto" => c.lerp_to.color = parse_color(&call.param),
        "setcolorsecondarylerpto" => c.lerp_to.color2 = parse_color(&call.param),
        "setstartdistlerpto" => c.lerp_to.start = f,
        "setenddistlerpto" => c.lerp_to.end = f,
        "startfogtransition" => {
            let from = c.values;
            c.transition = Some((from, now));
        }
        _ => return false,
    }
    true
}

/// The controller the player's view uses.
fn active_controller(world: &World) -> Option<Entity> {
    let registry = world.resource::<EntityRegistry>();
    if let Some(name) = &world.resource::<PlayerFog>().controller
        && let Some(&e) = registry.by_name.get(name).and_then(|l| l.first())
        && world.get::<FogController>(e).is_some()
    {
        return Some(e);
    }
    let all = registry.by_class.get("env_fog_controller")?;
    all.iter().copied().find(|&e| world.get::<MapEntity>(e).is_some_and(|m| m.has_flag(1))).or_else(|| all.first().copied())
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn apply_fog(world: &mut World) {
    let now = world.get_resource::<GameClock>().map(|c| c.curtime).unwrap_or(0.0);
    let active = world.get_resource::<EntityRegistry>().and_then(|_| active_controller(world));
    // Advance a running transition on the active controller.
    let fog = active.and_then(|e| {
        let mut c = world.get_mut::<FogController>(e)?;
        if let Some((from, started)) = c.transition {
            let t = if c.lerp_time > 0.0 { ((now - started) as f32 / c.lerp_time).clamp(0.0, 1.0) } else { 1.0 };
            c.values = from.lerp(c.lerp_to, t);
            if t >= 1.0 {
                c.transition = None;
            }
        }
        c.enabled.then(|| c.clone())
    });
    // The 3D sky box camera keeps the sky camera's own fog.
    let mut cameras = world.query_filtered::<
        (Entity, &GlobalTransform, Option<&DistanceFog>),
        (With<Camera3d>, Without<crate::world::sky::SkyboxCamera>, Without<crate::render::projected::ShadowCamera>),
    >();
    let views: Vec<(Entity, Vec3, bool)> = cameras.iter(world).map(|(e, gt, f)| (e, gt.forward().into(), f.is_some())).collect();
    for (e, forward, has_fog) in views {
        // The world and model shaders read the fog uniform, which only exists
        // on views with fog, so views without fog get a zero-density one.
        let Some(c) = &fog else {
            let clear = DistanceFog {
                color: Color::NONE,
                directional_light_color: Color::NONE,
                directional_light_exponent: 0.0,
                falloff: FogFalloff::Linear { start: 1.0e9, end: 2.0e9 },
            };
            if !has_fog || world.get::<DistanceFog>(e).is_some_and(|f| f.directional_light_exponent != 0.0) {
                world.entity_mut(e).insert(clear);
            }
            continue;
        };
        // With blending the colour follows how much the view faces the
        // primary direction.
        let gamma = if c.blend {
            let t = (forward.dot(c.dir) + 1.0) * 0.5;
            c.values.color2.lerp(c.values.color, t)
        } else {
            c.values.color
        } / 255.0;
        let linear = Vec3::new(srgb_to_linear(gamma.x), srgb_to_linear(gamma.y), srgb_to_linear(gamma.z)) * c.hdr_scale;
        let distance_fog = DistanceFog {
            color: Color::linear_rgb(linear.x, linear.y, linear.z),
            directional_light_color: Color::NONE,
            directional_light_exponent: c.max_density,
            falloff: FogFalloff::Linear { start: c.values.start, end: c.values.end.max(c.values.start + 1.0) },
        };
        world.entity_mut(e).insert(distance_fog);
    }
}

impl portal2_core::save::Restore for FogController {}

pub fn register_save(r: &mut portal2_core::save::SaveRegistry) {
    r.component::<FogController>("fog_controller");
}
