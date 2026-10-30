//! Props moving through linked portals, and props dissolved by fizzler
//! fields.

use avian3d::prelude::*;
use bevy::prelude::*;

use super::carry::Holding;
use crate::entities::io::FireOutput;
use crate::entities::props::PhysicsProp;
use crate::entities::triggers::{Trigger, TriggerKind};
use crate::portals::{Portal, Portals};
use bevy_transform_interpolation::{RotationEasingState, TranslationEasingState};
use portal2_world::collision::portal::{PortalFrame, transfer};
use portal2_world::collision::{Collider as TraceCollider, Physics};

pub struct PortalTravelPlugin;

impl Plugin for PortalTravelPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(FixedUpdate, (funnel_props, teleport_props, fizzle_props, run_dissolves).chain());
    }
}

/// Props dropping slowly onto a floor portal drift over its centre; rising
/// back out of one, sideways motion away from its centre is slowed
/// (`sv_props_funnel_into_portals`, props with `allowfunnel`).
fn funnel_props(
    time: Res<Time<Fixed>>,
    console: Res<portal2_core::console::Console>,
    portals: Res<Portals>,
    portal_query: Query<&Portal>,
    mut props: Query<(&portal2_core::entity::MapEntity, &Position, &RigidBody, &mut LinearVelocity), With<PhysicsProp>>,
) {
    if !console.bool("sv_props_funnel_into_portals") {
        return;
    }
    let Some(frames) = crate::portals::linked_frames(&portals, &portal_query) else { return };
    let decelerate = console.f32("sv_props_funnel_into_portals_deceleration");
    for (me, pos, body, mut lin) in &mut props {
        if !body.is_dynamic() || !me.get("allowfunnel").is_none_or(|v| source_formats::kv::parse_i32_lenient(v) != 0) {
            continue;
        }
        let v = lin.0;
        let flat_speed2 = v.truncate().length_squared();
        if v.z.abs() < 1.0 || flat_speed2 >= 64.0 * 64.0 {
            continue;
        }
        let Some((portal, delta)) = floor_portal_below(&frames, pos.0) else { continue };
        if v.z < 0.0 {
            let slow = (1.0 - flat_speed2 / (128.0 * 128.0)).clamp(0.0, 1.0);
            let high = ((delta.z + 128.0) / -128.0).clamp(0.0, 1.0);
            let k = slow * high;
            lin.0.x += (delta.x - v.x) * k;
            lin.0.y += (delta.y - v.y) * k;
        } else if decelerate > 0.0 {
            let n = portal.forward;
            let flat_v = v - n * n.dot(v);
            let flat_d = delta - n * n.dot(delta);
            let away = flat_d.dot(flat_v);
            if away < 0.0 {
                let amount = (-away / flat_d.length()).min(time.delta_secs() * decelerate);
                lin.0 += flat_d * amount;
            }
        }
    }
}

/// The closest open floor portal under `at` whose opening, grown by half,
/// `at` lies over, with the offset from `at` to its centre.
fn floor_portal_below(
    frames: &[portal2_world::collision::portal::PortalFrame; 2],
    at: Vec3,
) -> Option<(portal2_world::collision::portal::PortalFrame, Vec3)> {
    use portal2_world::collision::portal::{HALF_HEIGHT, HALF_WIDTH};
    let mut best: Option<(f32, portal2_world::collision::portal::PortalFrame, Vec3)> = None;
    for p in frames {
        let delta = p.center - at;
        if p.forward.z <= 0.8 || delta.z < -1024.0 || delta.z >= 0.0 {
            continue;
        }
        let along = |axis: Vec3| delta.truncate().dot(axis.truncate().normalize_or_zero()).abs();
        if along(p.left()) > HALF_WIDTH * 1.5 || along(p.up) > HALF_HEIGHT * 1.5 {
            continue;
        }
        let d2 = delta.length_squared();
        if best.as_ref().is_none_or(|(b, _, _)| d2 < *b) {
            best = Some((d2, *p, delta));
        }
    }
    best.map(|(_, p, d)| (p, d))
}

/// Centre of the prop on the previous tick, for detecting portal crossings.
#[derive(Component, Default)]
pub struct PortalTraveler {
    pub previous_center: Option<Vec3>,
}

/// A physics prop's placement and motion, to carry through a portal.
pub type TravelBody = (
    &'static mut Position,
    &'static mut Rotation,
    &'static mut LinearVelocity,
    &'static mut AngularVelocity,
    &'static mut PortalTraveler,
    Option<&'static mut TranslationEasingState>,
    Option<&'static mut RotationEasingState>,
);

/// Brings a carried prop out of portal `to` (just in front of the opening,
/// turned by the portal's transfer `m`) when the player goes through with
/// it still on the near side; the hold then steers it to their hands. It
/// is drawn there straight away rather than eased across.
pub fn carry_through(body: bevy::ecs::query::QueryItem<'_, '_, TravelBody>, m: &bevy::math::Affine3A, to: &PortalFrame, radius: f32) {
    let (mut pos, mut rot, mut lin, mut ang, mut traveler, eased_at, eased_turn) = body;
    let turn = Quat::from_mat3a(&m.matrix3);
    pos.0 = to.center + to.forward * (radius + 1.0);
    rot.0 = (turn * rot.0).normalize();
    lin.0 = Vec3::ZERO;
    ang.0 = Vec3::ZERO;
    traveler.previous_center = None;
    if let Some(mut state) = eased_at {
        state.start = None;
    }
    if let Some(mut state) = eased_turn {
        state.start = None;
    }
}

