//! Picking up, carrying and dropping physics props, plus pushing props by
//! walking into them. `+use` also uses non-prop entities.

use avian3d::prelude::*;
use bevy::prelude::*;

use super::{layers, layers_for};
use crate::entities::io::{EventQueue, EventTarget, FireOutput};
use crate::entities::props::PhysicsProp;
use crate::player::{Player, PlayerInput};
use portal2_core::coords::{angles_forward, angles_to_quat, quat_to_angles};
use portal2_core::events::{EventValue, GameEvent};
use portal2_core::sets::PlayerMoveSet;
use portal2_world::collision::portal::{HALF_HEIGHT, HALF_WIDTH, PortalHoles, transfer};
use portal2_world::collision::{Collider as TraceCollider, Physics, mask};

/// Horizontal radius of the player's hull, part of the hold distance.
const PLAYER_RADIUS: f32 = 22.627_417;
/// Reach for picking up and using.
const REACH: f32 = 80.0;
/// Top speed of the hold controller; larger implied speeds count half
/// toward the drop error.
const HOLD_MAX_SPEED: f32 = 1000.0;
/// Gap the steering leaves between the held prop and what it is pushed
/// against.
const HOLD_SKIN: f32 = 0.25;
/// Deepest the held prop may sink into the world (turning against a pane,
/// squeezed into a corner) before it is put back where it last was clear;
/// well above the contact solver's slop, below half the thinnest glass.
const HOLD_MAX_PENETRATION: f32 = 0.5;
/// A held prop that moved further than this in one tick went through a
/// portal: its new place counts as clear.
const HOLD_TELEPORT_DISTANCE: f32 = 64.0;
/// Half size of the box traced along the view to the hold point and to the
/// held prop: a box, not a ray, so that it passes into a portal's opening
/// (ray traces stop at the wall behind it, which portal beams rely on).
const SIGHT_BOX: Vec3 = Vec3::splat(0.5);

pub struct CarryPlugin;

impl Plugin for CarryPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Holding>()
            .init_resource::<ForcedPickup>()
            .add_systems(PreUpdate, forget_on_map_load.after(portal2_core::sets::LoadMapSet).before(portal2_core::sets::EntitySpawnSet))
            .add_systems(Update, use_or_grab)
            .add_systems(FixedUpdate, (hold_held, push_props).after(PlayerMoveSet));
    }
}

#[derive(Resource, Default, serde::Serialize, serde::Deserialize)]
pub struct Holding {
    #[serde(with = "portal2_core::save::ids::opt_entity")]
    pub entity: Option<Entity>,
    /// Prop yaw relative to the player's view when picked up (degrees).
    pub yaw_offset: f32,
    /// The held prop's bounding radius.
    #[serde(alias = "distance")]
    pub radius: f32,
    /// Weighted cubes ride lower than other props.
    #[serde(default)]
    pub cube: bool,
    /// Personality cores ride lower still as view model clones.
    #[serde(default)]
    pub core: bool,
    /// Running average of how far the prop trails its hold point.
    #[serde(default)]
    pub error: f32,
    /// Height the prop is lifted to clear the floor under its hold point.
    #[serde(default)]
    pub lift: f32,
    /// The held prop's last placement clear of the world.
    #[serde(skip)]
    pub clear: Option<(Vec3, Quat)>,
    /// The portal the player sees the held prop through, when it is on the
    /// other side of the pair from them (the game's held object on the
    /// opposite side of a portal): its hold point is carried through.
    #[serde(default)]
    pub portal: Option<usize>,
    /// `logic_playerproxy`'s `SetDropEnabled 0`: `+use` no longer lets go
    /// (Wheatley, once picked up). The game resets it when the player spawns.
    #[serde(default)]
    pub drop_locked: bool,
    /// Held as a view model clone (personality cores): put at its hold point
    /// whatever is in the way, and drawn over the world.
    #[serde(default)]
    pub view_model: bool,
    /// `logic_playerproxy`'s forced grab controller, used from the next
    /// pickup on: 1 view model, 2 physics, 0 the usual choice.
    #[serde(default)]
    pub forced_controller: u8,
}

