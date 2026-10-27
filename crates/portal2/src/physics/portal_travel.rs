//! Props moving through linked portals, and props dissolved by fizzler
//! fields.

use avian3d::prelude::*;
use bevy::prelude::*;

use super::carry::{Holding, release};
use crate::collision::{Collider as TraceCollider, Physics};
use crate::entities::io::FireOutput;
use crate::entities::props::PhysicsProp;
use crate::entities::triggers::{Trigger, TriggerKind};
use crate::portals::placement::transfer;
use crate::portals::{Portal, Portals};

pub struct PortalTravelPlugin;

impl Plugin for PortalTravelPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(FixedUpdate, (teleport_props, fizzle_props));
    }
}

/// Centre of the prop on the previous tick, for detecting portal crossings.
#[derive(Component, Default)]
pub struct PortalTraveler {
    pub previous_center: Option<Vec3>,
}

#[allow(clippy::type_complexity)]
fn teleport_props(
    portals: Res<Portals>,
    portal_query: Query<&Portal>,
    mut holding: ResMut<Holding>,
    mut commands: Commands,
    mut props: Query<(
        Entity,
        &TraceCollider,
        &mut Position,
        &mut Rotation,
        &mut LinearVelocity,
        &mut AngularVelocity,
        &mut PortalTraveler,
    )>,
) {
    let frames = match (portals.entities[0], portals.entities[1]) {
        (Some(a), Some(b)) => match (portal_query.get(a), portal_query.get(b)) {
            (Ok(a), Ok(b)) => Some([a.frame, b.frame]),
            _ => None,
        },
        _ => None,
    };
    for (e, col, mut pos, mut rot, mut lin, mut ang, mut traveler) in &mut props {
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
            info!("prop {e} went through portal {slot}: now at {:.0?} moving {:.0?}", new_center, lin.0);
            if holding.entity == Some(e) {
                holding.entity = None;
                release(&mut commands, e);
            }
            break;
        }
    }
}

/// Props touching an active fizzler field dissolve.
fn fizzle_props(
    physics: Physics,
    cleansers: Query<(Entity, &Trigger)>,
    props: Query<(Entity, &TraceCollider, &GlobalTransform), With<PhysicsProp>>,
    mut holding: ResMut<Holding>,
    mut out: MessageWriter<FireOutput>,
    mut commands: Commands,
) {
    for (te, trig) in &cleansers {
        if !trig.enabled || !matches!(trig.kind, TriggerKind::Cleanser) {
            continue;
        }
        for (pe, col, gt) in &props {
            let t = gt.compute_transform();
            let (mins, maxs) = super::world_bounds(&col.shape, t.translation, t.rotation);
            let center = (mins + maxs) * 0.5;
            let half = (maxs - mins) * 0.5;
            if physics.touches(te, center, -half, half) {
                if holding.entity == Some(pe) {
                    holding.entity = None;
                }
                out.write(FireOutput::new(pe, "OnFizzled"));
                out.write(FireOutput::new(te, "OnDissolve").with_activator(Some(pe)));
                commands.entity(pe).despawn();
            }
        }
    }
}
