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
use crate::world::MapObject;
use portal2_core::entity::{EntityRegistry, MapEntity};

pub fn register(app: &mut App) {
    app.add_systems(FixedUpdate, (build_hinges, spring_forces));
    let world = app.world_mut();
    {
        let mut s = world.resource_mut::<SpawnHandlers>();
        s.register("phys_hinge", spawn_hinge);
        s.register("phys_spring", spawn_spring);
    }
    world.resource_mut::<InputHandlers>().register("phys_spring", spring_input);
}

pub fn register_save(r: &mut portal2_core::save::SaveRegistry) {
    r.component::<Spring>("phys_spring");
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

/// Hinges join their objects once those exist as bodies; an object hinged
/// to the world turns about a fixed anchor.
fn build_hinges(mut hinges: Query<&mut Hinge>, registry: Res<EntityRegistry>, bodies: Query<&RigidBody>, mut commands: Commands) {
    for mut h in &mut hinges {
        if h.built {
            continue;
        }
        let Some(b1) = body(&registry, &h.attach1, &bodies) else { continue };
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
        let speed1 = q.get(world, b1).map(|f| f.velocity_at_point(p1.into()).dot(dir)).unwrap_or(0.0);
        let speed2 = b2.and_then(|b| q.get(world, b).ok()).map_or(0.0, |f| f.velocity_at_point(p2.into()).dot(dir));
        let force = dir * (constant * (distance - length) - damping * (speed1 - speed2));
        if let Ok(mut f) = q.get_mut(world, b1) {
            f.apply_force_at_point(force, p1.into());
        }
        if let Some(b) = b2
            && let Ok(mut f) = q.get_mut(world, b)
        {
            f.apply_force_at_point(-force, p2.into());
        }
    }
}
