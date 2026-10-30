//! The portal gun: firing, availability, the first-person viewmodel, and
//! portals placed by maps (`prop_portal`).

use portal2_core::layers::VIEWMODEL_LAYER;
use std::collections::HashMap;
use std::f32::consts::PI;

use bevy::camera::visibility::RenderLayers;
use bevy::camera::{Camera3dDepthLoadOp, Exposure, Hdr};
use bevy::ecs::system::SystemState;
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;
use bevy::render::view::Tonemapping;
use source_formats::bsp::surf;
use source_formats::vmt::material_path;

use super::placement::{Helper, PlacementContext};
use super::{FizzlePortals, PlacePortal};
use crate::entities::SpawnHandlers;
use crate::entities::io::InputCall;
use crate::entities::io::InputHandlers;
use crate::player::{Player, PlayerInput};
use portal2_core::args::Args;
use portal2_core::coords::{angles_forward, angles_to_quat};
use portal2_core::entity::{EntityRegistry, MapEntity};
use portal2_core::events::{EventValue, GameEvent};
use portal2_core::sets::PlayerCameraSet;
use portal2_core::vfs::GameFs;
use portal2_core::view::MainCamera;
use portal2_render::materials::{MaterialCache, MaterialContext, load_vmt};
use portal2_render::models::lighting;
use portal2_render::models::{ModelCache, SpawnModel, StudioAnimator, spawn_model_parts};
use portal2_render::source_material::{ModelMaterial, RefractMaterial};
use portal2_render::textures::TextureCache;
use portal2_world::collision::Physics;
use portal2_world::collision::portal::PortalFrame;
use portal2_world::{CurrentMap, PlayerStart};

/// Seconds between shots (`portalgun_fire_delay`).
pub const FIRE_DELAY: f64 = 0.2;
/// Viewmodel field of view in degrees, horizontal at 4:3 (`cl_viewmodelfov`).
pub const VIEWMODEL_FOV: f32 = 50.0;

pub struct GunPlugin;

impl Plugin for GunPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PortalGun>()
            .init_resource::<PortalableCache>()
            .add_systems(Startup, spawn_viewmodel_camera)
            .add_systems(Update, (reset_gun_on_map, pick_up_gun, fire_gun, attach_viewmodel).chain())
            .add_systems(Update, show_viewmodel)
            .add_systems(PostUpdate, place_viewmodel.after(PlayerCameraSet).before(TransformSystems::Propagate));
        let world = app.world_mut();
        world.resource_mut::<SpawnHandlers>().register("prop_portal", spawn_map_portal);
        world.resource_mut::<InputHandlers>().register("prop_portal", map_portal_input);
        world.resource_mut::<InputHandlers>().register("player_weaponstrip", weaponstrip_input);
    }
}

#[derive(Resource, Default, serde::Serialize, serde::Deserialize)]
pub struct PortalGun {
    pub can_fire: [bool; 2],
    /// When the gun may fire again, on this run's clock (not saved: a game
    /// loaded in another run would wait for its old clock to come round).
    #[serde(skip)]
    pub next_fire: f64,
    #[serde(skip)]
    pub viewmodel: Option<Entity>,
    /// Map entity to walk into to receive the gun.
    #[serde(with = "portal2_core::save::ids::opt_entity")]
    pub pickup: Option<Entity>,
}

/// `player_weaponstrip`: the player loses the portal gun.
fn weaponstrip_input(world: &mut World, call: &crate::entities::io::InputCall) -> bool {
    if !matches!(call.input.as_str(), "strip" | "stripweaponsandsuit") {
        return false;
    }
    world.resource_mut::<PortalGun>().can_fire = [false, false];
    true
}

#[derive(Component)]
struct ViewmodelCamera;

#[derive(Component)]
pub struct Viewmodel {
    light_timer: f32,
}

/// Portal acceptance per surface (texinfo), rebuilt for each map.
#[derive(Resource, Default)]
pub struct PortalableCache {
    map: String,
    by_surface: HashMap<i32, bool>,
}

