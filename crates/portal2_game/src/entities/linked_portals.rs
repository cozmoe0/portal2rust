//! `linked_portal_door`: a fixed pair of portal openings placed by the map
//! (`sp_a1_wakeup`'s incinerator drops the player through one into the
//! shaft that ends the level; `sp_a4_finale2` has two pairs).
//!
//! Each door is a `width` x `height` opening facing along its angles; it
//! passes the player while it is open and linked to a partner
//! (`partnername` or `SetPartner`, either side naming the other). The game
//! carves the wall behind an open door like a player portal; here the player
//! is sent through as soon as their box reaches the opening from the front,
//! which is all a solid wall behind it would otherwise stop. The view
//! through the opening is not drawn, and props are not sent through.

use bevy::math::{Affine3A, Mat3};
use bevy::prelude::*;

use super::io::{InputCall, InputHandlers};
use super::{EntityRegistry, MapEntity, SpawnHandlers, world_transform};
use crate::player::{Player, PlayerInput};
use portal2_core::coords::{angles_forward, angles_to_quat, quat_to_angles};
use portal2_core::sets::PlayerMoveSet;

pub fn register(app: &mut App) {
    app.add_systems(FixedUpdate, pass_player.after(PlayerMoveSet));
    let world = app.world_mut();
    world.resource_mut::<SpawnHandlers>().register("linked_portal_door", spawn_door);
    world.resource_mut::<InputHandlers>().register("linked_portal_door", door_input);
}

#[derive(Component, Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct LinkedDoor {
    pub partner: Option<String>,
    pub open: bool,
    pub half_width: f32,
    pub half_height: f32,
}

impl portal2_core::save::Restore for LinkedDoor {}

fn spawn_door(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e) else { return };
    let door = LinkedDoor {
        partner: me.get("partnername").filter(|s| !s.is_empty()).map(str::to_ascii_lowercase),
        open: me.get_bool("startactive"),
        half_width: me.get_f32("width").unwrap_or(128.0) * 0.5,
        half_height: me.get_f32("height").unwrap_or(128.0) * 0.5,
    };
    world.entity_mut(e).insert(door);
}

fn door_input(world: &mut World, call: &InputCall) -> bool {
    let Some(mut door) = world.get_mut::<LinkedDoor>(call.target) else { return false };
    match call.input.as_str() {
        "open" => door.open = true,
        "close" => door.open = false,
        "toggle" => door.open = !door.open,
        "setpartner" => door.partner = Some(call.param.trim().to_ascii_lowercase()).filter(|s| !s.is_empty()),
        _ => return false,
    }
    true
}

/// A door's opening: centre, facing (out of the opening) and up.
#[derive(Clone, Copy)]
struct Opening {
    center: Vec3,
    forward: Vec3,
    up: Vec3,
    half_width: f32,
    half_height: f32,
}

impl Opening {
    fn rotation(&self) -> Quat {
        Quat::from_mat3(&Mat3::from_cols(self.forward, self.up.cross(self.forward), self.up)).normalize()
    }

    fn affine(&self) -> Affine3A {
        Affine3A::from_rotation_translation(self.rotation(), self.center)
    }
}

/// Maps the front of `from` onto the front of `to`, as a player portal does.
fn transfer(from: &Opening, to: &Opening) -> Affine3A {
    to.affine() * Affine3A::from_rotation_z(std::f32::consts::PI) * from.affine().inverse()
}

fn opening(world: &World, e: Entity, door: &LinkedDoor) -> Opening {
    let t = world_transform(world, e).compute_transform();
    Opening {
        center: t.translation,
        forward: t.rotation * Vec3::X,
        up: t.rotation * Vec3::Z,
        half_width: door.half_width,
        half_height: door.half_height,
    }
}

/// Open doors whose partner is open too, as (entry, exit) pairs both ways.
fn linked_pairs(world: &mut World) -> Vec<(Opening, Opening)> {
    let mut q = world.query::<(Entity, &LinkedDoor)>();
    let doors: Vec<(Entity, LinkedDoor)> = q.iter(world).map(|(e, d)| (e, d.clone())).collect();
    let registry = world.resource::<EntityRegistry>();
    let name_of = |e: Entity| world.get::<MapEntity>(e).and_then(|m| m.targetname.clone()).map(|n| n.to_ascii_lowercase());
    let mut pairs = Vec::new();
    for (a, door_a) in &doors {
        let partner = door_a.partner.as_deref().and_then(|n| registry.first_named(n)).or_else(|| {
            // A door is also linked to whichever door names it.
            let me = name_of(*a)?;
            doors.iter().find(|(_, d)| d.partner.as_deref() == Some(me.as_str())).map(|(e, _)| *e)
        });
        let Some(b) = partner else { continue };
        let Some(door_b) = doors.iter().find(|(e, _)| *e == b).map(|(_, d)| d) else { continue };
        if door_a.open && door_b.open {
            pairs.push((opening(world, *a, door_a), opening(world, b, door_b)));
        }
    }
    pairs
}

fn pass_player(world: &mut World) {
    let pairs = linked_pairs(world);
    if pairs.is_empty() {
        return;
    }
    let Some(pe) = world.resource::<EntityRegistry>().player else { return };
    let Some(p) = world.get::<Player>(pe) else { return };
    if p.noclip {
        return;
    }
    let (mins, maxs) = p.state.hull();
    let half = (maxs - mins) * 0.5;
    let center = p.state.origin + (mins + maxs) * 0.5;
    let velocity = p.state.velocity;
    for (from, to) in pairs {
        let offset = center - from.center;
        let local = Vec3::new(offset.dot(from.forward), offset.dot(from.up.cross(from.forward)), offset.dot(from.up));
        // How far the box reaches toward the opening along its facing.
        let reach = |f: Vec3| f.x.abs() * half.x + f.y.abs() * half.y + f.z.abs() * half.z;
        let arriving = local.x >= -0.5 && local.x <= reach(from.forward) + 0.5 && velocity.dot(from.forward) <= 0.0;
        if !arriving || local.y.abs() > from.half_width || local.z.abs() > from.half_height {
            continue;
        }
        let m = transfer(&from, &to);
        // Come out with the box just clear of the exit, keeping the offset
        // across the opening.
        let across = m.transform_point3(center) - to.center;
        let across = across - to.forward * across.dot(to.forward);
        let new_center = to.center + across + to.forward * (reach(to.forward) + 0.5);
        let new_velocity = m.transform_vector3(velocity);
        let angles = world.resource::<PlayerInput>().input.angles;
        let view = m.transform_vector3(angles_forward(angles)).normalize_or(Vec3::X);
        let roll_up = m.transform_vector3(angles_to_quat(angles) * Vec3::Z);
        let up = (roll_up - view * view.dot(roll_up)).normalize_or(Vec3::Z);
        let a = quat_to_angles(Quat::from_mat3(&Mat3::from_cols(view, up.cross(view), up)));
        if let Some(mut p) = world.get_mut::<Player>(pe) {
            p.state.origin = new_center - (mins + maxs) * 0.5;
            p.previous_origin = p.state.origin;
            p.state.velocity = new_velocity;
            p.state.on_ground = false;
            p.state.ground_entity = None;
            p.ground_ref = None;
        }
        world.resource_mut::<PlayerInput>().input.angles = Vec3::new(a.x.clamp(-89.0, 89.0), a.y, 0.0);
        info!("linked portal door: player through to {new_center:.1?}");
        return;
    }
}
