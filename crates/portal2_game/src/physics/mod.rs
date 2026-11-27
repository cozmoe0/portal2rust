//! Rigid-body physics (Avian): the static map, moving brushes and props,
//! physics props such as cubes, carrying objects, and props moving through
//! portals.

pub mod carry;
pub mod constraints;
pub mod dissolve;
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
            .add_systems(PostUpdate, apply_debris)
            .add_systems(Update, fold_child_bodies.after(attach_bodies))
            .add_systems(FixedUpdate, rest_new_props)
            .add_systems(
                FixedPostUpdate,
                drive_kinematic_bodies.in_set(PhysicsSystems::Prepare).after(avian3d::physics_transform::PhysicsTransformSystems::TransformToPosition),
            )
            .init_resource::<RiderPlacements>()
            .add_systems(FixedPostUpdate, remember_rider_placements.before(PhysicsSystems::Prepare))
            .add_systems(
                FixedPostUpdate,
                (keep_followers_on_joints, restore_rider_placements)
                    .after(avian3d::physics_transform::PhysicsTransformSystems::PositionToTransform),
            )
            .add_plugins((carry::CarryPlugin, held_clone::HeldClonePlugin, portal_travel::PortalTravelPlugin, dissolve::DissolvePlugin));
        inputs::register(app);
        constraints::register(app);
        app.init_resource::<DisabledPairs>().add_systems(FixedUpdate, gather_collision_pairs);
        let world = app.world_mut();
        world.resource_mut::<crate::entities::SpawnHandlers>().register("logic_collision_pair", spawn_collision_pair);
        world.resource_mut::<crate::entities::io::InputHandlers>().register("logic_collision_pair", collision_pair_input);
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

/// A physics prop that starts at rest where the map put it, until something
/// moves it: a floor turret placed with a leg over a gap
/// (`sp_a2_turret_intro`'s first, standing on the broken floor) stands
/// there as in the game rather than toppling the moment the map starts.
/// The physics engine wakes a body as it first joins the simulation, so
/// it is put to sleep a couple of ticks in (the physics ticks it has had).
#[derive(Component, Default)]
pub struct StartsAtRest(pub u8);

fn rest_new_props(mut commands: Commands, mut props: Query<(Entity, &mut StartsAtRest), With<RigidBody>>) {
    for (e, mut rest) in &mut props {
        rest.0 += 1;
        if rest.0 >= 2 {
            commands.entity(e).insert(Sleeping).remove::<StartsAtRest>();
        }
    }
}

/// Gives every entity with trace collision a matching physics body: physics
/// props become dynamic, everything else solid becomes kinematic.
fn attach_bodies(
    mut commands: Commands,
    fs: Res<GameFs>,
    map: Option<Res<CurrentMap>>,
    new: Query<(Entity, &TraceCollider, &MapEntity, Has<PhysicsProp>, Has<ChildOf>), (Without<RigidBody>, Without<RidesPhysicsObject>)>,
) {
    let bsp = map.as_ref().map(|m| m.bsp.as_ref());
    for (e, col, me, is_prop, parented) in &new {
        if !col.solid {
            continue;
        }
        // A `func_physbox` without its motion disabled is a physics object
        // from the start (`sp_a2_bts3`'s lamp), like a prop.
        let is_prop = is_prop || is_physbox(me);
        let collider = if is_prop { prop_collider(&col.shape) } else { shape_collider(&col.shape) };
        let Some(collider) = collider else { continue };
        if is_prop && !parented && !motion_disabled(me) {
            make_dynamic(&mut commands, e, collider, prop_mass(&fs, bsp, me));
            commands.entity(e).insert(PhysicsProp);
        } else {
            // Spawnflag 8 on physics props: start motion disabled.
            commands.entity(e).insert((RigidBody::Kinematic, collider, layers_for(layers::MOVER, layers::PROP | layers::HELD)));
        }
    }
}

/// Physics prop spawnflag: motion disabled. `DisableMotion` and
/// `EnableMotion` keep it up to date, so a saved game knows too.
pub const SF_MOTION_DISABLED: u32 = 8;
/// Physics prop spawnflag: debris, which never touches the player.
pub const SF_DEBRIS: u32 = 4;

