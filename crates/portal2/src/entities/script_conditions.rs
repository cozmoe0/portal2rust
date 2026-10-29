//! `ai_script_conditions`: waits until the player meets every condition set
//! on it, relative to its `target` entity (in view cone, within range, line
//! of sight), for `RequiredTime` seconds and then fires
//! `OnConditionsSatisfied`; `OnConditionsTimeout` fires instead once
//! `MinTimeout` (or a random time up to `MaxTimeout`) has passed. The maps
//! use it to wait for the player to look at something. NPC actor conditions
//! are not evaluated (no single-player map names an actor). Facts in
//! docs/re/script_conditions.md.

use bevy::prelude::*;
use source_formats::bsp::contents;

use super::io::{FireOutput, InputCall, InputHandlers};
use super::{EntityRegistry, MapEntity, SpawnHandlers};
use crate::clock::{GameClock, ThinkSet};
use crate::collision::Physics;
use crate::coords::angles_forward;
use crate::player::{Player, PlayerInput};

pub fn register(app: &mut App) {
    app.add_systems(FixedUpdate, evaluate.in_set(ThinkSet));
    let world = app.world_mut();
    world.resource_mut::<SpawnHandlers>().register("ai_script_conditions", spawn);
    world.resource_mut::<InputHandlers>().register("ai_script_conditions", input);
}

pub fn register_save(r: &mut crate::save::SaveRegistry) {
    r.component::<ScriptConditions>("script_conditions");
}

/// Seconds between evaluations.
const THINK_INTERVAL: f64 = 0.25;
/// Slack when comparing times, as the game does.
const TIME_SLACK: f64 = 0.001;
const LOS_MASK: u32 = contents::SOLID | contents::MOVEABLE | contents::BLOCKLOS;

/// Three-way keyvalue: the condition must be false, true, or is ignored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum Tristate {
    No,
    Yes,
    DontCare,
}

impl Tristate {
    fn parse(v: Option<i32>) -> Self {
        match v {
            Some(0) => Self::No,
            Some(1) => Self::Yes,
            _ => Self::DontCare,
        }
    }
}

/// One evaluation in progress (the game keeps one per actor; without an
/// actor there is a single one).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct Watch {
    /// When the conditions, holding until then, are satisfied.
    satisfied_at: f64,
    timeout_at: f64,
}

#[derive(Component, Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ScriptConditions {
    disabled: bool,
    next_think: f64,
    watches: Vec<Watch>,
    target_name: Option<String>,
    #[serde(with = "crate::save::ids::opt_entity")]
    target: Option<Entity>,
    required_time: f32,
    min_timeout: f32,
    max_timeout: f32,
    /// Player to target distance: positive within, negative beyond, 0 any.
    proximity: f32,
    /// Full view cone in degrees: 360 any, negative outside the cone.
    fov: f32,
    true_cone: bool,
    los: Tristate,
}

fn spawn(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e) else { return };
    let c = ScriptConditions {
        disabled: true,
        next_think: 0.0,
        watches: Vec::new(),
        target_name: me.get("target").filter(|s| !s.is_empty()).map(str::to_ascii_lowercase),
        target: None,
        required_time: me.get_f32("requiredtime").unwrap_or(0.0),
        min_timeout: me.get_f32("mintimeout").unwrap_or(0.0),
        max_timeout: me.get_f32("maxtimeout").unwrap_or(0.0),
        proximity: me.get_f32("playertargetproximity").unwrap_or(0.0),
        fov: me.get_f32("playertargetfov").unwrap_or(360.0),
        true_cone: me.get_bool("playertargetfovtruecone"),
        los: Tristate::parse(me.get_i32("playertargetlos")),
    };
    let start = !me.get_bool("startdisabled");
    world.entity_mut(e).insert(c);
    if start {
        enable(world, e);
    }
}

/// Starts (or resumes) evaluating: looks the target up again and adds the
/// watch unless one is still pending from before a `Disable`.
fn enable(world: &mut World, e: Entity) {
    let now = world.resource::<GameClock>().curtime;
    let target = {
        let Some(c) = world.get::<ScriptConditions>(e) else { return };
        c.target_name.as_ref().and_then(|n| world.resource::<EntityRegistry>().by_name.get(n)).and_then(|l| l.first().copied())
    };
    let Some(mut c) = world.get_mut::<ScriptConditions>(e) else { return };
    c.target = target;
    if c.watches.is_empty() {
        let timeout = if c.max_timeout > 0.0 { crate::audio::ops::random_range(c.min_timeout, c.max_timeout) } else { c.min_timeout };
        let satisfied_at = if c.required_time > 0.0 { now + c.required_time as f64 } else { -1.0 };
        c.watches.push(Watch { satisfied_at, timeout_at: now + timeout as f64 });
    }
    c.disabled = false;
    c.next_think = now + THINK_INTERVAL;
}

