//! The player's own body (`models/player/chell/player.mdl`), drawn only in
//! the views through portals, as the game does: walking up to a portal you
//! see yourself in it. The first-person view never shows it.
//!
//! The body stands at the player's origin turned to their view's yaw and
//! plays the model's movement sequences the way the player animation state
//! picks them: `nogun_` or `portalgun_` (whether the gun is held), then
//! `standing_idle`, `run`, `crouch_idle`, `crouchWalk` or `airwalk`, with
//! the run blends steered by `move_x` / `move_y` (the velocity in the body's
//! frame over the walking speed) and the aim matrix layered on top
//! (`body_pitch` = minus the view's pitch).

use bevy::camera::visibility::{NoFrustumCulling, RenderLayers};
use bevy::ecs::system::SystemState;
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;
use portal2_core::layers::PLAYER_BODY_LAYER;

use super::{Player, PlayerInput};
use portal2_core::coords::angles_to_quat;
use portal2_core::sets::PlayerCameraSet;
use portal2_core::vfs::GameFs;
use portal2_render::materials::{MaterialCache, MaterialContext};
use portal2_render::models::animation::Layer;
use portal2_render::models::{ModelCache, SpawnModel, StudioAnimator, lighting, spawn_model_parts};
use portal2_render::source_material::{ModelMaterial, RefractMaterial};
use portal2_render::textures::TextureCache;
use portal2_world::{CurrentMap, MapObject};

pub const BODY_MODEL: &str = "models/player/chell/player.mdl";
/// Below this speed the body stands idle.
const MOVING_SPEED: f32 = 10.0;

pub struct BodyPlugin;

impl Plugin for BodyPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, spawn_body).add_systems(PostUpdate, pose_body.after(PlayerCameraSet).before(TransformSystems::Propagate));
    }
}

#[derive(Component)]
pub struct PlayerBody {
    light_timer: f32,
}

type BodyParams<'w, 's> = (
    Commands<'w, 's>,
    Res<'w, GameFs>,
    Res<'w, CurrentMap>,
    ResMut<'w, ModelCache>,
    ResMut<'w, MaterialCache>,
    ResMut<'w, TextureCache>,
    ResMut<'w, Assets<Image>>,
    ResMut<'w, Assets<Mesh>>,
    ResMut<'w, Assets<ModelMaterial>>,
    ResMut<'w, Assets<RefractMaterial>>,
    ResMut<'w, Assets<SkinnedMeshInverseBindposes>>,
);

/// Gives the player a body once a map is up (it goes with the map).
fn spawn_body(world: &mut World) {
    if world.get_resource::<CurrentMap>().is_none() || world.query::<&Player>().iter(world).next().is_none() {
        return;
    }
    if world.query_filtered::<(), With<PlayerBody>>().iter(world).next().is_some() {
        return;
    }
    let root =
        world.spawn((Name::new("player body"), PlayerBody { light_timer: 0.0 }, MapObject, Transform::default(), Visibility::Visible)).id();
    let mut state: SystemState<BodyParams> = SystemState::new(world);
    let (mut commands, fs, map, mut cache, mut mats, mut textures, mut images, mut meshes, mut model_mats, mut refract_mats, mut bindposes) =
        state.get_mut(world).expect("render resources available");
    let Some(model) = cache.get(BODY_MODEL, &fs, &mut meshes, &mut bindposes) else {
        state.apply(world);
        warn!("no player body model {BODY_MODEL}");
        world.entity_mut(root).remove::<Visibility>();
        return;
    };
    let light = lighting::compute(&map.bsp, Vec3::ZERO);
    let mut ctx = MaterialContext { fs: &fs, textures: &mut textures, images: &mut images };
    let joints = spawn_model_parts(
        &mut commands,
        root,
        SpawnModel { model: &model, skin: 0, body: 0, vhv: None, animated: true, lighting: Some(light), envmap: None },
        &fs,
        &mut ctx,
        &mut mats,
        &mut meshes,
        &mut model_mats,
        &mut refract_mats,
    );
    let mut animator = StudioAnimator::new(model.clone(), joints);
    animator.set_sequence("nogun_standing_idle");
    commands.entity(root).insert(animator);
    state.apply(world);
    // The body's bind pose is not where it stands, so its bounds cannot cull it.
    let mut stack = vec![root];
    while let Some(e) = stack.pop() {
        world.entity_mut(e).insert((RenderLayers::layer(PLAYER_BODY_LAYER), NoFrustumCulling));
        if let Some(children) = world.get::<Children>(e) {
            stack.extend(children.iter());
        }
    }
}