/// `func_physbox` spawnflags, which differ from the props' (`physobj.h`).
pub mod physbox_flags {
    pub const IGNORE_USE: u32 = 0x2000;
    pub const DEBRIS: u32 = 0x4000;
    pub const MOTION_DISABLED: u32 = 0x8000;
    pub const NEVER_PICK_UP: u32 = 0x20_0000;
}

pub fn is_physbox(me: &MapEntity) -> bool {
    me.classname.starts_with("func_physbox")
}

/// The spawnflag that holds an object still, which `func_physbox` keeps
/// elsewhere than props.
fn motion_flag(me: &MapEntity) -> u32 {
    if is_physbox(me) { physbox_flags::MOTION_DISABLED } else { SF_MOTION_DISABLED }
}

pub fn motion_disabled(me: &MapEntity) -> bool {
    me.has_flag(motion_flag(me))
}

pub fn set_motion_disabled(me: &mut MapEntity, disabled: bool) {
    let flag = motion_flag(me);
    me.set_flag(flag, disabled);
}

/// Debris collides with the world and other objects but never with the
/// player, who walks through it and cannot stand on it: `sp_a2_core`'s fake
/// vault door falls over through where the player stands, `sp_a2_bts5`'s
/// cut panel halves drop past the catwalks.
pub fn is_debris(me: &MapEntity) -> bool {
    if is_physbox(me) {
        me.has_flag(physbox_flags::DEBRIS)
    } else {
        me.classname.starts_with("prop_physics") && me.has_flag(SF_DEBRIS)
    }
}

/// Whether the player can never pick the object up.
pub fn prevents_pickup(me: &MapEntity) -> bool {
    if is_physbox(me) {
        me.has_flag(physbox_flags::IGNORE_USE | physbox_flags::NEVER_PICK_UP)
    } else {
        me.has_flag(crate::entities::props::SF_PREVENT_PICKUP)
    }
}

/// A solid brush parented to an object that moves freely, which drops out
/// of the physics simulation: the physics engine keeps nested bodies
/// apart, so on its own body `sp_a2_core`'s fake vault door handle stayed
/// hanging on the wall while the door fell, and taken into the door's body
/// it swung through the floor as it turned and threw the door about. It
/// follows the object and still stops the player and traces. A model
/// riding one keeps its collider as a weightless part of the object's body.
#[derive(Component)]
pub struct RidesPhysicsObject;

fn fold_child_bodies(world: &mut World) {
    let movers: Vec<Entity> = world
        .query_filtered::<(Entity, &RigidBody), (With<PhysicsProp>, Changed<RigidBody>)>()
        .iter(world)
        .filter(|(_, b)| b.is_dynamic())
        .map(|(e, _)| e)
        .collect();
    let mut riders = Vec::new();
    for body in movers {
        let mut stack: Vec<Entity> = world.get::<Children>(body).map(|c| c.iter().collect()).unwrap_or_default();
        while let Some(child) = stack.pop() {
            if world.get::<PhysicsProp>(child).is_some() {
                continue;
            }
            if world.get::<RigidBody>(child).is_some_and(|b| b.is_kinematic()) {
                riders.push(child);
            }
            if let Some(children) = world.get::<Children>(child) {
                stack.extend(children.iter());
            }
        }
    }
    // Bodies that join an object already moving: `sp_a3_crazy_box`'s
    // cracked panels are parented to the box at map spawn and get theirs
    // after it, and left on their own they did not follow it, so the cube
    // slipped out once the intact panel in front went.
    let late: Vec<Entity> = world
        .query_filtered::<(Entity, &RigidBody), (Without<PhysicsProp>, Or<(Added<RigidBody>, Changed<ChildOf>)>)>()
        .iter(world)
        .filter(|(_, b)| b.is_kinematic())
        .map(|(e, _)| e)
        .collect();
    for child in late {
        let mut up = world.get::<ChildOf>(child).map(|c| c.parent());
        while let Some(p) = up {
            if world.get::<PhysicsProp>(p).is_some() {
                if world.get::<RigidBody>(p).is_some_and(|b| b.is_dynamic()) && !riders.contains(&child) {
                    riders.push(child);
                }
                break;
            }
            up = world.get::<ChildOf>(p).map(|c| c.parent());
        }
    }
    for child in riders {
        // A model riding the object (`sp_a3_crazy_box`'s glass panels on
        // the hanging box) becomes part of its body, weightless, so what
        // is inside bounces off it rather than flying through.
        let model = world.get::<MapEntity>(child).is_some_and(|me| me.brush_model().is_none() && me.studio_model().is_some());
        debug!(
            "{child} ({:?}) rides a physics object; collider kept: {model}",
            world.get::<MapEntity>(child).and_then(|m| m.targetname.clone())
        );
        let mut c = world.entity_mut(child);
        c.remove::<(RigidBody, KinematicTrack, LinearVelocity, AngularVelocity)>().insert(RidesPhysicsObject);
        if model {
            c.insert(ColliderDensity(0.0));
        } else {
            c.remove::<Collider>();
        }
    }
}

