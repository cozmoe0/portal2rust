//! Muzzle flashes, started by models' `AE_NPC_MUZZLEFLASH` events: the
//! floor turret's `fire` and `fire2` sequences flash at a pair of its gun
//! muzzles each time a shot starts them (`COMBINE <attachment>`).
//!
//! The `COMBINE` flash, as the game's client draws it for NPCs (client.dll
//! `sub_103FFC80`, `MuzzleFlash_Combine_NPC`), in the muzzle's own frame
//! (X out of the barrel), with a size scale of 1 to 1.5:
//!
//! - five sparks of `effects/combinemuzzle1` or `2` along the barrel, 2
//!   units apart (times the scale), flying out at 50 to 150 units/s for
//!   0.1 s, 6 to 8 units across (scaled) and smaller further out;
//! - three spikes of five more across the barrel (up, and down to either
//!   side, a "Y" seen from the front), 1 unit apart, flying out at a quarter
//!   of their own 50 to 150 units/s for 0.2 s, 2 to 4 units across and
//!   smaller further out;
//! - a glow of `effects/strider_muzzle` at the muzzle, 12 to 16 units
//!   across, shrinking away over 0.3 to 0.4 s;
//! - all white, randomly turned, fading out over their lives, and a short
//!   light at the muzzle (32 to 128 units, gone in 0.05 s).
//!
//! With the visual overhaul's lights the flash lights its surroundings for
//! that moment, warm like the bright core of the sprites.

use bevy::camera::visibility::NoFrustumCulling;
use bevy::prelude::*;

use crate::visuals::fx::lights::{FxLightData, FxLightShape, FxLights, FxProduceSet, light_flags};
use crate::visuals::fx::{FxSettings, features};
use portal2_core::rng::random_range;
use portal2_core::vfs::GameFs;
use portal2_particles::render::{self, Batch, Looks, ParticleMaterial};
use portal2_render::textures::TextureCache;
use portal2_world::MapLoaded;

/// The sprites: the two spark textures, then the glow.
const MATERIALS: [&str; 3] = ["effects/combinemuzzle1", "effects/combinemuzzle2", "effects/strider_muzzle"];
const GLOW: usize = 2;
/// The spikes across the barrel, in the muzzle's frame (X out of it).
const SPIKES: [Vec3; 3] = [Vec3::new(0.0, 1.0, -1.0), Vec3::new(0.0, -1.0, -1.0), Vec3::new(0.0, 0.0, 1.0)];
const LIGHT_LIFE: f32 = 0.05;
/// The overhaul's light: warm white, strong close by.
const FX_LIGHT_COLOR: Vec3 = Vec3::new(1.0, 0.8, 0.55);
const FX_LIGHT_INTENSITY: f32 = 280.0;
const FX_LIGHT_RANGE: f32 = 300.0;
/// The game draws these additive sprites onto its displayed image, where
/// overlapping ones clip to white (the flash's solid white core); on the
/// HDR picture they are brightened to read the same.
const SPRITE_BRIGHTNESS: f32 = 2.0;

/// A muzzle flash to draw at a muzzle (X along the barrel).
#[derive(Message, Clone, Copy, Debug)]
pub struct MuzzleFlash {
    pub at: Transform,
}

#[derive(Clone, Copy, Debug)]
struct Spark {
    material: usize,
    position: Vec3,
    velocity: Vec3,
    age: f32,
    life: f32,
    size: (f32, f32),
    roll: f32,
}

#[derive(Clone, Copy, Debug)]
struct Flare {
    position: Vec3,
    radius: f32,
    age: f32,
}

#[derive(Resource, Default)]
struct Flashes {
    sparks: Vec<Spark>,
    flares: Vec<Flare>,
}

/// The mesh of one sprite material's sparks.
#[derive(Component)]
struct FlashBatch(usize);

pub fn register(app: &mut App) {
    app.add_message::<MuzzleFlash>()
        .init_resource::<Flashes>()
        .add_systems(Update, (start_flashes, age_flashes).chain())
        .add_systems(PostUpdate, (draw_flashes.after(TransformSystems::Propagate), flash_lights.in_set(FxProduceSet)));
}

fn random_material() -> usize {
    if random_range(0.0, 1.0) < 0.5 { 0 } else { 1 }
}