/// The movement sequence for the player's state, without its gun prefix.
fn movement_sequence(on_ground: bool, ducked: bool, speed: f32) -> &'static str {
    match (on_ground, ducked, speed > MOVING_SPEED) {
        (false, _, _) => "airwalk",
        (true, true, true) => "crouchWalk",
        (true, true, false) => "crouch_idle",
        (true, false, true) => "run",
        (true, false, false) => "standing_idle",
    }
}

pub(crate) fn pose_body(
    time: Res<Time>,
    fixed: Res<Time<Fixed>>,
    input: Res<PlayerInput>,
    gun: Option<Res<crate::portals::gun::PortalGun>>,
    map: Option<Res<CurrentMap>>,
    players: Query<&Player>,
    mut bodies: Query<(&mut Transform, &mut StudioAnimator, &mut PlayerBody, &Children)>,
    mesh_mats: Query<&MeshMaterial3d<ModelMaterial>>,
    mut materials: ResMut<Assets<ModelMaterial>>,
) {
    let Ok(p) = players.single() else { return };
    let Ok((mut t, mut anim, mut body, children)) = bodies.single_mut() else { return };
    let origin = p.previous_origin.lerp(p.state.origin, fixed.overstep_fraction());
    let yaw = input.input.angles.y;
    t.translation = origin;
    t.rotation = angles_to_quat(Vec3::new(0.0, yaw, 0.0));

    let has_gun = gun.is_some_and(|g| g.can_fire.iter().any(|&f| f));
    let prefix = if has_gun { "portalgun_" } else { "nogun_" };
    let s = &p.state;
    let horizontal = s.velocity.truncate();
    let speed = horizontal.length();
    let wanted = format!("{prefix}{}", movement_sequence(s.on_ground, s.ducked, speed));
    if anim.sequence_name() != wanted {
        anim.set_sequence(&wanted);
    }
    // Velocity in the body's frame, over the speed the run is made for.
    let (sin, cos) = yaw.to_radians().sin_cos();
    let walk = if s.ducked { s.max_speed * crate::player::movement::DUCK_SPEED_SCALE } else { s.max_speed };
    let forward = (horizontal.x * cos + horizontal.y * sin) / walk.max(1.0);
    let left = (-horizontal.x * sin + horizontal.y * cos) / walk.max(1.0);
    anim.set_pose_parameter("move_x", forward.clamp(-1.0, 1.0));
    anim.set_pose_parameter("move_y", left.clamp(-1.0, 1.0));
    anim.set_pose_parameter("body_pitch", -input.input.angles.x);
    anim.set_pose_parameter("body_yaw", 0.0);
    anim.rate = if speed > MOVING_SPEED { (speed / walk.max(1.0)).max(0.1) } else { 1.0 };
    let stance = if s.ducked { "crouch" } else { "standing" };
    let aim = format!("{prefix}{stance}_aimmatrix");
    if let Some(aim) = anim.model.studio.find_sequence(&aim)
        && anim.layers().first().is_none_or(|l| l.sequence != aim)
    {
        anim.set_layers(vec![Layer { sequence: aim, cycle: 0.0, weight: 1.0 }]);
    }

    // Refresh lighting a few times a second.
    body.light_timer -= time.delta_secs();
    if body.light_timer <= 0.0
        && let Some(map) = map.as_ref()
    {
        body.light_timer = 0.25;
        let light = lighting::compute(&map.bsp, origin + Vec3::Z * 36.0);
        for c in children.iter() {
            if let Ok(h) = mesh_mats.get(c)
                && let Some(mut m) = materials.get_mut(&h.0)
            {
                light.apply(&mut m.params);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequence_follows_movement() {
        assert_eq!(movement_sequence(true, false, 0.0), "standing_idle");
        assert_eq!(movement_sequence(true, false, 175.0), "run");
        assert_eq!(movement_sequence(true, true, 60.0), "crouchWalk");
        assert_eq!(movement_sequence(false, false, 175.0), "airwalk");
    }
}
