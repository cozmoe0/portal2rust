//! A floor turret a laser set on fire (`turret`): its flames while it
//! burns, then how it blows apart.
//!
//! Burning (server.dll: the laser ignites it, `CEntityFlame`): the client's
//! `burning_character` smoke and flames ride it (control point 1, spread
//! over its model) and `General.BurningFlesh` loops until it goes.
//!
//! Blowing apart (server.dll `sub_103248D0`, `sub_103260E0`):
//!
//! - an `env_explosion` of magnitude 232 and radius 244 with no damage,
//!   smoke, sparks, sound, fireball smoke or light (spawn flags 1385), so
//!   its client effect is only the debris sparks: 8 to 16 streaks of
//!   `effects/spark` thrown out at 1500 to 2500 units/s for 0.1 to 0.15 s,
//!   2 to 16 units wide, slowing hard and falling;
//! - a screen shake of 20 at 150 Hz for 0.75 s, out to 750 units;
//! - blast damage of 15 out to 120 units, from 12 units above its centre;
//! - `NPC_FloorTurret.Destruct`, and its burning sound stops;
//! - the pieces its collision model lists (`turret.phy`'s `break`
//!   sections: ten `turret_fx_break_gib` models) where they sit in the
//!   whole, each thrown out from its centre at its `burst` speed (500) on
//!   top of the turret's own, tumbling at up to 800°/s each way, ignoring
//!   the player (`debris`) and gone after `fadetime` (20 s); each piece
//!   carries the particles its model names (`turret_burning_gib_01`,
//!   `turret_smoke_gib1`) at its attachment.
//!
//! With the visual overhaul: a flickering firelight on the burning turret,
//! and the blast flashes hot light around, throws a short-lived fireball of
//! `effects/fire_cloud1` and `2` billows and leaves rising dark smoke.

use avian3d::prelude::{AngularVelocity, CollisionLayers, LayerMask, LinearVelocity, RigidBody};
use bevy::camera::visibility::NoFrustumCulling;
use bevy::prelude::*;

use super::turret::{Turret, TurretExploded, TurretState};
use crate::entities::props::PhysicsProp;
use crate::player::Player;
use crate::player::health::DamagePlayer;
use crate::player::shake::{ShakeCommand, screen_shake};
use crate::visuals::fx::lights::{FxLightData, FxLightShape, FxLights, FxProduceSet, light_flags};
use crate::visuals::fx::{FxSettings, features};
use portal2_audio::SoundRequest;
use portal2_core::entity::MapEntity;
use portal2_core::rng::random_range;
use portal2_core::vfs::GameFs;
use portal2_particles::ParticleEffect;
use portal2_particles::render::{self, Batch, Looks, ParticleMaterial};
use portal2_render::models::StudioAnimator;
use portal2_render::textures::TextureCache;
use portal2_world::collision::Collider as TraceCollider;
use portal2_world::{MapLoaded, MapObject};

const FLAMES: &str = "burning_character";
const BURN_SOUND: &str = "General.BurningFlesh";
const STOP_BURN_SOUND: &str = "General.StopBurning";
const DESTRUCT_SOUND: &str = "NPC_FloorTurret.Destruct";
/// Height of the flames and the blast above the turret's origin.
const CENTER_HEIGHT: f32 = 24.0;
/// Screen shake: amplitude, frequency, seconds, radius.
const SHAKE: (f32, f32, f32, f32) = (20.0, 150.0, 0.75, 750.0);
const BLAST_DAMAGE: f32 = 15.0;
const BLAST_RADIUS: f32 = 120.0;
/// Pieces tumble at up to this many degrees a second about each axis.
const GIB_SPIN: f32 = 800.0;
/// Debris sparks.
const SPARK_MATERIAL: &str = "effects/spark";
const SPARK_COUNT: (f32, f32) = (8.0, 17.0);
const SPARK_SPEED: (f32, f32) = (1500.0, 2500.0);
const SPARK_LIFE: (f32, f32) = (0.1, 0.15);
const SPARK_WIDTH: (f32, f32) = (2.0, 16.0);
const SPARK_LENGTH: (f32, f32) = (0.05, 0.1);
const SPARK_GRAVITY: f32 = 200.0;
const SPARK_DAMPEN: f32 = 8.0;
/// The overhaul's fireball and smoke.
const FIRE_MATERIALS: [&str; 2] = ["effects/fire_cloud1", "effects/fire_cloud2"];
const SMOKE_MATERIAL: &str = "particle/particle_noisesphere";
const FIREBALLS: usize = 9;
const SMOKE_PUFFS: usize = 7;
/// The overhaul's lights: the blast's flash and a burning turret's glow.
const FLASH_COLOR: Vec3 = Vec3::new(1.0, 0.62, 0.3);
const FLASH_INTENSITY: f32 = 2600.0;
const FLASH_TIME: f32 = 0.45;
const FLASH_RANGE: f32 = 520.0;
const FIRE_COLOR: Vec3 = Vec3::new(1.0, 0.45, 0.12);
const FIRE_INTENSITY: f32 = 420.0;
const FIRE_RANGE: f32 = 260.0;