/// Whether a `classname` prop is held as a view model clone: forced by
/// `logic_playerproxy`, else by `player_held_object_use_view_model`, whose
/// -1 picks personality cores only in single player.
pub fn held_as_view_model(console: &portal2_core::console::Console, forced: u8, classname: &str) -> bool {
    match forced {
        1 => true,
        2 => false,
        _ => match console.i32("player_held_object_use_view_model") {
            0 => false,
            n if n > 0 => true,
            _ => classname == "npc_personality_core",
        },
    }
}

/// Collision for a held prop: a view model clone meets nothing.
fn held_layers(view_model: bool) -> CollisionLayers {
    if view_model { layers_for(layers::HELD, 0) } else { layers_for(layers::HELD, layers::WORLD | layers::PROP | layers::MOVER) }
}

/// A prop to put in the player's hands (`ForcePickup`), taken as soon as it
/// can be held.
#[derive(Resource, Default)]
pub struct ForcedPickup(pub Option<Entity>);

/// A new map starts with nothing held and dropping allowed (a saved game
/// then restores what it had).
fn forget_on_map_load(mut loaded: MessageReader<portal2_world::MapLoaded>, mut holding: ResMut<Holding>, mut forced: ResMut<ForcedPickup>) {
    if loaded.read().last().is_some() {
        *holding = Holding::default();
        forced.0 = None;
    }
}

impl Holding {
    /// The held prop crossed portal `slot`: it is now seen through that
    /// portal, or (coming back) no longer through any.
    pub fn prop_crossed(&mut self, slot: usize) {
        self.portal = if self.portal.is_some() { None } else { Some(slot) };
        self.clear = None;
    }

    /// The player crossed portal `slot`: a prop beyond it is with them
    /// again, one left behind is now seen through the exit portal.
    pub fn player_crossed(&mut self, slot: usize) {
        if self.entity.is_none() {
            return;
        }
        self.portal = match self.portal {
            Some(k) if k == slot => None,
            None => Some(1 - slot),
            other => other,
        };
        self.clear = None;
    }
}

fn prop_radius(col: &TraceCollider) -> f32 {
    ((col.shape.maxs - col.shape.mins) * 0.5).length()
}

