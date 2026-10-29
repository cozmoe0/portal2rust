//! `env_ar2explosion`: a cloud of dust thrown out from a point when the
//! entity gets `Explode` (the first map's container breaking through the
//! exit wall). Facts in `docs/re/particles.md`, "AR2 explosion dust".
//!
//! Seventy puffs of the entity's material (`particle/particle_noisesphere`
//! by default). Each first tries three times to sit on the world: a chord
//! between two random points 240 units out is traced, and a puff goes where
//! it hits, flying off along its direction from the centre mirrored in the
//! surface at 10000 × (1 − distance / 240)² units/s and living 4 to 10 s.
//! The rest start 60 units out in a random direction, a further 0 to 50
//! counting towards their distance, at 10000 × (1 − distance / 120)² and
//! living 2 to 5 s. Puffs appear after `distance × 0.0000767 + 0.1` s.
//! Each frame (at most 0.05 s) a puff moves, grows by the distance it moved
//! from 8 units across, and slows by `100 / (moved² + 100)`; it spins at a
//! random rate that falls to 0.3 of itself each second while above 0.2.
//! Colour goes from (0.35, 0.345, 0.33) to (0.75, 0.75, 0.7) over its life;
//! it fades in over a quarter second to half opacity and then out, and is
//! hidden within 50 units of the view, fading in up to 150.

use bevy::camera::visibility::NoFrustumCulling;
use bevy::prelude::*;

use super::render::{self, Batch, Looks, ParticleMaterial};
use crate::audio::ops::random_range;
use crate::camera::MainCamera;
use crate::collision::Physics;
use crate::entities::io::{InputCall, InputHandlers};
use crate::entities::{MapEntity, SpawnHandlers, world_transform};
use crate::render::textures::TextureCache;
use crate::vfs::GameFs;
use crate::world::MapObject;

const PUFFS: usize = 70;
const RADIUS: f32 = 240.0;
const SPEED: f32 = 10000.0;
const START_SIZE: f32 = 8.0;
const FADE_IN: f32 = 0.25;
const ALPHA: f32 = 0.5;
const COLOR_START: Vec3 = Vec3::new(0.35, 0.345, 0.33);
const COLOR_END: Vec3 = Vec3::new(0.75, 0.75, 0.7);
const MAX_STEP: f32 = 0.05;

pub fn register(app: &mut App) {
    app.add_systems(PostUpdate, (simulate, draw).chain().after(TransformSystems::Propagate));
    let world = app.world_mut();
    world.resource_mut::<SpawnHandlers>().register("env_ar2explosion", spawn);
    world.resource_mut::<InputHandlers>().register("env_ar2explosion", input);
}

#[derive(Component)]
struct Ar2Explosion {
    material: String,
}

#[derive(Clone, Copy)]
struct Puff {
    position: Vec3,
    velocity: Vec3,
    /// Negative until the puff appears.
    age: f32,
    life: f32,
    size: f32,
    roll: f32,
    roll_speed: f32,
}

/// A cloud thrown out by one `Explode`.
#[derive(Component)]
struct DustCloud {
    material: String,
    puffs: Vec<Puff>,
}

fn spawn(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e) else { return };
    let material = me.get("material").filter(|m| !m.trim().is_empty()).unwrap_or("particle/particle_noisesphere").trim().to_owned();
    world.entity_mut(e).insert(Ar2Explosion { material });
}

fn input(world: &mut World, call: &InputCall) -> bool {
    if call.input != "explode" {
        return false;
    }
    let Some(material) = world.get::<Ar2Explosion>(call.target).map(|x| x.material.clone()) else { return false };
    let centre = world_transform(world, call.target).translation();
    let puffs = throw_puffs(world, centre);
    world.spawn((
        Name::new("ar2 explosion dust"),
        DustCloud { material, puffs },
        Mesh3d::default(),
        Transform::from_translation(centre),
        Visibility::Hidden,
        NoFrustumCulling,
        MapObject,
    ));
    true
}

fn unit_random() -> Vec3 {
    Vec3::new(random_range(-1.0, 1.0), random_range(-1.0, 1.0), random_range(-1.0, 1.0)).normalize_or(Vec3::X)
}