/// The sparks and light of one flash.
fn flash_sparks(at: &Transform, out: &mut Flashes) {
    let to_world = |local: Vec3| at.rotation * local;
    let scale = random_range(1.0, 1.5);
    let mut push = |offset: Vec3, velocity: Vec3, life: f32, size: (f32, f32), material: usize| {
        out.sparks.push(Spark {
            material,
            position: at.translation + to_world(offset),
            velocity: to_world(velocity),
            age: 0.0,
            life,
            size,
            roll: random_range(0.0, std::f32::consts::TAU),
        });
    };
    let speed = random_range(50.0, 150.0);
    for i in 1..=5 {
        let i = i as f32;
        let size = random_range(6.0, 8.0) * (7.5 - i) / 6.0 * scale;
        push(Vec3::X * (i * 2.0 * scale), Vec3::X * speed, 0.1, (size, size), random_material());
    }
    for dir in SPIKES {
        let speed = random_range(50.0, 150.0);
        for k in 1..=5 {
            let size = random_range(2.0, 4.0) * (6 - k) as f32 / 3.0 * scale;
            push(dir * (k as f32 * scale), dir * speed * 0.25, 0.2, (size, size), random_material());
        }
    }
    push(Vec3::ZERO, Vec3::ZERO, random_range(0.3, 0.4), (random_range(12.0, 16.0) * scale, 0.0), GLOW);
    out.flares.push(Flare { position: at.translation, radius: random_range(32.0, 128.0), age: 0.0 });
}

fn start_flashes(mut requests: MessageReader<MuzzleFlash>, mut flashes: ResMut<Flashes>, mut loaded: MessageReader<MapLoaded>) {
    if loaded.read().last().is_some() {
        flashes.sparks.clear();
        flashes.flares.clear();
    }
    for r in requests.read() {
        flash_sparks(&r.at, &mut flashes);
    }
}

fn age_flashes(time: Res<Time>, mut flashes: ResMut<Flashes>) {
    let dt = time.delta_secs();
    flashes.sparks.retain_mut(|s| {
        s.age += dt;
        s.position += s.velocity * dt;
        s.age < s.life
    });
    flashes.flares.retain_mut(|f| {
        f.age += dt;
        f.age < LIGHT_LIFE
    });
}

#[allow(clippy::too_many_arguments)]
fn draw_flashes(
    flashes: Res<Flashes>,
    mut batches: Query<(&FlashBatch, &mut Mesh3d, &mut Visibility)>,
    mut looks: ResMut<Looks>,
    fs: Res<GameFs>,
    mut textures: ResMut<TextureCache>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<ParticleMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut commands: Commands,
) {
    if batches.is_empty() {
        if flashes.sparks.is_empty() {
            return;
        }
        for (i, name) in MATERIALS.iter().enumerate() {
            let look = looks.get(name, &fs, &mut textures, &mut images, &mut materials);
            commands.spawn((
                Name::new("muzzle flash sprites"),
                FlashBatch(i),
                Mesh3d(meshes.add(render::empty_mesh())),
                MeshMaterial3d(look.material),
                Transform::IDENTITY,
                Visibility::Hidden,
                NoFrustumCulling,
            ));
        }
        return;
    }
    for (batch_of, mesh, mut vis) in &mut batches {
        let mut batch = Batch::default();
        for s in flashes.sparks.iter().filter(|s| s.material == batch_of.0) {
            let t = (s.age / s.life).clamp(0.0, 1.0);
            let size = s.size.0 + (s.size.1 - s.size.0) * t;
            let b = SPRITE_BRIGHTNESS;
            batch.sprite(s.position, size, s.roll, [b, b, b, 1.0 - t]);
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

/// The overhaul's light from each flash, shrinking as the game's does.
fn flash_lights(settings: Res<FxSettings>, flashes: Res<Flashes>, mut lights: ResMut<FxLights>) {
    if !settings.has(features::LIGHTS) {
        return;
    }
    for f in &flashes.flares {
        let left = 1.0 - (f.age / LIGHT_LIFE).clamp(0.0, 1.0);
        let reach = f.radius / 128.0;
        lights.frame.push(FxLightData {
            position: f.position,
            shape: FxLightShape::Point { attenuation: Vec3::new(600.0, 0.0, 1.0) },
            color: FX_LIGHT_COLOR * FX_LIGHT_INTENSITY * left * (0.5 + 0.5 * reach),
            range: FX_LIGHT_RANGE,
            flags: light_flags::DIFFUSE | light_flags::SPECULAR,
            radius: 6.0,
            priority: 2.0,
            shadow: false,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flash makes 21 sparks and a glow around the muzzle, the five along
    /// the barrel in front of it, and one light.
    #[test]
    fn a_flash_has_its_sparks_glow_and_light() {
        let mut out = Flashes::default();
        let at = Transform::from_xyz(10.0, 0.0, 0.0).with_rotation(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2));
        flash_sparks(&at, &mut out);
        assert_eq!(out.sparks.len(), 21);
        assert_eq!(out.sparks.iter().filter(|s| s.material == GLOW).count(), 1);
        assert_eq!(out.flares.len(), 1);
        // Barrel along +Y here: the first five sparks lie in front of it.
        for s in &out.sparks[..5] {
            assert!(s.position.y > 0.0 && (s.position.x - 10.0).abs() < 1e-3, "{:?}", s.position);
            assert!(s.life == 0.1 && s.velocity.y >= 50.0);
        }
        let glow = out.sparks.iter().find(|s| s.material == GLOW).unwrap();
        assert!(glow.size.1 == 0.0 && (0.3..=0.4).contains(&glow.life));
    }
}
