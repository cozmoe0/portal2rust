//! `phys_hinge` and `phys_spring`: a physics object hinged to the world
//! (or another object) about the line from the hinge's origin through
//! `hingeaxis`, and springs from a point on an object (`attach1`, at the
//! spring's origin) to `springaxis` in the world (or on `attach2`). A
//! spring pulls (and pushes) with `constant` × stretch beyond `length`,
//! less `damping` × the speed the ends part at; `SetSpringConstant`,
//! `SetSpringLength` and `SetSpringDamping` change it. In the first map a
//! lab pod hangs from a hinge and two stiff springs until the springs
//! slacken and it tips over as the container ride passes.

use avian3d::prelude::*;
use bevy::prelude::*;

use crate::entities::io::{InputCall, InputHandlers};
use crate::entities::{SpawnHandlers, world_transform};
use portal2_core::entity::{EntityRegistry, MapEntity};
use portal2_world::MapObject;

pub fn register(app: &mut App) {
    app.add_systems(FixedUpdate, (build_hinges, spring_forces, drive_motors, build_point_constraints));
    let world = app.world_mut();
    {
        let mut s = world.resource_mut::<SpawnHandlers>();
        s.register("phys_hinge", spawn_hinge);
        s.register("phys_spring", spawn_spring);
        s.register("phys_motor", spawn_motor);
        s.register("phys_ballsocket", spawn_point_constraint);
        s.register("phys_ragdollconstraint", spawn_point_constraint);
        s.register("phys_lengthconstraint", spawn_point_constraint);
        // Groups constraints for the solver; nothing to do here.
        s.register("phys_constraintsystem", |_, _| {});
    }
    let mut h = world.resource_mut::<InputHandlers>();
    h.register("phys_spring", spring_input);
    h.register("phys_motor", motor_input);
    for class in ["phys_ballsocket", "phys_ragdollconstraint", "phys_lengthconstraint"] {
        h.register(class, point_constraint_input);
    }
}

pub fn register_save(r: &mut portal2_core::save::SaveRegistry) {
    r.component::<Spring>("phys_spring");
    r.component::<Motor>("phys_motor");
}

#[derive(Component)]
struct Hinge {
    attach1: String,
    attach2: Option<String>,
    origin: Vec3,
    axis: Vec3,
    built: bool,
}

#[derive(Component, serde::Serialize, serde::Deserialize)]
pub struct Spring {
    attach1: String,
    attach2: Option<String>,
    start: Vec3,
    end: Vec3,
    length: f32,
    constant: f32,
    damping: f32,
    /// The ends in their objects' space, once found.
    #[serde(skip)]
    local: Option<(Vec3, Vec3)>,
}

impl portal2_core::save::Restore for Spring {}

fn name_of(me: &MapEntity, key: &str) -> Option<String> {
    me.get(key).map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty())
}

fn spawn_hinge(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e) else { return };
    let origin = me.origin();
    let axis_point = me.get("hingeaxis").and_then(source_formats::kv::parse_vec3).unwrap_or(origin + Vec3::Z);
    let hinge = Hinge {
        attach1: name_of(me, "attach1").unwrap_or_default(),
        attach2: name_of(me, "attach2"),
        origin,
        axis: (axis_point - origin).normalize_or(Vec3::Z),
        built: false,
    };
    world.entity_mut(e).insert(hinge);
}

fn spawn_spring(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e) else { return };
    let start = me.origin();
    let spring = Spring {
        attach1: name_of(me, "attach1").unwrap_or_default(),
        attach2: name_of(me, "attach2"),
        start,
        end: me.get("springaxis").and_then(source_formats::kv::parse_vec3).unwrap_or(start),
        length: me.get_f32("length").unwrap_or(0.0),
        constant: me.get_f32("constant").unwrap_or(50.0),
        damping: me.get_f32("damping").unwrap_or(2.0),
        local: None,
    };
    world.entity_mut(e).insert(spring);
}

fn spring_input(world: &mut World, call: &InputCall) -> bool {
    let Some(mut s) = world.get_mut::<Spring>(call.target) else { return false };
    let f = source_formats::kv::parse_f32_lenient(&call.param);
    match call.input.as_str() {
        "setspringconstant" => s.constant = f,
        "setspringlength" => s.length = f,
        "setspringdamping" => s.damping = f,
        _ => return false,
    }
    true
}