pub struct TurretBreakPlugin;

impl Plugin for TurretBreakPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Blasts>()
            .add_systems(Update, (ignite, follow_flames, blow_up, age_blasts, settle_pieces, expire_pieces).chain())
            .add_systems(PostUpdate, (draw_blasts.after(TransformSystems::Propagate), blast_lights.in_set(FxProduceSet)));
    }
}

/// A burning turret's flames.
#[derive(Component)]
struct Flames {
    effect: Entity,
}

/// A piece of a blown-apart turret.
#[derive(Component)]
pub struct Piece {
    /// Seconds left.
    life: f32,
    settled: bool,
}

#[derive(Clone, Copy, Debug)]
struct Spark {
    position: Vec3,
    velocity: Vec3,
    age: f32,
    life: f32,
    width: f32,
    length: f32,
}

/// A billow of the overhaul's fireball, or a puff of its smoke.
#[derive(Clone, Copy, Debug)]
struct Billow {
    material: usize,
    position: Vec3,
    velocity: Vec3,
    age: f32,
    life: f32,
    size: (f32, f32),
    roll: f32,
    spin: f32,
}

#[derive(Clone, Copy, Debug)]
struct Flash {
    position: Vec3,
    age: f32,
}

#[derive(Resource, Default)]
struct Blasts {
    sparks: Vec<Spark>,
    billows: Vec<Billow>,
    flashes: Vec<Flash>,
}

/// One batch per sprite material: sparks, the two fire billows, smoke.
#[derive(Component)]
struct BlastBatch(usize);
const BATCH_MATERIALS: [&str; 4] = [SPARK_MATERIAL, FIRE_MATERIALS[0], FIRE_MATERIALS[1], SMOKE_MATERIAL];
const SMOKE: usize = 3;

fn random_unit() -> Vec3 {
    Vec3::new(random_range(-1.0, 1.0), random_range(-1.0, 1.0), random_range(-1.0, 1.0)).normalize_or(Vec3::Z)
}

/// Flames and their sound on every burning turret (also one a saved game
/// left burning).
fn ignite(
    mut commands: Commands,
    turrets: Query<(Entity, &Turret, Option<&StudioAnimator>, Has<Flames>)>,
    mut sounds: MessageWriter<SoundRequest>,
) {
    for (e, t, anim, lit) in &turrets {
        if t.state != TurretState::Burning || lit {
            continue;
        }
        // The flames spread over the model.
        let mut flames = ParticleEffect::new(FLAMES, true);
        flames.model_box = Some(anim.map_or((Vec3::new(-12.0, -12.0, 0.0), Vec3::new(12.0, 12.0, 60.0)), |a| {
            let mdl = &a.model.studio.mdl;
            (mdl.hull_min, mdl.hull_max)
        }));
        let effect = commands
            .spawn((
                Name::new("turret flames"),
                Transform::from_xyz(0.0, 0.0, CENTER_HEIGHT),
                Visibility::default(),
                flames,
                MapObject,
                ChildOf(e),
            ))
            .id();
        commands.entity(e).try_insert(Flames { effect });
        sounds.write(SoundRequest::on(BURN_SOUND, e));
    }
}