fn use_or_grab(
    buttons: Res<portal2_core::console::binds::Buttons>,
    mut holding: ResMut<Holding>,
    physics: Physics,
    players: Query<(Entity, &Player)>,
    input: Res<PlayerInput>,
    props: Query<(&TraceCollider, &GlobalTransform, &RigidBody, &portal2_core::entity::MapEntity), With<PhysicsProp>>,
    mut queue: ResMut<EventQueue>,
    mut out: MessageWriter<FireOutput>,
    mut commands: Commands,
    mut auto: ResMut<crate::player::autopilot::AutoUse>,
    mut sounds: MessageWriter<portal2_audio::SoundRequest>,
    mut events: MessageWriter<GameEvent>,
    mut forced: ResMut<ForcedPickup>,
    console: Res<portal2_core::console::Console>,
) {
    let pressed = buttons.just_pressed("use") || std::mem::take(&mut auto.0);
    let Ok((pe, p)) = players.single() else { return };
    if let Some(target) = forced.0 {
        match props.get(target) {
            Ok((col, gt, body, me)) if *body == RigidBody::Dynamic => {
                forced.0 = None;
                if let Some(held) = holding.entity.take() {
                    release(&mut commands, held);
                }
                let yaw = quat_to_angles(gt.compute_transform().rotation).y;
                grab(&mut holding, &mut commands, &mut out, &mut sounds, &console, pe, target, col, yaw - input.input.angles.y, me);
            }
            Ok(_) => {}
            Err(_) => forced.0 = None,
        }
    }
    if !pressed {
        return;
    }
    // `player_use` / `player_drop` for the instructor's carry lessons.
    let event = |name: &str, e: Entity| {
        GameEvent::new(name).with("userid", EventValue::Entity(Some(pe))).with("entity", EventValue::Entity(Some(e)))
    };
    if let Some(held) = holding.entity {
        if holding.drop_locked {
            return;
        }
        holding.entity = None;
        sounds.write(portal2_audio::SoundRequest::stop("Weapon_Portalgun.HoldSound", pe));
        release(&mut commands, held);
        out.write(FireOutput::new(held, "OnPhysGunDrop").with_activator(Some(pe)));
        out.write(FireOutput::new(held, "OnPlayerDrop").with_activator(Some(pe)));
        events.write(event("player_drop", held));
        return;
    }
    let eye = p.state.origin + Vec3::Z * p.state.eye_offset();
    let dir = angles_forward(input.input.angles);
    let r = Vec3::splat(2.0);
    let tr = physics.trace(eye, eye + dir * REACH, -r, r, mask::SOLID_ALL, &[pe]);
    debug!(
        "use: eye {eye:.0?} dir {dir:.2?} fraction {:.2} entity {:?} body {:?}",
        tr.fraction,
        tr.entity,
        tr.entity.map(|e| props.get(e).map(|p| *p.2))
    );
    let Some(target) = tr.entity else {
        sounds.write(portal2_audio::SoundRequest::on("PortalPlayer.UseDeny", pe));
        return;
    };
    events.write(event("player_use", target));
    match props.get(target) {
        Ok((col, gt, body, me)) if *body == RigidBody::Dynamic && !me.has_flag(crate::entities::props::SF_PREVENT_PICKUP) => {
            let yaw = quat_to_angles(gt.compute_transform().rotation).y;
            grab(&mut holding, &mut commands, &mut out, &mut sounds, &console, pe, target, col, yaw - input.input.angles.y, me);
        }
        _ => queue.push(0.0, EventTarget::Entity(target), "use", "", Some(pe), Some(pe)),
    }
}

/// Takes `target` into the player's hands, turned `yaw_offset` from their view.
#[allow(clippy::too_many_arguments)]
fn grab(
    holding: &mut Holding,
    commands: &mut Commands,
    out: &mut MessageWriter<FireOutput>,
    sounds: &mut MessageWriter<portal2_audio::SoundRequest>,
    console: &portal2_core::console::Console,
    pe: Entity,
    target: Entity,
    col: &TraceCollider,
    yaw_offset: f32,
    me: &portal2_core::entity::MapEntity,
) {
    holding.entity = Some(target);
    // Reflection cubes ask to be carried facing the view (their preferred
    // carry angles are zero), so the beam leaves where the player looks.
    holding.yaw_offset = if is_reflective(me) { 0.0 } else { yaw_offset };
    holding.radius = prop_radius(col);
    holding.cube = me.classname == "prop_weighted_cube";
    holding.core = me.classname == "npc_personality_core";
    holding.error = 0.0;
    holding.lift = 0.0;
    holding.clear = None;
    holding.portal = None;
    holding.view_model = held_as_view_model(console, holding.forced_controller, &me.classname);
    commands.entity(target).insert((GravityScale(0.0), held_layers(holding.view_model)));
    sounds.write(portal2_audio::SoundRequest::on("Weapon_Portalgun.HoldSound", pe));
    out.write(FireOutput::new(target, "OnPlayerPickup").with_activator(Some(pe)));
    out.write(FireOutput::new(target, "OnPhysGunPickup").with_activator(Some(pe)));
}

fn is_reflective(me: &portal2_core::entity::MapEntity) -> bool {
    me.classname == "prop_weighted_cube" && me.get_i32("cubetype") == Some(crate::entities::cubes::REFLECTIVE)
}

