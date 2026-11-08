//! Light bridges: a projector casts a flat, walkable surface forward until it
//! meets a wall, continuing through linked portals. How they look is in
//! `bridge_visual`.

use std::sync::Arc;

use avian3d::prelude::{Collider as PhysCollider, CollisionLayers, LayerMask, RigidBody};
use bevy::prelude::*;
use source_formats::bsp::contents;

use super::bridge_visual::{self, BridgeMaterial};
use super::{linked_frames, with_ancestors};
use crate::entities::SpawnHandlers;
use crate::entities::io::{InputCall, InputHandlers};
use crate::entities::spawner::{ModelOptions, attach_studio_model};
use crate::physics::layers;
use crate::player::Player;
use crate::portals::{Portal, Portals};
use portal2_core::entity::MapEntity;
use portal2_world::MapObject;
use portal2_world::collision::portal::transfer;
use portal2_world::collision::volume::ConvexVolume;
use portal2_world::collision::{Collider as TraceCollider, Physics, Shape, mask};

/// Bridges stop at walls and glass.
const BRIDGE_MASK: u32 = contents::SOLID | contents::MOVEABLE | contents::WINDOW;
/// Bridges hold up the player and block line of sight (turrets cannot see
/// through them), but beams and other bridges pass.
pub const BRIDGE_CONTENTS: u32 = contents::PLAYERCLIP | contents::BLOCKLOS;
pub(super) const THICKNESS: f32 = 2.0;
/// Every bridge is 64 units wide, whatever the projector's model
/// (`CProjectedWallEntity`, server.dll `sub_103D7D90`).
pub const WIDTH: f32 = 64.0;

pub struct BridgePlugin;

impl Plugin for BridgePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<BridgeMaterial>::default())
            .add_systems(Update, (update_bridges, bridge_visual::draw_bridges).chain())
            .add_systems(PostUpdate, bridge_visual::bridge_lights.in_set(crate::visuals::fx::lights::FxProduceSet));
        let world = app.world_mut();
        world.resource_mut::<SpawnHandlers>().register("prop_wall_projector", spawn_projector);
        world.resource_mut::<InputHandlers>().register("prop_wall_projector", projector_input);
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BridgePiece {
    pub start: Vec3,
    pub end: Vec3,
    pub up: Vec3,
    /// The portal the piece comes out of, and the one it goes into.
    pub leaves: Option<PortalPlane>,
    pub enters: Option<PortalPlane>,
}

impl BridgePiece {
    pub fn new(start: Vec3, end: Vec3, up: Vec3) -> Self {
        Self { start, end, up, leaves: None, enters: None }
    }
}

/// A portal's plane: its centre and outward facing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PortalPlane {
    pub point: Vec3,
    pub normal: Vec3,
}

impl PortalPlane {
    fn of(frame: &portal2_world::collision::portal::PortalFrame) -> Self {
        Self { point: frame.center, normal: frame.forward }
    }
}

#[derive(Component, serde::Serialize, serde::Deserialize)]
pub struct LightBridge {
    pub enabled: bool,
    #[serde(skip)]
    pub pieces: Vec<BridgePiece>,
    #[serde(skip)]
    entities: Vec<Entity>,
}

impl LightBridge {
    /// The entity of each piece (its collision; the drawn parts are its
    /// children).
    pub fn entities(&self) -> &[Entity] {
        &self.entities
    }
}

fn spawn_projector(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e).cloned() else { return };
    if let Some(p) = me.studio_model() {
        attach_studio_model(world, e, &p, ModelOptions::default());
    }
    world.entity_mut(e).insert(LightBridge {
        enabled: me.get_i32("startenabled").unwrap_or(1) != 0,
        pieces: Vec::new(),
        entities: Vec::new(),
    });
}

fn projector_input(world: &mut World, call: &InputCall) -> bool {
    let Some(mut b) = world.get_mut::<LightBridge>(call.target) else { return false };
    match call.input.as_str() {
        "enable" => b.enabled = true,
        "disable" => b.enabled = false,
        "toggle" => b.enabled = !b.enabled,
        _ => return false,
    }
    true
}