/// The flames take the burning turret as their control point 1 (the
/// client's `CEntityFlame` does): they spread over its model from there.
fn follow_flames(turrets: Query<(&Flames, &GlobalTransform)>, mut effects: Query<&mut ParticleEffect>) {
    for (flames, gt) in &turrets {
        if let Ok(mut fx) = effects.get_mut(flames.effect) {
            fx.cp_frames.clear();
            fx.cp_frames.push((1, gt.compute_transform()));
        }
    }
}

/// The blast of each turret that blew apart this frame.
fn blow_up(
    mut exploded: MessageReader<TurretExploded>,
    mut blasts: ResMut<Blasts>,
    flames: Query<&Flames>,
    players: Query<&Player>,
    mut sounds: MessageWriter<SoundRequest>,
    mut damage: MessageWriter<DamagePlayer>,
    mut loaded: MessageReader<MapLoaded>,
    mut commands: Commands,
) {
    if loaded.read().last().is_some() {
        *blasts = Blasts::default();
    }
    for ev in exploded.read() {
        debug!("turret {} blew apart at {:?}", ev.entity, ev.at.translation);
        let up = ev.at.rotation * Vec3::Z;
        let center = ev.at.translation + up * CENTER_HEIGHT;
        sounds.write(SoundRequest::stop(BURN_SOUND, ev.entity));
        sounds.write(SoundRequest::at(STOP_BURN_SOUND, center));
        sounds.write(SoundRequest::at(DESTRUCT_SOUND, center));
        if let Ok(f) = flames.get(ev.entity) {
            commands.entity(f.effect).try_despawn();
        }
        // The blast hurts a player close by.
        let blast = center + up * 12.0;
        if let Ok(p) = players.single() {
            let (mins, maxs) = p.state.hull();
            let d = p.state.origin + (mins + maxs) * 0.5 - blast;
            let dist = d.length();
            if dist < BLAST_RADIUS {
                let amount = BLAST_DAMAGE * (1.0 - dist / BLAST_RADIUS);
                damage.write(DamagePlayer { amount, push: d.normalize_or(Vec3::Z) * amount * 4.0, source: None });
            }
        }
        add_blast(&mut blasts, center, up);
        let (model, at, velocity) = (ev.model.clone(), ev.at, ev.velocity);
        commands.queue(move |world: &mut World| {
            screen_shake(world, center, SHAKE.0, SHAKE.1, SHAKE.2, SHAKE.3, ShakeCommand::Start, false);
            if let Some(model) = model {
                spawn_pieces(world, &model, at, velocity);
            }
        });
    }
}

/// The debris sparks, and the overhaul's fireball, smoke and flash.
fn add_blast(blasts: &mut Blasts, center: Vec3, up: Vec3) {
    let count = random_range(SPARK_COUNT.0, SPARK_COUNT.1) as usize;
    for _ in 0..count {
        let dir = (random_unit() + up).normalize_or(up);
        blasts.sparks.push(Spark {
            position: center,
            velocity: dir * random_range(SPARK_SPEED.0, SPARK_SPEED.1),
            age: 0.0,
            life: random_range(SPARK_LIFE.0, SPARK_LIFE.1),
            width: random_range(SPARK_WIDTH.0, SPARK_WIDTH.1),
            length: random_range(SPARK_LENGTH.0, SPARK_LENGTH.1),
        });
    }
    for i in 0..FIREBALLS {
        let dir = random_unit();
        blasts.billows.push(Billow {
            material: 1 + i % 2,
            position: center + dir * random_range(2.0, 8.0),
            velocity: dir * random_range(40.0, 120.0) + up * 50.0,
            age: 0.0,
            life: random_range(0.3, 0.5),
            size: (random_range(6.0, 10.0), random_range(16.0, 26.0)),
            roll: random_range(0.0, std::f32::consts::TAU),
            spin: random_range(-2.0, 2.0),
        });
    }
    for _ in 0..SMOKE_PUFFS {
        let dir = random_unit();
        blasts.billows.push(Billow {
            material: SMOKE,
            position: center + dir * random_range(4.0, 16.0),
            velocity: dir * random_range(20.0, 60.0) + up * random_range(30.0, 70.0),
            age: -random_range(0.05, 0.25),
            life: random_range(1.8, 3.2),
            size: (random_range(10.0, 16.0), random_range(40.0, 64.0)),
            roll: random_range(0.0, std::f32::consts::TAU),
            spin: random_range(-0.6, 0.6),
        });
    }
    blasts.flashes.push(Flash { position: center, age: 0.0 });
}