/// Puts a prop back in the player's hands (a restored saved game): no
/// gravity, held collision and the hold sound.
pub fn resume_hold(world: &mut World, prop: Entity) {
    // Saves from before view model holding was kept do not say; the choice
    // is the pickup rule's either way.
    let classname = world.get::<portal2_core::entity::MapEntity>(prop).map(|me| me.classname.clone()).unwrap_or_default();
    let reflective = world.get::<portal2_core::entity::MapEntity>(prop).is_some_and(is_reflective);
    let view_model =
        held_as_view_model(world.resource::<portal2_core::console::Console>(), world.resource::<Holding>().forced_controller, &classname);
    if let Ok(mut e) = world.get_entity_mut(prop) {
        e.insert((GravityScale(0.0), held_layers(view_model)));
    }
    // Saves from before the radius was kept stored a hold distance instead.
    let radius = world.get::<TraceCollider>(prop).map(prop_radius);
    let cube = classname == "prop_weighted_cube";
    let mut holding = world.resource_mut::<Holding>();
    holding.view_model = view_model;
    if let Some(r) = radius {
        holding.radius = r;
    }
    holding.cube = cube;
    holding.core = classname == "npc_personality_core";
    if reflective {
        holding.yaw_offset = 0.0;
    }
    if let Some(p) = world.resource::<portal2_core::entity::EntityRegistry>().player {
        world.write_message(portal2_audio::SoundRequest::on("Weapon_Portalgun.HoldSound", p));
    }
}

/// Restores normal physics on a prop that is no longer held.
pub fn release(commands: &mut Commands, prop: Entity) {
    if let Ok(mut e) = commands.get_entity(prop) {
        e.insert((
            GravityScale(1.0),
            layers_for(layers::PROP, layers::WORLD | layers::PROP | layers::PLAYER | layers::MOVER | layers::FOLLOWER | layers::HELD),
        ));
    }
}

/// Where the held prop should be (`docs/re/gameplay.md`, carrying): along
/// the view (pitch limited to ±75°) far enough out to clear the player,
/// in a column in front of them when `player_hold_object_in_column`,
/// pulled in by walls and lifted off floors.
fn hold_point(
    physics: &Physics,
    ignore: &[Entity],
    console: &portal2_core::console::Console,
    holding: &mut Holding,
    player: &crate::player::movement::MoveState,
    view: Vec3,
) -> Vec3 {
    let eye = player.origin + Vec3::Z * player.eye_offset();
    let mut view = view;
    view.x = view.x.clamp(-75.0, 75.0);
    let q = portal2_core::coords::angles_to_quat(view);
    let (forward, up) = (q * Vec3::X, q * Vec3::Z);
    let gap = console.f32("player_held_object_distance");
    let radii = holding.radius + PLAYER_RADIUS;
    let mut distance = gap + radii;
    if console.bool("player_hold_object_in_column") {
        // How far along the view a vertical plane that far ahead lies.
        let flat = forward.with_z(0.0).normalize_or_zero();
        let along = forward.dot(flat);
        let reach = if along > 1e-4 { distance / along } else { f32::MAX };
        distance = reach.clamp(gap, console.f32("player_hold_column_max_size"));
    }
    let raise = if holding.cube { console.f32("player_held_object_offset_up_cube") * (1.0 - view.x.abs() / 75.0) } else { 0.0 };
    let end = eye + forward * distance + up * raise;
    let mask = mask::SOLID_BRUSH_ONLY;
    let tr = physics.trace(eye, end, -SIGHT_BOX, SIGHT_BOX, mask, ignore);
    let dir = (end - eye).normalize_or(forward);
    let mut target = eye + dir * (tr.fraction * distance).max(radii) + up * raise;
    // Keep the prop above a floor under the hold point.
    let half = radii * 0.5 + 1.0;
    let down = physics.trace(target + Vec3::Z * half, target - Vec3::Z * half, -SIGHT_BOX, SIGHT_BOX, mask, ignore);
    if !down.start_solid {
        holding.lift = if down.fraction >= 1.0 { 0.0 } else { (1.0 - down.fraction) * radii };
    }
    target.z += holding.lift;
    // Never closer to the player's vertical axis than the two radii.
    let (mins, maxs) = player.hull();
    let axis = (player.origin + Vec3::Z * mins.z, player.origin + Vec3::Z * maxs.z);
    let along = ((target - axis.0).dot(Vec3::Z) / (axis.1 - axis.0).length()).clamp(0.0, 1.0);
    let nearest = axis.0.lerp(axis.1, along);
    let away = target - nearest;
    if away.length() < radii {
        target = nearest + away.normalize_or(forward.with_z(0.0).normalize_or(Vec3::X)) * radii;
    }
    target
}