fn vertical_fov(horizontal_4x3_deg: f32) -> f32 {
    2.0 * ((horizontal_4x3_deg.to_radians() * 0.5).tan() * 0.75).atan()
}

fn spawn_viewmodel_camera(mut commands: Commands) {
    commands.spawn((
        Name::new("viewmodel camera"),
        ViewmodelCamera,
        Camera3d { depth_load_op: Camera3dDepthLoadOp::Clear(0.0), ..default() },
        Camera { order: 1, clear_color: ClearColorConfig::None, ..default() },
        Hdr,
        Tonemapping::None,
        bevy::render::view::DebandDither::Disabled,
        Exposure::default(),
        Msaa::Sample4,
        Projection::Perspective(PerspectiveProjection { fov: vertical_fov(VIEWMODEL_FOV), near: 1.0, far: 512.0, ..default() }),
        RenderLayers::layer(VIEWMODEL_LAYER),
        Transform::default(),
    ));
}

/// Gun availability: `--portalgun none|blue|both`, a gun entity in the map
/// to pick up, or both portals by default.
/// Gun availability per map: `--portalgun none|blue|both` overrides; a new
/// game starts without it (the map's transition script grants the right gun
/// through `give_portalgun` / `upgrade_portalgun`); a level transition keeps
/// what the player had.
fn reset_gun_on_map(
    start: Option<Res<PlayerStart>>,
    args: Res<Args>,
    map: Option<Res<CurrentMap>>,
    registry: Res<EntityRegistry>,
    mut gun: ResMut<PortalGun>,
) {
    let Some(start) = start else { return };
    if !start.is_changed() {
        return;
    }
    gun.pickup = registry.by_class.get("weapon_portalgun").and_then(|l| l.first().copied());
    let load = map.map(|m| m.load_type).unwrap_or_default();
    gun.can_fire = match args.portalgun.as_deref() {
        Some("none") => [false, false],
        Some("blue") => [true, false],
        Some("both") => [true, true],
        _ if load == portal2_world::MapLoadType::NewGame => [false, false],
        _ => gun.can_fire,
    };
}

fn pick_up_gun(
    mut gun: ResMut<PortalGun>,
    players: Query<(Entity, &Player)>,
    entities: Query<(&MapEntity, &GlobalTransform)>,
    mut commands: Commands,
    mut events: MessageWriter<GameEvent>,
) {
    let Some(pickup) = gun.pickup else { return };
    let Ok((pe, p)) = players.single() else { return };
    let Ok((me, gt)) = entities.get(pickup) else { return };
    let center = p.state.origin + Vec3::Z * 36.0;
    if center.distance(gt.translation()) > 48.0 {
        return;
    }
    let one = me.get_i32("canfireportal1").unwrap_or(1) != 0;
    let two = me.get_i32("canfireportal2").unwrap_or(1) != 0;
    for slot in newly_enabled(&gun, [one, two]) {
        events.write(portal_enabled(Some(pe), slot));
    }
    gun.can_fire = [gun.can_fire[0] || one, gun.can_fire[1] || two];
    gun.pickup = None;
    commands.entity(pickup).insert(Visibility::Hidden);
    info!("picked up the portal gun ({one}, {two})");
}

fn newly_enabled(gun: &PortalGun, slots: [bool; 2]) -> Vec<usize> {
    (0..2).filter(|&s| slots[s] && !gun.can_fire[s]).collect()
}

/// The `portal_enabled` event the instructor's portal lessons open on.
fn portal_enabled(player: Option<Entity>, slot: usize) -> GameEvent {
    GameEvent::new("portal_enabled").with("userid", EventValue::Entity(player)).with("leftportal", EventValue::Int((slot == 0) as i32))
}

