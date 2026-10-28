//! Rigid-body physics (Avian): the static map, moving brushes and props,
//! physics props such as cubes, carrying objects, and props moving through
//! portals.

pub mod carry;
pub mod portal_travel;

use std::sync::Arc;

use avian3d::prelude::*;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::collision::volume::{ConvexVolume, polytope_vertices};
use crate::collision::{Collider as TraceCollider, CollisionWorld, Shape};
use crate::entities::MapEntity;
use crate::entities::props::PhysicsProp;
use crate::player::Player;
use crate::portals::PortalHoles;
use crate::vfs::GameFs;
use crate::world::{CurrentMap, MapLoaded, MapObject};

/// Inches per metre: Avian scales its tolerances by this.
pub const LENGTH_UNIT: f32 = 39.37;

pub mod layers {
    pub const WORLD: u32 = 1 << 0;
    pub const PROP: u32 = 1 << 1;
    pub const PLAYER: u32 = 1 << 2;
    pub const HELD: u32 = 1 << 3;
    pub const MOVER: u32 = 1 << 4;
}

pub struct GamePhysicsPlugin;

impl Plugin for GamePhysicsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(
            PhysicsPlugins::default().with_length_unit(LENGTH_UNIT).with_collision_hooks::<PortalContactHooks>(),
        )
        .insert_resource(Gravity(Vec3::NEG_Z * crate::player::movement::GRAVITY))
        .add_systems(
            PreUpdate,
            build_static_world.after(crate::collision::rebuild_on_map_load).before(crate::entities::EntitySpawnSet),
        )
        .add_systems(Update, (attach_bodies, attach_player_body, sync_portal_slabs))
        .add_plugins((carry::CarryPlugin, portal_travel::PortalTravelPlugin));
    }
}

fn layers_for(membership: u32, filters: u32) -> CollisionLayers {
    CollisionLayers::new(LayerMask(membership), LayerMask(filters))
}

/// Convex hull collider for one of our collision volumes.
fn volume_collider(v: &ConvexVolume) -> Option<Collider> {
    let pts = polytope_vertices(&v.planes);
    if pts.len() < 4 {
        return None;
    }
    Collider::convex_hull(&pts)
}

fn shape_collider(shape: &Shape) -> Option<Collider> {
    let parts: Vec<(Vec3, Quat, Collider)> =
        shape.volumes.iter().filter_map(volume_collider).map(|c| (Vec3::ZERO, Quat::IDENTITY, c)).collect();
    (!parts.is_empty()).then(|| Collider::compound(parts))
}

/// Marks the static map body.
#[derive(Component)]
pub struct StaticWorld;

/// Builds the static map collider once the trace world exists. Terrain goes
/// into a triangle mesh, everything else into convex hulls.
fn build_static_world(
    mut commands: Commands,
    mut loaded: MessageReader<MapLoaded>,
    world: Res<CollisionWorld>,
    map: Option<Res<CurrentMap>>,
) {
    if loaded.read().last().is_none() {
        return;
    }
    let Some(map) = map else { return };
    let started = std::time::Instant::now();
    let solid = crate::collision::mask::SOLID_ALL;
    let mut hulls: Vec<(Vec3, Quat, Collider)> = Vec::new();
    for v in &world.statics {
        // Triangle volumes from terrain are rebuilt as a mesh below.
        if v.contents & solid == 0 || v.planes.first().is_some_and(|p| p.surface == -2) {
            continue;
        }
        if let Some(c) = volume_collider(v) {
            hulls.push((Vec3::ZERO, Quat::IDENTITY, c));
        }
    }
    let count = hulls.len();
    let mut e = commands.spawn((
        Name::new("physics: static map"),
        StaticWorld,
        MapObject,
        RigidBody::Static,
        layers_for(layers::WORLD, layers::PROP | layers::HELD),
        Transform::IDENTITY,
    ));
    if !hulls.is_empty() {
        e.insert(Collider::compound(hulls));
    }
    let tris = crate::collision::build::displacement_triangles(&map.bsp);
    if !tris.is_empty() {
        let mut verts = Vec::with_capacity(tris.len() * 3);
        let mut indices = Vec::with_capacity(tris.len());
        for t in &tris {
            let base = verts.len() as u32;
            verts.extend_from_slice(t);
            indices.push([base, base + 1, base + 2]);
        }
        commands.spawn((
            Name::new("physics: terrain"),
            StaticWorld,
            MapObject,
            RigidBody::Static,
            Collider::trimesh(verts, indices),
            layers_for(layers::WORLD, layers::PROP | layers::HELD),
            Transform::IDENTITY,
        ));
    }
    info!("physics: {count} static hulls, {} terrain triangles in {:?}", tris.len(), started.elapsed());
}

/// Mass of a model from its collision keyvalues, else from the model header.
pub fn model_mass(fs: &GameFs, path: &str) -> Option<f32> {
    let base = source_formats::text::strip_extension(path);
    if let Some(bytes) = fs.read(&format!("{base}.phy"))
        && let Ok(phy) = source_formats::phy::Phy::parse(&bytes)
    {
        let total: f32 = source_formats::kv::parse_document(&phy.keyvalues)
            .roots
            .iter()
            .filter(|r| r.key.eq_ignore_ascii_case("solid"))
            .filter_map(|r| r.get_f32("mass"))
            .sum();
        if total > 0.0 {
            return Some(total);
        }
    }
    let mdl = source_formats::mdl::Mdl::parse(fs.read(path)?).ok()?;
    (mdl.mass > 0.0).then_some(mdl.mass)
}