fn body(registry: &EntityRegistry, name: &str, bodies: &Query<&RigidBody>) -> Option<Entity> {
    registry.first_named(name).filter(|&b| bodies.get(b).is_ok())
}

/// Whether the physics engine has taken in a body's mass. A body made
/// dynamic this tick still has the mass its collider gave it (hundreds of
/// times too heavy for a brush) until the next step; a joint solved across
/// that change turns its impulse up by the same factor and flings the body
/// (`sp_a2_core`'s fake vault door spun 34 radians a second on its hinge).
pub(crate) fn mass_settled(mass: Option<&Mass>, computed: Option<&ComputedMass>) -> bool {
    match (mass, computed) {
        // Colliders parented to the body (a door's handle) add theirs.
        (Some(m), Some(c)) => c.value() <= m.0 * 4.0,
        _ => true,
    }
}

/// Hinges join their objects once those exist as bodies; an object hinged
/// to the world turns about a fixed anchor.
fn build_hinges(
    mut hinges: Query<&mut Hinge>,
    registry: Res<EntityRegistry>,
    bodies: Query<&RigidBody>,
    masses: Query<(Option<&Mass>, Option<&ComputedMass>)>,
    mut commands: Commands,
) {
    for mut h in &mut hinges {
        if h.built {
            continue;
        }
        let Some(b1) = body(&registry, &h.attach1, &bodies) else { continue };
        if masses.get(b1).is_ok_and(|(m, c)| !mass_settled(m, c)) {
            continue;
        }
        let b2 = match &h.attach2 {
            Some(name) => match body(&registry, name, &bodies) {
                Some(b) => b,
                None => continue,
            },
            None => commands.spawn((Name::new("hinge anchor"), RigidBody::Static, Transform::from_translation(h.origin), MapObject)).id(),
        };
        let frame = JointFrame::global(Isometry3d::new(h.origin, Quat::from_rotation_arc(Vec3::Z, h.axis)));
        let mut joint = RevoluteJoint::new(b2, b1);
        joint.frame1 = frame;
        joint.frame2 = frame;
        commands.spawn((Name::new("phys_hinge joint"), joint, MapObject));
        h.built = true;
    }
}

fn spring_forces(world: &mut World) {
    let springs: Vec<Entity> = world.query_filtered::<Entity, With<Spring>>().iter(world).collect();
    for e in springs {
        let Some(s) = world.get::<Spring>(e) else { continue };
        let (attach1, attach2, start, end, length, constant, damping, local) =
            (s.attach1.clone(), s.attach2.clone(), s.start, s.end, s.length, s.constant, s.damping, s.local);
        let registry = world.resource::<EntityRegistry>();
        let Some(b1) = registry.first_named(&attach1) else { continue };
        let b2 = attach2.as_deref().and_then(|n| registry.first_named(n));
        let t1 = world_transform(world, b1).compute_transform();
        let t2 = b2.map(|b| world_transform(world, b).compute_transform());
        let (l1, l2) = local.unwrap_or_else(|| {
            (t1.compute_affine().inverse().transform_point3(start), t2.map_or(end, |t| t.compute_affine().inverse().transform_point3(end)))
        });
        if local.is_none()
            && let Some(mut s) = world.get_mut::<Spring>(e)
        {
            s.local = Some((l1, l2));
        }
        let p1 = t1.transform_point(l1);
        let p2 = t2.map_or(l2, |t| t.transform_point(l2));
        let span = p2 - p1;
        let distance = span.length();
        if distance < 1e-3 {
            continue;
        }
        let dir = span / distance;
        let mut q = world.query::<Forces>();
        let speed1 = q.get(world, b1).map(|f| f.velocity_at_point(p1).dot(dir)).unwrap_or(0.0);
        let speed2 = b2.and_then(|b| q.get(world, b).ok()).map_or(0.0, |f| f.velocity_at_point(p2).dot(dir));
        let force = dir * (constant * (distance - length) - damping * (speed1 - speed2));
        if let Ok(mut f) = q.get_mut(world, b1) {
            f.apply_force_at_point(force, p1);
        }
        if let Some(b) = b2
            && let Ok(mut f) = q.get_mut(world, b)
        {
            f.apply_force_at_point(-force, p2);
        }
    }
}