/// Where a view model clone is held (`docs/re/gameplay.md`, view model
/// holding): out along the view, moved along the flat forward as the view
/// pitches and lowered by the object's offset as it levels, then pulled in
/// to what is in the way, but no nearer than the minimum distance.
fn view_model_hold_point(
    physics: &Physics,
    ignore: &[Entity],
    console: &portal2_core::console::Console,
    holding: &Holding,
    player: &crate::player::movement::MoveState,
    view: Vec3,
) -> Vec3 {
    let eye = player.origin + Vec3::Z * player.eye_offset();
    let q = portal2_core::coords::angles_to_quat(view);
    let (forward, right, up) = (q * Vec3::X, -(q * Vec3::Y), q * Vec3::Z);
    let t = (view.x.abs() / 75.0).clamp(0.0, 1.0);
    let lowered = if holding.cube {
        console.f32("player_held_object_offset_up_cube_vm")
    } else if holding.core {
        console.f32("player_held_object_offset_up_sphere")
    } else {
        0.0
    };
    let offset = forward * console.f32("player_held_object_distance_vm")
        + Vec3::Z.cross(right) * (console.f32("player_held_object_look_down_adjustment") * t)
        + up * (lowered * (1.0 - t));
    let tr = physics.trace(eye, eye + offset, Vec3::ZERO, Vec3::ZERO, mask::SOLID_ALL, ignore);
    let length = (offset.length() * tr.fraction).max(console.f32("player_held_object_min_distance"));
    eye + offset.normalize_or(forward) * length
}