/// Debris bodies stop colliding with the player's body and its movement.
fn apply_debris(
    mut commands: Commands,
    mut bodies: Query<(Entity, &MapEntity, &CollisionLayers, &mut TraceCollider), Changed<CollisionLayers>>,
) {
    for (e, me, collision, mut col) in &mut bodies {
        if !is_debris(me) {
            continue;
        }
        if collision.filters.0 & layers::PLAYER != 0 {
            commands.entity(e).insert(CollisionLayers::new(collision.memberships, LayerMask(collision.filters.0 & !layers::PLAYER)));
        }
        if !col.debris {
            col.debris = true;
        }
    }
}

/// A physics object's mass times `massscale`: a brush entity's
/// (`func_physbox`) is the one vbsp stored with its physics, a prop's its
/// model's (40 without one). `sp_a3_crazy_box`'s hanging glass box weighs
/// 875 × 0.5; taken as 40 × 0.5 a bouncing cube flung it about.
pub fn prop_mass(fs: &GameFs, bsp: Option<&source_formats::bsp::Bsp>, me: &MapEntity) -> f32 {
    let base = match (me.brush_model().filter(|&m| m > 0), bsp) {
        (Some(model), Some(bsp)) => portal2_world::collision::build::model_physics_mass(bsp, model),
        _ => None,
    };
    let base = base.or_else(|| me.studio_model().and_then(|p| model_mass(fs, &p))).unwrap_or(40.0);
    me.get_f32("massscale").filter(|s| *s > 0.0).unwrap_or(1.0) * base
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
    let frozen = motion_disabled(me);
    if frozen || world.get::<PhysicsProp>(e).is_none() || world.get::<RigidBody>(e).is_none_or(|b| b.is_dynamic()) {
        return;
    }
    let bsp = world.get_resource::<CurrentMap>().map(|m| m.bsp.clone());
    let mass = prop_mass(world.resource::<GameFs>(), bsp.as_deref(), me);
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

/// Where a moving brush or bone follower was at the last physics step.
#[derive(Component, Default)]
pub struct KinematicTrack(Option<(Vec3, Quat)>);

/// Farther than this in one tick (or turned more than a quarter turn), a
/// moving body was put somewhere rather than moved there.
const KINEMATIC_TELEPORT: f32 = 128.0;

/// Moving brushes and bone followers are placed each tick by the game
/// (doors, trains, animated arms). Placed as such, the physics engine sees
/// a body that jumped into the props in its way and pushes them out of it
/// all at once, flinging them: `sp_a2_core`'s fake vault door, shoved over
/// by two closing brushes, flew about the room. As in the engine, where
/// these are shadow-controlled objects moving at the speed they cover, each
/// one is put back where it was and given the velocity that carries it to
/// its new placement over the step, so what it pushes is pushed along.
///
/// The physics engine turns a body about its centre of mass and moves that
/// centre at the linear velocity, so the velocity is the centre's, not the
/// origin's: a body turning about an origin away from its centre would
/// otherwise slide sideways as it turned. `sp_a1_wakeup`'s breaker room,
/// whose centre lies 16 units off its origin, turns a quarter turn over a
/// one unit move; the slide made it turn further each tick, so it spun
/// round in three ticks (flinging the player out of it) and came to rest
/// 22 units off its track.
fn drive_kinematic_bodies(
    mut commands: Commands,
    time: Res<Time<Fixed>>,
    mut bodies: Query<
        (
            Entity,
            &RigidBody,
            &mut Position,
            &mut Rotation,
            &mut LinearVelocity,
            &mut AngularVelocity,
            Option<&mut KinematicTrack>,
            Option<&ComputedCenterOfMass>,
        ),
        (Without<PhysicsProp>, Without<Player>),
    >,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    for (e, body, mut position, mut rotation, mut linear, mut angular, track, com) in &mut bodies {
        if !body.is_kinematic() {
            continue;
        }
        let Some(mut track) = track else {
            commands.entity(e).insert(KinematicTrack(Some((position.0, rotation.0))));
            continue;
        };
        let (to, to_rot) = (position.0, rotation.0);
        let (mut v, mut w) = (Vec3::ZERO, Vec3::ZERO);
        if let Some((from, from_rot)) = track.0 {
            let mut turn = to_rot * from_rot.inverse();
            if turn.w < 0.0 {
                turn = -turn;
            }
            let moved = to - from;
            if moved.length() <= KINEMATIC_TELEPORT && turn.w >= std::f32::consts::FRAC_1_SQRT_2 && (moved != Vec3::ZERO || turn != Quat::IDENTITY) {
                let com = com.map_or(Vec3::ZERO, |c| c.0);
                v = (moved + to_rot * com - from_rot * com) / dt;
                w = turn.to_scaled_axis() / dt;
                position.0 = from;
                rotation.0 = from_rot;
            }
        }
        if linear.0 != v {
            linear.0 = v;
        }
        if angular.0 != w {
            angular.0 = w;
        }
        track.0 = Some((to, to_rot));
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

/// Bone followers sit exactly on their joints. After each step the physics
/// engine writes a body's place back into its transform relative to its
/// parent, so wherever it left a follower short of its joint became a
/// lasting offset from it: a moving model's collision fell a tick behind
/// and stayed that far off once it stopped, and a joint that jumped (a
/// sequence restarted part way through) left its follower where it was.
/// `sp_a2_column_blocker`'s exit door, opened, closed and opened again
/// within half a second as a relay flickered, stood open with its leaves'
/// collision still across the doorway.
fn keep_followers_on_joints(mut followers: Query<&mut Transform, With<portal2_world::collision::BoneFollower>>) {
    for mut t in &mut followers {
        t.set_if_neq(Transform::IDENTITY);
    }
}

/// Where each kinematic body riding a parent sits on it, kept across the
/// physics step (see `restore_rider_placements`).
#[derive(Resource, Default)]
struct RiderPlacements(Vec<(Entity, Transform)>);

fn remember_rider_placements(
    mut kept: ResMut<RiderPlacements>,
    riders: Query<(Entity, &Transform, &RigidBody), (With<ChildOf>, Without<portal2_world::collision::BoneFollower>)>,
) {
    kept.0.clear();
    kept.0.extend(riders.iter().filter(|(_, _, body)| body.is_kinematic()).map(|(e, t, _)| (e, *t)));
}

/// A kinematic body riding a parent (a wall panel on a room arm, a brush on
/// a train) stays where it sits on it: the physics engine's write-back,
/// like a bone follower's, turned wherever the step left it into a new
/// offset, so `sp_a2_core`'s trap walls closed with their panels shifted,
/// tilted up to ten degrees and gapped.
fn restore_rider_placements(kept: Res<RiderPlacements>, mut transforms: Query<&mut Transform>) {
    for (e, t) in &kept.0 {
        if let Ok(mut current) = transforms.get_mut(*e) {
            current.set_if_neq(*t);
        }
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
        // Swept against moving objects too, as the game's continuous physics
        // is: a bounce-painted cube at 500 u/s otherwise passed through the
        // inch-thin lid and glass of `sp_a3_crazy_box`'s swinging box.
        SweptCcd::default().with_filter(CcdFilter::ALL),
        // Drawn eased between physics ticks, like the player's view; a
        // carried prop would otherwise judder against it at high frame rates.
        TransformInterpolation,
        layers_for(layers::PROP, layers::WORLD | layers::PROP | layers::PLAYER | layers::MOVER | layers::FOLLOWER | layers::HELD),
        ActiveCollisionHooks::MODIFY_CONTACTS | ActiveCollisionHooks::FILTER_PAIRS,
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
    parents: Query<'w, 's, &'static ChildOf>,
    pairs: Res<'w, DisabledPairs>,
}

impl PortalContactHooks<'_, '_> {
    /// Whether `ancestor` is somewhere up `e`'s parent chain.
    fn descends_from(&self, mut e: Entity, ancestor: Entity) -> bool {
        for _ in 0..16 {
            match self.parents.get(e) {
                Ok(c) if c.parent() == ancestor => return true,
                Ok(c) => e = c.parent(),
                Err(_) => return false,
            }
        }
        false
    }
}

impl CollisionHooks for PortalContactHooks<'_, '_> {
    /// An object never collides with what is parented to it, nor with what
    /// it is parented to, as in the engine: `sp_a2_core`'s fake vault door
    /// carries its handle (a rotating brush inside it), which otherwise
    /// threw the door about the room the moment it could move.
    fn filter_pairs(&self, collider1: Entity, collider2: Entity, _commands: &mut Commands) -> bool {
        let key = if collider1 < collider2 { (collider1, collider2) } else { (collider2, collider1) };
        !(self.pairs.0.contains(&key) || self.descends_from(collider1, collider2) || self.descends_from(collider2, collider1))
    }

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

/// `logic_collision_pair`: two objects (`attach1`, `attach2`) that do not
/// collide with each other while disabled (`startdisabled` 1 starts so;
/// `DisableCollisions` / `EnableCollisions`). `sp_a2_core`'s closing
/// blockers pass through the fake vault door this way.
#[derive(Component, serde::Serialize, serde::Deserialize)]
pub struct CollisionPair {
    attach1: String,
    attach2: String,
    pub disabled: bool,
}

impl portal2_core::save::Restore for CollisionPair {}

/// The object pairs that do not collide now, ordered.
#[derive(Resource, Default)]
pub struct DisabledPairs(pub bevy::platform::collections::HashSet<(Entity, Entity)>);

fn spawn_collision_pair(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e) else { return };
    let name = |k: &str| me.get(k).unwrap_or("").trim().to_ascii_lowercase();
    let pair = CollisionPair { attach1: name("attach1"), attach2: name("attach2"), disabled: me.get_bool("startdisabled") };
    world.entity_mut(e).insert(pair);
}

fn collision_pair_input(world: &mut World, call: &crate::entities::io::InputCall) -> bool {
    let Some(mut pair) = world.get_mut::<CollisionPair>(call.target) else { return false };
    match call.input.as_str() {
        "disablecollisions" => pair.disabled = true,
        "enablecollisions" => pair.disabled = false,
        _ => return false,
    }
    true
}

/// The pair filter runs for a pair once either object asks for it.
fn gather_collision_pairs(
    mut commands: Commands,
    pairs: Query<&CollisionPair>,
    registry: Res<portal2_core::entity::EntityRegistry>,
    hooks: Query<Option<&ActiveCollisionHooks>, With<RigidBody>>,
    mut disabled: ResMut<DisabledPairs>,
) {
    let mut now = bevy::platform::collections::HashSet::new();
    for pair in pairs.iter().filter(|p| p.disabled) {
        let (Some(a), Some(b)) = (registry.first_named(&pair.attach1), registry.first_named(&pair.attach2)) else { continue };
        now.insert(if a < b { (a, b) } else { (b, a) });
        for e in [a, b] {
            if let Ok(current) = hooks.get(e) {
                let current = current.copied().unwrap_or(ActiveCollisionHooks::empty());
                if !current.contains(ActiveCollisionHooks::FILTER_PAIRS) {
                    commands.entity(e).insert(current | ActiveCollisionHooks::FILTER_PAIRS);
                }
            }
        }
    }
    if disabled.0 != now {
        disabled.0 = now;
    }
}