/// `phys_motor`: turns `attach1` about the line from the motor's origin
/// through `axis` at `speed` degrees a second while on. With spawnflag 4
/// ("hinge object") the object is also hinged on that line for good, so
/// once the motor stops it swings and falls on its own: `sp_a2_core`'s fake
/// vault door is pushed over this way and drops flat. Spawnflag 1 starts it
/// on. The hinge and motor are built once the object is a moving body.
#[derive(Component, serde::Serialize, serde::Deserialize)]
pub struct Motor {
    attach1: String,
    origin: Vec3,
    axis: Vec3,
    /// Degrees a second.
    speed: f32,
    on: bool,
    hinge: bool,
    #[serde(skip)]
    joint: Option<Entity>,
}

impl portal2_core::save::Restore for Motor {}

const SF_MOTOR_START_ON: u32 = 1;
const SF_MOTOR_HINGE: u32 = 4;

fn spawn_motor(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e) else { return };
    let origin = me.origin();
    let axis_point = me.get("axis").and_then(source_formats::kv::parse_vec3).unwrap_or(origin + Vec3::Z);
    let motor = Motor {
        attach1: name_of(me, "attach1").unwrap_or_default(),
        origin,
        axis: (axis_point - origin).normalize_or(Vec3::Z),
        speed: me.get_f32("speed").unwrap_or(0.0),
        on: me.has_flag(SF_MOTOR_START_ON),
        hinge: me.has_flag(SF_MOTOR_HINGE),
        joint: None,
    };
    world.entity_mut(e).insert(motor);
}

fn motor_input(world: &mut World, call: &InputCall) -> bool {
    let Some(mut m) = world.get_mut::<Motor>(call.target) else { return false };
    match call.input.as_str() {
        "turnon" => m.on = true,
        "turnoff" => m.on = false,
        "setspeed" => m.speed = source_formats::kv::parse_f32_lenient(&call.param),
        _ => return false,
    }
    true
}

/// How hard a motor drives its object toward its speed. The solver turns
/// this times the speed still missing into the change of speed of each
/// substep, so 1 would reach the speed at once and anything above
/// overshoots: at 20, `sp_a2_core`'s fake vault door was flung about at 20
/// times the motor's speed. A quarter reaches it within a tick.
const MOTOR_DAMPING: f32 = 0.25;

fn drive_motors(
    mut motors: Query<&mut Motor>,
    registry: Res<EntityRegistry>,
    bodies: Query<&RigidBody>,
    mut joints: Query<&mut RevoluteJoint>,
    mut velocities: Query<&mut AngularVelocity>,
    masses: Query<(Option<&Mass>, Option<&ComputedMass>)>,
    mut commands: Commands,
) {
    for mut m in &mut motors {
        let Some(object) = registry.first_named(&m.attach1).filter(|&b| bodies.get(b).is_ok()) else { continue };
        // An object held still (`DisableMotion`) is not turned: the physics
        // engine would carry a kinematic body round at the motor's speed
        // (the fake vault door rose off the floor and swung over).
        if !bodies.get(object).is_ok_and(|b| b.is_dynamic()) {
            if let Some(mut joint) = m.joint.and_then(|j| joints.get_mut(j).ok())
                && joint.motor.enabled
            {
                joint.motor.enabled = false;
            }
            continue;
        }
        if masses.get(object).is_ok_and(|(mass, c)| !mass_settled(mass, c)) {
            continue;
        }
        // The joint drives its second body (the object) relative to the
        // first (the anchor), right-handed about the axis as the engine's
        // motor turns: `sp_a2_core`'s door, -100 about +Y, tips towards -X,
        // away from the wall behind it.
        let target = m.speed.to_radians();
        // A body at rest is put to sleep, which a joint motor does not wake.
        if m.on {
            commands.entity(object).remove::<Sleeping>();
        }
        if m.hinge {
            match m.joint.and_then(|j| joints.get_mut(j).ok()) {
                Some(mut joint) => {
                    joint.motor.enabled = m.on;
                    joint.motor.target_velocity = target;
                }
                None if m.joint.is_none() => {
                    let anchor = commands
                        .spawn((Name::new("motor anchor"), RigidBody::Static, Transform::from_translation(m.origin), MapObject))
                        .id();
                    let frame = JointFrame::global(Isometry3d::new(m.origin, Quat::from_rotation_arc(Vec3::Z, m.axis)));
                    let mut joint = RevoluteJoint::new(anchor, object);
                    joint.frame1 = frame;
                    joint.frame2 = frame;
                    joint.motor = AngularMotor::new(MotorModel::AccelerationBased { stiffness: 0.0, damping: MOTOR_DAMPING })
                        .with_target_velocity(target)
                        .with_max_torque(f32::MAX);
                    joint.motor.enabled = m.on;
                    m.joint = Some(commands.spawn((Name::new("phys_motor joint"), joint, MapObject)).id());
                    debug!("phys_motor: hinged {object} about {:?} through {:?}", m.axis, m.origin);
                }
                None => {}
            }
        } else if m.on
            && let Ok(mut w) = velocities.get_mut(object)
        {
            // Spun about the axis, keeping any spin it has about others.
            let along = w.0.dot(m.axis);
            w.0 += m.axis * (m.speed.to_radians() - along);
        }
    }
}



