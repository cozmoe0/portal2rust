//! Rigid-body physics (Avian): the static map, moving brushes and props,
//! physics props such as cubes, carrying objects, and props moving through
//! portals.

pub mod carry;
pub mod constraints;
pub mod held_clone;
pub mod inputs;
pub mod portal_travel;

use std::sync::Arc;

use avian3d::prelude::*;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::entities::props::PhysicsProp;
use crate::player::Player;
use portal2_core::entity::MapEntity;
use portal2_core::vfs::GameFs;
use portal2_world::collision::portal::{HALF_HEIGHT, HALF_WIDTH, PortalHoles};
use portal2_world::collision::volume::{ConvexVolume, polytope_vertices};
use portal2_world::collision::{Collider as TraceCollider, CollisionWorld, Shape};
use portal2_world::{CurrentMap, MapLoaded, MapObject};

/// Inches per metre: Avian scales its tolerances by this.
pub const LENGTH_UNIT: f32 = 39.37;

pub mod layers {
    pub const WORLD: u32 = 1 << 0;
    pub const PROP: u32 = 1 << 1;
    pub const PLAYER: u32 = 1 << 2;
    pub const HELD: u32 = 1 << 3;
    pub const MOVER: u32 = 1 << 4;
    /// Collision riding a model's bones (a prop_dynamic's moving parts).
    /// Props resting on it collide with it, but not one the player holds.
    pub const FOLLOWER: u32 = 1 << 5;
}

pub struct GamePhysicsPlugin;

impl Plugin for GamePhysicsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(PhysicsPlugins::default().with_length_unit(LENGTH_UNIT).with_collision_hooks::<PortalContactHooks>())
            .insert_resource(Gravity(Vec3::NEG_Z * crate::player::movement::GRAVITY))
            .add_systems(
                PreUpdate,
                build_static_world.after(portal2_world::collision::rebuild_on_map_load).before(portal2_core::sets::EntitySpawnSet),
            )
            .add_systems(Update, (attach_bodies, attach_follower_bodies, attach_player_body, sync_portal_slabs, sync_disabled_colliders))
            .add_plugins((carry::CarryPlugin, held_clone::HeldClonePlugin, portal_travel::PortalTravelPlugin));
        inputs::register(app);
        constraints::register(app);
    }
}

fn layers_for(membership: u32, filters: u32) -> CollisionLayers {
    CollisionLayers::new(LayerMask(membership), LayerMask(filters))
}

/// Planes closer than this in angle (cosine) and offset are one face.
const COPLANAR_COS: f32 = 0.999;
const COPLANAR_DIST: f32 = 0.5;
/// Points closer than this are one hull corner.
const WELD_DISTANCE: f32 = 0.1;
/// Points this close to a plane lie on its face.
const ON_FACE: f32 = 0.05;

/// Avian colliders for one of our collision volumes.
///
/// Model hulls come from triangles whose planes are each pushed out to the
/// hull's furthest point, so a face made of slightly uneven triangles
/// becomes a very low dome; nearly coplanar planes are merged into one flat
/// face first. The contact solver supports a resting box on at most four
/// corners of the face below it, so on a face with more corners (a floor
/// button's round plate) props are held up by part of it only and keep
/// rocking. Such hulls are split into tetrahedra fanned out from their
/// centre, each face of which is a triangle.
fn volume_colliders(v: &ConvexVolume) -> Vec<Collider> {
    let mut planes: Vec<portal2_world::collision::volume::Plane> = Vec::with_capacity(v.planes.len());
    for p in &v.planes {
        match planes.iter_mut().find(|q| q.normal.dot(p.normal) > COPLANAR_COS && (q.dist - p.dist).abs() < COPLANAR_DIST) {
            Some(q) => q.dist = q.dist.max(p.dist),
            None => planes.push(*p),
        }
    }
    let mut pts: Vec<Vec3> = Vec::new();
    for p in polytope_vertices(&planes) {
        if !pts.iter().any(|q| q.distance_squared(p) < WELD_DISTANCE * WELD_DISTANCE) {
            pts.push(p);
        }
    }
    if pts.len() < 4 {
        return Vec::new();
    }
    let faces: Vec<Vec<Vec3>> = planes.iter().map(|pl| face_polygon(&pts, pl.normal, pl.dist)).filter(|f| f.len() >= 3).collect();
    if faces.iter().all(|f| f.len() <= 4) {
        return Collider::convex_hull(&pts).into_iter().collect();
    }
    let centre = pts.iter().copied().sum::<Vec3>() / pts.len() as f32;
    faces
        .iter()
        .flat_map(|f| (1..f.len() - 1).map(move |i| [centre, f[0], f[i], f[i + 1]]))
        .filter_map(|tet| Collider::convex_hull(&tet))
        .collect()
}