/// Gives every entity with trace collision a matching physics body: physics
/// props become dynamic, everything else solid becomes kinematic.
#[allow(clippy::type_complexity)]
fn attach_bodies(
    mut commands: Commands,
    fs: Res<GameFs>,
    new: Query<(Entity, &TraceCollider, &MapEntity, Has<PhysicsProp>, Has<ChildOf>), Without<RigidBody>>,
) {
    for (e, col, me, is_prop, parented) in &new {
        if !col.solid {
            continue;
        }
        let Some(collider) = shape_collider(&col.shape) else { continue };
        if is_prop && !parented && !me.has_flag(8) {
            let mass = me
                .get_f32("massscale")
                .filter(|s| *s > 0.0)
                .unwrap_or(1.0)
                * me.studio_model().and_then(|p| model_mass(&fs, &p)).unwrap_or(40.0);
            make_dynamic(&mut commands, e, collider, mass);
        } else {
            // Spawnflag 8 on physics props: start motion disabled.
            commands.entity(e).insert((
                RigidBody::Kinematic,
                collider,
                layers_for(layers::MOVER, layers::PROP | layers::HELD),
            ));
        }
    }
}

/// Simulates `e` as a free physics prop from now on.
pub fn make_dynamic(commands: &mut Commands, e: Entity, collider: Collider, mass: f32) {
    commands.entity(e).insert((
        RigidBody::Dynamic,
        collider,
        Mass(mass),
        Friction::new(0.8),
        Restitution::new(0.05),
        LinearDamping(0.05),
        AngularDamping(0.2),
        SweptCcd::default(),
        layers_for(layers::PROP, layers::WORLD | layers::PROP | layers::PLAYER | layers::MOVER | layers::HELD),
        ActiveCollisionHooks::MODIFY_CONTACTS,
        portal_travel::PortalTraveler::default(),
    ));
}

/// Collider for one of our collision shapes, if it has any volume.
pub fn collider_for(shape: &Shape) -> Option<Collider> {
    shape_collider(shape)
}

/// The player pushes props through a kinematic box that follows them.
fn attach_player_body(mut commands: Commands, players: Query<Entity, (With<Player>, Without<RigidBody>)>) {
    for e in &players {
        let hull = Collider::compound(vec![(Vec3::Z * 36.0, Quat::IDENTITY, Collider::cuboid(32.0, 32.0, 72.0))]);
        commands.entity(e).insert((RigidBody::Kinematic, hull, layers_for(layers::PLAYER, layers::PROP)));
    }
}

/// Frame pieces around linked portal openings, so props inside an opening
/// cannot slide sideways into the wall.
#[derive(Component)]
struct PortalSlab;

fn sync_portal_slabs(
    mut commands: Commands,
    holes: Res<PortalHoles>,
    slabs: Query<Entity, With<PortalSlab>>,
) {
    if !holes.is_changed() {
        return;
    }
    for e in &slabs {
        commands.entity(e).despawn();
    }
    for hole in &holes.holes {
        for v in &hole.frame_volumes {
            if let Some(c) = volume_collider(v) {
                commands.spawn((
                    PortalSlab,
                    MapObject,
                    RigidBody::Static,
                    c,
                    layers_for(layers::WORLD, layers::PROP | layers::HELD),
                    Transform::IDENTITY,
                ));
            }
        }
    }
}

/// Drops world contacts inside a linked portal's opening for props that are
/// passing through it, so they can enter the wall the portal is on.
#[derive(SystemParam)]
pub struct PortalContactHooks<'w, 's> {
    holes: Res<'w, PortalHoles>,
    statics: Query<'w, 's, (), With<StaticWorld>>,
    shapes: Query<'w, 's, (&'static TraceCollider, &'static Position, &'static Rotation)>,
}

impl CollisionHooks for PortalContactHooks<'_, '_> {
    fn modify_contacts(&self, contacts: &mut ContactPair, _commands: &mut Commands) -> bool {
        if self.holes.holes.is_empty() {
            return true;
        }
        let prop = if self.statics.contains(contacts.collider2) {
            contacts.collider1
        } else if self.statics.contains(contacts.collider1) {
            contacts.collider2
        } else {
            return true;
        };
        let Ok((col, pos, rot)) = self.shapes.get(prop) else { return true };
        let (mins, maxs) = world_bounds(&col.shape, pos.0, rot.0);
        let center = (mins + maxs) * 0.5;
        let half = (maxs - mins) * 0.5;
        for hole in &self.holes.holes {
            if !hole.contains_box(center, -half, half) {
                continue;
            }
            // Everything of the static map behind the opening is ignored; the
            // separate frame slabs keep the prop inside the tunnel.
            let f = &hole.frame;
            for m in &mut contacts.manifolds {
                m.points.retain(|p| {
                    let l = f.to_local(Vec3::from(p.point));
                    !(l.x < 2.0 && f.in_opening(l, -2.0))
                });
            }
            contacts.manifolds.retain(|m| !m.points.is_empty());
        }
        !contacts.manifolds.is_empty()
    }
}

/// World-space bounds of a shape placed at `pos` / `rot`.
pub fn world_bounds(shape: &Arc<Shape>, pos: Vec3, rot: Quat) -> (Vec3, Vec3) {
    let c = (shape.mins + shape.maxs) * 0.5;
    let e = (shape.maxs - shape.mins) * 0.5;
    let m = Mat3::from_quat(rot);
    let wc = pos + m * c;
    let we = Vec3::new(
        m.x_axis.x.abs() * e.x + m.y_axis.x.abs() * e.y + m.z_axis.x.abs() * e.z,
        m.x_axis.y.abs() * e.x + m.y_axis.y.abs() * e.y + m.z_axis.y.abs() * e.z,
        m.x_axis.z.abs() * e.x + m.y_axis.z.abs() * e.y + m.z_axis.z.abs() * e.z,
    );
    (wc - we, wc + we)
}