/// `phys_ballsocket` and `phys_ragdollconstraint` (two objects, or one and
/// the world, joined at the constraint's origin, free to turn about it) and
/// `phys_lengthconstraint` (a rope or rod between `attach1` at the origin
/// and `attach2` at `attachpoint`, `minlength` to its length plus
/// `addlength`; spawnflag 2 makes it rigid). `sp_a2_bts5`'s cut panels hang
/// from one corner by a ball socket once the laser has cut the weld, until
/// the socket is cut too. Spawnflags 4 and 16 wait for `TurnOn`; `TurnOff`,
/// `Break` and `Kill` let go. A ragdoll constraint also limits the turn
/// about each of its axes (`xmin`..`zmax`, degrees): `sp_a3_crazy_box`'s
/// glass box swings up to 90° but never spins about its rope (z 0..0).
#[derive(Component)]
pub struct PointConstraint {
    attach1: Option<String>,
    attach2: Option<String>,
    origin: Vec3,
    /// The other end of a length constraint.
    end: Option<Vec3>,
    min_length: f32,
    add_length: f32,
    rigid: bool,
    active: bool,
    built: bool,
    limits: Option<RagdollLimits>,
}

/// A ragdoll constraint's limits as a cone around its z axis (the largest
/// of the x and y ranges) and a twist range about that axis, in radians.
#[derive(Clone, Copy, Debug)]
struct RagdollLimits {
    axes: Quat,
    swing: f32,
    twist: (f32, f32),
}

fn ragdoll_limits(me: &MapEntity) -> RagdollLimits {
    let deg = |k: &str| me.get_f32(k).unwrap_or(0.0).to_radians();
    let swing = [deg("xmin"), deg("xmax"), deg("ymin"), deg("ymax")].into_iter().map(f32::abs).fold(0.0, f32::max);
    RagdollLimits { axes: portal2_core::coords::angles_to_quat(me.angles()), swing, twist: (deg("zmin"), deg("zmax")) }
}

const SF_CONSTRAINT_START_INACTIVE: u32 = 4;
const SF_CONSTRAINT_NO_CONNECT_UNTIL_ACTIVE: u32 = 16;
const SF_LENGTH_RIGID: u32 = 2;

fn spawn_point_constraint(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e) else { return };
    let length = me.classname == "phys_lengthconstraint";
    let c = PointConstraint {
        attach1: name_of(me, "attach1"),
        attach2: name_of(me, "attach2"),
        origin: me.origin(),
        end: length.then(|| me.get("attachpoint").and_then(source_formats::kv::parse_vec3).unwrap_or(me.origin())),
        min_length: me.get_f32("minlength").unwrap_or(0.0),
        add_length: me.get_f32("addlength").unwrap_or(0.0),
        rigid: length && me.has_flag(SF_LENGTH_RIGID),
        active: !(me.has_flag(SF_CONSTRAINT_START_INACTIVE) || me.has_flag(SF_CONSTRAINT_NO_CONNECT_UNTIL_ACTIVE)),
        built: false,
        limits: (me.classname == "phys_ragdollconstraint").then(|| ragdoll_limits(me)),
    };
    world.entity_mut(e).insert(c);
}

