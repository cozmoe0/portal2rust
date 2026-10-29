//! Scripted views. `point_viewcontrol` puts the player's view on itself,
//! usually parented to an attachment of an animated model (the first map's
//! wake-up from bed, the `ghostAnim` falls). `point_viewproxy` moves the
//! view into a stand-in space: the view sits at its proxy entity, offset by
//! where the player stands relative to the view proxy's own spawn point and
//! turned by the yaw of the proxy (or of an attachment on the proxy's
//! parent). The first map's container ride uses it: the player walks a
//! hidden box while seeing the animated room. Enabling a view replaces the
//! one before; disabling any of them gives the player their own view back.
//! Facts in docs/re/view_control.md.

use bevy::prelude::*;

use super::io::{InputCall, InputHandlers};
use super::{EntityRegistry, MapEntity, SpawnHandlers, world_transform};
use crate::player::{Player, PlayerInput};
use portal2_core::clock::GameClock;
use portal2_core::coords::{angles_to_quat, quat_to_angles};
use portal2_core::sets::PlayerCameraSet;

pub fn register(app: &mut App) {
    app.init_resource::<ActiveView>()
        .add_systems(FixedUpdate, update_proxies.after(portal2_core::sets::PlayerMoveSet))
        .add_systems(PostUpdate, track_proxy_motion.before(TransformSystems::Propagate))
        .add_systems(
            PostUpdate,
            apply_view.in_set(PlayerCameraSet).after(crate::player::place_camera).before(crate::camera::sync_camera_transform),
        );
    let world = app.world_mut();
    {
        let mut s = world.resource_mut::<SpawnHandlers>();
        s.register("point_viewcontrol", spawn_control);
        s.register("point_viewproxy", spawn_proxy);
    }
    let mut h = world.resource_mut::<InputHandlers>();
    h.register("point_viewcontrol", control_input);
    h.register("point_viewproxy", proxy_input);
}

pub fn register_save(r: &mut portal2_core::save::SaveRegistry) {
    r.component::<ViewControl>("view_control");
    r.component::<ViewProxy>("view_proxy");
    r.resource::<ActiveView>("active_view");
}

/// The entity the player sees from, if not their own eyes.
#[derive(Resource, Default, serde::Serialize, serde::Deserialize)]
pub struct ActiveView(#[serde(with = "portal2_core::save::ids::opt_entity")] pub Option<Entity>);

impl portal2_core::save::Restore for ActiveView {}

/// `point_viewcontrol` spawnflags: hold the player still.
const SF_FREEZE: u32 = 4;

#[derive(Component, serde::Serialize, serde::Deserialize)]
pub struct ViewControl {
    flags: u32,
}

impl portal2_core::save::Restore for ViewControl {}

#[derive(Component, serde::Serialize, serde::Deserialize)]
pub struct ViewProxy {
    flags: u32,
    proxy: String,
    attachment: Option<String>,
    /// 0: at the proxy; 1: from where the player was, easing onto the proxy
    /// over a second; 2: the player's place relative to `spawn_origin`.
    offset_type: i32,
    /// Share of the proxy's pitch and roll the view takes (yaw is whole).
    tilt: f32,
    spawn_origin: Vec3,
    enabled_at: f64,
    start_offset: Vec3,
    /// `usefakeacceleration`: the proxy's jolts push the player around.
    #[serde(default)]
    fake_acceleration: bool,
    /// `skewaccelerationforward`: jolts along the proxy's facing push less.
    #[serde(default)]
    skew_forward: bool,
    #[serde(default)]
    acceleration_scale: f32,
    /// The proxy's place and velocity when last seen (each frame, after
    /// animation), and its change of velocity since the last tick.
    #[serde(default)]
    last_position: Vec3,
    #[serde(default)]
    last_velocity: Vec3,
    #[serde(skip)]
    pending_accel: Vec3,
}

impl portal2_core::save::Restore for ViewProxy {}

fn spawn_control(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e) else { return };
    let flags = me.spawnflags();
    world.entity_mut(e).insert(ViewControl { flags });
}

fn spawn_proxy(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e) else { return };
    let p = ViewProxy {
        flags: me.spawnflags(),
        proxy: me.get("proxy").unwrap_or("").to_ascii_lowercase(),
        attachment: me.get("proxyattachment").filter(|s| !s.is_empty()).map(str::to_owned),
        offset_type: me.get_i32("offsettype").unwrap_or(0),
        tilt: me.get_f32("tiltfraction").unwrap_or(0.5).clamp(0.0, 1.0),
        spawn_origin: me.origin(),
        enabled_at: 0.0,
        start_offset: Vec3::ZERO,
        fake_acceleration: me.get_bool("usefakeacceleration"),
        skew_forward: me.get_bool("skewaccelerationforward"),
        acceleration_scale: me.get_f32("accelerationscalar").unwrap_or(1.0),
        last_position: Vec3::ZERO,
        last_velocity: Vec3::ZERO,
        pending_accel: Vec3::ZERO,
    };
    world.entity_mut(e).insert(p);
}

