//! Shadow maps for the overhaul's strongest lights (the portals first), so
//! a portal's glow stops at the cube or pillar in front of it.
//!
//! They work like the projected textures' maps (`visuals/projected.rs`): a
//! camera per map draws its view into a distance texture, which the world
//! and model shaders fill with the distance to the light. Every extra view
//! costs the render thread a full pass over the scene, so a map is only
//! redrawn when its light moves or takes the slot, and otherwise every
//! twelfth frame (staggered between maps) to follow moving props; ultra
//! redraws every frame.

use bevy::camera::visibility::RenderLayers;
use bevy::camera::{Exposure, Hdr, RenderTarget};
use bevy::prelude::*;
use bevy::render::view::Tonemapping;
use portal2_render::fx::{FX, HEADER_FLOATS, LIGHT_FLOATS, MAX_SHADOWS};

use super::lights::{FxActiveLights, FxLightShape};
use super::material::{FxBuffer, FxFillSet};
use super::{FxPreset, FxSettings, features};
use crate::visuals::projected::{SHADOW_CLEAR, SHADOW_SIZE, shadow_fog};
use portal2_core::coords::camera_rotation;

/// The shadow views' field of view: wide, since a portal lights a whole
/// hemisphere; what falls outside is left unshadowed.
const FOV_DEGREES: f32 = 130.0;
const NEAR: f32 = 1.0;
/// Past every overhaul light's range.
const FAR: f32 = 1000.0;
/// Where the buffer's shadow matrices and shadow info start.
const MATRICES: usize = 32;
const SHADOW_INFO: usize = 112;

/// Draws one of the overhaul's shadow maps.
#[derive(Component)]
pub struct FxShadowCamera(pub usize);

/// What each shadow map shows and when it was last drawn.
#[derive(Resource, Default)]
struct ShadowSlots {
    slots: [Option<SlotState>; MAX_SHADOWS],
    frame: u32,
}

#[derive(Clone, Copy, PartialEq)]
struct SlotState {
    position: Vec3,
    rotation: Quat,
    age: u32,
}

pub struct FxShadowsPlugin;

impl Plugin for FxShadowsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ShadowSlots>()
            .add_systems(Startup, spawn_cameras)
            .add_systems(PostUpdate, assign.in_set(FxFillSet).after(super::lights::gather));
    }
}

fn spawn_cameras(mut commands: Commands) {
    let Some(handles) = FX.get() else { return };
    for (slot, image) in handles.shadows.iter().enumerate() {
        commands.spawn((
            Name::new(format!("overhaul shadow {slot}")),
            FxShadowCamera(slot),
            Camera3d::default(),
            Camera {
                order: -40 + slot as isize,
                is_active: false,
                clear_color: ClearColorConfig::Custom(Color::linear_rgb(SHADOW_CLEAR, SHADOW_CLEAR, SHADOW_CLEAR)),
                ..default()
            },
            RenderTarget::Image(image.clone().into()),
            Hdr,
            Tonemapping::None,
            bevy::render::view::DebandDither::Disabled,
            Exposure::default(),
            Msaa::Off,
            Projection::Perspective(PerspectiveProjection {
                fov: FOV_DEGREES.to_radians(),
                aspect_ratio: 1.0,
                near: NEAR,
                far: FAR,
                ..default()
            }),
            shadow_fog(),
            RenderLayers::layer(0),
            Transform::default(),
        ));
    }
}

/// Frames between redraws of an unchanged shadow map (a moved light is
/// redrawn at once); this only follows props moving in the light.
fn refresh_interval(preset: FxPreset) -> u32 {
    if preset == FxPreset::Ultra { 1 } else { 12 }
}

/// Gives the strongest shadow-casting lights a map, writes their matrices
/// and slots into the buffer, and points (and wakes) the cameras.
fn assign(
    settings: Res<FxSettings>,
    active: Res<FxActiveLights>,
    mut buffer: ResMut<FxBuffer>,
    mut state: ResMut<ShadowSlots>,
    mut cameras: Query<(&FxShadowCamera, &mut Camera, &mut Transform, &mut GlobalTransform)>,
) {
    let slots = if settings.has(features::SHADOWS) && settings.has(features::LIGHTS) { settings.shadow_slots.min(MAX_SHADOWS) } else { 0 };
    state.frame = state.frame.wrapping_add(1);
    let frame = state.frame;
    let interval = refresh_interval(settings.preset);
    let tan_half = (FOV_DEGREES.to_radians() * 0.5).tan();
    let mut wanted: [Option<(Vec3, Quat)>; MAX_SHADOWS] = [None; MAX_SHADOWS];
    let mut used = 0;
    for (i, light) in active.0.iter().enumerate() {
        if used >= slots {
            break;
        }
        if !light.shadow {
            continue;
        }
        let (forward, up) = match light.shape {
            FxLightShape::Ellipse { normal, up, .. } => (normal, up),
            FxLightShape::Spot { axis, .. } => (axis, if axis.z.abs() > 0.9 { Vec3::X } else { Vec3::Z }),
            _ => continue,
        };
        let left = up.cross(forward).normalize_or_zero();
        let up = forward.cross(left);
        let rotation = Quat::from_mat3(&Mat3::from_cols(forward, left, up));
        wanted[used] = Some((light.position, rotation));
        let o = HEADER_FLOATS + i * LIGHT_FLOATS;
        buffer.data[o + 14] = used as f32;
        used += 1;
    }
    buffer.data[SHADOW_INFO..SHADOW_INFO + 4].copy_from_slice(&[SHADOW_SIZE as f32, 0.0, tan_half, 0.0]);
    for (cam, mut camera, mut transform, mut global) in &mut cameras {
        let slot = cam.0;
        let Some((position, rotation)) = wanted.get(slot).copied().flatten() else {
            state.slots[slot] = None;
            if camera.is_active {
                camera.is_active = false;
            }
            continue;
        };
        let moved =
            state.slots[slot].is_none_or(|s| s.position.distance_squared(position) > 0.01 || s.rotation.angle_between(rotation) > 1e-3);
        let age = state.slots[slot].map_or(0, |s| s.age + 1);
        let due = moved || age >= interval || (frame + slot as u32 * 2).is_multiple_of(interval);
        let t = Transform { translation: position, rotation: camera_rotation(rotation), ..default() };
        let p = PerspectiveProjection { fov: FOV_DEGREES.to_radians(), aspect_ratio: 1.0, near: NEAR, far: FAR, ..default() };
        let clip_from_world = bevy::camera::CameraProjection::get_clip_from_view(&p) * t.to_matrix().inverse();
        let m = MATRICES + slot * 16;
        buffer.data[m..m + 16].copy_from_slice(&clip_from_world.to_cols_array());
        if due {
            // Set the global transform too: propagation has run this frame.
            *transform = t;
            *global = GlobalTransform::from(t);
            camera.is_active = true;
            state.slots[slot] = Some(SlotState { position, rotation, age: 0 });
        } else {
            if camera.is_active {
                camera.is_active = false;
            }
            if let Some(s) = state.slots[slot].as_mut() {
                s.age = age;
            }
        }
    }
}
