//! The player: input, fixed-tick movement, camera placement and "use".

pub mod autopilot;
pub mod health;
pub mod movement;

use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};

use crate::args::Args;
use crate::camera::{FlyCamera, MainCamera, ViewAngles};
use crate::collision::Physics;
use crate::entities::EntityRegistry;
use crate::world::PlayerStart;
use movement::{MoveInput, MoveState, Mover};

/// Source runs gameplay at a fixed tick; 60 Hz matches the single-player game.
pub const TICK_RATE: f64 = 60.0;

pub struct PlayerPlugin;

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((autopilot::AutopilotPlugin, health::HealthPlugin))
            .insert_resource(Time::<Fixed>::from_hz(TICK_RATE))
            .init_resource::<PlayerInput>()
            .add_systems(Update, (spawn_or_reset_player, gather_input).chain())
            .add_systems(FixedUpdate, player_tick.in_set(PlayerMoveSet))
            .add_systems(PostUpdate, place_camera.in_set(PlayerCameraSet).before(TransformSystems::Propagate));
    }
}

#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct PlayerMoveSet;

/// Placement of the main camera at the player's eye.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct PlayerCameraSet;

#[derive(Component)]
pub struct Player {
    pub state: MoveState,
    pub previous_origin: Vec3,
    pub noclip: bool,
    /// Entity stood on last tick and its placement then, so moving
    /// platforms can carry the player.
    pub ground_ref: Option<(Entity, bevy::math::Affine3A)>,
}

#[derive(Resource, Default)]
pub struct PlayerInput {
    pub input: MoveInput,
    pub fast: bool,
}

fn spawn_or_reset_player(
    mut commands: Commands,
    start: Option<Res<PlayerStart>>,
    args: Res<Args>,
    mut players: Query<(Entity, &mut Player, &mut Transform)>,
    mut registry: ResMut<EntityRegistry>,
    mut input: ResMut<PlayerInput>,
    mut fly: ResMut<FlyCamera>,
) {
    let Some(start) = start else { return };
    // Keep the registry pointing at the player after entity respawns.
    if let Ok((e, _, _)) = players.single() {
        registry.player = Some(e);
    }
    if !start.is_changed() {
        return;
    }
    let origin = args.pos.map(|p| p - Vec3::Z * movement::EYE_HEIGHT).unwrap_or(start.origin);
    let angles = args.angles.unwrap_or(start.angles);
    input.input.angles = Vec3::new(angles.x, angles.y, 0.0);
    let state = MoveState { origin, ..Default::default() };
    fly.enabled = false;
    if let Ok((_, mut p, mut t)) = players.single_mut() {
        p.state = state;
        p.previous_origin = origin;
        p.noclip = args.noclip;
        p.ground_ref = None;
        t.translation = origin;
        return;
    }
    let e = commands
        .spawn((
            Name::new("player"),
            Player { state, previous_origin: origin, noclip: args.noclip, ground_ref: None },
            Transform::from_translation(origin),
            Visibility::Hidden,
        ))
        .id();
    registry.player = Some(e);
}

fn gather_input(
    keys: Res<ButtonInput<KeyCode>>,
    motion: Res<AccumulatedMouseMotion>,
    cursor: Query<&CursorOptions, With<PrimaryWindow>>,
    mut input: ResMut<PlayerInput>,
    mut players: Query<&mut Player>,
) {
    let grabbed = cursor.single().is_ok_and(|c| c.grab_mode != CursorGrabMode::None);
    let i = &mut input.input;
    if grabbed {
        // m_yaw / m_pitch of 0.022 degrees per count at sensitivity 3.
        let sens = 0.022 * 3.0;
        i.angles.y -= motion.delta.x * sens;
        i.angles.x = (i.angles.x + motion.delta.y * sens).clamp(-89.0, 89.0);
        i.angles.y = i.angles.y.rem_euclid(360.0);
    }
    let axis = |pos: KeyCode, neg: KeyCode| (keys.pressed(pos) as i32 - keys.pressed(neg) as i32) as f32;
    i.forward = axis(KeyCode::KeyW, KeyCode::KeyS);
    i.right = axis(KeyCode::KeyD, KeyCode::KeyA);
    i.up = axis(KeyCode::Space, KeyCode::ControlLeft);
    i.jump = keys.pressed(KeyCode::Space);
    i.duck = keys.pressed(KeyCode::ControlLeft);
    input.fast = keys.pressed(KeyCode::ShiftLeft);
    if keys.just_pressed(KeyCode::KeyV) {
        for mut p in &mut players {
            p.noclip = !p.noclip;
            info!("noclip {}", if p.noclip { "on" } else { "off" });
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn player_tick(
    time: Res<Time<Fixed>>,
    input: Res<PlayerInput>,
    physics: Physics,
    placements: Query<&GlobalTransform>,
    holding: Option<Res<crate::physics::carry::Holding>>,
    pull: Option<Res<crate::devices::funnel::PlayerPull>>,
    mut air: Option<ResMut<crate::devices::catapult::AirControl>>,
    mut players: Query<(Entity, &mut Player, &mut Transform)>,
) {
    let dt = time.delta_secs();
    let now = time.elapsed_secs_f64();
    let held = holding.and_then(|h| h.entity);
    let pull = pull.and_then(|p| p.0);
    for (e, mut p, mut t) in &mut players {
        p.previous_origin = p.state.origin;
        // Ride along with whatever the player stands on.
        if let Some((g, before)) = p.ground_ref
            && let Ok(now) = placements.get(g)
        {
            let now = now.affine();
            if now != before {
                let carried = now.transform_point3(before.inverse().transform_point3(p.state.origin));
                p.state.origin = carried;
            }
        }
        // The carried prop never blocks its carrier.
        let ignore: Vec<Entity> = std::iter::once(e).chain(held).collect();
        let mover = Mover { physics: &physics, ignore: &ignore };
        let mut state = p.state;
        let mut move_input = input.input;
        // After a launch, steering stays off until the timer or landing.
        if let Some(air) = air.as_deref_mut() {
            if state.on_ground {
                air.suppressed_until = 0.0;
            } else if now < air.suppressed_until {
                move_input.forward = 0.0;
                move_input.right = 0.0;
            }
        }
        if p.noclip {
            mover.noclip(&mut state, &input.input, dt, input.fast);
        } else if let Some(pull) = pull {
            let (mins, maxs) = state.hull();
            let center = state.origin + (mins + maxs) * 0.5;
            let q = crate::coords::angles_to_quat(move_input.angles);
            let wish = q * Vec3::X * move_input.forward - q * Vec3::Y * move_input.right;
            let flow = crate::devices::funnel::player_flow(&pull, center, wish);
            mover.funnel_move(&mut state, &move_input, flow, dt);
        } else {
            mover.step(&mut state, &move_input, dt);
        }
        p.state = state;
        p.ground_ref = state.ground_entity.and_then(|g| placements.get(g).ok().map(|gt| (g, gt.affine())));
        t.translation = state.origin;
    }
}

fn place_camera(
    fixed: Res<Time<Fixed>>,
    input: Res<PlayerInput>,
    players: Query<&Player>,
    mut cams: Query<&mut ViewAngles, With<MainCamera>>,
) {
    let Ok(p) = players.single() else { return };
    let alpha = fixed.overstep_fraction();
    let origin = p.previous_origin.lerp(p.state.origin, alpha);
    for mut view in &mut cams {
        view.origin = origin + Vec3::Z * p.state.eye_offset();
        view.angles = input.input.angles;
    }
}