/// Steers the held prop toward its hold point, dropping it when it trails
/// too far behind for too long.
fn hold_held(
    time: Res<Time<Fixed>>,
    mut holding: ResMut<Holding>,
    physics: Physics,
    console: Res<portal2_core::console::Console>,
    players: Query<(Entity, &Player)>,
    input: Res<PlayerInput>,
    mut props: Query<(&Position, &Rotation, &mut LinearVelocity, &mut AngularVelocity, &Collider, &TraceCollider), With<PhysicsProp>>,
    solids: Query<(&Collider, &Position, &Rotation), Without<PhysicsProp>>,
    statics: Query<(), With<super::StaticWorld>>,
    brushes: BrushShapes,
    holes: Res<PortalHoles>,
    portals: Res<crate::portals::Portals>,
    portal_query: Query<&crate::portals::Portal>,
    spatial: SpatialQuery,
    mut commands: Commands,
    (bodies, mut out, mut sounds, followers): (
        Query<(&RigidBody, Has<ChildOf>)>,
        MessageWriter<FireOutput>,
        MessageWriter<portal2_audio::SoundRequest>,
        Query<&portal2_world::collision::BoneFollower>,
    ),
) {
    let Some(held) = holding.entity else { return };
    let Ok((pe, p)) = players.single() else { return };
    // Frozen or given a parent (Wheatley plugged into his socket), it leaves
    // the player's hands.
    if bodies.get(held).is_ok_and(|(body, parented)| !body.is_dynamic() || parented) {
        holding.entity = None;
        release(&mut commands, held);
        sounds.write(portal2_audio::SoundRequest::stop("Weapon_Portalgun.HoldSound", pe));
        out.write(FireOutput::new(held, "OnPhysGunDrop").with_activator(Some(pe)));
        out.write(FireOutput::new(held, "OnPlayerDrop").with_activator(Some(pe)));
        return;
    }
    let Ok((pos, rot, mut lin, mut ang, collider, shape)) = props.get_mut(held) else {
        holding.entity = None;
        return;
    };
    // Held beyond a portal, the hold point (and the prop's facing) are
    // carried through it; the prop is let go when that portal closes.
    let through = match (holding.portal, crate::portals::linked_frames(&portals, &portal_query)) {
        (Some(k), Some(frames)) => Some(transfer(&frames[k], &frames[1 - k])),
        (Some(_), None) => {
            holding.entity = None;
            release(&mut commands, held);
            return;
        }
        (None, _) => None,
    };
    let turn = through.map_or(Quat::IDENTITY, |m| Quat::from_mat3a(&m.matrix3));
    if holding.view_model {
        let at = view_model_hold_point(&physics, &[pe, held], &console, &holding, &p.state, input.input.angles);
        let facing = angles_to_quat(input.input.angles.with_z(0.0)) * Quat::from_rotation_z(holding.yaw_offset.to_radians());
        commands
            .entity(held)
            .insert((Position(through.map_or(at, |m| m.transform_point3(at))), Rotation::from((turn * facing).normalize())));
        lin.0 = Vec3::ZERO;
        ang.0 = Vec3::ZERO;
        holding.error = 0.0;
        return;
    }
    // The contact solver only pushes back softly, so a prop turned or
    // squeezed into a wall can work its way in and through a thin pane.
    // Sunk in too deep, it goes back to where it last was clear (passing
    // through a portal opening, where the wall behind does not count, or
    // through a portal itself, is fine).
    let (mins, maxs) = super::world_bounds(&shape.shape, pos.0, rot.0);
    let (centre, half) = ((mins + maxs) * 0.5, (maxs - mins) * 0.5);
    let in_portal = holes.holes.iter().any(|h| h.overlaps_box(centre, -half, half));
    let teleported = holding.clear.is_some_and(|(at, _)| at.distance(pos.0) > HOLD_TELEPORT_DISTANCE);
    let world = SpatialQueryFilter::from_mask(LayerMask(layers::WORLD | layers::MOVER)).with_excluded_entities([held, pe]);
    if in_portal || teleported || sunk_depth(&spatial, &solids, collider, pos.0, rot.0, &world) <= HOLD_MAX_PENETRATION {
        holding.clear = Some((pos.0, rot.0));
    } else if let Some((at, turn)) = holding.clear {
        // The spatial query reads every body's placement, so the move back
        // goes through commands (applied before the next physics step).
        commands.entity(held).insert((Position(at), Rotation::from(turn)));
        lin.0 = Vec3::ZERO;
        ang.0 = Vec3::ZERO;
        return;
    }
    let dt = time.delta_secs().max(1e-4);
    let eye = p.state.origin + Vec3::Z * p.state.eye_offset();
    // Models with collision on their bones do not hold a carried prop back
    // (see `layers::FOLLOWER`), so neither do they move its hold point.
    let mut ignore = vec![pe, held];
    for owner in followers.iter().map(|f| f.owner) {
        if !ignore.contains(&owner) {
            ignore.push(owner);
        }
    }
    let seen_at = hold_point(&physics, &ignore, &console, &mut holding, &p.state, input.input.angles);
    let target = through.map_or(seen_at, |m| m.transform_point3(seen_at));
    let error = target - pos.0;

    // The drop error averages the trailing distance over about a second;
    // it triples each tick the prop is out of sight behind something.
    let t = dt.min(1.0);
    let mut d = error.length();
    if d / t > HOLD_MAX_SPEED {
        d *= 0.5;
    }
    holding.error = (1.0 - t) * holding.error + t * d;
    let apparent = through.map_or(pos.0, |m| m.inverse().transform_point3(pos.0));
    if physics.trace(eye, apparent, -SIGHT_BOX, SIGHT_BOX, mask::SOLID_BRUSH_ONLY, &ignore).fraction < 1.0 {
        holding.error *= 3.0;
    }
    trace!("hold: target {target:.0?} at {:.0?} lift {:.1} error {:.1}", pos.0, holding.lift, holding.error);
    let limit = if console.bool("player_held_object_collide_with_player") { 12.0 } else { 40.0 };
    if holding.error > limit || p.state.ground_entity == Some(held) {
        holding.entity = None;
        release(&mut commands, held);
        return;
    }
    // The game's controller pushes a physics shadow that the world stops;
    // here the move is swept first so the prop comes to rest against a
    // wall or pane (and slides along it) instead of being driven into it.
    let filter = SpatialQueryFilter::from_mask(LayerMask(layers::WORLD | layers::PROP | layers::MOVER)).with_excluded_entities([held, pe]);
    let blockers = Blockers { spatial: &spatial, holes: &holes, statics: &statics, brushes: &brushes };
    let step = swept_step(&blockers, collider, pos.0, rot.0, error, &filter);
    let wanted = step / dt * 0.5;
    lin.0 = wanted.clamp_length_max(HOLD_MAX_SPEED);
    let target_rot = turn * Quat::from_rotation_z((input.input.angles.y + holding.yaw_offset).to_radians());
    let (axis, angle) = (target_rot * rot.0.inverse()).normalize().to_axis_angle();
    let angle = if angle > std::f32::consts::PI { angle - std::f32::consts::TAU } else { angle };
    ang.0 = axis * (angle / dt * 0.3);
}

