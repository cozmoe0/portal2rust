//! A floor turret's glowing eye, and the light its eye and laser sight
//! throw with the visual overhaul.
//!
//! The eye (server.dll's eye states, `sub_10328910`) is `sprites/glow1` in
//! red at the model's `light` attachment, drawn as a world glow (over
//! everything, by how much of it the view can see): bright and big on its
//! target (brightness 255, scale 0.4), blinking between 224 at 0.3 and 192
//! at 0.2 each think while it searches, dimmed to 192 at 0.2 over half a
//! second closed, and out (0 at 0.1) over a quarter second when it dies or
//! a second when switched off. Changes take a tenth of a second otherwise.
//!
//! With the overhaul's lights, the eye casts a small red glow on what is
//! around it, the laser sight a thin red light along its length (and a
//! faint red line in the air, with the volumetrics) and a brighter spot
//! where it lands.

use bevy::camera::visibility::NoFrustumCulling;
use bevy::prelude::*;

use super::turret::{EyeState, Turret, TurretState};
use crate::visuals::fx::lights::{FxLightData, FxLightShape, FxLights, FxProduceSet, light_flags};
use crate::visuals::fx::{FxSettings, features};
use crate::visuals::sprites::{SpriteMaterial, SpriteParams, load_sprite_source, sprite_quad};
use portal2_core::vfs::GameFs;
use portal2_core::view::MainCamera;
use portal2_render::models::StudioAnimator;
use portal2_render::textures::TextureCache;
use portal2_world::MapObject;
use portal2_world::collision::{Physics, mask};

const EYE_SPRITE: &str = "sprites/glow1";
const EYE_ATTACHMENT: &str = "light";
/// World glow (render mode 9).
const MODE_WORLD_GLOW: f32 = 9.0;
/// The overhaul's lights.
const EYE_LIGHT_COLOR: Vec3 = Vec3::new(1.0, 0.04, 0.02);
const EYE_LIGHT_INTENSITY: f32 = 260.0;
const EYE_LIGHT_RANGE: f32 = 220.0;
pub const LASER_LIGHT_COLOR: Vec3 = Vec3::new(1.0, 0.03, 0.02);
const LASER_LIGHT_INTENSITY: f32 = 36.0;
const LASER_LIGHT_RANGE: f32 = 110.0;
const LASER_SPOT_INTENSITY: f32 = 170.0;
const LASER_SPOT_RANGE: f32 = 130.0;

pub struct TurretVisualPlugin;

impl Plugin for TurretVisualPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostUpdate, (draw_eyes.after(TransformSystems::Propagate), turret_lights.in_set(FxProduceSet)));
    }
}

/// A turret's eye glow and how it looks now.
#[derive(Component)]
pub struct TurretEye {
    sprite: Entity,
    material: Handle<SpriteMaterial>,
    /// Texture size, in units at scale 1.
    size: Vec2,
    brightness: f32,
    scale: f32,
    visible: f32,
    /// Where the glow is (for the lights).
    pub position: Vec3,
}

/// The eye's brightness (0..255), scale and the seconds it takes to get
/// there, for its state.
pub fn eye_look(state: EyeState, blink: bool) -> (f32, f32, f32) {
    match state {
        EyeState::SeeTarget => (255.0, 0.4, 0.1),
        EyeState::Seeking if blink => (224.0, 0.3, 0.1),
        EyeState::Seeking => (192.0, 0.2, 0.1),
        EyeState::Dormant => (192.0, 0.2, 0.5),
        EyeState::Dead => (0.0, 0.1, 0.25),
        EyeState::Disabled => (0.0, 0.1, 1.0),
    }
}

/// Moves `value` toward `target` at the pace that covers `span` in `time`.
fn approach(value: f32, target: f32, span: f32, time: f32, dt: f32) -> f32 {
    let step = span * dt / time.max(1e-3);
    value + (target - value).clamp(-step, step)
}