/// The corners of the hull on a plane, in order around the face.
fn face_polygon(pts: &[Vec3], normal: Vec3, dist: f32) -> Vec<Vec3> {
    let mut on: Vec<Vec3> = pts.iter().copied().filter(|p| (normal.dot(*p) - dist).abs() < ON_FACE).collect();
    if on.len() < 3 {
        return on;
    }
    let c = on.iter().copied().sum::<Vec3>() / on.len() as f32;
    let u = (on[0] - c).normalize_or_zero();
    let w = normal.cross(u);
    on.sort_by(|a, b| {
        let (da, db) = (*a - c, *b - c);
        da.dot(w).atan2(da.dot(u)).total_cmp(&db.dot(w).atan2(db.dot(u)))
    });
    on
}

/// A physics prop's own body: one convex hull per piece, from its corners.
/// (The hulls of what props rest on are split up instead, see
/// [`volume_colliders`].) A rounded model kept whole stays cheap to sweep:
/// Wheatley's sphere split into hundreds of thin tetrahedra made every
/// physics tick sweep each of them against the map as a fast, thin body.
pub fn prop_collider(shape: &Shape) -> Option<Collider> {
    let parts: Vec<(Vec3, Quat, Collider)> = shape
        .volumes
        .iter()
        .filter_map(|v| {
            let solved;
            let points = if v.points.len() >= 4 {
                &v.points
            } else {
                solved = polytope_vertices(&v.planes);
                &solved
            };
            Collider::convex_hull(points)
        })
        .map(|c| (Vec3::ZERO, Quat::IDENTITY, c))
        .collect();
    (!parts.is_empty()).then(|| Collider::compound(parts))
}

