//! Picking up, carrying and dropping physics props, plus pushing props by
//! walking into them. `+use` also uses non-prop entities.

use avian3d::prelude::*;
use bevy::prelude::*;

use super::{layers, layers_for};
use crate::collision::{Collider as TraceCollider, Physics, mask};
use crate::coords::{angles_forward, quat_to_angles};
use crate::entities::io::{EventQueue, EventTarget, FireOutput};
use crate::entities::props::PhysicsProp;
use crate::hud::instructor::{EventValue, GameEvent};
use crate::player::{Player, PlayerInput, PlayerMoveSet};

/// Horizontal radius of the player's hull, part of the hold distance.
const PLAYER_RADIUS: f32 = 22.627_417;
/// Reach for picking up and using.
const REACH: f32 = 80.0;
/// Top speed of the hold controller; larger implied speeds count half
/// toward the drop error.
const HOLD_MAX_SPEED: f32 = 1000.0;

pub struct CarryPlugin;

impl Plugin for CarryPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Holding>()
            .add_systems(Update, use_or_grab)
            .add_systems(FixedUpdate, (hold_held, push_props).after(PlayerMoveSet));
    }
}

#[derive(Resource, Default, serde::Serialize, serde::Deserialize)]
pub struct Holding {
    #[serde(with = "crate::save::ids::opt_entity")]
    pub entity: Option<Entity>,
    /// Prop yaw relative to the player's view when picked up (degrees).
    pub yaw_offset: f32,
    /// The held prop's bounding radius.
    #[serde(alias = "distance")]
    pub radius: f32,
    /// Weighted cubes ride lower than other props.
    #[serde(default)]
    pub cube: bool,
    /// Running average of how far the prop trails its hold point.
    #[serde(default)]
    pub error: f32,
    /// Height the prop is lifted to clear the floor under its hold point.
    #[serde(default)]
    pub lift: f32,
}

fn prop_radius(col: &TraceCollider) -> f32 {
    ((col.shape.maxs - col.shape.mins) * 0.5).length()
}

#[allow(clippy::too_many_arguments)]
fn use_or_grab(
    buttons: Res<crate::console::binds::Buttons>,
    mut holding: ResMut<Holding>,
    physics: Physics,
    players: Query<(Entity, &Player)>,
    input: Res<PlayerInput>,
    props: Query<(&TraceCollider, &GlobalTransform, &RigidBody, &crate::entities::MapEntity), With<PhysicsProp>>,
    mut queue: ResMut<EventQueue>,
    mut out: MessageWriter<FireOutput>,
    mut commands: Commands,
    mut auto: ResMut<crate::player::autopilot::AutoUse>,
    mut sounds: MessageWriter<crate::audio::SoundRequest>,
    mut events: MessageWriter<GameEvent>,
) {
    let pressed = buttons.just_pressed("use") || std::mem::take(&mut auto.0);
    if !pressed {
        return;
    }
    let Ok((pe, p)) = players.single() else { return };
    // `player_use` / `player_drop` for the instructor's carry lessons.
    let event = |name: &str, e: Entity| {
        GameEvent::new(name).with("userid", EventValue::Entity(Some(pe))).with("entity", EventValue::Entity(Some(e)))
    };
    if let Some(held) = holding.entity.take() {
        sounds.write(crate::audio::SoundRequest::stop("Weapon_Portalgun.HoldSound", pe));
        release(&mut commands, held);
        out.write(FireOutput::new(held, "OnPhysGunDrop").with_activator(Some(pe)));
        events.write(event("player_drop", held));
        return;
    }
    let eye = p.state.origin + Vec3::Z * p.state.eye_offset();
    let dir = angles_forward(input.input.angles);
    let r = Vec3::splat(2.0);
    let tr = physics.trace(eye, eye + dir * REACH, -r, r, mask::SOLID_ALL, &[pe]);
    debug!(
        "use: eye {eye:.0?} dir {dir:.2?} fraction {:.2} entity {:?} body {:?}",
        tr.fraction,
        tr.entity,
        tr.entity.map(|e| props.get(e).map(|p| *p.2))
    );
    let Some(target) = tr.entity else {
        sounds.write(crate::audio::SoundRequest::on("PortalPlayer.UseDeny", pe));
        return;
    };
    events.write(event("player_use", target));
    match props.get(target) {
        Ok((col, gt, body, me)) if *body == RigidBody::Dynamic => {
            let prop_yaw = quat_to_angles(gt.compute_transform().rotation).y;
            holding.entity = Some(target);
            holding.yaw_offset = prop_yaw - input.input.angles.y;
            holding.radius = prop_radius(col);
            holding.cube = me.classname == "prop_weighted_cube";
            holding.error = 0.0;
            holding.lift = 0.0;
            commands.entity(target).insert((GravityScale(0.0), layers_for(layers::HELD, layers::WORLD | layers::PROP | layers::MOVER)));
            sounds.write(crate::audio::SoundRequest::on("Weapon_Portalgun.HoldSound", pe));
            out.write(FireOutput::new(target, "OnPlayerPickup").with_activator(Some(pe)));
            out.write(FireOutput::new(target, "OnPhysGunPickup").with_activator(Some(pe)));
        }
        _ => queue.push(0.0, EventTarget::Entity(target), "use", "", Some(pe), Some(pe)),
    }
}

