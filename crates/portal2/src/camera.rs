//! Main view camera and the free-fly (noclip) controller used before the
//! player controller takes over, and for debugging.

use bevy::camera::Hdr;
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};

use crate::world::PlayerStart;
use portal2_core::args::Args;
use portal2_core::coords::{angles_to_quat, camera_rotation};
pub use portal2_core::view::{MainCamera, ViewAngles};

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
            .add_systems(Update, (place_at_start, grab_cursor, fly_camera, apply_fov).chain())
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
        Projection::Perspective(PerspectiveProjection { fov: vertical_fov(75.0), near: 4.0, far: 30000.0, ..default() }),
        bevy::camera::Exposure { ev100 },
        bevy::render::view::Tonemapping::None,
        bevy::render::view::DebandDither::Disabled,
        Msaa::Sample4,
        // Refract materials read a copy of the opaque pass.
        bevy::pbr::ScreenSpaceTransmission::default(),
        ViewAngles::default(),
        bevy::camera::visibility::RenderLayers::from_layers(&[0, portal2_core::layers::PORTAL_LAYER]),
        Transform::default(),
    ));
}

/// `fov_desired` is horizontal at 4:3; wider screens see more to the sides.
pub fn vertical_fov(horizontal_4x3_deg: f32) -> f32 {
    2.0 * ((horizontal_4x3_deg.to_radians() * 0.5).tan() * 0.75).atan()
}

fn apply_fov(console: Res<portal2_core::console::Console>, mut q: Query<&mut Projection, With<MainCamera>>) {
    let fov = vertical_fov(console.f32("fov_desired").clamp(1.0, 179.0));
    for mut p in &mut q {
        if let Projection::Perspective(pp) = p.as_mut()
            && (pp.fov - fov).abs() > 1e-6
        {
            pp.fov = fov;
        }
    }
}

fn place_at_start(start: Option<Res<PlayerStart>>, args: Res<Args>, mut q: Query<&mut ViewAngles, With<MainCamera>>) {
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
    ui: Option<Res<crate::gameui::GameUi>>,
) {
    let Ok(mut cursor) = cursor.single_mut() else { return };
    // The menus own the mouse while they are up.
    if ui.is_some_and(|u| u.active()) {
        return;
    }
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

pub fn sync_camera_transform(mut q: Query<(&ViewAngles, &mut Transform), With<MainCamera>>) {
    for (view, mut t) in &mut q {
        t.translation = view.origin;
        t.rotation = camera_rotation(angles_to_quat(view.angles));
    }
}