fn shape_collider(shape: &Shape) -> Option<Collider> {
    let parts: Vec<(Vec3, Quat, Collider)> =
        shape.volumes.iter().flat_map(volume_colliders).map(|c| (Vec3::ZERO, Quat::IDENTITY, c)).collect();
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
    let started = bevy::platform::time::Instant::now();
    let solid = portal2_world::collision::mask::SOLID_ALL;
    let mut hulls: Vec<(Vec3, Quat, Collider)> = Vec::new();
    for v in &world.statics {
        // Triangle volumes from terrain are rebuilt as a mesh below.
        if v.contents & solid == 0 || v.planes.first().is_some_and(|p| p.surface == -2) {
            continue;
        }
        hulls.extend(volume_colliders(v).into_iter().map(|c| (Vec3::ZERO, Quat::IDENTITY, c)));
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
    let tris = portal2_world::collision::build::displacement_triangles(&map.bsp);
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
fn attach_bodies(
    mut commands: Commands,
    fs: Res<GameFs>,
    new: Query<(Entity, &TraceCollider, &MapEntity, Has<PhysicsProp>, Has<ChildOf>), Without<RigidBody>>,
) {
    for (e, col, me, is_prop, parented) in &new {
        if !col.solid {
            continue;
        }
        let collider = if is_prop { prop_collider(&col.shape) } else { shape_collider(&col.shape) };
        let Some(collider) = collider else { continue };
        if is_prop && !parented && !me.has_flag(SF_MOTION_DISABLED) {
            make_dynamic(&mut commands, e, collider, prop_mass(&fs, me));
        } else {
            // Spawnflag 8 on physics props: start motion disabled.
            commands.entity(e).insert((RigidBody::Kinematic, collider, layers_for(layers::MOVER, layers::PROP | layers::HELD)));
        }
    }
}

/// Physics prop spawnflag: motion disabled. `DisableMotion` and
/// `EnableMotion` keep it up to date, so a saved game knows too.
pub const SF_MOTION_DISABLED: u32 = 8;

/// A physics prop's mass: its model's (40 without one) times `massscale`.
pub fn prop_mass(fs: &GameFs, me: &MapEntity) -> f32 {
    me.get_f32("massscale").filter(|s| *s > 0.0).unwrap_or(1.0) * me.studio_model().and_then(|p| model_mass(fs, &p)).unwrap_or(40.0)
}

/// A physics prop given a parent rides it rather than falling.
pub fn on_parented(world: &mut World, e: Entity) {
    if world.get::<PhysicsProp>(e).is_some() && world.get::<RigidBody>(e).is_some_and(|b| b.is_dynamic()) {
        world.entity_mut(e).insert((RigidBody::Kinematic, LinearVelocity::ZERO, AngularVelocity::ZERO));
    }
}

/// A physics prop its parent lets go of falls free, unless its motion is
/// disabled (`sp_a1_intro7`'s Wheatley popping off his management rail).
pub fn on_unparented(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e) else { return };
    let frozen = me.has_flag(SF_MOTION_DISABLED);
    if frozen || world.get::<PhysicsProp>(e).is_none() || world.get::<RigidBody>(e).is_none_or(|b| b.is_dynamic()) {
        return;
    }
    let mass = prop_mass(world.resource::<GameFs>(), me);
    let Some(collider) = world.get::<TraceCollider>(e).and_then(|c| prop_collider(&c.shape)) else { return };
    let mut commands = world.commands();
    make_dynamic(&mut commands, e, collider, mass);
    world.flush();
}

/// Collision switched off (a disabled brush) stops props as well as the
/// player.
fn sync_disabled_colliders(
    mut commands: Commands,
    bodies: Query<(Entity, &TraceCollider, Has<ColliderDisabled>), (With<RigidBody>, Or<(Changed<TraceCollider>, Added<RigidBody>)>)>,
) {
    for (e, col, disabled) in &bodies {
        if col.enabled && disabled {
            commands.entity(e).remove::<ColliderDisabled>();
        } else if !col.enabled && !disabled {
            commands.entity(e).insert(ColliderDisabled);
        }
    }
}

/// Bone followers are kinematic bodies carried by their joints.
fn attach_follower_bodies(
    mut commands: Commands,
    new: Query<(Entity, &TraceCollider), (With<portal2_world::collision::BoneFollower>, Without<RigidBody>)>,
) {
    for (e, col) in &new {
        let Some(collider) = shape_collider(&col.shape) else { continue };
        commands.entity(e).insert((RigidBody::Kinematic, collider, layers_for(layers::FOLLOWER, layers::PROP)));
    }
}

/// The air drag physics props fly with (faith plates take it off for a
/// launch, see `devices::catapult`).
pub const PROP_LINEAR_DAMPING: f32 = 0.05;
pub const PROP_ANGULAR_DAMPING: f32 = 0.2;

/// Simulates `e` as a free physics prop from now on.
pub fn make_dynamic(commands: &mut Commands, e: Entity, collider: Collider, mass: f32) {
    commands.entity(e).insert((
        RigidBody::Dynamic,
        collider,
        Mass(mass),
        Friction::new(0.8),
        Restitution::new(0.05),
        LinearDamping(PROP_LINEAR_DAMPING),
        AngularDamping(PROP_ANGULAR_DAMPING),
        SweptCcd::default(),
        // Drawn eased between physics ticks, like the player's view; a
        // carried prop would otherwise judder against it at high frame rates.
        TransformInterpolation,
        layers_for(layers::PROP, layers::WORLD | layers::PROP | layers::PLAYER | layers::MOVER | layers::FOLLOWER | layers::HELD),
        ActiveCollisionHooks::MODIFY_CONTACTS,
        // For impact sounds (`sound::impacts`).
        CollisionEventsEnabled,
        portal_travel::PortalTraveler::default(),
    ));
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
    sleepers: Query<(Entity, &TraceCollider, &Position, &Rotation), With<Sleeping>>,
    props: Query<(), With<PhysicsProp>>,
    mut contacts: ResMut<ContactGraph>,
) {
    if !holes.is_changed() {
        return;
    }
    // Props at rest where an opening appeared (a cube on the floor a
    // portal is shot under) start moving again, so they can drop in.
    for (e, col, pos, rot) in &sleepers {
        let (mins, maxs) = world_bounds(&col.shape, pos.0, rot.0);
        let (center, half) = ((mins + maxs) * 0.5, (maxs - mins) * 0.5 + Vec3::ONE);
        if holes.holes.iter().any(|h| h.overlaps_box(center, -half, half)) {
            commands.entity(e).remove::<Sleeping>();
        }
    }
    // A prop that has hardly moved keeps the contacts worked out before
    // (the narrow phase recycles them without asking the contact hooks),
    // so the floor under one at rest would still hold it up: its contacts
    // are worked out afresh against the new openings.
    let forget = |pairs: &mut [ContactPair]| {
        for pair in pairs.iter_mut().filter(|p| props.contains(p.collider1) || props.contains(p.collider2)) {
            pair.recycling_cache = None;
        }
    };
    forget(contacts.active_pairs_mut());
    forget(contacts.sleeping_pairs_mut());
    for e in &slabs {
        commands.entity(e).despawn();
    }
    for hole in &holes.holes {
        for v in &hole.frame_volumes {
            if let Some(c) = shape_collider(&Shape::new(vec![v.clone()])) {
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
/// passing through it, so they can enter the wall the portal is on: the
/// static map's, and those of brush entities (a floor panel) the opening
/// cuts through, as traces treat them. A prop counts as passing through
/// over the whole tick's move: speculative contacts are made a tick ahead,
/// and one kept for a fast prop (a cube a faith plate flings at a portal)
/// would take its speed into the wall before it reached the opening.
#[derive(SystemParam)]
pub struct PortalContactHooks<'w, 's> {
    holes: Res<'w, PortalHoles>,
    time: Res<'w, Time<Fixed>>,
    velocities: Query<'w, 's, &'static LinearVelocity>,
    statics: Query<'w, 's, (), With<StaticWorld>>,
    props: Query<'w, 's, (), With<PhysicsProp>>,
    shapes: Query<'w, 's, (&'static TraceCollider, &'static Position, &'static Rotation)>,
}

impl CollisionHooks for PortalContactHooks<'_, '_> {
    fn modify_contacts(&self, contacts: &mut ContactPair, _commands: &mut Commands) -> bool {
        if self.holes.holes.is_empty() {
            return true;
        }
        let (prop, other) = if self.props.contains(contacts.collider1) {
            (contacts.collider1, contacts.collider2)
        } else if self.props.contains(contacts.collider2) {
            (contacts.collider2, contacts.collider1)
        } else {
            return true;
        };
        let static_map = self.statics.contains(other);
        let other_bounds = if static_map || self.props.contains(other) {
            None
        } else {
            self.shapes.get(other).ok().map(|(c, p, r)| world_bounds(&c.shape, p.0, r.0))
        };
        if !static_map && other_bounds.is_none() {
            return true;
        }
        let Ok((col, pos, rot)) = self.shapes.get(prop) else { return true };
        let (mins, maxs) = world_bounds(&col.shape, pos.0, rot.0);
        let step = self.velocities.get(prop).map_or(Vec3::ZERO, |v| v.0 * self.time.timestep().as_secs_f32());
        let (mins, maxs) = (mins.min(mins + step), maxs.max(maxs + step));
        let center = (mins + maxs) * 0.5;
        let half = (maxs - mins) * 0.5;
        for hole in &self.holes.holes {
            if !hole.overlaps_box(center, -half, half) || other_bounds.is_some_and(|(lo, hi)| !carves(hole, lo, hi)) {
                continue;
            }
            // Everything behind the opening's rectangle is ignored (the hole
            // the frame slabs line); the slabs keep the prop inside the
            // tunnel. A contact's point is midway across any gap, so a
            // speculative one is judged by where it lies on the wall.
            let f = &hole.frame;
            let toward_other = if other == contacts.collider2 { 0.5 } else { -0.5 };
            for m in &mut contacts.manifolds {
                let normal = m.normal;
                m.points.retain(|p| {
                    let on_other = p.point + normal * (-p.penetration).max(0.0) * toward_other;
                    let l = f.to_local(on_other);
                    !(l.x < 2.0 && l.y.abs() < HALF_WIDTH + 2.0 && l.z.abs() < HALF_HEIGHT + 2.0)
                });
            }
            contacts.manifolds.retain(|m| !m.points.is_empty());
        }
        !contacts.manifolds.is_empty()
    }
}

/// Whether an opening cuts through a moving solid with these world bounds.
pub fn carves(hole: &portal2_world::collision::portal::PortalHole, mins: Vec3, maxs: Vec3) -> bool {
    let corners: Vec<Vec3> = (0..8)
        .map(|i| {
            Vec3::new(
                if i & 1 == 0 { mins.x } else { maxs.x },
                if i & 2 == 0 { mins.y } else { maxs.y },
                if i & 4 == 0 { mins.z } else { maxs.z },
            )
        })
        .collect();
    hole.carves_bounds(&corners)
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
