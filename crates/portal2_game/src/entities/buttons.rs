//! Brush buttons (`func_button`, `func_rot_button`). Pressing one (use with
//! spawnflag 1024, touch with 256, or the `Press` inputs) fires `OnPressed`
//! and moves it in; once in it fires `OnIn` and, after `wait` seconds, moves
//! back out and fires `OnOut`. Toggle buttons (32) stay in until pressed
//! again, buttons with "wait" -1 stay in for good, locked ones only fire
//! `OnUseLocked`, and spawnflag 1 keeps the brush from moving.

use bevy::prelude::*;

use super::io::{FireOutput, InputCall, InputHandlers, fire_output};
use super::{EntityRegistry, MapEntity, SpawnHandlers};
use crate::player::Player;
use portal2_core::clock::GameClock;
use portal2_core::coords::angles_forward;
use portal2_core::sets::ThinkSet;
use portal2_world::CurrentMap;
use portal2_world::collision::Physics;

const SF_DONT_MOVE: u32 = 1;
const SF_TOGGLE: u32 = 32;
const SF_TOUCH: u32 = 256;
const SF_USE: u32 = 1024;
const SF_LOCKED: u32 = 2048;

pub fn register(app: &mut App) {
    app.add_systems(FixedUpdate, (touch_buttons, move_buttons).chain().in_set(ThinkSet));
    let world = app.world_mut();
    {
        let mut s = world.resource_mut::<SpawnHandlers>();
        s.register("func_button", spawn_button);
        s.register("func_rot_button", spawn_button);
    }
    let mut h = world.resource_mut::<InputHandlers>();
    h.register("func_button", button_input);
    h.register("func_rot_button", button_input);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum Phase {
    Out,
    GoingIn,
    In,
    GoingOut,
}

#[derive(Component, Clone, serde::Serialize, serde::Deserialize)]
pub struct BrushButton {
    phase: Phase,
    locked: bool,
    toggle: bool,
    use_activates: bool,
    touch_activates: bool,
    /// Seconds in before returning (-1 = stays in).
    wait: f32,
    return_at: Option<f64>,
    #[serde(skip)]
    out_pose: Transform,
    #[serde(skip)]
    in_pose: Transform,
    /// Distance (units or degrees) of a full move; 0 moves at once.
    travel: f32,
    speed: f32,
    progress: f32,
    #[serde(with = "portal2_core::save::ids::opt_entity")]
    activator: Option<Entity>,
}

fn spawn_button(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e).cloned() else { return };
    let out_pose = world.get::<Transform>(e).copied().unwrap_or_default();
    let (in_pose, travel) = if me.has_flag(SF_DONT_MOVE) {
        (out_pose, 0.0)
    } else if me.classname == "func_rot_button" {
        let distance = me.get_f32("distance").unwrap_or(90.0);
        // Spawnflags pick the axis as for rotating doors: 64 = X, 128 = Y.
        let axis = if me.has_flag(64) {
            Vec3::X
        } else if me.has_flag(128) {
            Vec3::Y
        } else {
            Vec3::Z
        };
        let rot = Quat::from_axis_angle(axis, distance.to_radians());
        (Transform { rotation: out_pose.rotation * rot, ..out_pose }, distance.abs())
    } else {
        let movedir = angles_forward(me.get_vec3("movedir").unwrap_or(Vec3::ZERO));
        let size = world
            .get_resource::<CurrentMap>()
            .and_then(|m| me.brush_model().and_then(|i| m.bsp.models.get(i)))
            .map(|m| (m.maxs - m.mins).abs())
            .unwrap_or(Vec3::ZERO);
        let distance = (movedir.abs().dot(size) - me.get_f32("lip").unwrap_or(0.0)).max(0.0);
        (Transform { translation: out_pose.translation + movedir * distance, ..out_pose }, distance)
    };
    world.entity_mut(e).insert(BrushButton {
        phase: Phase::Out,
        locked: me.has_flag(SF_LOCKED),
        toggle: me.has_flag(SF_TOGGLE),
        use_activates: me.has_flag(SF_USE),
        touch_activates: me.has_flag(SF_TOUCH),
        wait: me.get_f32("wait").unwrap_or(3.0),
        return_at: None,
        out_pose,
        in_pose,
        travel,
        speed: me.get_f32("speed").filter(|s| *s > 0.0).unwrap_or(5.0),
        progress: 0.0,
        activator: None,
    });
}