/// Casts the bridge path, carrying its "up" through portals.
fn cast(
    physics: &Physics,
    frames: Option<[portal2_world::collision::portal::PortalFrame; 2]>,
    start: Vec3,
    dir: Vec3,
    up: Vec3,
    ignore: &[Entity],
) -> Vec<BridgePiece> {
    let mut out = Vec::new();
    let (mut start, mut dir, mut up) = (start, dir.normalize_or(Vec3::X), up.normalize_or(Vec3::Z));
    let (mut trace_from, mut leaves) = (start, None);
    for _ in 0..8 {
        let tr = physics.trace(trace_from, trace_from + dir * 16384.0, Vec3::ZERO, Vec3::ZERO, BRIDGE_MASK, ignore);
        let mut piece = BridgePiece { leaves, ..BridgePiece::new(start, tr.end, up) };
        let slot = frames.filter(|_| tr.fraction < 1.0).and_then(|frames| {
            (0..2).find(|&s| {
                let l = frames[s].to_local(tr.end);
                l.x.abs() < 2.0 && frames[s].in_opening(l, 0.0) && dir.dot(frames[s].forward) < 0.0
            })
        });
        let (Some(frames), Some(slot)) = (frames, slot) else {
            out.push(piece);
            break;
        };
        piece.enters = Some(PortalPlane::of(&frames[slot]));
        out.push(piece);
        let m = transfer(&frames[slot], &frames[1 - slot]);
        dir = m.transform_vector3(dir).normalize_or(Vec3::X);
        up = m.transform_vector3(up).normalize_or(Vec3::Z);
        // The next piece starts where this one went in, carried through, so
        // the path (and the texture and pulses laid along it) runs on
        // without a step; its trace starts a unit out, clear of the wall.
        start = m.transform_point3(tr.end);
        trace_from = start + dir;
        leaves = Some(PortalPlane::of(&frames[1 - slot]));
    }
    out
}

pub(super) fn piece_basis(p: &BridgePiece) -> (Vec3, f32, Quat) {
    let d = p.end - p.start;
    let len = d.length().max(0.01);
    let fwd = d / len;
    let up = (p.up - fwd * p.up.dot(fwd)).normalize_or(Vec3::Z);
    let left = up.cross(fwd);
    (p.start + d * 0.5, len, Quat::from_mat3(&Mat3::from_cols(fwd, left, up)))
}

/// How far a player box overlapping a bridge piece must move along the
/// piece's facing to clear it (onto it when the box's centre is above its
/// plane, under it otherwise), with the box's centre and half size; None
/// when they do not overlap. Separating-axis test on the piece's three axes
/// and the world's.
fn clearance(p: &BridgePiece, center: Vec3, half: Vec3) -> Option<(Vec3, Vec3)> {
    /// Gap left between the player's box and the surface.
    const EPSILON: f32 = 0.03125;
    let (mid, len, rot) = piece_basis(p);
    let axes = [rot * Vec3::X, rot * Vec3::Y, rot * Vec3::Z];
    let piece_half = Vec3::new(len * 0.5, WIDTH * 0.5, THICKNESS * 0.5);
    let rel = center - mid;
    let box_along = |a: Vec3| a.x.abs() * half.x + a.y.abs() * half.y + a.z.abs() * half.z;
    let piece_along = |a: Vec3| axes.iter().zip(piece_half.to_array()).map(|(x, h)| x.dot(a).abs() * h).sum::<f32>();
    for a in axes.into_iter().chain([Vec3::X, Vec3::Y, Vec3::Z]) {
        if rel.dot(a).abs() >= box_along(a) + piece_along(a) {
            return None;
        }
    }
    let up = axes[2];
    let d = rel.dot(up);
    let reach = box_along(up) + piece_half.z + EPSILON;
    let toward = if d >= 0.0 { up } else { -up };
    Some((toward * (reach - d.abs()), -toward * (reach + d.abs())))
}

/// A bridge appearing through the player (a portal opened under their feet
/// sends it across where they stand) moves them out of it, onto it when
/// their middle is above it: otherwise they would stand stuck inside the
/// surface, unable to jump onto it. The far side is tried when the near one
/// is blocked; with both blocked the player stays where they are.
fn displace_players(physics: &Physics, pieces: &[BridgePiece], ignore: &[Entity], players: &mut Query<(Entity, &mut Player)>) {
    for (e, mut p) in players.iter_mut() {
        if p.noclip {
            continue;
        }
        let (mins, maxs) = p.state.hull();
        let mut ignore = ignore.to_vec();
        ignore.push(e);
        for piece in pieces {
            let center = p.state.origin + (mins + maxs) * 0.5;
            let Some((near, far)) = clearance(piece, center, (maxs - mins) * 0.5) else { continue };
            let free = |to: Vec3| !physics.trace(to, to, mins, maxs, mask::PLAYER_SOLID, &ignore).start_solid;
            let Some(to) = [p.state.origin + near, p.state.origin + far].into_iter().find(|to| free(*to)) else { continue };
            info!("light bridge: moved the player out of it, {:.1?} -> {:.1?}", p.state.origin, to);
            p.state.origin = to;
            p.previous_origin = to;
            p.state.on_ground = false;
            p.state.ground_entity = None;
            p.ground_ref = None;
            p.state.velocity.z = p.state.velocity.z.max(0.0);
        }
    }
}