/// Puts a prop back in the player's hands (a restored saved game): no
/// gravity, held collision and the hold sound.
pub fn resume_hold(world: &mut World, prop: Entity) {
    if let Ok(mut e) = world.get_entity_mut(prop) {
        e.insert((GravityScale(0.0), layers_for(layers::HELD, layers::WORLD | layers::PROP | layers::MOVER)));
    }
    // Saves from before the radius was kept stored a hold distance instead.
    let radius = world.get::<TraceCollider>(prop).map(prop_radius);
    let cube = world.get::<crate::entities::MapEntity>(prop).is_some_and(|me| me.classname == "prop_weighted_cube");
    let mut holding = world.resource_mut::<Holding>();
    if let Some(r) = radius {
        holding.radius = r;
    }
    holding.cube = cube;
    if let Some(p) = world.resource::<crate::entities::EntityRegistry>().player {
        world.write_message(crate::audio::SoundRequest::on("Weapon_Portalgun.HoldSound", p));
    }
}

/// Restores normal physics on a prop that is no longer held.
pub fn release(commands: &mut Commands, prop: Entity) {
    if let Ok(mut e) = commands.get_entity(prop) {
        e.insert((
            GravityScale(1.0),
            layers_for(layers::PROP, layers::WORLD | layers::PROP | layers::PLAYER | layers::MOVER | layers::HELD),
        ));
    }
}

/// Where the held prop should be (`docs/re/gameplay.md`, carrying): along
/// the view (pitch limited to ±75°) far enough out to clear the player,
/// in a column in front of them when `player_hold_object_in_column`,
/// pulled in by walls and lifted off floors.
fn hold_point(
    physics: &Physics,
    ignore: &[Entity],
    console: &crate::console::Console,
    holding: &mut Holding,
    player: &crate::player::movement::MoveState,
    view: Vec3,
) -> Vec3 {
    let eye = player.origin + Vec3::Z * player.eye_offset();
    let mut view = view;
    view.x = view.x.clamp(-75.0, 75.0);
    let q = crate::coords::angles_to_quat(view);
    let (forward, up) = (q * Vec3::X, q * Vec3::Z);
    let gap = console.f32("player_held_object_distance");
    let radii = holding.radius + PLAYER_RADIUS;
    let mut distance = gap + radii;
    if console.bool("player_hold_object_in_column") {
        // How far along the view a vertical plane that far ahead lies.
        let flat = forward.with_z(0.0).normalize_or_zero();
        let along = forward.dot(flat);
        let reach = if along > 1e-4 { distance / along } else { f32::MAX };
        distance = reach.clamp(gap, console.f32("player_hold_column_max_size"));
    }
    let raise = if holding.cube { console.f32("player_held_object_offset_up_cube") * (1.0 - view.x.abs() / 75.0) } else { 0.0 };
    let end = eye + forward * distance + up * raise;
    let mask = mask::SOLID_BRUSH_ONLY;
    let tr = physics.trace(eye, end, Vec3::ZERO, Vec3::ZERO, mask, ignore);
    let dir = (end - eye).normalize_or(forward);
    let mut target = eye + dir * (tr.fraction * distance).max(radii) + up * raise;
    // Keep the prop above a floor under the hold point.
    let half = radii * 0.5 + 1.0;
    let down = physics.trace(target + Vec3::Z * half, target - Vec3::Z * half, Vec3::ZERO, Vec3::ZERO, mask, ignore);
    if !down.start_solid {
        holding.lift = if down.fraction >= 1.0 { 0.0 } else { (1.0 - down.fraction) * radii };
    }
    target.z += holding.lift;
    // Never closer to the player's vertical axis than the two radii.
    let (mins, maxs) = player.hull();
    let axis = (player.origin + Vec3::Z * mins.z, player.origin + Vec3::Z * maxs.z);
    let along = ((target - axis.0).dot(Vec3::Z) / (axis.1 - axis.0).length()).clamp(0.0, 1.0);
    let nearest = axis.0.lerp(axis.1, along);
    let away = target - nearest;
    if away.length() < radii {
        target = nearest + away.normalize_or(forward.with_z(0.0).normalize_or(Vec3::X)) * radii;
    }
    target
}

