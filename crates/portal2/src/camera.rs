//! Main view camera and the free-fly (noclip) controller used before the
//! player controller takes over, and for debugging.

use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::camera::Hdr;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};

use crate::args::Args;
use crate::coords::{angles_to_quat, camera_rotation};
use crate::world::PlayerStart;

/// Source-space eye position and view angles (pitch, yaw, roll in degrees).
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct ViewAngles {
    pub origin: Vec3,
    pub angles: Vec3,
}

#[derive(Component)]
pub struct MainCamera;

#[derive(Resource)]
pub struct FlyCamera {
    pub enabled: bool,
    pub speed: f32,
}

pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(FlyCamera { enabled: true, speed: 400.0 })
            .add_systems(Startup, spawn_camera)
            .add_systems(Update, (place_at_start, grab_cursor, fly_camera).chain())
            .add_systems(PostUpdate, sync_camera_transform.before(TransformSystems::Propagate));
    }
}

fn spawn_camera(mut commands: Commands, args: Res<Args>) {
    // Bevy's view exposure is 1 / (1.2 * 2^ev100); solve for the requested scale.
    let ev100 = -(1.2 * args.exposure.max(0.001)).log2();
    commands.spawn((
        Name::new("main camera"),
        MainCamera,
        Camera3d::default(),
        Hdr,
        Projection::Perspective(PerspectiveProjection {
            // fov_desired 75 is horizontal at 4:3; widescreen widens it.
            fov: 2.0 * ((75.0f32.to_radians() * 0.5).tan() * 0.75).atan(),
            near: 4.0,
            far: 30000.0,
            ..default()
        }),
        bevy::camera::Exposure { ev100 },
        bevy::render::view::Tonemapping::None,
        bevy::render::view::DebandDither::Disabled,
        Msaa::Sample4,
        ViewAngles::default(),
        bevy::camera::visibility::RenderLayers::from_layers(&[0, crate::portals::PORTAL_LAYER]),
        Transform::default(),
    ));
}

fn place_at_start(
    start: Option<Res<PlayerStart>>,
    args: Res<Args>,
    mut q: Query<&mut ViewAngles, With<MainCamera>>,
) {
    let Some(start) = start else { return };
    if !start.is_changed() {
        return;
    }
    for mut view in &mut q {
        view.origin = args.pos.unwrap_or(start.origin + Vec3::new(0.0, 0.0, 64.0));
        view.angles = args.angles.unwrap_or(start.angles);
    }
}

fn grab_cursor(
    mut cursor: Query<&mut CursorOptions, With<PrimaryWindow>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    let Ok(mut cursor) = cursor.single_mut() else { return };
    if mouse.just_pressed(MouseButton::Left) {
        cursor.grab_mode = CursorGrabMode::Locked;
        cursor.visible = false;
    }
    if keys.just_pressed(KeyCode::Escape) {
        cursor.grab_mode = CursorGrabMode::None;
        cursor.visible = true;
    }
}

fn fly_camera(
    fly: Res<FlyCamera>,
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    motion: Res<AccumulatedMouseMotion>,
    cursor: Query<&CursorOptions, With<PrimaryWindow>>,
    mut q: Query<&mut ViewAngles, With<MainCamera>>,
) {
    if !fly.enabled {
        return;
    }
    let grabbed = cursor.single().is_ok_and(|c| c.grab_mode != CursorGrabMode::None);
    let dt = time.delta_secs();
    for mut view in &mut q {
        if grabbed {
            // m_yaw / m_pitch 0.022 deg per count at sensitivity 3.
            let sens = 0.022 * 3.0;
            view.angles.y -= motion.delta.x * sens;
            view.angles.x = (view.angles.x + motion.delta.y * sens).clamp(-89.0, 89.0);
        }
        let rot = angles_to_quat(view.angles);
        let forward = rot * Vec3::X;
        let left = rot * Vec3::Y;
        let mut wish = Vec3::ZERO;
        if keys.pressed(KeyCode::KeyW) {
            wish += forward;
        }
        if keys.pressed(KeyCode::KeyS) {
            wish -= forward;
        }
        if keys.pressed(KeyCode::KeyA) {
            wish += left;
        }
        if keys.pressed(KeyCode::KeyD) {
            wish -= left;
        }
        if keys.pressed(KeyCode::Space) {
            wish += Vec3::Z;
        }
        if keys.pressed(KeyCode::ControlLeft) {
            wish -= Vec3::Z;
        }
        let speed = if keys.pressed(KeyCode::ShiftLeft) { fly.speed * 3.0 } else { fly.speed };
        view.origin += wish.normalize_or_zero() * speed * dt;
    }
}

fn sync_camera_transform(mut q: Query<(&ViewAngles, &mut Transform), With<MainCamera>>) {
    for (view, mut t) in &mut q {
        t.translation = view.origin;
        t.rotation = camera_rotation(angles_to_quat(view.angles));
    }
}