/// Lets the gun place more portals (`give_portalgun`, the upgrade).
pub fn enable_portals(world: &mut World, slots: [bool; 2]) {
    let player = world.resource::<EntityRegistry>().player;
    let newly = newly_enabled(world.resource::<PortalGun>(), slots);
    let mut gun = world.resource_mut::<PortalGun>();
    gun.can_fire = [gun.can_fire[0] || slots[0], gun.can_fire[1] || slots[1]];
    for slot in newly {
        world.write_message(portal_enabled(player, slot));
    }
}

/// Whether portals may be placed on a surface (texinfo index from a trace):
/// not tool or sky faces, not flagged no-portal, and not a `%noportal` material.
/// Portal gel is looked for this close around each placement sample.
const PORTAL_PAINT_RADIUS: f32 = 2.0;

fn surface_portalable(map: &CurrentMap, fs: &GameFs, surface: i32) -> bool {
    if surface < 0 {
        return false;
    }
    let Some(ti) = map.bsp.texinfo.get(surface as usize) else { return false };
    if ti.flags & (surf::NOPORTAL | surf::SKY | surf::NODRAW | surf::TRIGGER) != 0 {
        return false;
    }
    map.bsp
        .texinfo_material(surface as usize)
        .and_then(|name| load_vmt(fs, &material_path(name)))
        .is_none_or(|vmt| !vmt.get_bool("%noportal"))
}