/// Steers the held prop toward its hold point, dropping it when it trails
/// too far behind for too long.
#[allow(clippy::too_many_arguments)]
fn hold_held(
    time: Res<Time<Fixed>>,
    mut holding: ResMut<Holding>,
    physics: Physics,
    console: Res<crate::console::Console>,
    players: Query<(Entity, &Player)>,
    input: Res<PlayerInput>,
    mut props: Query<(&Position, &Rotation, &mut LinearVelocity, &mut AngularVelocity)>,
    mut commands: Commands,
) {
    let Some(held) = holding.entity else { return };
    let Ok((pe, p)) = players.single() else { return };
    let Ok((pos, rot, mut lin, mut ang)) = props.get_mut(held) else {
        holding.entity = None;
        return;
    };
    let dt = time.delta_secs().max(1e-4);
    let eye = p.state.origin + Vec3::Z * p.state.eye_offset();
    let ignore = [pe, held];
    let target = hold_point(&physics, &ignore, &console, &mut holding, &p.state, input.input.angles);
    let error = target - pos.0;

    // The drop error averages the trailing distance over about a second;
    // it triples each tick the prop is out of sight behind something.
    let t = dt.min(1.0);
    let mut d = error.length();
    if d / t > HOLD_MAX_SPEED {
        d *= 0.5;
    }
    holding.error = (1.0 - t) * holding.error + t * d;
    if physics.trace(eye, pos.0, Vec3::ZERO, Vec3::ZERO, mask::SOLID_BRUSH_ONLY, &ignore).fraction < 1.0 {
        holding.error *= 3.0;
    }
    trace!("hold: target {target:.0?} at {:.0?} lift {:.1} error {:.1}", pos.0, holding.lift, holding.error);
    let limit = if console.bool("player_held_object_collide_with_player") { 12.0 } else { 40.0 };
    if holding.error > limit || p.state.ground_entity == Some(held) {
        holding.entity = None;
        release(&mut commands, held);
        return;
    }
    let wanted = error / dt * 0.5;
    lin.0 = wanted.clamp_length_max(2000.0);
    let target_rot = Quat::from_rotation_z((input.input.angles.y + holding.yaw_offset).to_radians());
    let (axis, angle) = (target_rot * rot.0.inverse()).normalize().to_axis_angle();
    let angle = if angle > std::f32::consts::PI { angle - std::f32::consts::TAU } else { angle };
    ang.0 = axis * (angle / dt * 0.3);
}

/// Walking into a prop gives it a shove along the walking direction.
fn push_props(
    physics: Physics,
    holding: Res<Holding>,
    players: Query<(Entity, &Player)>,
    mut props: Query<(&RigidBody, &mut LinearVelocity), With<PhysicsProp>>,
) {
    let Ok((pe, p)) = players.single() else { return };
    let v = p.state.velocity.with_z(0.0);
    let speed = v.length();
    if speed < 10.0 || p.noclip {
        return;
    }
    let dir = v / speed;
    let (mins, maxs) = p.state.hull();
    let start = p.state.origin + Vec3::Z * 2.0;
    let tr = physics.trace(start, start + dir * 4.0, mins, maxs, mask::PLAYER_SOLID, &[pe]);
    let Some(hit) = tr.entity else { return };
    if holding.entity == Some(hit) {
        return;
    }
    if let Ok((body, mut lin)) = props.get_mut(hit)
        && *body == RigidBody::Dynamic
    {
        let along = lin.0.dot(dir);
        let want = speed * 0.6;
        if along < want {
            lin.0 += dir * (want - along);
        }
    }
}

impl crate::save::Restore for Holding {}