fn update_bridges(
    mut commands: Commands,
    physics: Physics,
    portals: Res<Portals>,
    portal_query: Query<&Portal>,
    mut bridges: Query<(Entity, &mut LightBridge, &GlobalTransform)>,
    parents: Query<&ChildOf>,
    mut players: Query<(Entity, &mut Player)>,
    holding: Option<Res<crate::physics::carry::Holding>>,
) {
    let frames = linked_frames(&portals, &portal_query);
    for (e, mut bridge, gt) in &mut bridges {
        let t = gt.compute_transform();
        let pieces = if bridge.enabled {
            let fwd = t.rotation * Vec3::X;
            cast(&physics, frames, t.translation + fwd * 8.0, fwd, t.rotation * Vec3::Z, &with_ancestors(e, &parents))
        } else {
            Vec::new()
        };
        let same = pieces.len() == bridge.pieces.len()
            && pieces
                .iter()
                .zip(&bridge.pieces)
                .all(|(a, b)| a.start.distance(b.start) < 0.5 && a.end.distance(b.end) < 0.5 && a.up.dot(b.up) > 0.999);
        if same {
            continue;
        }
        // The old pieces are still in the collision world until the
        // commands run, and the carried prop never blocks its carrier.
        let ignore: Vec<Entity> = bridge.entities.iter().copied().chain(holding.as_ref().and_then(|h| h.entity)).collect();
        displace_players(&physics, &pieces, &ignore, &mut players);
        for old in bridge.entities.drain(..) {
            commands.entity(old).despawn();
        }
        let width = WIDTH;
        for p in &pieces {
            let (center, len, rot) = piece_basis(p);
            let local = ConvexVolume::from_box(
                Vec3::new(-len * 0.5, -width * 0.5, -THICKNESS * 0.5),
                Vec3::new(len * 0.5, width * 0.5, THICKNESS * 0.5),
                BRIDGE_CONTENTS,
                -1,
            );
            let id = commands
                .spawn((
                    Name::new("light bridge piece"),
                    MapObject,
                    Transform::from_translation(center).with_rotation(rot),
                    Visibility::Visible,
                    TraceCollider::new(Arc::new(Shape::new(vec![local])), true, true),
                    RigidBody::Static,
                    PhysCollider::cuboid(len, width, THICKNESS),
                    CollisionLayers::new(LayerMask(layers::WORLD), LayerMask(layers::PROP | layers::HELD)),
                ))
                .id();
            bridge.entities.push(id);
        }
        bridge.pieces = pieces;
    }
}

impl portal2_core::save::Restore for LightBridge {
    fn keep(&mut self, live: Self) {
        self.pieces = live.pieces;
        self.entities = live.entities;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HALF: Vec3 = Vec3::new(16.0, 16.0, 36.0);

    /// A flat bridge along +X with its middle 1 unit up: the player whose
    /// feet sit 12 units under its top is lifted onto it; one standing on it,
    /// beside it or clear above it is left alone.
    #[test]
    fn clearance_lifts_a_player_onto_a_bridge_through_their_legs() {
        let piece = BridgePiece::new(Vec3::new(-200.0, 0.0, 0.0), Vec3::new(200.0, 0.0, 0.0), Vec3::Z);
        let top = THICKNESS * 0.5;
        let feet = top - 12.0;
        let (near, _) = clearance(&piece, Vec3::new(0.0, 0.0, feet + HALF.z), HALF).unwrap();
        assert!((feet + near.z - top - 0.03125).abs() < 1e-4, "{near:?}");
        assert!(near.x == 0.0 && near.y == 0.0);
        // Standing on it.
        assert!(clearance(&piece, Vec3::new(0.0, 0.0, top + 0.03125 + HALF.z), HALF).is_none());
        // Beside it (the bridge is 64 wide).
        assert!(clearance(&piece, Vec3::new(0.0, WIDTH * 0.5 + HALF.y + 1.0, feet + HALF.z), HALF).is_none());
        // Past its end.
        assert!(clearance(&piece, Vec3::new(200.0 + HALF.x + 1.0, 0.0, feet + HALF.z), HALF).is_none());
    }

    /// A bridge through the player's head pushes them down first, with the
    /// far side (on top of it) as the fallback.
    #[test]
    fn clearance_below_when_the_middle_is_under_it() {
        let piece = BridgePiece::new(Vec3::new(-200.0, 0.0, 60.0), Vec3::new(200.0, 0.0, 60.0), Vec3::Z);
        let (near, far) = clearance(&piece, Vec3::new(0.0, 0.0, HALF.z), HALF).unwrap();
        assert!(near.z < 0.0 && far.z > 0.0, "{near:?} {far:?}");
        // The far side puts the feet on its top.
        assert!((far.z - (60.0 + THICKNESS * 0.5 + 0.03125)).abs() < 1e-4, "{far:?}");
    }
}