fn fire_gun(
    time: Res<Time>,
    buttons: Res<portal2_core::console::binds::Buttons>,
    mut gun: ResMut<PortalGun>,
    players: Query<(Entity, &Player)>,
    input: Res<PlayerInput>,
    physics: Physics,
    map: Option<Res<CurrentMap>>,
    fs: Res<GameFs>,
    mut cache: ResMut<PortalableCache>,
    entities: Query<(Entity, &MapEntity, &GlobalTransform)>,
    portals: Res<super::Portals>,
    portal_query: Query<&super::Portal>,
    (mut place, mut sounds, mut fx): (
        MessageWriter<PlacePortal>,
        MessageWriter<portal2_audio::SoundRequest>,
        MessageWriter<super::PortalFx>,
    ),
    mut animators: Query<&mut StudioAnimator>,
    mut auto: ResMut<crate::player::autopilot::AutoFire>,
    (paint, console, mut events): (
        Option<Res<portal2_world::paint_map::PaintMap>>,
        Res<portal2_core::console::Console>,
        MessageWriter<GameEvent>,
    ),
) {
    let slot = if let Some(slot) = auto.0.take() {
        slot
    } else if buttons.just_pressed("attack") {
        0
    } else if buttons.just_pressed("attack2") {
        1
    } else {
        return;
    };
    let now = time.elapsed_secs_f64();
    if !gun.can_fire[slot] || now < gun.next_fire {
        return;
    }
    let (Ok((pe, p)), Some(map)) = (players.single(), map) else { return };
    gun.next_fire = now + FIRE_DELAY;
    events.write(
        GameEvent::new("portal_fired").with("userid", EventValue::Entity(Some(pe))).with("leftportal", EventValue::Int((slot == 0) as i32)),
    );
    sounds.write(portal2_audio::SoundRequest::on(if slot == 0 { "Weapon_Portalgun.fire_blue" } else { "Weapon_Portalgun.fire_red" }, pe));
    if let Some(vm) = gun.viewmodel
        && let Ok(mut a) = animators.get_mut(vm)
    {
        a.set_sequence("fire1");
    }

    let mut noportal = Vec::new();
    let mut bumpers = Vec::new();
    let mut cleansers = Vec::new();
    let mut helpers = Vec::new();
    let mut ignore = vec![pe];
    for (e, me, gt) in &entities {
        if super::placement::shot_ignores(&me.classname) {
            ignore.push(e);
        }
        match me.classname.as_str() {
            "func_noportal_volume" if me.get_i32("startactive").unwrap_or(1) != 0 => noportal.push(e),
            "func_portal_bumper" => bumpers.push(e),
            "trigger_portal_cleanser" if !me.get_bool("startdisabled") => cleansers.push(e),
            "info_placement_helper" => helpers.push(Helper {
                origin: gt.translation(),
                angles: me.get_vec3("angles"),
                radius: me.get_f32("radius").unwrap_or(16.0).max(1.0),
            }),
            _ => {}
        }
    }

    if cache.map != map.name {
        cache.map = map.name.clone();
        cache.by_surface.clear();
    }
    let known = std::cell::RefCell::new(std::mem::take(&mut cache.by_surface));
    let on_paint = console.bool("sv_portal_placement_on_paint");
    let min_paint = console.f32("paint_min_valid_alpha_value");
    let portalable = |surface: i32, point: Vec3, normal: Vec3| -> bool {
        // Portal gel takes portals whatever the surface.
        if on_paint
            && let Some(paint) = paint.as_deref()
            && paint.dominant(0, point + normal * 0.1, PORTAL_PAINT_RADIUS, min_paint) == Some(portal2_world::paint_map::PaintType::Portal)
        {
            return true;
        }
        if let Some(&v) = known.borrow().get(&surface) {
            return v;
        }
        let ok = surface_portalable(&map, &fs, surface);
        known.borrow_mut().insert(surface, ok);
        ok
    };
    let other = portals.entities[1 - slot].and_then(|e| portal_query.get(e).ok()).map(|p| p.frame);
    let ctx = PlacementContext {
        physics: &physics,
        ignore: &ignore,
        portalable: &portalable,
        noportal_volumes: &noportal,
        bumpers: &bumpers,
        cleansers: &cleansers,
        helpers: &helpers,
        other,
    };
    let eye = p.state.origin + Vec3::Z * p.state.eye_offset();
    let dir = angles_forward(input.input.angles);
    let result = ctx.place(eye, dir, dir);
    cache.by_surface = known.into_inner();
    // The shot leaves from about where the viewmodel's muzzle is.
    let (right, up) = {
        let r = dir.cross(Vec3::Z).normalize_or(Vec3::Y);
        (r, r.cross(dir))
    };
    let muzzle = eye + dir * 20.0 + right * 7.0 - up * 6.0;
    match result {
        Ok(frame) => {
            info!("portal {slot} placed at {:.0?} facing {:.2?}", frame.center, frame.forward);
            place.write(PlacePortal { slot, frame, settle: false });
            fx.write(super::PortalFx::Shot { slot, start: muzzle, end: frame.center, aim: dir });
        }
        Err(e) => {
            info!("portal {slot} shot failed: {e:?}");
            let tr = physics.trace(eye, eye + dir * 56756.0, Vec3::ZERO, Vec3::ZERO, portal2_world::collision::mask::SOLID_ALL, &ignore);
            sounds.write(portal2_audio::SoundRequest::at("Portal.fizzle_invalid_surface", tr.end));
            fx.write(super::PortalFx::Shot { slot, start: muzzle, end: tr.end, aim: dir });
            if tr.fraction < 1.0 {
                fx.write(super::PortalFx::BadSurface { slot, at: tr.end, normal: tr.normal });
            }
        }
    }
}