#[allow(clippy::too_many_arguments)]
fn draw_eyes(
    time: Res<Time>,
    physics: Physics,
    mut commands: Commands,
    mut turrets: Query<(Entity, &Turret, &StudioAnimator, Option<&mut TurretEye>)>,
    joints: Query<&GlobalTransform>,
    main: Query<&GlobalTransform, With<MainCamera>>,
    mut placed: Query<(&mut Transform, &mut Visibility), Without<Turret>>,
    mut sprites: ResMut<Assets<SpriteMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut textures: ResMut<TextureCache>,
    fs: Res<GameFs>,
    mut quad: Local<Option<Handle<Mesh>>>,
) {
    let dt = time.delta_secs();
    let eye_pos = main.single().map(|g| g.translation()).ok();
    for (e, t, anim, eye) in &mut turrets {
        let mdl = &anim.model.studio.mdl;
        let place = mdl.find_attachment(EYE_ATTACHMENT).and_then(|i| mdl.attachments.get(i)).and_then(|a| {
            Some(joints.get(*anim.joints.get(a.bone.max(0) as usize)?).ok()?.compute_transform() * Transform::from_matrix(a.local))
        });
        let Some(place) = place else { continue };
        let Some(mut eye) = eye else {
            let source = load_sprite_source(&fs, &mut textures, &mut images, EYE_SPRITE);
            let (brightness, scale, _) = eye_look(t.eye_state, t.eye_blink);
            let material = sprites.add(SpriteMaterial {
                params: SpriteParams {
                    color: Vec3::new(1.0, 0.0, 0.0).extend(brightness / 255.0),
                    shape: Vec4::new(source.size.x * scale * 0.5, source.size.y * scale * 0.5, source.orientation, MODE_WORLD_GLOW),
                    state: Vec4::new(0.0, 1.0, 0.0, 0.0),
                },
                base: source.base,
                alpha_mode: AlphaMode::Add,
                no_depth_test: true,
            });
            let mesh = quad.get_or_insert_with(|| meshes.add(sprite_quad())).clone();
            let sprite = commands
                .spawn((
                    Name::new("turret eye glow"),
                    MapObject,
                    Mesh3d(mesh),
                    MeshMaterial3d(material.clone()),
                    Transform::from_translation(place.translation),
                    Visibility::Visible,
                    NoFrustumCulling,
                ))
                .id();
            commands.entity(e).insert(TurretEye {
                sprite,
                material,
                size: source.size,
                brightness,
                scale,
                visible: 0.0,
                position: place.translation,
            });
            continue;
        };
        let (target, target_scale, fade) = eye_look(t.eye_state, t.eye_blink);
        eye.brightness = approach(eye.brightness, target, 255.0, fade, dt);
        eye.scale = approach(eye.scale, target_scale, 0.3, fade, dt);
        eye.position = place.translation;
        // How much of the glow the view sees (the turret's own shell aside).
        let seen = eye_pos.map_or(0.0, |from| {
            let tr = physics.trace(from, place.translation, Vec3::ZERO, Vec3::ZERO, mask::SOLID_ALL, &[e]);
            if !tr.hit() || tr.fraction > 0.995 { 1.0 } else { 0.0 }
        });
        eye.visible += (seen - eye.visible) * (dt * 10.0).min(1.0);
        let on = eye.brightness > 0.5;
        if let Ok((mut tf, mut vis)) = placed.get_mut(eye.sprite) {
            tf.translation = place.translation;
            let want = if on { Visibility::Visible } else { Visibility::Hidden };
            if *vis != want {
                *vis = want;
            }
        }
        let params = SpriteParams {
            color: Vec3::new(1.0, 0.0, 0.0).extend(eye.brightness / 255.0),
            shape: Vec4::new(eye.size.x * eye.scale * 0.5, eye.size.y * eye.scale * 0.5, 0.0, MODE_WORLD_GLOW),
            state: Vec4::new(eye.visible, 1.0, 0.0, 0.0),
        };
        let changed = sprites.get(&eye.material).is_some_and(|m| {
            (m.params.color - params.color).abs().max_element() > 1e-3
                || (m.params.shape - params.shape).abs().max_element() > 1e-3
                || (m.params.state.x - params.state.x).abs() > 1e-3
        });
        if changed && let Some(mut m) = sprites.get_mut(&eye.material) {
            m.params.color = params.color;
            m.params.shape.x = params.shape.x;
            m.params.shape.y = params.shape.y;
            m.params.state.x = params.state.x;
        }
    }
}

/// The overhaul's light from each turret's eye and laser sight.
fn turret_lights(
    settings: Res<FxSettings>,
    turrets: Query<(&Turret, &TurretEye, Option<&super::turret::LaserSightPath>)>,
    mut lights: ResMut<FxLights>,
) {
    if !settings.has(features::LIGHTS) {
        return;
    }
    for (t, eye, sight) in &turrets {
        let glow = eye.brightness / 255.0;
        if glow > 0.02 {
            lights.frame.push(FxLightData {
                position: eye.position,
                shape: FxLightShape::Point { attenuation: Vec3::new(48.0, 0.0, 1.0) },
                color: EYE_LIGHT_COLOR * EYE_LIGHT_INTENSITY * glow,
                range: EYE_LIGHT_RANGE,
                flags: light_flags::DIFFUSE | light_flags::SPECULAR,
                radius: 2.0,
                priority: 1.0,
                shadow: false,
            });
        }
        let Some(path) = sight.filter(|_| t.enabled && !matches!(t.state, TurretState::Dead | TurretState::Closing)) else {
            continue;
        };
        lights.frame.push(FxLightData {
            position: path.start,
            shape: FxLightShape::Line { end: path.end, facing: Vec3::ZERO },
            color: LASER_LIGHT_COLOR * LASER_LIGHT_INTENSITY,
            range: LASER_LIGHT_RANGE,
            flags: light_flags::DIFFUSE | light_flags::SPECULAR | light_flags::VOLUMETRIC,
            radius: 3.0,
            priority: 1.2,
            shadow: false,
        });
        lights.frame.push(FxLightData {
            position: path.end - path.dir * 2.0,
            shape: FxLightShape::Point { attenuation: Vec3::new(30.0, 0.0, 1.0) },
            color: LASER_LIGHT_COLOR * LASER_SPOT_INTENSITY,
            range: LASER_SPOT_RANGE,
            flags: light_flags::DIFFUSE | light_flags::SPECULAR,
            radius: 1.5,
            priority: 1.1,
            shadow: false,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eye_looks_follow_the_game() {
        assert_eq!(eye_look(EyeState::SeeTarget, false), (255.0, 0.4, 0.1));
        assert_ne!(eye_look(EyeState::Seeking, true), eye_look(EyeState::Seeking, false));
        assert_eq!(eye_look(EyeState::Dead, false).0, 0.0);
        // A quarter second takes a full eye out.
        let mut b = 255.0;
        for _ in 0..25 {
            b = approach(b, 0.0, 255.0, 0.25, 0.01);
        }
        assert!(b.abs() < 1e-3, "{b}");
    }
}