/// The `break` sections of a model's collision model: each piece's model,
/// its burst speed and seconds before it goes.
pub fn break_pieces(fs: &GameFs, model: &str) -> Vec<(String, f32, f32)> {
    let base = source_formats::text::strip_extension(model);
    let Some(phy) = fs.read(&format!("{base}.phy")).and_then(|b| source_formats::phy::Phy::parse(&b).ok()) else {
        return Vec::new();
    };
    source_formats::kv::parse_document(&phy.keyvalues)
        .roots
        .iter()
        .filter(|r| r.key.eq_ignore_ascii_case("break"))
        .filter_map(|r| {
            let name = r.get_str("model")?.replace('\\', "/");
            let name = name.trim_start_matches('/');
            let path = if name.to_ascii_lowercase().starts_with("models/") { name.to_owned() } else { format!("models/{name}") };
            let path = if path.to_ascii_lowercase().ends_with(".mdl") { path } else { format!("{path}.mdl") };
            Some((path, r.get_f32("burst").unwrap_or(100.0), r.get_f32("fadetime").unwrap_or(20.0)))
        })
        .collect()
}

/// The particle systems a model's keyvalues name, with the attachment each
/// follows.
pub fn model_particles(keyvalues: &str) -> Vec<(String, Option<String>)> {
    let doc = source_formats::kv::parse_document(keyvalues);
    let mut out = Vec::new();
    for root in &doc.roots {
        for particles in root.get_all("particles") {
            for effect in particles.get_all("effect") {
                if let Some(name) = effect.get_str("name") {
                    out.push((name.to_owned(), effect.get_str("attachment_point").map(str::to_owned)));
                }
            }
        }
    }
    out
}

/// An attachment's placement in its model's space (rest pose).
fn attachment_matrix(mdl: &source_formats::mdl::Mdl, name: &str) -> Option<Mat4> {
    let a = mdl.attachments.get(mdl.find_attachment(name)?)?;
    let bone = mdl.bones.get(a.bone.max(0) as usize).map_or(Mat4::IDENTITY, |b| b.pose_to_bone.inverse());
    Some(bone * a.local)
}

/// The pieces of a model blown apart at `at` while moving at `velocity`.
fn spawn_pieces(world: &mut World, model: &str, at: Transform, velocity: Vec3) {
    let fs = world.resource::<GameFs>().clone();
    let origin = at.translation;
    for (path, burst, life) in break_pieces(&fs, model) {
        let Some(mdl) = fs.read(&path).and_then(|b| source_formats::mdl::Mdl::parse(b).ok()) else { continue };
        // Each piece is modelled where it sits in the whole, around its
        // `placementOrigin`.
        let place = attachment_matrix(&mdl, "placementOrigin").map_or(Mat4::IDENTITY, |m| m.inverse());
        let place = Transform::from_matrix(at.to_matrix() * place);
        let origin_kv = format!("{} {} {}", place.translation.x, place.translation.y, place.translation.z);
        let piece = crate::entities::spawn_runtime_entity(world, "prop_physics", &[("model", &path), ("origin", &origin_kv)]);
        let center = place.transform_point((mdl.hull_min + mdl.hull_max) * 0.5);
        let out = (center - origin).normalize_or(Vec3::Z);
        let spin = Vec3::new(random_range(-GIB_SPIN, GIB_SPIN), random_range(-GIB_SPIN, GIB_SPIN), random_range(-GIB_SPIN, GIB_SPIN));
        world.entity_mut(piece).insert((
            place,
            GlobalTransform::from(place),
            LinearVelocity(velocity + out * burst),
            AngularVelocity(spin * std::f32::consts::PI / 180.0),
            Piece { life, settled: false },
        ));
        for (effect, attachment) in model_particles(&mdl.keyvalues) {
            let local = attachment.as_deref().and_then(|a| attachment_matrix(&mdl, a)).map_or(Transform::IDENTITY, Transform::from_matrix);
            world.spawn((
                Name::new(format!("piece {effect}")),
                local,
                Visibility::default(),
                ParticleEffect::new(&effect, true),
                MapObject,
                ChildOf(piece),
            ));
        }
    }
}

