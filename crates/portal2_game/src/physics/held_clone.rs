//! A prop held as a view model clone (Wheatley) is drawn over the world, as
//! the game's clone in the view model is: walls and the breaker train's
//! socket housing it is held inside do not hide it. It keeps its place and
//! the player's field of view, so it is drawn by a camera that follows the
//! main one, between the world and the weapon.

use bevy::camera::visibility::RenderLayers;
use bevy::camera::{Camera3dDepthLoadOp, Exposure, Hdr};
use bevy::prelude::*;
use bevy::render::view::Tonemapping;
use portal2_core::layers::HELD_CLONE_LAYER;
use portal2_core::sets::PlayerCameraSet;
use portal2_core::view::MainCamera;

use super::carry::Holding;

pub struct HeldClonePlugin;

impl Plugin for HeldClonePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_camera)
            .add_systems(Update, draw_clone)
            .add_systems(PostUpdate, follow_main.after(PlayerCameraSet).before(TransformSystems::Propagate));
    }
}

#[derive(Component)]
struct HeldCloneCamera;

fn spawn_camera(mut commands: Commands) {
    commands.spawn((
        Name::new("held clone camera"),
        HeldCloneCamera,
        Camera3d { depth_load_op: Camera3dDepthLoadOp::Clear(0.0), ..default() },
        Camera { order: 1, clear_color: ClearColorConfig::None, is_active: false, ..default() },
        Hdr,
        Tonemapping::None,
        bevy::render::view::DebandDither::Disabled,
        Exposure::default(),
        Msaa::Sample4,
        Projection::default(),
        RenderLayers::layer(HELD_CLONE_LAYER),
        Transform::default(),
    ));
}

/// Moves the held clone's meshes onto the clone layer (and back to the
/// world when it is let go). A scripted view sees it in the world.
fn draw_clone(
    holding: Res<Holding>,
    view: Res<crate::entities::viewcontrol::ActiveView>,
    children: Query<&Children>,
    layered: Query<&RenderLayers>,
    mut shown: Local<Option<Entity>>,
    mut cams: Query<&mut Camera, With<HeldCloneCamera>>,
    mut commands: Commands,
) {
    let want = holding.entity.filter(|_| holding.view_model && view.0.is_none());
    if *shown != want
        && let Some(old) = shown.take()
    {
        for e in std::iter::once(old).chain(children.iter_descendants(old)) {
            commands.entity(e).try_remove::<RenderLayers>();
        }
    }
    if let Some(held) = want {
        // Every frame: a new skin or body group rebuilds the model's meshes.
        let clone = RenderLayers::layer(HELD_CLONE_LAYER);
        for e in std::iter::once(held).chain(children.iter_descendants(held)) {
            if layered.get(e).is_ok_and(|l| *l == clone) {
                continue;
            }
            commands.entity(e).try_insert(clone.clone());
        }
    }
    *shown = want;
    for mut cam in &mut cams {
        if cam.is_active != want.is_some() {
            cam.is_active = want.is_some();
        }
    }
}

fn follow_main(
    main: Query<(&Transform, &Projection, &Exposure), (With<MainCamera>, Without<HeldCloneCamera>)>,
    mut cam: Query<(&mut Transform, &mut Projection, &mut Exposure), With<HeldCloneCamera>>,
) {
    let (Ok((t, p, e)), Ok((mut ct, mut cp, mut ce))) = (main.single(), cam.single_mut()) else { return };
    *ct = *t;
    *cp = p.clone();
    *ce = *e;
}
