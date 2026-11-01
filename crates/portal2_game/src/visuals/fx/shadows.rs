//! Shadow maps for the overhaul's strongest lights (the portals first), so
//! a portal's glow stops at the cube or pillar in front of it.
//!
//! They work like the projected textures' maps (`visuals/projected.rs`): a
//! camera per map draws its view into a distance texture, which the world
//! and model shaders fill with the distance to the light. Every extra view
//! costs the render thread a full pass over the scene, so a map is only
//! redrawn when its light moves or takes the slot, and otherwise every
//! twelfth frame (staggered between maps) to follow moving props.
//!
//! Ultra keeps every map exact instead: only opaque world and model meshes
//! draw into them (every other shader discards in shadow views), so a map
//! is redrawn the frame anything of those within its light's reach moves,
//! animates, appears, disappears, changes shape or changes which of its
//! pixels draw (its material's alpha test, translucency, culling or portal
//! clip plane), and is kept otherwise (see `visuals/shadow_casters.rs`). A
//! portal's light over a still scene then costs no extra passes.

use bevy::camera::visibility::{RenderLayers, VisibilitySystems};
use bevy::camera::{Exposure, Hdr, RenderTarget};
use bevy::prelude::*;
use bevy::render::view::Tonemapping;
use portal2_render::fx::{FX, HEADER_FLOATS, LIGHT_FLOATS, MAX_SHADOWS};

use super::lights::{FxActiveLights, FxLightShape};
use super::material::{FxBuffer, FxFillSet};
use super::{FxPreset, FxSettings, features};
use crate::visuals::projected::{SHADOW_CLEAR, SHADOW_SIZE, shadow_fog};
use crate::visuals::shadow_casters::{ShadowCasters, ShadowCastersPlugin, ShadowCastersSet};
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
        if !app.is_plugin_added::<ShadowCastersPlugin>() {
            app.add_plugins(ShadowCastersPlugin);
        }
        // Woken cameras are aimed before their frusta and visible meshes are
        // worked out for the frame.
        app.init_resource::<ShadowSlots>().add_systems(Startup, spawn_cameras).add_systems(
            PostUpdate,
            assign.in_set(FxFillSet).after(super::lights::gather).after(ShadowCastersSet).before(VisibilitySystems::UpdateFrusta),
        );
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

/// Frames between redraws of a shadow map whose light has not moved (a
/// moved light is redrawn at once), following props moving in the light;
/// none on ultra, which redraws exactly when what the map shows changes.
fn refresh_interval(preset: FxPreset) -> Option<u32> {
    (preset != FxPreset::Ultra).then_some(12)
}

/// Whether a change in a sphere shows in a light's shadow map: within the
/// light's reach and not behind it.
fn shows_in(light: Vec3, forward: Vec3, reach: f32, (center, radius): (Vec3, f32)) -> bool {
    let d = center - light;
    d.length_squared() < (reach + radius) * (reach + radius) && d.dot(forward) > -radius
}

/// Gives the strongest shadow-casting lights a map, writes their matrices
/// and slots into the buffer, and points (and wakes) the cameras.
fn assign(
    settings: Res<FxSettings>,
    active: Res<FxActiveLights>,
    mut buffer: ResMut<FxBuffer>,
    mut state: ResMut<ShadowSlots>,
    casters: Res<ShadowCasters>,
    mut cameras: Query<(&FxShadowCamera, &mut Camera, &mut Transform, &mut GlobalTransform)>,
) {
    let slots = if settings.has(features::SHADOWS) && settings.has(features::LIGHTS) { settings.shadow_slots.min(MAX_SHADOWS) } else { 0 };
    state.frame = state.frame.wrapping_add(1);
    let frame = state.frame;
    let interval = refresh_interval(settings.preset);
    let tan_half = (FOV_DEGREES.to_radians() * 0.5).tan();
    // The strongest shadow-casting lights, with their index in the buffer.
    let mut lights: Vec<(usize, Vec3, Quat, f32)> = Vec::new();
    for (i, light) in active.0.iter().enumerate() {
        if lights.len() >= slots {
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
        lights.push((i, light.position, rotation, light.range.min(FAR)));
    }
    // A light keeps the map it had (lights of equal score, such as the two
    // portals', trade places as they flicker); the rest take free ones.
    let same = |s: &SlotState, position: Vec3, rotation: Quat| {
        s.position.distance_squared(position) <= 0.01 && s.rotation.angle_between(rotation) <= 1e-3
    };
    let mut wanted: [Option<(Vec3, Quat, f32)>; MAX_SHADOWS] = [None; MAX_SHADOWS];
    let mut placed = vec![false; lights.len()];
    for (n, &(i, position, rotation, reach)) in lights.iter().enumerate() {
        let kept = (0..slots).find(|&k| wanted[k].is_none() && state.slots[k].is_some_and(|s| same(&s, position, rotation)));
        if let Some(k) = kept {
            wanted[k] = Some((position, rotation, reach));
            buffer.data[HEADER_FLOATS + i * LIGHT_FLOATS + 14] = k as f32;
            placed[n] = true;
        }
    }
    for (n, &(i, position, rotation, reach)) in lights.iter().enumerate() {
        if placed[n] {
            continue;
        }
        let Some(k) =
            (0..slots).find(|&k| wanted[k].is_none() && !lights.iter().any(|l| state.slots[k].is_some_and(|s| same(&s, l.1, l.2))))
        else {
            continue;
        };
        wanted[k] = Some((position, rotation, reach));
        buffer.data[HEADER_FLOATS + i * LIGHT_FLOATS + 14] = k as f32;
    }
    buffer.data[SHADOW_INFO..SHADOW_INFO + 4].copy_from_slice(&[SHADOW_SIZE as f32, 0.0, tan_half, 0.0]);
    for (cam, mut camera, mut transform, mut global) in &mut cameras {
        let slot = cam.0;
        let Some((position, rotation, reach)) = wanted.get(slot).copied().flatten() else {
            state.slots[slot] = None;
            if camera.is_active {
                camera.is_active = false;
            }
            continue;
        };
        let moved = state.slots[slot].is_none_or(|s| !same(&s, position, rotation));
        let age = state.slots[slot].map_or(0, |s| s.age + 1);
        let forward = rotation * Vec3::X;
        let touched = casters.stale(|c| shows_in(position, forward, reach, (c.center, c.radius)));
        let due = match interval {
            // Ultra: exactly when what the map shows changes.
            None => moved || touched,
            Some(interval) => moved || age >= interval || (frame + slot as u32 * 2).is_multiple_of(interval),
        };
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_show_within_reach_and_in_front() {
        let light = Vec3::ZERO;
        assert!(shows_in(light, Vec3::X, 420.0, (Vec3::new(400.0, 0.0, 0.0), 30.0)));
        assert!(!shows_in(light, Vec3::X, 420.0, (Vec3::new(500.0, 0.0, 0.0), 30.0)));
        assert!(!shows_in(light, Vec3::X, 420.0, (Vec3::new(-100.0, 0.0, 0.0), 30.0)), "behind the light");
        assert!(shows_in(light, Vec3::X, 420.0, (Vec3::new(-10.0, 0.0, 0.0), 30.0)), "reaching past its plane");
    }
}