/// How far the prop at `pos` / `rot` reaches into the solids it overlaps.
fn sunk_depth(
    spatial: &SpatialQuery,
    solids: &Query<(&Collider, &Position, &Rotation), Without<PhysicsProp>>,
    collider: &Collider,
    pos: Vec3,
    rot: Quat,
    filter: &SpatialQueryFilter,
) -> f32 {
    spatial
        .shape_intersections(collider, pos, rot, filter)
        .into_iter()
        .filter_map(|e| {
            let (other, at, turn) = solids.get(e).ok()?;
            avian3d::collision::collider::contact_query::contact(collider, pos, rot, other, at.0, *turn, 0.0).ok().flatten()
        })
        .map(|c| c.penetration)
        .fold(0.0, f32::max)
}

/// What stops a prop moved by the player: everything solid except the wall
/// (or brush) behind a linked portal's opening, which props pass into (the
/// contact hooks let them, and the frame slabs keep them inside the tunnel).
struct Blockers<'a, 'w, 's> {
    spatial: &'a SpatialQuery<'w, 's>,
    holes: &'a PortalHoles,
    statics: &'a Query<'w, 's, (), With<super::StaticWorld>>,
    brushes: &'a BrushShapes<'w, 's>,
}

/// Brushes and other solids that are not props, with their placement.
type BrushShapes<'w, 's> = Query<'w, 's, (&'static TraceCollider, &'static Position, &'static Rotation), Without<PhysicsProp>>;

impl Blockers<'_, '_, '_> {
    fn cast(
        &self,
        collider: &Collider,
        from: Vec3,
        rot: Quat,
        dir: Dir3,
        config: &ShapeCastConfig,
        filter: &SpatialQueryFilter,
    ) -> Option<ShapeHitData> {
        let hit = self.spatial.cast_shape(collider, from, rot, dir, config, filter)?;
        let static_map = self.statics.contains(hit.entity);
        let bounds = self.brushes.get(hit.entity).ok().map(|(c, p, r)| super::world_bounds(&c.shape, p.0, r.0));
        let into_opening = self.holes.holes.iter().any(|h| {
            let l = h.frame.to_local(hit.point1);
            let cut = static_map || bounds.is_some_and(|(lo, hi)| super::carves(h, lo, hi));
            cut && l.x < 2.0 && l.y.abs() < HALF_WIDTH + 2.0 && l.z.abs() < HALF_HEIGHT + 2.0
        });
        if !into_opening {
            return Some(hit);
        }
        let mut past = filter.clone();
        past.excluded_entities.insert(hit.entity);
        self.spatial.cast_shape(collider, from, rot, dir, config, &past)
    }
}