type ViewmodelParams<'w, 's> = (
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

/// Spawns the first-person gun (model named by the weapon script) when the
/// player can fire, and removes it otherwise.
fn attach_viewmodel(world: &mut World) {
    let has_gun = {
        let gun = world.resource::<PortalGun>();
        gun.can_fire.iter().any(|c| *c)
    };
    let current = world.resource::<PortalGun>().viewmodel;
    let alive = current.is_some_and(|e| world.get_entity(e).is_ok());
    if has_gun == alive {
        return;
    }
    if !has_gun {
        if let Some(e) = current {
            world.entity_mut(e).despawn();
        }
        world.resource_mut::<PortalGun>().viewmodel = None;
        return;
    }
    if world.get_resource::<CurrentMap>().is_none() {
        return;
    }
    let path = {
        let fs = world.resource::<GameFs>();
        fs.read_string("scripts/weapon_portalgun.txt")
            .and_then(|t| source_formats::kv::parse(&t))
            .and_then(|kv| kv.get_str("viewmodel").map(str::to_owned))
    };
    let Some(path) = path else {
        warn!("weapon script has no viewmodel; drawing no gun");
        world.resource_mut::<PortalGun>().can_fire = [false, false];
        return;
    };
    let root = world.spawn((Name::new("viewmodel"), Viewmodel { light_timer: 0.0 }, Transform::default(), Visibility::Visible)).id();
    let mut state: SystemState<ViewmodelParams> = SystemState::new(world);
    let (mut commands, fs, map, mut cache, mut mats, mut textures, mut images, mut meshes, mut model_mats, mut refract_mats, mut bindposes) =
        state.get_mut(world).expect("render resources available");
    let Some(model) = cache.get(&path, &fs, &mut meshes, &mut bindposes) else {
        state.apply(world);
        world.entity_mut(root).despawn();
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
    animator.set_default_sequence("idle");
    animator.set_sequence("idle");
    commands.entity(root).insert(animator);
    state.apply(world);
    // Every part of the gun renders on the viewmodel layer only.
    let mut stack = vec![root];
    while let Some(e) = stack.pop() {
        // The gun's bind pose can lie away from its animated position, so its
        // bind-pose bounds are not reliable for culling.
        world.entity_mut(e).insert((RenderLayers::layer(VIEWMODEL_LAYER), bevy::camera::visibility::NoFrustumCulling));
        if let Some(children) = world.get::<Children>(e) {
            stack.extend(children.iter());
        }
    }
    world.resource_mut::<PortalGun>().viewmodel = Some(root);
    info!("viewmodel ready ({} bones)", model.bone_count());
}

/// `r_drawviewmodel 0` hides the first-person weapon, as does a scripted
/// view.
fn show_viewmodel(
    console: Res<portal2_core::console::Console>,
    view: Res<crate::entities::viewcontrol::ActiveView>,
    mut cams: Query<&mut Camera, With<ViewmodelCamera>>,
) {
    let show = console.bool("r_drawviewmodel") && view.0.is_none();
    for mut c in &mut cams {
        if c.is_active != show {
            c.is_active = show;
        }
    }
}

/// The weapon's walking sway: a time base advanced by the share of the
/// player's maximum speed they move at, read as a 0.45 s vertical and a
/// 0.9 s sideways cycle (`docs/re/gameplay.md`).
#[derive(Default)]
struct WeaponBob {
    time: f32,
    last: f32,
}

impl WeaponBob {
    /// Vertical and sideways bob for a player moving at `speed`.
    fn update(&mut self, now: f32, speed: f32, max_speed: f32) -> (f32, f32) {
        let speed = speed.clamp(-max_speed, max_speed);
        let share = if max_speed == 0.0 { if speed >= 0.0 { 1.0 } else { 0.0 } } else { speed / max_speed };
        self.time += (now - self.last) * share;
        self.last = now;
        let wave = |period: f32| {
            let c = (self.time - (self.time / period).trunc() * period) / period;
            let c = if c < 0.5 { PI * c / 0.5 } else { PI + PI * (c - 0.5) / 0.5 };
            let b = speed * 0.005;
            (b * 0.3 + b * 0.7 * c.sin()).clamp(-7.0, 4.0)
        };
        (wave(0.45), wave(0.9))
    }
}

fn place_viewmodel(
    time: Res<Time>,
    input: Res<PlayerInput>,
    map: Option<Res<CurrentMap>>,
    players: Query<&Player>,
    mut bob: Local<WeaponBob>,
    main: Query<(&Transform, &Exposure), (With<MainCamera>, Without<ViewmodelCamera>, Without<Viewmodel>)>,
    mut cam: Query<(&mut Transform, &mut Exposure), (With<ViewmodelCamera>, Without<Viewmodel>)>,
    mut vm: Query<(&mut Transform, &mut Viewmodel, &Children), Without<MainCamera>>,
    mesh_mats: Query<&MeshMaterial3d<ModelMaterial>>,
    mut materials: ResMut<Assets<ModelMaterial>>,
) {
    let Ok((main_t, exposure)) = main.single() else { return };
    if let Ok((mut t, mut e)) = cam.single_mut() {
        *t = *main_t;
        *e = *exposure;
    }
    let speed = players.single().map(|p| p.state.velocity.length()).unwrap_or(0.0);
    let (vertical, lateral) = bob.update(time.elapsed_secs(), speed, crate::player::movement::MAX_SPEED);
    let mut angles = input.input.angles;
    let q = angles_to_quat(angles);
    let (forward, right) = (q * Vec3::X, -(q * Vec3::Y));
    let origin = main_t.translation + forward * (vertical * 0.1) + Vec3::Z * (vertical * 0.1) + right * (lateral * 0.8);
    angles.z += vertical * 0.5;
    angles.x -= vertical * 0.4;
    angles.y -= lateral * 0.3;
    for (mut t, mut v, children) in &mut vm {
        t.translation = origin;
        t.rotation = angles_to_quat(angles);
        // Refresh lighting a few times a second as the player moves.
        v.light_timer -= time.delta_secs();
        if v.light_timer <= 0.0
            && let Some(map) = map.as_ref()
        {
            v.light_timer = 0.25;
            let light = lighting::compute(&map.bsp, main_t.translation);
            for c in children.iter() {
                if let Ok(h) = mesh_mats.get(c)
                    && let Some(mut m) = materials.get_mut(&h.0)
                {
                    light.apply(&mut m.params);
                }
            }
        }
    }
}

/// Portals placed by the map (`prop_portal`), usually pre-opened pairs.
fn spawn_map_portal(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e).cloned() else { return };
    // Only the player's linkage group shares the gun's pair.
    if me.get_i32("linkagegroupid").unwrap_or(0) != 0 {
        return;
    }
    if me.get_i32("activated").unwrap_or(0) != 0 {
        let slot = me.get_i32("portaltwo").unwrap_or(0).clamp(0, 1) as usize;
        let q = angles_to_quat(me.angles());
        let frame = PortalFrame { center: me.origin(), forward: q * Vec3::X, up: q * Vec3::Z };
        world.resource_mut::<Messages<PlacePortal>>().write(PlacePortal { slot, frame, settle: true });
    }
}

fn map_portal_input(world: &mut World, call: &InputCall) -> bool {
    let Some(me) = world.get::<MapEntity>(call.target).cloned() else { return false };
    let slot = me.get_i32("portaltwo").unwrap_or(0).clamp(0, 1) as usize;
    match call.input.as_str() {
        "setactivatedstate" => {
            if source_formats::kv::parse_i32_lenient(&call.param) != 0 {
                spawn_map_portal_now(world, &me, slot, None);
            } else {
                world.resource_mut::<Messages<FizzlePortals>>().write(FizzlePortals { slot: Some(slot) });
            }
        }
        "fizzle" => {
            world.resource_mut::<Messages<FizzlePortals>>().write(FizzlePortals { slot: Some(slot) });
        }
        "newlocation" => {
            let v = source_formats::kv::parse_floats(&call.param);
            if v.len() >= 6 {
                spawn_map_portal_now(world, &me, slot, Some((Vec3::new(v[0], v[1], v[2]), Vec3::new(v[3], v[4], v[5]))));
            }
        }
        _ => return false,
    }
    true
}

fn spawn_map_portal_now(world: &mut World, me: &MapEntity, slot: usize, at: Option<(Vec3, Vec3)>) {
    let (origin, angles) = at.unwrap_or((me.origin(), me.angles()));
    let q = angles_to_quat(angles);
    let frame = PortalFrame { center: origin, forward: q * Vec3::X, up: q * Vec3::Z };
    world.resource_mut::<Messages<PlacePortal>>().write(PlacePortal { slot, frame, settle: true });
}

impl portal2_core::save::Restore for PortalGun {
    fn keep(&mut self, live: Self) {
        self.viewmodel = live.viewmodel;
        self.next_fire = live.next_fire;
    }
}