fn throw_puffs(world: &mut World, centre: Vec3) -> Vec<Puff> {
    let mut state: bevy::ecs::system::SystemState<Physics> = bevy::ecs::system::SystemState::new(world);
    let physics = state.get(world).expect("physics");
    let mut puffs = Vec::with_capacity(PUFFS);
    let delay = |distance: f32| -distance * 0.000_076_737_14 - 0.1;
    for _ in 0..PUFFS {
        for _ in 0..3 {
            let (a, b) = (centre + unit_random() * RADIUS, centre + unit_random() * RADIUS);
            let tr = physics.trace(a, b, Vec3::ZERO, Vec3::ZERO, crate::collision::mask::SOLID_BRUSH_ONLY, &[]);
            if !tr.hit() || tr.start_solid {
                continue;
            }
            let out = tr.end - centre;
            let distance = out.length().min(RADIUS);
            let dir = out.normalize_or_zero();
            let bounced = (dir - tr.normal * (2.0 * dir.dot(tr.normal))).normalize_or_zero();
            puffs.push(Puff {
                position: tr.end,
                velocity: bounced * SPEED * (1.0 - distance / RADIUS).powi(2),
                age: delay(distance),
                life: random_range(0.0, 6.0) + 4.0,
                size: START_SIZE,
                roll: random_range(0.0, std::f32::consts::TAU),
                roll_speed: random_range(-1.0, 1.0) * 0.4,
            });
            break;
        }
    }
    for _ in puffs.len()..PUFFS {
        let dir = unit_random();
        let position = centre + dir * RADIUS * 0.25;
        let distance = (RADIUS * 0.25 + random_range(0.0, 50.0)).min(RADIUS);
        puffs.push(Puff {
            position,
            velocity: dir * SPEED * (1.0 - distance / 120.0).powi(2),
            age: delay(distance),
            life: (random_range(0.0, 6.0) + 4.0) * 0.5,
            size: START_SIZE,
            roll: random_range(0.0, std::f32::consts::TAU),
            roll_speed: random_range(-1.0, 1.0) * 4.0,
        });
    }
    puffs
}

fn simulate(time: Res<Time>, mut clouds: Query<(Entity, &mut DustCloud)>, mut commands: Commands) {
    let dt = time.delta_secs();
    let step = dt.min(MAX_STEP);
    let roll_decay = 0.3f32.powf(step);
    for (e, mut cloud) in &mut clouds {
        cloud.puffs.retain_mut(|p| {
            p.age += step;
            if p.age > p.life {
                return false;
            }
            p.roll += p.roll_speed * dt;
            if p.age >= 0.0 {
                let moved = p.velocity * step;
                p.position += moved;
                p.size += moved.length();
                p.velocity *= 100.0 / (moved.length_squared() + 100.0);
                if p.roll_speed.abs() > 0.2 {
                    p.roll_speed *= roll_decay;
                }
            }
            true
        });
        if cloud.puffs.is_empty() {
            commands.entity(e).despawn();
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw(
    mut clouds: Query<(&DustCloud, &GlobalTransform, &mut Mesh3d, Option<&MeshMaterial3d<ParticleMaterial>>, &mut Visibility, Entity)>,
    main: Query<&GlobalTransform, With<MainCamera>>,
    mut looks: ResMut<Looks>,
    fs: Res<GameFs>,
    mut textures: ResMut<TextureCache>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<ParticleMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut commands: Commands,
) {
    let Ok(view) = main.single() else { return };
    let (eye, forward) = (view.translation(), Vec3::from(view.forward()));
    for (cloud, gt, mut mesh, material, mut vis, e) in &mut clouds {
        let look = looks.get(&cloud.material, &fs, &mut textures, &mut images, &mut materials);
        if material.is_none_or(|m| m.0 != look.material) {
            commands.entity(e).insert(MeshMaterial3d(look.material.clone()));
        }
        let origin = gt.translation();
        let mut order: Vec<(f32, &Puff)> =
            cloud.puffs.iter().filter(|p| p.age >= 0.0).map(|p| ((p.position - eye).dot(forward), p)).collect();
        order.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut batch = Batch::default();
        for (depth, p) in order {
            let near = if depth > 150.0 {
                1.0
            } else if depth > 50.0 {
                (depth - 50.0) * 0.01
            } else {
                0.0
            };
            let t = (p.age - FADE_IN) / p.life;
            let fade = if p.age >= FADE_IN { (1.0 - t) * ALPHA } else { p.age / FADE_IN * ALPHA };
            let colour = COLOR_START * (1.0 - t) + COLOR_END * t;
            let alpha = (near * fade).clamp(0.0, 1.0);
            if alpha > 0.0 {
                batch.sprite(p.position - origin, p.size, p.roll, [colour.x, colour.y, colour.z, alpha]);
            }
        }
        if batch.is_empty() {
            *vis = Visibility::Hidden;
            continue;
        }
        if mesh.0 == Handle::default() {
            mesh.0 = meshes.add(render::empty_mesh());
        }
        if let Some(mut m) = meshes.get_mut(&mesh.0) {
            batch.write(&mut m);
        }
        *vis = Visibility::Inherited;
    }
}