/// Points the player's view at `view` (or back at their eyes).
fn set_view(world: &mut World, view: Option<Entity>) {
    world.resource_mut::<ActiveView>().0 = view;
    let frozen = view.and_then(|v| world.get::<ViewControl>(v).map(|c| c.flags).or(world.get::<ViewProxy>(v).map(|p| p.flags)));
    world.resource_mut::<PlayerInput>().frozen = frozen.is_some_and(|f| f & SF_FREEZE != 0);
}

/// Where the player's eye is, and the player entity.
fn player_eye(world: &World) -> Option<(Entity, Vec3)> {
    let pe = world.resource::<EntityRegistry>().player?;
    let p = world.get::<Player>(pe)?;
    Some((pe, p.state.origin + Vec3::Z * p.state.eye_offset()))
}

/// Puts the player where the view is, eyes at its origin, facing its way.
fn teleport_player_to(world: &mut World, e: Entity) {
    let t = world_transform(world, e).compute_transform();
    let angles = quat_to_angles(t.rotation);
    let Some(pe) = world.resource::<EntityRegistry>().player else { return };
    if let Some(mut p) = world.get_mut::<Player>(pe) {
        let origin = t.translation - Vec3::Z * p.state.eye_offset();
        p.state.origin = origin;
        p.previous_origin = origin;
        p.state.velocity = Vec3::ZERO;
        p.ground_ref = None;
    }
    world.resource_mut::<PlayerInput>().input.angles = Vec3::new(angles.x, angles.y, 0.0);
}

fn control_input(world: &mut World, call: &InputCall) -> bool {
    let e = call.target;
    match call.input.as_str() {
        "enable" => {
            // A view already in use is turned off first.
            set_view(world, Some(e));
        }
        "disable" => set_view(world, None),
        "teleporttoview" => {
            teleport_player_to(world, e);
            set_view(world, None);
        }
        _ => return false,
    }
    true
}

fn proxy_input(world: &mut World, call: &InputCall) -> bool {
    let e = call.target;
    match call.input.as_str() {
        "enable" => {
            let now = world.resource::<GameClock>().curtime;
            let proxy = proxy_origin(world, e);
            if let (Some(o), Some(mut p)) = (proxy, world.get_mut::<ViewProxy>(e))
                && p.last_position == Vec3::ZERO
            {
                p.last_position = o;
                p.last_velocity = Vec3::ZERO;
            }
            let eye = player_eye(world).map(|(_, eye)| eye);
            if let Some(mut p) = world.get_mut::<ViewProxy>(e) {
                p.enabled_at = now;
                if let (Some(o), Some(eye)) = (proxy, eye)
                    && p.offset_type != 2
                {
                    p.start_offset = eye - o;
                }
            }
            place_proxy(world, e, None);
            set_view(world, Some(e));
        }
        "disable" => set_view(world, None),
        "teleportplayertoproxy" => {
            teleport_player_to(world, e);
            set_view(world, None);
        }
        _ => return false,
    }
    true
}

fn proxy_entity(world: &World, e: Entity) -> Option<Entity> {
    let name = &world.get::<ViewProxy>(e)?.proxy;
    world.resource::<EntityRegistry>().first_named(name)
}

fn proxy_origin(world: &World, e: Entity) -> Option<Vec3> {
    proxy_entity(world, e).map(|p| world_transform(world, p).translation())
}

/// The proxy's orientation: its named attachment (on the proxy's parent
/// model) if it has one, otherwise the proxy's own.
fn proxy_rotation(world: &World, e: Entity, proxy: Entity) -> Quat {
    let own = world_transform(world, proxy).compute_transform().rotation;
    let Some(name) = world.get::<ViewProxy>(e).and_then(|p| p.attachment.clone()) else { return own };
    super::map_parent(world, proxy).and_then(|parent| super::attachment_world(world, parent, &name)).map_or(own, |t| t.rotation)
}