/// A press: out (or returning) buttons go in, toggle buttons that are in
/// come back out; moving ones ignore it.
fn press(world: &mut World, e: Entity, activator: Option<Entity>) {
    let Some(mut b) = world.get_mut::<BrushButton>(e) else { return };
    if b.locked {
        fire_output(world, e, "OnUseLocked", activator, None);
        return;
    }
    let output = match b.phase {
        Phase::Out => {
            b.phase = Phase::GoingIn;
            b.activator = activator;
            true
        }
        Phase::In if b.toggle => {
            b.phase = Phase::GoingOut;
            b.return_at = None;
            b.activator = activator;
            true
        }
        _ => false,
    };
    if output {
        fire_output(world, e, "OnPressed", activator, None);
    }
}

fn button_input(world: &mut World, call: &InputCall) -> bool {
    let e = call.target;
    let Some(b) = world.get::<BrushButton>(e).cloned() else { return false };
    match call.input.as_str() {
        "use" => {
            if b.use_activates {
                press(world, e, call.activator);
            }
        }
        "press" => press(world, e, call.activator),
        "pressin" => {
            if matches!(b.phase, Phase::Out | Phase::GoingOut)
                && let Some(mut b) = world.get_mut::<BrushButton>(e)
            {
                b.phase = Phase::GoingIn;
                b.activator = call.activator;
                fire_output(world, e, "OnPressed", call.activator, None);
            }
        }
        "pressout" => {
            if matches!(b.phase, Phase::In | Phase::GoingIn)
                && let Some(mut b) = world.get_mut::<BrushButton>(e)
            {
                b.phase = Phase::GoingOut;
                b.return_at = None;
                b.activator = call.activator;
                fire_output(world, e, "OnPressed", call.activator, None);
            }
        }
        "lock" | "unlock" => {
            if let Some(mut b) = world.get_mut::<BrushButton>(e) {
                b.locked = call.input == "lock";
            }
        }
        _ => return false,
    }
    true
}

/// Touch-activated buttons press when the player brushes against them.
fn touch_buttons(world: &mut World) {
    let Some(player) = world.resource::<EntityRegistry>().player else { return };
    let Some(state) = world.get::<Player>(player).map(|p| p.state) else { return };
    let candidates: Vec<Entity> = world
        .query::<(Entity, &BrushButton)>()
        .iter(world)
        .filter(|(_, b)| b.touch_activates && b.phase == Phase::Out)
        .map(|(e, _)| e)
        .collect();
    if candidates.is_empty() {
        return;
    }
    let (mins, maxs) = state.hull();
    let touched: Vec<Entity> = {
        let mut sys: bevy::ecs::system::SystemState<Physics> = bevy::ecs::system::SystemState::new(world);
        let Ok(physics) = sys.get(world) else { return };
        let grow = Vec3::splat(1.0);
        candidates.into_iter().filter(|&e| physics.touches(e, state.origin, mins - grow, maxs + grow)).collect()
    };
    for e in touched {
        press(world, e, Some(player));
    }
}

fn move_buttons(clock: Res<GameClock>, mut buttons: Query<(Entity, &mut BrushButton, &mut Transform)>, mut out: MessageWriter<FireOutput>) {
    let dt = clock.frametime();
    for (e, mut b, mut t) in &mut buttons {
        if b.phase == Phase::In
            && let Some(at) = b.return_at
            && clock.due(at)
        {
            b.return_at = None;
            b.phase = Phase::GoingOut;
        }
        let target = match b.phase {
            Phase::GoingIn => 1.0,
            Phase::GoingOut => 0.0,
            _ => continue,
        };
        let step = if b.travel > 0.0 { b.speed * dt / b.travel } else { 1.0 };
        b.progress = if target > b.progress { (b.progress + step).min(1.0) } else { (b.progress - step).max(0.0) };
        t.translation = b.out_pose.translation.lerp(b.in_pose.translation, b.progress);
        t.rotation = b.out_pose.rotation.slerp(b.in_pose.rotation, b.progress);
        if (b.progress - target).abs() > 1e-6 {
            continue;
        }
        let activator = b.activator;
        if b.phase == Phase::GoingIn {
            b.phase = Phase::In;
            if !b.toggle && b.wait >= 0.0 {
                b.return_at = Some(clock.curtime + b.wait as f64);
            }
            out.write(FireOutput::new(e, "OnIn").with_activator(activator));
        } else {
            b.phase = Phase::Out;
            out.write(FireOutput::new(e, "OnOut").with_activator(activator));
        }
    }
}

impl portal2_core::save::Restore for BrushButton {
    fn keep(&mut self, live: Self) {
        self.out_pose = live.out_pose;
        self.in_pose = live.in_pose;
    }
}

pub fn register_save(r: &mut portal2_core::save::SaveRegistry) {
    r.component::<BrushButton>("brush_button");
}
