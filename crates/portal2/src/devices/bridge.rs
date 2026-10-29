//! Light bridges: a projector casts a flat, walkable surface forward until it
//! meets a wall, continuing through linked portals.

use std::sync::Arc;

use avian3d::prelude::{Collider as PhysCollider, CollisionLayers, LayerMask, RigidBody};
use bevy::prelude::*;
use source_formats::bsp::contents;

use super::{EffectMaterial, linked_frames, with_ancestors};
use crate::collision::volume::ConvexVolume;
use crate::collision::{Collider as TraceCollider, Physics, Shape};
use crate::entities::SpawnHandlers;
use crate::entities::io::{InputCall, InputHandlers};
use crate::entities::spawner::{ModelOptions, attach_studio_model};
use crate::physics::layers;
use crate::portals::placement::transfer;
use crate::portals::{Portal, Portals};
use crate::world::MapObject;
use portal2_core::entity::MapEntity;

/// Bridges stop at walls and glass.
const BRIDGE_MASK: u32 = contents::SOLID | contents::MOVEABLE | contents::WINDOW;
/// Bridges hold up the player and block line of sight (turrets cannot see
/// through them), but beams and other bridges pass.
pub const BRIDGE_CONTENTS: u32 = contents::PLAYERCLIP | contents::BLOCKLOS;
const THICKNESS: f32 = 2.0;
const DEFAULT_WIDTH: f32 = 64.0;

pub struct BridgePlugin;

impl Plugin for BridgePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, update_bridges);
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
}

#[derive(Component, serde::Serialize, serde::Deserialize)]
pub struct LightBridge {
    pub enabled: bool,
    pub width: f32,
    #[serde(skip)]
    pub pieces: Vec<BridgePiece>,
    #[serde(skip)]
    entities: Vec<Entity>,
}

fn spawn_projector(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e).cloned() else { return };
    let model = me.studio_model().and_then(|p| attach_studio_model(world, e, &p, ModelOptions::default()));
    // The projector's mouth spans its sideways (local Y) extent.
    let width = model
        .map(|m| {
            let mdl = &m.studio.mdl;
            let lo = mdl.hull_min.min(mdl.view_bbmin);
            let hi = mdl.hull_max.max(mdl.view_bbmax);
            (hi.y - lo.y).abs()
        })
        .filter(|w| (16.0..=256.0).contains(w))
        .unwrap_or(DEFAULT_WIDTH);
    world.entity_mut(e).insert(LightBridge {
        enabled: me.get_i32("startenabled").unwrap_or(1) != 0,
        width,
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
    frames: Option<[crate::portals::placement::PortalFrame; 2]>,
    start: Vec3,
    dir: Vec3,
    up: Vec3,
    ignore: &[Entity],
) -> Vec<BridgePiece> {
    let mut out = Vec::new();
    let (mut start, mut dir, mut up) = (start, dir.normalize_or(Vec3::X), up.normalize_or(Vec3::Z));
    for _ in 0..8 {
        let tr = physics.trace(start, start + dir * 16384.0, Vec3::ZERO, Vec3::ZERO, BRIDGE_MASK, ignore);
        out.push(BridgePiece { start, end: tr.end, up });
        let Some(frames) = frames else { break };
        if tr.fraction >= 1.0 {
            break;
        }
        let Some(slot) = (0..2).find(|&s| {
            let l = frames[s].to_local(tr.end);
            l.x.abs() < 2.0 && frames[s].in_opening(l, 0.0) && dir.dot(frames[s].forward) < 0.0
        }) else {
            break;
        };
        let m = transfer(&frames[slot], &frames[1 - slot]);
        dir = m.transform_vector3(dir).normalize_or(Vec3::X);
        up = m.transform_vector3(up).normalize_or(Vec3::Z);
        start = m.transform_point3(tr.end) + dir;
    }
    out
}

fn piece_basis(p: &BridgePiece) -> (Vec3, f32, Quat) {
    let d = p.end - p.start;
    let len = d.length().max(0.01);
    let fwd = d / len;
    let up = (p.up - fwd * p.up.dot(fwd)).normalize_or(Vec3::Z);
    let left = up.cross(fwd);
    (p.start + d * 0.5, len, Quat::from_mat3(&Mat3::from_cols(fwd, left, up)))
}

#[allow(clippy::too_many_arguments)]
fn update_bridges(
    mut commands: Commands,
    physics: Physics,
    portals: Res<Portals>,
    portal_query: Query<&Portal>,
    mut bridges: Query<(Entity, &mut LightBridge, &GlobalTransform)>,
    parents: Query<&ChildOf>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<EffectMaterial>>,
    mut shared: Local<Option<(Handle<Mesh>, Handle<EffectMaterial>)>>,
) {
    let (mesh, material) = shared
        .get_or_insert_with(|| {
            (
                meshes.add(Cuboid::new(1.0, 1.0, 1.0)),
                materials.add(EffectMaterial::translucent(Vec3::new(0.45, 0.85, 1.0) * 1.5, 0.45, 0.4, 0.02)),
            )
        })
        .clone();
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
        for old in bridge.entities.drain(..) {
            commands.entity(old).despawn();
        }
        let width = bridge.width;
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
                    TraceCollider { shape: Arc::new(Shape::new(vec![local])), enabled: true, solid: true },
                    RigidBody::Static,
                    PhysCollider::cuboid(len, width, THICKNESS),
                    CollisionLayers::new(LayerMask(layers::WORLD), LayerMask(layers::PROP | layers::HELD)),
                ))
                .with_child((
                    Mesh3d(mesh.clone()),
                    MeshMaterial3d(material.clone()),
                    Transform::from_scale(Vec3::new(len, width, THICKNESS * 0.5)),
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