fn point_constraint_input(world: &mut World, call: &InputCall) -> bool {
    let e = call.target;
    let Some(mut c) = world.get_mut::<PointConstraint>(e) else { return false };
    match call.input.as_str() {
        "turnon" => c.active = true,
        "turnoff" | "break" => {
            c.active = false;
            c.built = false;
            world.entity_mut(e).remove::<(SphericalJoint, DistanceJoint)>();
            if call.input == "break" {
                crate::entities::io::fire_output(world, e, "OnBreak", call.activator, None);
            }
        }
        _ => return false,
    }
    true
}

/// Joins each active constraint's objects once they are bodies whose mass
/// the physics engine has taken in. The joint lives on the constraint, so
/// killing the constraint lets go; an end on the world is a fixed anchor
/// (cleared with the map).
fn build_point_constraints(
    mut constraints: Query<(Entity, &mut PointConstraint)>,
    registry: Res<EntityRegistry>,
    bodies: Query<&RigidBody>,
    masses: Query<(Option<&Mass>, Option<&ComputedMass>)>,
    mut commands: Commands,
) {
    for (e, mut c) in &mut constraints {
        if !c.active || c.built {
            continue;
        }
        let find = |name: &Option<String>| -> Result<Option<Entity>, ()> {
            match name {
                None => Ok(None),
                Some(n) => match body(&registry, n, &bodies) {
                    Some(b) if masses.get(b).is_ok_and(|(m, cm)| !mass_settled(m, cm)) => Err(()),
                    Some(b) => Ok(Some(b)),
                    None => Err(()),
                },
            }
        };
        let (Ok(b1), Ok(b2)) = (find(&c.attach1), find(&c.attach2)) else {
            if let Some(b) = c.attach1.as_deref().and_then(|n| registry.first_named(n)) {
                trace!(
                    "point constraint {e} waits: {:?} {:?} {:?}",
                    bodies.get(b).ok(),
                    masses.get(b).ok().map(|(m, _)| m.map(|m| m.0)),
                    masses.get(b).ok().and_then(|(_, c)| c.map(|c| c.value()))
                );
            }
            continue;
        };
        let (b1, b2, frame1, frame2) = match (b1, b2) {
            (None, None) => continue,
            (Some(a), Some(b)) => (a, b, c.origin, c.end.unwrap_or(c.origin)),
            (Some(a), None) | (None, Some(a)) => {
                let fixed_at = if b1.is_some() { c.end.unwrap_or(c.origin) } else { c.origin };
                let moving_at = if b1.is_some() { c.origin } else { c.end.unwrap_or(c.origin) };
                let anchor = commands.spawn((Name::new("constraint anchor"), RigidBody::Static, Transform::from_translation(fixed_at), MapObject)).id();
                (anchor, a, fixed_at, moving_at)
            }
        };
        let to_frame = |p: Vec3| JointFrame::global(Isometry3d::from_translation(p));
        match c.end {
            None => {
                let mut joint = SphericalJoint::new(b1, b2);
                joint.frame1 = to_frame(frame1);
                joint.frame2 = to_frame(frame1);
                if let Some(l) = c.limits {
                    let frame = |p: Vec3| JointFrame::global(Isometry3d::new(p, l.axes));
                    joint.frame1 = frame(frame1);
                    joint.frame2 = frame(frame1);
                    // Avian measures the swing cone about the axis it picks at
                    // right angles to `twist_axis` (-Z for its default Y) and
                    // the twist about that cone axis, so the default Y gives a
                    // tilt cone about the constraint's z and a turn about it.
                    joint = joint.with_twist_axis(Vec3::Y).with_swing_limits(0.0, l.swing).with_twist_limits(l.twist.0, l.twist.1);
                }
                debug!("point constraint {e} joins {b1} and {b2} at {frame1}");
                commands.entity(e).insert(joint);
            }
            Some(_) => {
                let length = frame1.distance(frame2) + c.add_length;
                let min = if c.rigid { length } else { c.min_length.min(length) };
                let joint = DistanceJoint::new(b1, b2)
                    .with_local_anchor1(Vec3::ZERO)
                    .with_local_anchor2(Vec3::ZERO)
                    .with_limits(min, length);
                // The anchors are given where the ends are in the world.
                let mut joint = joint;
                joint.anchor1 = JointAnchor::FromGlobal(frame1);
                joint.anchor2 = JointAnchor::FromGlobal(frame2);
                commands.entity(e).insert(joint);
            }
        }
        c.built = true;
    }
}
