//! Portals: placement, linked views rendered through each opening, moving
//! the player through, and walking into openings cut in walls.

pub mod gun;
pub mod placement;
pub mod projection;

use bevy::asset::RenderAssetUsages;
use bevy::camera::{Exposure, Hdr, RenderTarget, visibility::RenderLayers};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, Extent3d, ShaderType, TextureDimension, TextureFormat, TextureUsages};
use bevy::render::view::Tonemapping;
use bevy::shader::ShaderRef;
use bevy::window::PrimaryWindow;
use portal2_core::layers::{PORTAL_END_LAYER, PORTAL_INNER_LAYER, PORTAL_LAYER};

use crate::player::{Player, PlayerInput};
use portal2_core::args::Args;
use portal2_core::coords::{angles_forward, quat_to_angles};
use portal2_core::sets::PlayerMoveSet;
use portal2_core::view::MainCamera;
use portal2_world::collision::CollisionWorld;
use portal2_world::collision::portal::{HALF_HEIGHT, HALF_WIDTH, PortalFrame, PortalHole, PortalHoles, transfer};
use portal2_world::{MapLoaded, MapObject};
use projection::PortalProjection;

pub const PORTAL_SHADER: &str = "embedded://portal2_render/shaders/portal.wesl";

pub struct PortalsPlugin;

impl Plugin for PortalsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<PortalMaterial>::default())
            .init_resource::<Portals>()
            .init_resource::<PortalHoles>()
            .init_resource::<PortalColors>()
            .init_resource::<PortalEnvironment>()
            .add_message::<PlacePortal>()
            .add_message::<FizzlePortals>()
            .add_message::<PortalFx>()
            .add_systems(Update, (apply_placements, fizzle_portals, clear_on_map_load, animate_portals, update_holes).chain())
            .add_systems(FixedUpdate, (teleport_player, portal_sounds).chain().after(PlayerMoveSet))
            .add_systems(PostUpdate, update_portal_cameras.after(portal2_core::sets::PlayerCameraSet).before(TransformSystems::Propagate))
            .add_plugins(gun::GunPlugin);
    }
}

/// Ring colours of the two portals (linear RGB); configurable.
#[derive(Resource, Clone, Copy)]
pub struct PortalColors(pub [Vec3; 2]);

impl Default for PortalColors {
    fn default() -> Self {
        Self([Vec3::new(0.10, 0.45, 1.0), Vec3::new(1.0, 0.45, 0.05)])
    }
}

#[derive(Clone, Copy, Debug, Default, ShaderType)]
pub struct PortalParams {
    pub color: Vec4,
    pub state: Vec4,
}

/// A portal's opening and rim (the game's `PortalRefract` stages, see
/// `docs/re/portals.md`): the linked view inside the hole, a fiery rim from
/// the noise texture through the portal's colour gradient, and the same
/// fire filling the hole while the portal shows static.
#[derive(Asset, TypePath, AsBindGroup, Clone, Debug)]
pub struct PortalMaterial {
    /// color: rgb fallback rim colour, a: `$PortalColorScale`;
    /// state: x linked, y time, z open amount, w clarity (1 - static).
    #[uniform(0)]
    pub params: PortalParams,
    #[texture(1)]
    #[sampler(2)]
    pub view: Option<Handle<Image>>,
    /// `models/portals/noise-blur-256x256`.
    #[texture(3)]
    #[sampler(4)]
    pub noise: Option<Handle<Image>>,
    /// `models/portals/portal-blue-color` / `portal-orange-color`.
    #[texture(5)]
    #[sampler(6)]
    pub gradient: Option<Handle<Image>>,
}

impl Material for PortalMaterial {
    fn fragment_shader() -> ShaderRef {
        PORTAL_SHADER.into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
    }

    fn enable_prepass() -> bool {
        false
    }

    fn enable_shadows() -> bool {
        false
    }
}

/// One placed portal.
#[derive(Component)]
pub struct Portal {
    pub slot: usize,
    pub frame: PortalFrame,
    pub surface: Entity,
    pub camera: Entity,
    pub material: Handle<PortalMaterial>,
    pub texture: Handle<Image>,
    pub opened_at: f64,
    /// The portal seen again inside its own view (the game draws two
    /// levels, `r_portal_stencil_depth 2`): a second camera one more trip
    /// through the pair, and the copies of the surface each view sees.
    pub inner_camera: Entity,
    pub inner_material: Handle<PortalMaterial>,
    /// Past the last level the portal is drawn full of static.
    pub end_material: Handle<PortalMaterial>,
}