/// Pieces are debris: once their bodies exist they stop colliding with the
/// player and with traces (no standing on, carrying or blocking them), and
/// stay out of saved games.
fn settle_pieces(mut commands: Commands, mut pieces: Query<(Entity, &mut Piece), With<RigidBody>>) {
    use crate::physics::layers;
    for (e, mut piece) in &mut pieces {
        if piece.settled {
            continue;
        }
        piece.settled = true;
        commands.entity(e).insert(CollisionLayers::new(
            LayerMask(layers::PROP),
            LayerMask(layers::WORLD | layers::MOVER | layers::FOLLOWER | layers::PROP),
        ));
        commands.entity(e).remove::<(TraceCollider, PhysicsProp, MapEntity)>();
    }
}

fn expire_pieces(time: Res<Time>, mut pieces: Query<(Entity, &mut Piece)>, mut commands: Commands) {
    let dt = time.delta_secs();
    for (e, mut piece) in &mut pieces {
        piece.life -= dt;
        if piece.life <= 0.0 {
            commands.entity(e).despawn();
        }
    }
}

fn age_blasts(time: Res<Time>, mut blasts: ResMut<Blasts>) {
    let dt = time.delta_secs().min(0.05);
    blasts.sparks.retain_mut(|s| {
        s.age += dt;
        s.velocity.z -= SPARK_GRAVITY * dt;
        s.velocity *= (1.0 - SPARK_DAMPEN * dt).max(0.0);
        s.position += s.velocity * dt;
        s.age < s.life
    });
    blasts.billows.retain_mut(|b| {
        b.age += dt;
        if b.age > 0.0 {
            b.position += b.velocity * dt;
            // Billows slow as they spread; smoke keeps rising.
            let drag = if b.material == SMOKE { 0.6 } else { 3.5 };
            b.velocity *= (1.0 - drag * dt).max(0.0);
            if b.material == SMOKE {
                b.velocity.z += 12.0 * dt;
            }
            b.roll += b.spin * dt;
        }
        b.age < b.life
    });
    blasts.flashes.retain_mut(|f| {
        f.age += dt;
        f.age < FLASH_TIME
    });
}

#[allow(clippy::too_many_arguments)]
fn draw_blasts(
    blasts: Res<Blasts>,
    settings: Res<FxSettings>,
    mut batches: Query<(&BlastBatch, &Mesh3d, &mut Visibility)>,
    mut looks: ResMut<Looks>,
    fs: Res<GameFs>,
    mut textures: ResMut<TextureCache>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<ParticleMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut commands: Commands,
) {
    if batches.is_empty() {
        if blasts.sparks.is_empty() && blasts.billows.is_empty() {
            return;
        }
        for (i, name) in BATCH_MATERIALS.iter().enumerate() {
            let look = looks.get(name, &fs, &mut textures, &mut images, &mut materials);
            commands.spawn((
                Name::new("turret blast sprites"),
                BlastBatch(i),
                Mesh3d(meshes.add(render::empty_mesh())),
                MeshMaterial3d(look.material),
                Transform::IDENTITY,
                Visibility::Hidden,
                NoFrustumCulling,
            ));
        }
        return;
    }
    let overhaul = settings.has(features::EMISSIVE);
    for (batch_of, mesh, mut vis) in &mut batches {
        let mut batch = Batch::default();
        if batch_of.0 == 0 {
            for s in &blasts.sparks {
                let fade = 1.0 - (s.age / s.life).clamp(0.0, 1.0);
                let tail = s.position - s.velocity * s.length;
                batch.streak(s.position, tail, s.width * 0.5, [1.0, 1.0, 1.0, fade]);
            }
        } else if overhaul {
            for b in blasts.billows.iter().filter(|b| b.material == batch_of.0 && b.age > 0.0) {
                let t = (b.age / b.life).clamp(0.0, 1.0);
                let size = b.size.0 + (b.size.1 - b.size.0) * (1.0 - (1.0 - t) * (1.0 - t));
                let color = if b.material == SMOKE {
                    // Dark smoke thickening in, then thinning out.
                    let alpha = (t * 6.0).min(1.0) * (1.0 - t) * 0.55;
                    [0.09, 0.085, 0.08, alpha]
                } else {
                    // White-hot to deep orange, fading.
                    let heat = 1.0 - t;
                    let k = 1.8 * heat * heat + 0.35;
                    [k, k * (0.35 + 0.4 * heat), k * (0.1 + 0.25 * heat * heat), heat]
                };
                batch.sprite(b.position, size, b.roll, color);
            }
        }
        let want = if batch.is_empty() { Visibility::Hidden } else { Visibility::Inherited };
        if !batch.is_empty()
            && let Some(mut m) = meshes.get_mut(&mesh.0)
        {
            batch.write(&mut m);
        }
        if *vis != want {
            *vis = want;
        }
    }
}