fn teleport_props(
    portals: Res<Portals>,
    portal_query: Query<&Portal>,
    mut holding: ResMut<Holding>,
    mut props: Query<(
        Entity,
        &TraceCollider,
        &mut Position,
        &mut Rotation,
        &mut LinearVelocity,
        &mut AngularVelocity,
        &mut PortalTraveler,
        Option<&mut TranslationEasingState>,
        Option<&mut RotationEasingState>,
    )>,
) {
    let frames = match (portals.entities[0], portals.entities[1]) {
        (Some(a), Some(b)) => match (portal_query.get(a), portal_query.get(b)) {
            (Ok(a), Ok(b)) => Some([a.frame, b.frame]),
            _ => None,
        },
        _ => None,
    };
    for (e, col, mut pos, mut rot, mut lin, mut ang, mut traveler, mut eased_at, mut eased_turn) in &mut props {
        let local_center = (col.shape.mins + col.shape.maxs) * 0.5;
        let center = pos.0 + rot.0 * local_center;
        let previous = traveler.previous_center.replace(center);
        let (Some(frames), Some(before)) = (frames, previous) else { continue };
        for slot in 0..2 {
            let (from, to) = (frames[slot], frames[1 - slot]);
            let (lb, la) = (from.to_local(before), from.to_local(center));
            if !(lb.x >= 0.0 && la.x < 0.0) {
                continue;
            }
            let t = lb.x / (lb.x - la.x);
            if !from.in_opening(lb.lerp(la, t), 0.0) {
                continue;
            }
            let m = transfer(&from, &to);
            let turn = Quat::from_mat3a(&m.matrix3);
            let new_center = m.transform_point3(center);
            rot.0 = (turn * rot.0).normalize();
            pos.0 = new_center - rot.0 * local_center;
            lin.0 = turn * lin.0;
            ang.0 = turn * ang.0;
            traveler.previous_center = Some(new_center);
            // Drawn at the exit straight away, not eased across from the
            // entrance.
            if let Some(state) = eased_at.as_deref_mut() {
                state.start = None;
            }
            if let Some(state) = eased_turn.as_deref_mut() {
                state.start = None;
            }
            info!("prop {e} went through portal {slot}: now at {:.0?} moving {:.0?}", new_center, lin.0);
            // A carried prop stays in the player's hands.
            if holding.entity == Some(e) {
                holding.prop_crossed(slot);
            }
            break;
        }
    }
}

/// Seconds a fizzled prop takes to dissolve, and when its model vanishes.
const DISSOLVE_TIME: f64 = 2.0;
const DISSOLVE_MODEL_GONE: f64 = 1.95;

/// A fizzled prop: no longer a prop the map can address or the player can
/// touch, drifting without gravity until it has dissolved.
#[derive(Component)]
pub struct Dissolving {
    pub since: f64,
}

/// Props touching an active fizzler field dissolve: the game swaps them for
/// a non-solid copy that keeps half their velocity (half the player's,
/// plus a little forward and down, when carried), spins 20°/s more about
/// the vertical, ignores gravity and is gone after 2 seconds.
fn fizzle_props(
    clock: Res<portal2_core::clock::GameClock>,
    physics: Physics,
    cleansers: Query<(Entity, &Trigger)>,
    mut props: Query<(Entity, &TraceCollider, &GlobalTransform, &mut LinearVelocity, &mut AngularVelocity), With<PhysicsProp>>,
    players: Query<&crate::player::Player>,
    input: Res<crate::player::PlayerInput>,
    mut holding: ResMut<Holding>,
    mut registry: ResMut<portal2_core::entity::EntityRegistry>,
    mut out: MessageWriter<FireOutput>,
    mut sounds: MessageWriter<portal2_audio::SoundRequest>,
    mut commands: Commands,
) {
    for (te, trig) in &cleansers {
        if !trig.enabled || !matches!(trig.kind, TriggerKind::Cleanser) {
            continue;
        }
        for (pe, col, gt, mut lin, mut ang) in &mut props {
            let t = gt.compute_transform();
            let (mins, maxs) = super::world_bounds(&col.shape, t.translation, t.rotation);
            let center = (mins + maxs) * 0.5;
            let half = (maxs - mins) * 0.5;
            if !physics.touches(te, center, -half, half) {
                continue;
            }
            let mut drift = lin.0;
            if holding.entity == Some(pe) {
                holding.entity = None;
                if let Ok(p) = players.single() {
                    let forward = portal2_core::coords::angles_forward(input.input.angles);
                    drift = p.state.velocity + forward * 4.0 - Vec3::Z * 32.0;
                }
            }
            out.write(FireOutput::new(pe, "OnFizzled"));
            out.write(FireOutput::new(te, "OnDissolve").with_activator(Some(pe)));
            sounds.write(portal2_audio::SoundRequest::at("Prop.Fizzled", center));
            lin.0 = drift * 0.5;
            ang.0 += Vec3::Z * 20f32.to_radians();
            registry.remove(pe);
            commands
                .entity(pe)
                .remove::<PhysicsProp>()
                .insert((Dissolving { since: clock.curtime }, GravityScale(0.0), CollisionLayers::NONE))
                // The copy is not solid to traces either.
                .queue(|mut e: EntityWorldMut| {
                    if let Some(mut c) = e.get_mut::<TraceCollider>() {
                        c.solid = false;
                    }
                });
        }
    }
}

fn run_dissolves(
    clock: Res<portal2_core::clock::GameClock>,
    mut dissolving: Query<(Entity, &Dissolving, &mut Visibility)>,
    mut commands: Commands,
) {
    for (e, d, mut vis) in &mut dissolving {
        let age = clock.curtime - d.since;
        if age >= DISSOLVE_TIME {
            commands.entity(e).despawn();
        } else if age >= DISSOLVE_MODEL_GONE && *vis != Visibility::Hidden {
            *vis = Visibility::Hidden;
        }
    }
}