/// The pair of portals (slot 0 and 1).
#[derive(Resource, Default)]
pub struct Portals {
    pub entities: [Option<Entity>; 2],
}

/// Request to (re)place a portal.
#[derive(Message, Clone, Copy, Debug)]
pub struct PlacePortal {
    pub slot: usize,
    pub frame: PortalFrame,
    /// Placed by the map: settle onto the surface behind it first.
    pub settle: bool,
}

/// Request to remove portals (both when `slot` is None).
#[derive(Message, Clone, Copy, Debug)]
pub struct FizzlePortals {
    pub slot: Option<usize>,
}

/// Particle effects of portal shots and portals (drawn by `crate::particles`).
#[derive(Message, Clone, Copy, Debug)]
pub enum PortalFx {
    /// A shot left the gun at `start` aimed along `aim` and lands at `end`.
    Shot { slot: usize, start: Vec3, end: Vec3, aim: Vec3 },
    /// A portal opened.
    Opened { slot: usize, frame: PortalFrame },
    /// A shot hit a surface that takes no portals.
    BadSurface { slot: usize, at: Vec3, normal: Vec3 },
    /// A portal closed (fizzled or moved away).
    Closed { slot: usize, frame: PortalFrame },
}

fn portal_quad() -> Mesh {
    // In portal local space: X out of the surface, Y left, Z up.
    let (w, h) = (HALF_WIDTH, HALF_HEIGHT);
    let x = 0.4;
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, vec![[x, w, -h], [x, -w, -h], [x, -w, h], [x, w, h]])
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[1.0, 0.0, 0.0]; 4])
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]])
        // Counter-clockwise seen from the front (+X), where +Y is screen right.
        .with_inserted_indices(Indices::U32(vec![0, 2, 1, 0, 3, 2]))
}