fn input(world: &mut World, call: &InputCall) -> bool {
    match call.input.as_str() {
        "enable" => enable(world, call.target),
        "disable" => {
            let Some(mut c) = world.get_mut::<ScriptConditions>(call.target) else { return false };
            c.disabled = true;
        }
        _ => return false,
    }
    true
}

/// Whether `viewed` lies in the cone of `fov` degrees around the view
/// direction: in 3D with `true_cone`, otherwise in the horizontal plane.
fn in_view_cone(eye: Vec3, angles: Vec3, viewed: Vec3, fov: f32, true_cone: bool) -> bool {
    if fov >= 360.0 {
        return true;
    }
    let to = viewed - eye;
    let dot = if true_cone {
        angles_forward(angles).dot(to / (to.length() + f32::EPSILON))
    } else {
        let look = angles_forward(Vec3::new(0.0, angles.y, 0.0)).truncate();
        look.dot(to.truncate().normalize_or_zero())
    };
    if dot >= (fov * 0.5).to_radians().cos() { fov >= 0.0 } else { fov < 0.0 }
}

#[allow(clippy::too_many_arguments)]
fn evaluate(
    clock: Res<GameClock>,
    physics: Physics,
    input: Res<PlayerInput>,
    players: Query<(Entity, &Player)>,
    transforms: Query<&GlobalTransform>,
    mut conditions: Query<(Entity, &mut ScriptConditions)>,
    mut out: MessageWriter<FireOutput>,
) {
    let now = clock.curtime;
    let player = players.single().ok();
    for (e, mut c) in &mut conditions {
        if c.disabled || !clock.due(c.next_think) {
            continue;
        }
        c.next_think = now + THINK_INTERVAL;
        let target = c.target.and_then(|t| transforms.get(t).ok().map(|gt| (t, gt.translation())));
        // Conditions about a missing player or target hold.
        let holds = match (player, target) {
            (Some((pe, p)), Some((te, target_pos))) => {
                let eye = p.state.origin + Vec3::Z * p.state.eye_offset();
                let near = c.proximity == 0.0 || {
                    let inside = p.state.origin.distance_squared(target_pos) < c.proximity * c.proximity;
                    inside == (c.proximity > 0.0)
                };
                let looking = in_view_cone(eye, input.input.angles, target_pos, c.fov, c.true_cone);
                let sight = c.los == Tristate::DontCare
                    || (physics.trace(target_pos, eye, Vec3::ZERO, Vec3::ZERO, LOS_MASK, &[te, pe]).fraction >= 1.0)
                        == (c.los == Tristate::Yes);
                near && looking && sight
            }
            _ => true,
        };
        let (required, timed) = (c.required_time as f64, c.min_timeout > 0.0);
        let mut fired = Vec::new();
        for w in &mut c.watches {
            if timed && now - w.timeout_at > -TIME_SLACK {
                fired.push("OnConditionsTimeout");
            } else if !holds {
                w.satisfied_at = now + required;
            } else if now - w.satisfied_at > -TIME_SLACK {
                fired.push("OnConditionsSatisfied");
            }
        }
        let all_done = !c.watches.is_empty() && fired.len() == c.watches.len();
        for output in fired {
            out.write(FireOutput::new(e, output).with_activator(Some(e)));
        }
        if all_done {
            c.disabled = true;
            c.watches.clear();
        }
    }
}

impl crate::save::Restore for ScriptConditions {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_cone_matches_the_game_rules() {
        let eye = Vec3::ZERO;
        let ahead = Vec3::new(100.0, 0.0, 0.0);
        let above = Vec3::new(0.0, 0.0, 100.0);
        // Looking straight ahead; the ceiling is 90 degrees off.
        assert!(in_view_cone(eye, Vec3::ZERO, ahead, 110.0, true));
        assert!(!in_view_cone(eye, Vec3::ZERO, above, 110.0, true));
        // Pitch -80 (looking up) puts it inside a 110 degree cone.
        assert!(in_view_cone(eye, Vec3::new(-80.0, 0.0, 0.0), above, 110.0, true));
        // A negative cone passes only outside it.
        assert!(!in_view_cone(eye, Vec3::ZERO, ahead, -100.0, true));
        assert!(in_view_cone(eye, Vec3::new(0.0, 180.0, 0.0), ahead, -100.0, true));
        // The flat test ignores pitch.
        assert!(in_view_cone(eye, Vec3::new(-80.0, 0.0, 0.0), Vec3::new(100.0, 0.0, 50.0), 60.0, false));
        assert!(in_view_cone(eye, Vec3::ZERO, above, 360.0, true));
    }
}