/// Computes the proxy view from the player's place (`origin`: feet, or
/// the current one) and moves the view proxy entity there.
fn place_proxy(world: &mut World, e: Entity, origin: Option<Vec3>) -> Option<Transform> {
    let (Some(proxy), Some(p)) = (proxy_entity(world, e), world.get::<ViewProxy>(e)) else { return None };
    let (offset_type, tilt, spawn_origin, start_offset, enabled_at) = (p.offset_type, p.tilt, p.spawn_origin, p.start_offset, p.enabled_at);
    let pe = world.resource::<EntityRegistry>().player?;
    let player = world.get::<Player>(pe)?;
    let feet = origin.unwrap_or(player.state.origin);
    let now = world.resource::<GameClock>().curtime;
    let base = world_transform(world, proxy).translation();
    let rotation = proxy_rotation(world, e, proxy);
    let offset = match offset_type {
        1 => {
            let t = ((now - enabled_at) as f32).clamp(0.0, 1.0);
            start_offset * (1.0 - (3.0 * t * t - 2.0 * t * t * t))
        }
        // The game measures from 64 units above the feet whatever the stance.
        2 => rotation * (feet + Vec3::Z * 64.0 - spawn_origin),
        _ => Vec3::ZERO,
    };
    // Yaw whole, pitch and roll scaled by the tilt share, then the
    // player's own look on top.
    let a = quat_to_angles(rotation);
    let lean = angles_to_quat(Vec3::new(a.x * tilt, a.y, a.z * tilt));
    let look = angles_to_quat(world.resource::<PlayerInput>().input.angles);
    let t = Transform { translation: base + offset, rotation: lean * look, scale: Vec3::ONE };
    if let Some(mut own) = world.get_mut::<Transform>(e) {
        *own = t;
    }
    Some(t)
}

/// Each tick the enabled proxy views follow the player (other entities,
/// like Wheatley's swivels, aim at them).
fn update_proxies(world: &mut World) {
    let Some(view) = world.resource::<ActiveView>().0 else { return };
    if world.get::<ViewProxy>(view).is_some() {
        place_proxy(world, view, None);
        fake_acceleration(world, view);
    }
}

/// The proxy moves with animation, which runs each frame: its velocity is
/// measured there and the changes add up until the next tick takes them.
fn track_proxy_motion(world: &mut World) {
    let Some(view) = world.resource::<ActiveView>().0 else { return };
    let dt = world.resource::<Time>().delta_secs();
    let Some(proxy) = proxy_entity(world, view) else { return };
    let position = world_transform(world, proxy).translation();
    let Some(mut p) = world.get_mut::<ViewProxy>(view) else { return };
    if !p.fake_acceleration || dt <= 0.0 {
        return;
    }
    let velocity = (position - p.last_position) / dt;
    let change = velocity - p.last_velocity;
    p.pending_accel += change;
    p.last_position = position;
    p.last_velocity = velocity;
}

/// With `usefakeacceleration`, the proxy's change of velocity this tick
/// (ignored when tiny or huge) pushes the player the other way through
/// their base velocity, which otherwise winds down: it loses 100 units/s
/// each second and a quarter of itself.
fn fake_acceleration(world: &mut World, e: Entity) {
    let dt = world.resource::<Time<Fixed>>().delta_secs();
    let Some(proxy) = proxy_entity(world, e) else { return };
    let placed = world_transform(world, proxy).compute_transform();
    let Some(mut p) = world.get_mut::<ViewProxy>(e) else { return };
    if !p.fake_acceleration || dt <= 0.0 {
        return;
    }
    let mut accel = std::mem::take(&mut p.pending_accel);
    if !(20.0..=10000.0).contains(&accel.length_squared()) {
        accel = Vec3::ZERO;
    }
    if p.skew_forward {
        let forward = placed.rotation * Vec3::X;
        accel *= 0.75 - forward.dot(accel.normalize_or_zero()) * 0.35;
    }
    let scale = p.acceleration_scale;
    let Some(pe) = world.resource::<EntityRegistry>().player else { return };
    let Some(mut player) = world.get_mut::<Player>(pe) else { return };
    let s = &mut player.state;
    let speed = s.base_velocity.length();
    let base = if speed >= dt * 100.0 { s.base_velocity * ((speed - dt * 100.0) / speed) } else { Vec3::ZERO };
    s.base_velocity = base * (1.0 - dt * 0.25).max(0.0) - accel * 3.0 * scale;
    s.base_velocity_set = true;
}

/// Puts the main camera on the active view each frame.
fn apply_view(world: &mut World) {
    let Some(view) = world.resource::<ActiveView>().0 else { return };
    if world.get_entity(view).is_err() {
        set_view(world, None);
        return;
    }
    let t = if world.get::<ViewProxy>(view).is_some() {
        // Between ticks, from where the player is drawn.
        let alpha = world.resource::<Time<Fixed>>().overstep_fraction();
        let feet = world
            .resource::<EntityRegistry>()
            .player
            .and_then(|pe| world.get::<Player>(pe))
            .map(|p| p.previous_origin.lerp(p.state.origin, alpha));
        match place_proxy(world, view, feet) {
            Some(t) => t,
            None => return,
        }
    } else {
        world_transform(world, view).compute_transform()
    };
    let angles = quat_to_angles(t.rotation);
    debug!("view {:.2} at {:.1?} angles {:.1?}", world.resource::<GameClock>().curtime, t.translation, angles);
    let mut cams = world.query_filtered::<&mut portal2_core::view::ViewAngles, With<portal2_core::view::MainCamera>>();
    for mut v in cams.iter_mut(world) {
        v.origin = t.translation;
        v.angles = angles;
    }
}