/// The overhaul's light from each blast's flash and each burning turret.
fn blast_lights(
    time: Res<Time>,
    settings: Res<FxSettings>,
    blasts: Res<Blasts>,
    burning: Query<&GlobalTransform, With<Flames>>,
    mut lights: ResMut<FxLights>,
) {
    if !settings.has(features::LIGHTS) {
        return;
    }
    for f in &blasts.flashes {
        let left = 1.0 - (f.age / FLASH_TIME).clamp(0.0, 1.0);
        lights.frame.push(FxLightData {
            position: f.position,
            shape: FxLightShape::Point { attenuation: Vec3::Z },
            color: FLASH_COLOR * FLASH_INTENSITY * left * left,
            range: FLASH_RANGE,
            flags: light_flags::DIFFUSE | light_flags::SPECULAR | light_flags::VOLUMETRIC,
            radius: 12.0,
            priority: 4.0,
            shadow: true,
        });
    }
    let now = time.elapsed_secs();
    let flicker = settings.has(features::PROCEDURAL);
    for (i, gt) in burning.iter().enumerate() {
        let k = i as f32 * 1.7;
        let wobble =
            if flicker { 0.75 + 0.25 * (now * 23.0 + k).sin() * (now * 13.7 + k).sin() + 0.1 * (now * 41.0 + k).sin() } else { 1.0 };
        lights.frame.push(FxLightData {
            position: gt.transform_point(Vec3::new(0.0, 0.0, CENTER_HEIGHT + 6.0)),
            shape: FxLightShape::Point { attenuation: Vec3::Z },
            color: FIRE_COLOR * FIRE_INTENSITY * wobble,
            range: FIRE_RANGE,
            flags: light_flags::DIFFUSE | light_flags::SPECULAR,
            radius: 8.0,
            priority: 2.0,
            shadow: false,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_particles_read_the_effect_and_attachment() {
        let kv = r#"mdlkeyvalue
{
prop_data {
"base" "Metal.Medium" "health" "0"  }
particles {
"effect" {
"name" "turret_burning_gib_01" "attachment_type" "follow_attachment" "attachment_point" "turret_fx_break_attach_2"  }
 }
}"#;
        assert_eq!(model_particles(kv), vec![("turret_burning_gib_01".to_owned(), Some("turret_fx_break_attach_2".to_owned()))]);
    }

    #[test]
    fn a_blast_throws_sparks_billows_and_a_flash() {
        let mut blasts = Blasts::default();
        add_blast(&mut blasts, Vec3::ZERO, Vec3::Z);
        assert!((8..=16).contains(&blasts.sparks.len()), "{}", blasts.sparks.len());
        assert!(blasts.sparks.iter().all(|s| s.velocity.length() >= 1500.0 && s.velocity.length() <= 2500.0));
        assert_eq!(blasts.billows.len(), FIREBALLS + SMOKE_PUFFS);
        assert_eq!(blasts.flashes.len(), 1);
    }
}
