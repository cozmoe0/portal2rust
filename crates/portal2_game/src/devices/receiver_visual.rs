//! How a laser relay or catcher looks as it powers up and down.
//!
//! The game spins the model's `spin` sequence while it is powered and goes
//! back to `idle` when it is not (the server's power think,
//! `sub_103C61F0` / `sub_103C62C0`), and the client puts
//! `laser_relay_powered` at its `particle_emitter` attachment (see
//! `effects::hooks`).
//!
//! With the visual overhaul:
//!
//! - the rings spin up and wind down with some weight to them instead of
//!   starting and stopping at once (procedural motion);
//! - a hot red-orange core glows at the emitter inside a soft red haze
//!   that fills the glass, flaring as the beam first strikes it and
//!   shimmering slightly while it holds (emissive);
//! - the core lights the relay's glass and what stands around it, with a
//!   flash on power-up and a little haze in the air (lights).

use bevy::camera::visibility::NoFrustumCulling;
use bevy::prelude::*;

use super::laser::LaserReceiver;
use crate::visuals::fx::lights::{FxLightData, FxLightShape, FxLights, FxProduceSet, light_flags};
use crate::visuals::fx::{FxSettings, features};
use crate::visuals::sprites::{SpriteMaterial, SpriteParams, load_sprite_source, sprite_quad};
use portal2_core::entity::MapEntity;
use portal2_core::vfs::GameFs;
use portal2_core::view::MainCamera;
use portal2_render::models::StudioAnimator;
use portal2_render::textures::TextureCache;
use portal2_world::MapObject;
use portal2_world::collision::{Physics, mask};

const EMITTER_ATTACHMENT: &str = "particle_emitter";
const SPIN_SEQUENCE: &str = "spin";
const IDLE_SEQUENCE: &str = "idle";
/// Seconds the rings take to reach full speed, and to wind down.
const SPIN_UP: f32 = 0.7;
const SPIN_DOWN: f32 = 1.6;
/// Seconds the core takes to light, and to go out.
const LIGHT_UP: f32 = 0.08;
const LIGHT_DOWN: f32 = 0.35;
/// The flare as the beam strikes: extra brightness, and how quickly it
/// settles (seconds).
const FLARE: f32 = 2.2;
const FLARE_TIME: f32 = 0.16;
/// The glows: the hot core and the haze around it, each a half size
/// (units) and an HDR colour.
const CORE_SPRITE: &str = "sprites/glow1";
const GLOWS: [(f32, Vec3); 2] = [(11.0, Vec3::new(2.4, 0.5, 0.2)), (26.0, Vec3::new(0.55, 0.06, 0.03))];
/// World glow (render mode 9).
const MODE_WORLD_GLOW: f32 = 9.0;
/// The core's light.
const LIGHT_COLOR: Vec3 = Vec3::new(1.0, 0.14, 0.07);
const LIGHT_INTENSITY: f32 = 520.0;
const LIGHT_RANGE: f32 = 200.0;

pub struct ReceiverVisualPlugin;

impl Plugin for ReceiverVisualPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, animate_receivers)
            .add_systems(PostUpdate, (draw_cores.after(TransformSystems::Propagate), receiver_lights.in_set(FxProduceSet)));
    }
}

/// How a relay or catcher looks now.
#[derive(Component)]
pub struct ReceiverLook {
    /// Share of full spin speed.
    spin: f32,
    /// How lit the core is (0..1).
    level: f32,
    /// Seconds since it last powered up.
    since_on: f32,
    was_powered: bool,
    /// Where the core is.
    position: Vec3,
    /// The glow sprites (as `GLOWS`), and how much of them the view sees.
    glows: Vec<(Entity, Handle<SpriteMaterial>)>,
    visible: f32,
}

impl ReceiverLook {
    fn new(powered: bool) -> Self {
        let on = if powered { 1.0 } else { 0.0 };
        Self { spin: on, level: on, since_on: 10.0, was_powered: powered, position: Vec3::ZERO, glows: Vec::new(), visible: 0.0 }
    }