fn view_texture(width: u32, height: u32) -> Image {
    let mut image = Image::new_fill(
        Extent3d { width: width.max(1), height: height.max(1), depth_or_array_layers: 1 },
        TextureDimension::D2,
        &[0u8; 8],
        TextureFormat::Rgba16Float,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.texture_descriptor.usage = TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST | TextureUsages::RENDER_ATTACHMENT;
    image
}

/// A map-placed portal moves onto the surface behind it, 1/32 unit out, as
/// the game's placement check does (`sp_a1_intro1`'s orange portal is
/// placed 2 units in front of its wall and ends up on it).
fn settle_on_surface(physics: &portal2_world::collision::Physics, frame: &mut PortalFrame) {
    let start = frame.center + frame.forward;
    let tr = physics.trace(start, start - frame.forward * 17.0, Vec3::ZERO, Vec3::ZERO, placement::SHOT_MASK, &[]);
    if tr.fraction < 1.0 && !tr.start_solid && tr.normal.dot(frame.forward) > 0.99 {
        frame.center = tr.end + frame.forward / 32.0;
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_placements(
    mut commands: Commands,
    mut requests: MessageReader<PlacePortal>,
    mut portals: ResMut<Portals>,
    mut existing: Query<&mut Portal>,
    mut transforms: Query<&mut Transform>,
    colors: Res<PortalColors>,
    time: Res<Time>,
    windows: Query<&Window, With<PrimaryWindow>>,
    main: Query<(&Projection, &Exposure), With<MainCamera>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<PortalMaterial>>,
    mut sounds: MessageWriter<portal2_audio::SoundRequest>,
    mut fx: MessageWriter<PortalFx>,
    (fs, mut textures, physics): (
        Res<portal2_core::vfs::GameFs>,
        ResMut<portal2_render::textures::TextureCache>,
        portal2_world::collision::Physics,
    ),
) {
    for req in requests.read() {
        let mut req = *req;
        if req.settle {
            settle_on_surface(&physics, &mut req.frame);
        }
        let rotation = req.frame.rotation();
        fx.write(PortalFx::Opened { slot: req.slot, frame: req.frame });
        let open = if req.slot == 0 { "Portal.open_blue" } else { "Portal.open_red" };
        sounds.write(portal2_audio::SoundRequest::at(open, req.frame.center));
        if let Some(e) = portals.entities[req.slot]
            && let Ok(mut p) = existing.get_mut(e)
        {
            sounds.write(portal2_audio::SoundRequest::at("Portal.fizzle_moved", p.frame.center));
            fx.write(PortalFx::Closed { slot: req.slot, frame: p.frame });
            p.frame = req.frame;
            p.opened_at = time.elapsed_secs_f64();
            if let Ok(mut t) = transforms.get_mut(e) {
                *t = Transform::from_translation(req.frame.center).with_rotation(rotation);
            }
            continue;
        }
        let (width, height) = windows.single().map(|w| (w.physical_width(), w.physical_height())).unwrap_or((1600, 900));
        let texture = images.add(view_texture(width, height));
        let inner_texture = images.add(view_texture(width, height));
        let color = colors.0[req.slot];
        let gradient = if req.slot == 0 { "portal-blue-color" } else { "portal-orange-color" };
        let gradient = format!("materials/models/portals/{gradient}.vtf");
        let base_material = PortalMaterial {
            params: PortalParams { color: color.extend(4.0), state: Vec4::ZERO },
            view: Some(texture.clone()),
            noise: textures.load(&fs, &mut images, "materials/models/portals/noise-blur-256x256.vtf", false).map(|t| t.handle),
            gradient: textures.load(&fs, &mut images, &gradient, true).map(|t| t.handle),
        };
        let inner_material = materials.add(PortalMaterial { view: Some(inner_texture.clone()), ..base_material.clone() });
        let end_material = materials.add(PortalMaterial { view: None, ..base_material.clone() });
        let material = materials.add(base_material);
        let (base, exposure) = main
            .single()
            .ok()
            .and_then(|(p, e)| match p {
                Projection::Perspective(pp) => Some((pp.clone(), *e)),
                _ => None,
            })
            .unwrap_or_default();
        // The inner view renders first: the outer one shows it.
        let mut view_camera = |name: String, order: isize, target: &Handle<Image>, portal_layer: usize| {
            commands
                .spawn((
                    Name::new(name),
                    Camera3d::default(),
                    Camera { order, is_active: false, clear_color: ClearColorConfig::Custom(Color::BLACK), ..default() },
                    RenderTarget::Image(target.clone().into()),
                    Hdr,
                    Tonemapping::None,
                    bevy::render::view::DebandDither::Disabled,
                    exposure,
                    Msaa::Off,
                    bevy::pbr::ScreenSpaceTransmission::default(),
                    Projection::custom(PortalProjection::new(base.clone())),
                    RenderLayers::from_layers(&[
                        0,
                        portal2_core::layers::SKY2D_LAYER,
                        portal_layer,
                        portal2_core::layers::PLAYER_BODY_LAYER,
                    ]),
                    Transform::default(),
                ))
                .id()
        };
        let camera = view_camera(format!("portal {} view", req.slot), -10 + req.slot as isize, &texture, PORTAL_INNER_LAYER);
        let inner_camera =
            view_camera(format!("portal {} inner view", req.slot), -20 + req.slot as isize, &inner_texture, PORTAL_END_LAYER);
        let quad = meshes.add(portal_quad());
        let mut surface_on = |layer: usize, material: &Handle<PortalMaterial>| {
            commands.spawn((Mesh3d(quad.clone()), MeshMaterial3d(material.clone()), RenderLayers::layer(layer), Transform::IDENTITY)).id()
        };
        let surface = surface_on(PORTAL_LAYER, &material);
        let inner_surface = surface_on(PORTAL_INNER_LAYER, &inner_material);
        let end_surface = surface_on(PORTAL_END_LAYER, &end_material);
        let e = commands
            .spawn((
                Name::new(format!("portal {}", req.slot)),
                MapObject,
                Portal {
                    slot: req.slot,
                    frame: req.frame,
                    surface,
                    camera,
                    material,
                    texture,
                    opened_at: time.elapsed_secs_f64(),
                    inner_camera,
                    inner_material,
                    end_material,
                },
                Transform::from_translation(req.frame.center).with_rotation(rotation),
                Visibility::Visible,
            ))
            .add_children(&[surface, inner_surface, end_surface])
            .id();
        portals.entities[req.slot] = Some(e);
        sounds.write(portal2_audio::SoundRequest::on("Portal.ambient_loop", e));
    }
}

fn fizzle_portals(
    mut commands: Commands,
    mut requests: MessageReader<FizzlePortals>,
    mut portals: ResMut<Portals>,
    query: Query<&Portal>,
    mut sounds: MessageWriter<portal2_audio::SoundRequest>,
    mut fx: MessageWriter<PortalFx>,
) {
    for req in requests.read() {
        for slot in 0..2 {
            if req.slot.is_some_and(|s| s != slot) {
                continue;
            }
            if let Some(e) = portals.entities[slot].take() {
                if let Ok(p) = query.get(e) {
                    commands.entity(p.camera).despawn();
                    commands.entity(p.inner_camera).despawn();
                    let close = if slot == 0 { "Portal.close_blue" } else { "Portal.close_red" };
                    sounds.write(portal2_audio::SoundRequest::at(close, p.frame.center));
                    fx.write(PortalFx::Closed { slot, frame: p.frame });
                }
                commands.entity(e).despawn();
            }
        }
    }
}

/// Portal entities are map objects and vanish with the map; forget them.
pub(crate) fn clear_on_map_load(
    mut loaded: MessageReader<MapLoaded>,
    mut portals: ResMut<Portals>,
    mut commands: Commands,
    query: Query<&Portal>,
) {
    if loaded.read().last().is_none() {
        return;
    }
    for e in portals.entities.iter_mut().filter_map(|e| e.take()) {
        if let Ok(p) = query.get(e) {
            commands.entity(p.camera).despawn();
            commands.entity(p.inner_camera).despawn();
        }
    }
}

fn animate_portals(
    time: Res<Time>,
    portals: Res<Portals>,
    query: Query<&Portal>,
    mut materials: ResMut<Assets<PortalMaterial>>,
    mut cameras: Query<&mut Camera>,
) {
    let now = time.elapsed_secs_f64();
    let open_portals: Vec<&Portal> = portals.entities.iter().flatten().filter_map(|e| query.get(*e).ok()).collect();
    let linked = open_portals.len() == 2;
    // Static comes back whenever either portal of the pair (re)opens and
    // fades at 1 per second; an unlinked portal is all static.
    let last_opened = open_portals.iter().map(|p| p.opened_at).fold(f64::MIN, f64::max);
    for p in &open_portals {
        // The opening grows at 2 per second.
        let open = ((now - p.opened_at) * 2.0).clamp(0.0, 1.0) as f32;
        let static_amount = if linked { (1.0 - (now - last_opened)).clamp(0.0, 1.0) as f32 } else { 1.0 };
        let state = Vec4::new(if linked { 1.0 } else { 0.0 }, now as f32, open, 1.0 - static_amount);
        for (material, state) in [(&p.material, state), (&p.inner_material, state), (&p.end_material, Vec4::new(0.0, state.y, open, 0.0))] {
            if let Some(mut m) = materials.get_mut(material) {
                m.params.state = state;
            }
        }
        for camera in [p.camera, p.inner_camera] {
            if let Ok(mut c) = cameras.get_mut(camera) {
                c.is_active = linked;
            }
        }
    }
}

/// Both portals' placements while both are open.
pub fn linked_frames(portals: &Portals, query: &Query<&Portal>) -> Option<[PortalFrame; 2]> {
    let a = query.get(portals.entities[0]?).ok()?;
    let b = query.get(portals.entities[1]?).ok()?;
    Some([a.frame, b.frame])
}

fn update_holes(portals: Res<Portals>, query: Query<&Portal>, world: Res<CollisionWorld>, mut holes: ResMut<PortalHoles>) {
    let frames: Vec<PortalFrame> = linked_frames(&portals, &query).map(|f| f.to_vec()).unwrap_or_default();
    let same = frames.len() == holes.holes.len() && frames.iter().zip(&holes.holes).all(|(a, b)| *a == b.frame);
    if !same || world.is_changed() {
        holes.holes = frames.iter().map(|f| PortalHole::new(*f, &world)).collect();
        for h in &holes.holes {
            debug!("opening at {:.1?} facing {:.2?} cuts {} static volumes", h.frame.center, h.frame.forward, h.carved.len());
        }
    }
}

/// Places each portal's camera where the player's view would be if it were
/// looking out of the linked portal, clipped at that portal's surface, and
/// its inner camera one more trip through (the portal seen again inside
/// its own view, looking out of the same exit).
fn update_portal_cameras(
    portals: Res<Portals>,
    query: Query<&Portal>,
    main: Query<(&Transform, &Projection, &Exposure), (With<MainCamera>, Without<Portal>)>,
    mut cameras: Query<(&mut Transform, &mut Projection, &mut Exposure), (Without<MainCamera>, Without<Portal>)>,
) {
    let Some(frames) = linked_frames(&portals, &query) else { return };
    let Ok((main_t, main_proj, main_exposure)) = main.single() else { return };
    let Projection::Perspective(base) = main_proj else { return };
    for slot in 0..2 {
        let Some(p) = portals.entities[slot].and_then(|e| query.get(e).ok()) else { continue };
        let (from, to) = (frames[slot], frames[1 - slot]);
        let m = transfer(&from, &to);
        let outer = m * main_t.compute_affine();
        for (camera, virt) in [(p.camera, outer), (p.inner_camera, m * outer)] {
            let Ok((mut t, mut proj, mut exposure)) = cameras.get_mut(camera) else { continue };
            *t = Transform::from_matrix(virt.into());
            *exposure = *main_exposure;
            // Exit plane in view space, keeping what lies in front of it.
            let plane_world = to.forward.extend(-to.plane_dist() + 0.1);
            let world_from_view = Mat4::from(virt);
            let plane_view = world_from_view.transpose() * plane_world;
            if let Projection::Custom(custom) = proj.as_mut()
                && let Some(pp) = custom.get_mut::<PortalProjection>()
            {
                pp.base = base.clone();
                pp.clip_plane = Some(plane_view);
            }
        }
    }
}

/// Ticks after a teleport during which the player cannot teleport again,
/// so being pushed out of an obstruction cannot bounce them straight back.
const TELEPORT_COOLDOWN_TICKS: u32 = 6;
/// Fastest the player leaves any portal (`CProp_Portal`'s maximum exit
/// speed); slower than the minimum along the exit's facing is raised to it.
const MAX_EXIT_SPEED: f32 = 1000.0;
/// Least speed up out of a floor portal.
const MIN_FLOOR_EXIT_SPEED: f32 = 300.0;
/// Facing heights (normal z) of floor portals and of steep upward ones.
const FLOOR_NORMAL_Z: f32 = 0.866_025_4;

/// Least speed along the exit's facing for the player leaving `to` with
/// its centre at `center`: up out of a floor portal at 300; out of one
/// tilted up between 30 and 60 degrees, fast enough to fall clear of its
/// bottom edge (at most 300).
fn min_exit_speed(to: &PortalFrame, center: Vec3, half_height: f32, gravity: f32) -> f32 {
    let n = to.forward;
    if n.z > FLOOR_NORMAL_Z {
        return MIN_FLOOR_EXIT_SPEED;
    }
    if n.z <= 0.5 || gravity == 0.0 {
        return f32::MIN;
    }
    let bottom = to.center - to.up * HALF_HEIGHT;
    let right = -to.left();
    let to_bottom = bottom - center;
    let rise = to_bottom.z + half_height;
    let along = right.dot(Vec3::new(to_bottom.x, to_bottom.y, rise));
    let across = (to_bottom - right * along).truncate().length();
    // Solving a·s² + c = 0 for the launch speed s.
    let a = (n.truncate().length() * across - n.z * (rise - right.z * along)) * (n.z * -2.0);
    let c = across * across * gravity;
    if a >= 0.0 {
        return f32::MIN;
    }
    let s = (-c / a).sqrt();
    if s > 0.0 { s.min(MIN_FLOOR_EXIT_SPEED) } else { f32::MIN }
}

/// Moves the player through a linked portal when their centre passes
/// behind it inside the opening, the way `CPortalGameMovement` does
/// (facts in `docs/re/portals.md`, "Player transit").
#[allow(clippy::too_many_arguments)]
fn teleport_player(
    time: Res<Time<Fixed>>,
    portals: Res<Portals>,
    query: Query<&Portal>,
    physics: portal2_world::collision::Physics,
    mut players: Query<(Entity, &mut Player)>,
    mut input: ResMut<PlayerInput>,
    mut cooldown: Local<u32>,
    mut env: ResMut<PortalEnvironment>,
    console: Res<portal2_core::console::Console>,
) {
    let Some(frames) = linked_frames(&portals, &query) else { return };
    let Ok((pe, mut p)) = players.single_mut() else { return };
    if p.noclip {
        return;
    }
    if *cooldown > 0 {
        *cooldown -= 1;
        return;
    }
    let (mins, maxs) = p.state.hull();
    let half = (mins + maxs) * 0.5;
    let before = p.previous_origin + half;
    let after = p.state.origin + half;
    for slot in 0..2 {
        let (from, to) = (frames[slot], frames[1 - slot]);
        let (lb, la) = (from.to_local(before), from.to_local(after));
        // The centre ends the move behind the portal, within its rectangle.
        if !(lb.x >= 0.0 && la.x < 0.0) || la.y.abs() > HALF_WIDTH || la.z.abs() > HALF_HEIGHT {
            continue;
        }
        let m = transfer(&from, &to);
        // Gravity gained after the crossing is taken off before turning the
        // velocity and put back (a little stronger) after.
        let crossed = lb.x / (lb.x - la.x);
        let after_crossing = (1.0 - crossed) * time.delta_secs();
        let gravity = if p.state.on_ground { Vec3::ZERO } else { Vec3::NEG_Z * console.f32("sv_gravity") * after_crossing };
        let mut v = m.transform_vector3(p.state.velocity - gravity) + gravity * 1.008;
        let new_center = m.transform_point3(after);
        let min_speed = min_exit_speed(&to, new_center, half.z, console.f32("sv_gravity"));
        let along = v.dot(to.forward);
        if min_speed > along {
            v += to.forward * (min_speed - along);
        } else if v.length() > MAX_EXIT_SPEED {
            v *= MAX_EXIT_SPEED / v.length();
        }
        let max_velocity = console.f32("sv_maxvelocity");
        v = v.clamp(Vec3::splat(-max_velocity), Vec3::splat(max_velocity));
        // Falling or flying nearly straight through a floor or ceiling
        // portal leaves it exactly straight.
        if console.bool("sv_player_funnel_into_portals")
            && 1.0 - to.forward.z.abs() < 0.001
            && (v.z / (v.length() + 1.192_092_9e-7)).abs() > console.f32("sv_player_funnel_gimme_dot")
        {
            v = Vec3::new(0.0, 0.0, v.z);
        }
        // A portal that turns which way is up crouches the player, as does
        // leaving a tilted portal fast.
        let turns_up = m.matrix3.z_axis.z.abs() < FLOOR_NORMAL_Z;
        let out_speed = v.dot(to.forward);
        let tilted_fast = to.forward.z > 0.1 && to.forward.z < 0.9 && v.z > 1.0 && out_speed > 200.0;
        if (turns_up || tilted_fast) && !p.state.ducked {
            p.state.ducked = true;
        }
        let (mins, maxs) = p.state.hull();
        let half_new = (mins + maxs) * 0.5;
        let mut center = new_center;
        if tilted_fast || (turns_up && out_speed > 200.0) {
            // Shift the box so it fits inside the exit's rectangle.
            let extents = (maxs - mins) * 0.5;
            let offset = center - to.center;
            let in_plane = offset - to.forward * offset.dot(to.forward);
            let corner = center + Vec3::select(in_plane.cmplt(Vec3::ZERO), -extents, extents);
            let local = to.to_local(corner);
            let fit = |d: f32, limit: f32| {
                if d > limit {
                    d - limit
                } else if d < -limit {
                    d + limit
                } else {
                    0.0
                }
            };
            center -= to.left() * fit(local.y, HALF_WIDTH - 5.0) + to.up * fit(local.z, HALF_HEIGHT - 5.0);
        }
        p.state.origin = center - half_new;
        // If that is stuck, come out along the line through the exit's
        // centre, else step out along its facing.
        let solid = |at: Vec3| physics.trace(at, at, mins, maxs, portal2_world::collision::mask::PLAYER_SOLID, &[pe]).start_solid;
        if solid(p.state.origin) {
            let offset = p.state.origin + half_new - to.center;
            let axis = p.state.origin - (offset - to.forward * offset.dot(to.forward));
            let tr = physics.trace(axis, p.state.origin, mins, maxs, portal2_world::collision::mask::PLAYER_SOLID, &[pe]);
            if !tr.start_solid {
                p.state.origin = tr.end;
            } else if let Some(out) = (1..12).map(|i| p.state.origin + to.forward * (i as f32 * 4.0)).find(|&at| !solid(at)) {
                p.state.origin = out;
            }
        }
        *cooldown = TELEPORT_COOLDOWN_TICKS;
        // The view keeps moving smoothly: last tick's position comes along.
        p.previous_origin = m.transform_point3(p.previous_origin + half) - half_new;
        p.state.velocity = v;
        p.state.on_ground = false;
        p.state.ground_entity = None;
        p.ground_ref = None;
        env.slot = Some(1 - slot);
        // Turn the view with the portal and level the roll out.
        let forward = m.transform_vector3(angles_forward(input.input.angles)).normalize_or(Vec3::X);
        let up = (m.transform_vector3(Vec3::Z) - forward * forward.dot(m.transform_vector3(Vec3::Z))).normalize_or(Vec3::Z);
        let rot = Quat::from_mat3(&Mat3::from_cols(forward, up.cross(forward), up));
        let a = quat_to_angles(rot);
        input.input.angles = Vec3::new(a.x.clamp(-89.0, 89.0), a.y, 0.0);
        info!("teleported through portal {slot} at {:.1?} velocity {:.1?}", p.state.origin, p.state.velocity);
        break;
    }
}

/// The portal the player is passing through (whose opening they touch), for
/// the entry and exit sounds; after a teleport it is the exit portal.
#[derive(Resource, Default)]
pub struct PortalEnvironment {
    pub slot: Option<usize>,
    /// `PortalPlayer.EnterPortal` has played and `ExitPortal` not yet.
    sounding: bool,
}

/// Whether a box at `center` with half-size `half` touches a portal's
/// rectangle.
fn box_touches_portal(frame: &PortalFrame, center: Vec3, half: Vec3) -> bool {
    let l = frame.to_local(center);
    let extent = |axis: Vec3| axis.x.abs() * half.x + axis.y.abs() * half.y + axis.z.abs() * half.z;
    l.x.abs() <= extent(frame.forward) && l.y.abs() <= HALF_WIDTH + extent(frame.left()) && l.z.abs() <= HALF_HEIGHT + extent(frame.up)
}

/// `PortalPlayer.EnterPortal` as the player's core (a quarter of their box)
/// touches the opening, `ExitPortal` once a slightly bigger core (0.3) no
/// longer touches the portal they are in; louder and higher the faster
/// they go.
fn portal_sounds(
    portals: Res<Portals>,
    query: Query<&Portal>,
    players: Query<(Entity, &Player)>,
    mut env: ResMut<PortalEnvironment>,
    mut sounds: MessageWriter<portal2_audio::SoundRequest>,
) {
    let Ok((pe, p)) = players.single() else { return };
    let frames = linked_frames(&portals, &query).filter(|_| !p.noclip);
    let (mins, maxs) = p.state.hull();
    let center = p.state.origin + (mins + maxs) * 0.5;
    let half = (maxs - mins) * 0.5;
    // Keep the portal the player is in while they touch it, else take one
    // they touch.
    env.slot = frames.and_then(|f| {
        env.slot.filter(|&s| box_touches_portal(&f[s], center, half)).or_else(|| (0..2).find(|&s| box_touches_portal(&f[s], center, half)))
    });
    let speed = p.state.velocity.length();
    let mut play = |name: &str| {
        let mut r = portal2_audio::SoundRequest::on(name, pe);
        r.params.volume = Some((0.3 + speed * 0.00075).min(1.0));
        r.params.pitch = Some((80.0 + speed * 0.03).trunc());
        sounds.write(r);
    };
    let touching = |scale: f32| frames.zip(env.slot).is_some_and(|(f, s)| box_touches_portal(&f[s], center, half * scale));
    if !env.sounding {
        if touching(0.25) {
            env.sounding = true;
            play("PortalPlayer.EnterPortal");
        }
    } else if !touching(0.3) {
        env.sounding = false;
        play("PortalPlayer.ExitPortal");
    }
}

/// Command line: whether portals render at all (debugging).
pub fn portals_enabled(args: &Args) -> bool {
    !args.no_portals
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_portal_pushes_the_player_out_at_300() {
        let floor = PortalFrame { center: Vec3::ZERO, forward: Vec3::Z, up: Vec3::X };
        assert_eq!(min_exit_speed(&floor, Vec3::new(0.0, 0.0, 20.0), 36.0, 600.0), 300.0);
        let wall = PortalFrame { center: Vec3::ZERO, forward: Vec3::X, up: Vec3::Z };
        assert_eq!(min_exit_speed(&wall, Vec3::new(20.0, 0.0, 0.0), 36.0, 600.0), f32::MIN);
    }
}