/// How much of `step` the prop at `pos` can move: up to what is in the way
/// (less a skin), then on along that surface.
fn swept_step(blockers: &Blockers, collider: &Collider, pos: Vec3, rot: Quat, step: Vec3, filter: &SpatialQueryFilter) -> Vec3 {
    let mut out = Vec3::ZERO;
    let mut from = pos;
    let mut rest = step;
    for _ in 0..2 {
        let len = rest.length();
        if len < 1e-3 {
            return out + rest;
        }
        let Ok(dir) = Dir3::new(rest / len) else { return out };
        let config = ShapeCastConfig { max_distance: len, ignore_origin_penetration: true, ..default() };
        let Some(hit) = blockers.cast(collider, from, rot, dir, &config, filter) else { return out + rest };
        let travel = (hit.distance - HOLD_SKIN).max(0.0);
        out += dir * travel;
        from += dir * travel;
        // The surface normal points out of what was hit: keep only the part
        // of the remaining move that does not push into it.
        let n = hit.normal1.normalize_or_zero();
        let remaining = rest - dir * travel;
        rest = remaining - n * remaining.dot(n).min(0.0);
    }
    out
}

/// Walking into a prop gives it a shove along the walking direction, as far
/// as it has room to go: one already against a wall is not driven into it.
fn push_props(
    time: Res<Time<Fixed>>,
    physics: Physics,
    holding: Res<Holding>,
    players: Query<(Entity, &Player)>,
    mut props: Query<(&RigidBody, &mut LinearVelocity, &Position, &Rotation, &Collider), With<PhysicsProp>>,
    statics: Query<(), With<super::StaticWorld>>,
    brushes: BrushShapes,
    holes: Res<PortalHoles>,
    spatial: SpatialQuery,
) {
    let Ok((pe, p)) = players.single() else { return };
    let v = p.state.velocity.with_z(0.0);
    let speed = v.length();
    if speed < 10.0 || p.noclip {
        return;
    }
    let dir = v / speed;
    let (mins, maxs) = p.state.hull();
    let start = p.state.origin + Vec3::Z * 2.0;
    let tr = physics.trace(start, start + dir * 4.0, mins, maxs, mask::PLAYER_SOLID, &[pe]);
    let Some(hit) = tr.entity else { return };
    if holding.entity == Some(hit) {
        return;
    }
    if let Ok((body, mut lin, pos, rot, collider)) = props.get_mut(hit)
        && *body == RigidBody::Dynamic
    {
        let along = lin.0.dot(dir);
        let mut want = speed * 0.6;
        let dt = time.delta_secs().max(1e-4);
        let filter =
            SpatialQueryFilter::from_mask(LayerMask(layers::WORLD | layers::MOVER | layers::FOLLOWER)).with_excluded_entities([hit, pe]);
        let config = ShapeCastConfig { max_distance: want * dt + HOLD_SKIN, ignore_origin_penetration: true, ..default() };
        if let Ok(d) = Dir3::new(dir)
            && let Some(block) = (Blockers { spatial: &spatial, holes: &holes, statics: &statics, brushes: &brushes })
                .cast(collider, pos.0, rot.0, d, &config, &filter)
        {
            want = want.min((block.distance - HOLD_SKIN).max(0.0) / dt);
        }
        if along < want {
            lin.0 += dir * (want - along);
        }
    }
}

impl portal2_core::save::Restore for Holding {}