    /// The core's brightness: its level, the flare as the beam strikes and
    /// (with procedural motion) a faint shimmer.
    fn brightness(&self, now: f32, shimmer: bool) -> f32 {
        let flare = 1.0 + FLARE * (-self.since_on / FLARE_TIME).exp();
        let wobble = if shimmer { 1.0 + 0.05 * (now * 31.0).sin() * (now * 17.3).sin() } else { 1.0 };
        self.level * flare * wobble
    }
}

/// Moves `value` toward `target`, covering 0..1 in `up` seconds rising and
/// `down` falling.
fn ease_toward(value: f32, target: f32, up: f32, down: f32, dt: f32) -> f32 {
    if target > value { (value + dt / up).min(target) } else { (value - dt / down).max(target) }
}

/// The share of full speed the rings turn at for a spin level: slow to
/// start and to stop.
pub fn spin_rate(spin: f32) -> f32 {
    let s = spin.clamp(0.0, 1.0);
    s * s * (3.0 - 2.0 * s)
}

fn animate_receivers(
    time: Res<Time>,
    settings: Res<FxSettings>,
    mut commands: Commands,
    mut receivers: Query<(Entity, &LaserReceiver, &MapEntity, Option<&mut StudioAnimator>, Option<&mut ReceiverLook>)>,
) {
    let dt = time.delta_secs();
    let weighty = settings.has(features::PROCEDURAL);
    for (e, r, me, anim, look) in &mut receivers {
        if me.classname == "point_laser_target" {
            continue;
        }
        let Some(mut look) = look else {
            commands.entity(e).insert(ReceiverLook::new(r.powered));
            continue;
        };
        look.since_on += dt;
        if r.powered && !look.was_powered {
            look.since_on = 0.0;
        }
        look.was_powered = r.powered;
        let target = if r.powered { 1.0 } else { 0.0 };
        look.level = ease_toward(look.level, target, LIGHT_UP, LIGHT_DOWN, dt);
        look.spin = if weighty { ease_toward(look.spin, target, SPIN_UP, SPIN_DOWN, dt) } else { target };
        let Some(mut anim) = anim else { continue };
        let Some(spin) = anim.model.studio.find_sequence(SPIN_SEQUENCE) else { continue };
        let spinning = anim.sequence == spin;
        if r.powered && !spinning {
            // Spinning up from wherever the rings rest.
            anim.set_sequence_index(spin);
        } else if !r.powered && spinning && !weighty {
            anim.set_sequence(IDLE_SEQUENCE);
            anim.rate = 1.0;
        }
        if anim.sequence == spin {
            let rate = if weighty { spin_rate(look.spin) } else { 1.0 };
            if (anim.rate - rate).abs() > 1e-4 {
                anim.rate = rate;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_cores(
    time: Res<Time>,
    settings: Res<FxSettings>,
    physics: Physics,
    mut commands: Commands,
    mut receivers: Query<(Entity, &StudioAnimator, &mut ReceiverLook)>,
    joints: Query<&GlobalTransform>,
    main: Query<&GlobalTransform, With<MainCamera>>,
    mut placed: Query<(&mut Transform, &mut Visibility), Without<ReceiverLook>>,
    mut sprites: ResMut<Assets<SpriteMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut textures: ResMut<TextureCache>,
    fs: Res<GameFs>,
    mut quad: Local<Option<Handle<Mesh>>>,
) {
    let dt = time.delta_secs();
    let now = time.elapsed_secs();
    let view = main.single().map(|g| g.translation()).ok();
    let glowing = settings.has(features::EMISSIVE);
    for (e, anim, mut look) in &mut receivers {
        let mdl = &anim.model.studio.mdl;
        let place = mdl.find_attachment(EMITTER_ATTACHMENT).and_then(|i| mdl.attachments.get(i)).and_then(|a| {
            Some(joints.get(*anim.joints.get(a.bone.max(0) as usize)?).ok()?.compute_transform() * Transform::from_matrix(a.local))
        });
        let Some(place) = place else { continue };
        look.position = place.translation;
        let brightness = look.brightness(now, settings.has(features::PROCEDURAL));
        let show = glowing && brightness > 1e-3;
        if look.glows.is_empty() {
            if !show {
                continue;
            }
            let source = load_sprite_source(&fs, &mut textures, &mut images, CORE_SPRITE);
            let mesh = quad.get_or_insert_with(|| meshes.add(sprite_quad())).clone();
            for (half, color) in GLOWS {
                let material = sprites.add(SpriteMaterial {
                    params: SpriteParams {
                        color: color.extend(0.0),
                        shape: Vec4::new(half, half, source.orientation, MODE_WORLD_GLOW),
                        state: Vec4::new(0.0, 1.0, 0.0, 0.0),
                    },
                    base: source.base.clone(),
                    alpha_mode: AlphaMode::Add,
                    no_depth_test: true,
                });
                let sprite = commands
                    .spawn((
                        Name::new("laser receiver glow"),
                        MapObject,
                        Mesh3d(mesh.clone()),
                        MeshMaterial3d(material.clone()),
                        Transform::from_translation(place.translation),
                        Visibility::Hidden,
                        NoFrustumCulling,
                    ))
                    .id();
                look.glows.push((sprite, material));
            }
            continue;
        }
        // The glass around the core lets it show; anything else in the way
        // hides it.
        let seen = view.map_or(0.0, |from| {
            let tr = physics.trace(from, place.translation, Vec3::ZERO, Vec3::ZERO, mask::SOLID_ALL, &[e]);
            if !tr.hit() || tr.fraction > 0.995 { 1.0 } else { 0.0 }
        });
        look.visible += (seen - look.visible) * (dt * 12.0).min(1.0);
        // The flare also swells the glows.
        let swell = (0.6 + 0.4 * look.level) * (1.0 + 0.6 * (-look.since_on / FLARE_TIME).exp());
        for ((sprite, material), (half, _)) in look.glows.iter().zip(GLOWS) {
            if let Ok((mut tf, mut vis)) = placed.get_mut(*sprite) {
                tf.translation = place.translation;
                let want = if show { Visibility::Visible } else { Visibility::Hidden };
                if *vis != want {
                    *vis = want;
                }
            }
            if show && let Some(mut m) = sprites.get_mut(material) {
                m.params.color.w = brightness;
                m.params.shape.x = half * swell;
                m.params.shape.y = half * swell;
                m.params.state.x = look.visible;
            }
        }
    }
}

/// The overhaul's light from each lit core.
fn receiver_lights(time: Res<Time>, settings: Res<FxSettings>, receivers: Query<&ReceiverLook>, mut lights: ResMut<FxLights>) {
    if !settings.has(features::LIGHTS) {
        return;
    }
    let now = time.elapsed_secs();
    let shimmer = settings.has(features::PROCEDURAL);
    for look in &receivers {
        let brightness = look.brightness(now, shimmer);
        if brightness < 0.01 {
            continue;
        }
        lights.frame.push(FxLightData {
            position: look.position,
            shape: FxLightShape::Point { attenuation: Vec3::Z },
            color: LIGHT_COLOR * LIGHT_INTENSITY * brightness,
            range: LIGHT_RANGE,
            flags: light_flags::DIFFUSE | light_flags::SPECULAR | light_flags::VOLUMETRIC,
            radius: 6.0,
            priority: 1.8,
            shadow: false,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rings start and stop slowly and turn at full speed in between.
    #[test]
    fn rings_spin_up_and_down_smoothly() {
        assert_eq!(spin_rate(0.0), 0.0);
        assert_eq!(spin_rate(1.0), 1.0);
        assert!(spin_rate(0.1) < 0.1);
        let mut s = 0.0;
        for _ in 0..70 {
            s = ease_toward(s, 1.0, SPIN_UP, SPIN_DOWN, 0.01);
        }
        assert!((s - 1.0).abs() < 1e-3, "{s}");
        for _ in 0..80 {
            s = ease_toward(s, 0.0, SPIN_UP, SPIN_DOWN, 0.01);
        }
        assert!(s > 0.4 && s < 0.6, "half way down after 0.8 s: {s}");
    }

    /// The core flares as the beam strikes and settles to its level.
    #[test]
    fn the_core_flares_on_power_up() {
        let mut look = ReceiverLook::new(true);
        look.since_on = 0.0;
        assert!((look.brightness(0.0, false) - (1.0 + FLARE)).abs() < 1e-4);
        look.since_on = 2.0;
        assert!((look.brightness(0.0, false) - 1.0).abs() < 1e-3);
        assert_eq!(ReceiverLook::new(false).brightness(0.0, true), 0.0);
    }
}
