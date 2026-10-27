//! Picking up, carrying and dropping physics props, plus pushing props by
//! walking into them. `+use` also uses non-prop entities.

use avian3d::prelude::*;
use bevy::prelude::*;

use super::{layers, layers_for};
use crate::collision::{Collider as TraceCollider, Physics, mask};
use crate::coords::{angles_forward, quat_to_angles};
use crate::entities::io::{EventQueue, EventTarget, FireOutput};
use crate::entities::props::PhysicsProp;
use crate::player::{Player, PlayerInput, PlayerMoveSet};

/// How far in front of the player's hull a held prop floats
/// (`player_held_object_distance`).
const HELD_GAP: f32 = 15.0;
/// Held cubes ride a little below eye level (`player_held_object_offset_up_cube`).
const HELD_OFFSET_UP: f32 = -10.0;
/// Reach for picking up and using.
const REACH: f32 = 80.0;
/// A held prop this far from where it should be is dropped.
const BREAK_DISTANCE: f32 = 128.0;

pub struct CarryPlugin;

impl Plugin for CarryPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Holding>()
            .add_systems(Update, use_or_grab)
            .add_systems(FixedUpdate, (hold_held, push_props).after(PlayerMoveSet));
    }
}

#[derive(Resource, Default)]
pub struct Holding {
    pub entity: Option<Entity>,
    /// Prop yaw relative to the player's view when picked up (degrees).
    pub yaw_offset: f32,
    pub distance: f32,
}

fn prop_radius(col: &TraceCollider) -> f32 {
    ((col.shape.maxs - col.shape.mins) * 0.5).truncate().length()
}

#[allow(clippy::too_many_arguments)]
fn use_or_grab(
    buttons: Res<crate::console::binds::Buttons>,
    mut holding: ResMut<Holding>,
    physics: Physics,
    players: Query<(Entity, &Player)>,
    input: Res<PlayerInput>,
    props: Query<(&TraceCollider, &GlobalTransform, &RigidBody), With<PhysicsProp>>,
    mut queue: ResMut<EventQueue>,
    mut out: MessageWriter<FireOutput>,
    mut commands: Commands,
    mut auto: ResMut<crate::player::autopilot::AutoUse>,
    mut sounds: MessageWriter<crate::audio::SoundRequest>,
) {
    let pressed = buttons.just_pressed("use") || std::mem::take(&mut auto.0);
    if !pressed {
        return;
    }
    let Ok((pe, p)) = players.single() else { return };
    if let Some(held) = holding.entity.take() {
        sounds.write(crate::audio::SoundRequest::stop("Weapon_Portalgun.HoldSound", pe));
        release(&mut commands, held);
        out.write(FireOutput::new(held, "OnPhysGunDrop").with_activator(Some(pe)));
        return;
    }
    let eye = p.state.origin + Vec3::Z * p.state.eye_offset();
    let dir = angles_forward(input.input.angles);
    let r = Vec3::splat(2.0);
    let tr = physics.trace(eye, eye + dir * REACH, -r, r, mask::SOLID_ALL, &[pe]);
    debug!("use: eye {eye:.0?} dir {dir:.2?} fraction {:.2} entity {:?} body {:?}", tr.fraction, tr.entity, tr.entity.map(|e| props.get(e).map(|p| *p.2)));
    let Some(target) = tr.entity else {
        sounds.write(crate::audio::SoundRequest::on("PortalPlayer.UseDeny", pe));
        return;
    };
    match props.get(target) {
        Ok((col, gt, body)) if *body == RigidBody::Dynamic => {
            let prop_yaw = quat_to_angles(gt.compute_transform().rotation).y;
            holding.entity = Some(target);
            holding.yaw_offset = prop_yaw - input.input.angles.y;
            holding.distance = 16.0 + HELD_GAP + prop_radius(col);
            commands.entity(target).insert((
                GravityScale(0.0),
                layers_for(layers::HELD, layers::WORLD | layers::PROP | layers::MOVER),
            ));
            sounds.write(crate::audio::SoundRequest::on("Weapon_Portalgun.HoldSound", pe));
            out.write(FireOutput::new(target, "OnPlayerPickup").with_activator(Some(pe)));
            out.write(FireOutput::new(target, "OnPhysGunPickup").with_activator(Some(pe)));
        }
        _ => queue.push(0.0, EventTarget::Entity(target), "use", "", Some(pe), Some(pe)),
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

/// Steers the held prop toward a point in front of the player's view.
#[allow(clippy::type_complexity)]
fn hold_held(
    time: Res<Time<Fixed>>,
    mut holding: ResMut<Holding>,
    players: Query<&Player>,
    input: Res<PlayerInput>,
    mut props: Query<(&Position, &Rotation, &mut LinearVelocity, &mut AngularVelocity)>,
    mut commands: Commands,
) {
    let Some(held) = holding.entity else { return };
    let Ok(p) = players.single() else { return };
    let Ok((pos, rot, mut lin, mut ang)) = props.get_mut(held) else {
        holding.entity = None;
        return;
    };
    let dt = time.delta_secs().max(1e-4);
    let mut view = input.input.angles;
    // Looking far up or down holds the prop in a column in front of the player.
    view.x = view.x.clamp(-60.0, 75.0);
    let eye = p.state.origin + Vec3::Z * p.state.eye_offset();
    let target = eye + angles_forward(view) * holding.distance + Vec3::Z * HELD_OFFSET_UP;
    let error = target - pos.0;
    if error.length() > BREAK_DISTANCE {
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
