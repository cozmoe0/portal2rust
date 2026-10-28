//! Portals: placement, linked views rendered through each opening, moving
//! the player through, and walking into openings cut in walls.

pub mod gun;
pub mod placement;
pub mod projection;

use bevy::asset::RenderAssetUsages;
use bevy::camera::{Exposure, Hdr, RenderTarget, visibility::RenderLayers};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, Extent3d, ShaderType, TextureDimension, TextureFormat, TextureUsages,
};
use bevy::render::view::Tonemapping;
use bevy::shader::ShaderRef;
use bevy::window::PrimaryWindow;

use crate::args::Args;
use crate::camera::MainCamera;
use crate::collision::volume::{ConvexVolume, Plane};
use crate::coords::{angles_forward, quat_to_angles};
use crate::player::{Player, PlayerInput, PlayerMoveSet};
use crate::world::{MapLoaded, MapObject};
use placement::{HALF_HEIGHT, HALF_WIDTH, PortalFrame, transfer};
use projection::PortalProjection;

pub const PORTAL_SHADER: &str = "embedded://portal2/render/shaders/portal.wesl";
/// Render layer of portal surfaces (seen by the player camera only).
pub const PORTAL_LAYER: usize = 1;

pub struct PortalsPlugin;

impl Plugin for PortalsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<PortalMaterial>::default())
            .init_resource::<Portals>()
            .init_resource::<PortalHoles>()
            .init_resource::<PortalColors>()
            .add_message::<PlacePortal>()
            .add_message::<FizzlePortals>()
            .add_systems(
                Update,
                (apply_placements, fizzle_portals, clear_on_map_load, animate_portals, update_holes).chain(),
            )
            .add_systems(FixedUpdate, teleport_player.after(PlayerMoveSet))
            .add_systems(
                PostUpdate,
                update_portal_cameras
                    .after(crate::player::PlayerCameraSet)
                    .before(TransformSystems::Propagate),
            )
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

#[derive(Asset, TypePath, AsBindGroup, Clone, Debug)]
pub struct PortalMaterial {
    #[uniform(0)]
    pub params: PortalParams,
    #[texture(1)]
    #[sampler(2)]
    pub view: Option<Handle<Image>>,
}

impl Material for PortalMaterial {
    fn fragment_shader() -> ShaderRef {
        PORTAL_SHADER.into()
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
}

/// Request to remove portals (both when `slot` is None).
#[derive(Message, Clone, Copy, Debug)]
pub struct FizzlePortals {
    pub slot: Option<usize>,
}

/// Openings that let movement pass through the wall a linked portal is on.
#[derive(Resource, Default, Clone)]
pub struct PortalHoles {
    pub holes: Vec<PortalHole>,
}

#[derive(Clone, Debug)]
pub struct PortalHole {
    pub frame: PortalFrame,
    /// Walls around the opening that keep movers inside the tunnel.
    pub frame_volumes: Vec<ConvexVolume>,
}

impl PortalHole {
    fn new(frame: PortalFrame) -> Self {
        // Four slabs around the oval opening, reaching 96 units behind the surface.
        let depth = 96.0;
        let t = 48.0;
        let slab = |center_local: Vec3, half: Vec3| -> Option<ConvexVolume> {
            let axes = [frame.forward, frame.left(), frame.up];
            let c = frame.to_world(center_local);
            let mut planes = Vec::with_capacity(6);
            for (i, a) in axes.iter().enumerate() {
                planes.push(Plane { normal: *a, dist: a.dot(c) + half[i], surface: -1 });
                planes.push(Plane { normal: -*a, dist: -a.dot(c) + half[i], surface: -1 });
            }
            ConvexVolume::from_planes(planes, source_formats::bsp::contents::SOLID)
        };
        let (hw, hh) = (HALF_WIDTH - 2.0, HALF_HEIGHT - 2.0);
        let frame_volumes = [
            slab(Vec3::new(-depth / 2.0, hw + t, 0.0), Vec3::new(depth / 2.0, t, hh + 2.0 * t)),
            slab(Vec3::new(-depth / 2.0, -hw - t, 0.0), Vec3::new(depth / 2.0, t, hh + 2.0 * t)),
            slab(Vec3::new(-depth / 2.0, 0.0, hh + t), Vec3::new(depth / 2.0, hw, t)),
            slab(Vec3::new(-depth / 2.0, 0.0, -hh - t), Vec3::new(depth / 2.0, hw, t)),
        ]
        .into_iter()
        .flatten()
        .collect();
        Self { frame, frame_volumes }
    }

    /// Whether a box at `pos` sits inside the opening's tunnel, so the wall
    /// the portal is on should not block it.
    pub fn contains_box(&self, pos: Vec3, mins: Vec3, maxs: Vec3) -> bool {
        let c = pos + (mins + maxs) * 0.5;
        let h = (maxs - mins) * 0.5;
        let f = &self.frame;
        let extent = |axis: Vec3| axis.x.abs() * h.x + axis.y.abs() * h.y + axis.z.abs() * h.z;
        let l = f.to_local(c);
        let (ex, ey, ez) = (extent(f.forward), extent(f.left()), extent(f.up));
        l.y.abs() + ey <= HALF_WIDTH + 1.0
            && l.z.abs() + ez <= HALF_HEIGHT + 1.0
            && l.x - ex < 4.0
            && l.x + ex > -96.0
    }

    /// Whether a static volume belongs to the surface the portal is on.
    pub fn is_surface_volume(&self, v: &ConvexVolume) -> bool {
        let f = &self.frame;
        let d = f.plane_dist();
        v.planes.iter().any(|p| p.normal.dot(f.forward) > 0.99 && (p.dist - d).abs() < 1.0)
            && {
                // Only pieces of that surface near the opening.
                let c = (v.mins + v.maxs) * 0.5;
                let l = f.to_local(c);
                let r = (v.maxs - v.mins).length() * 0.5;
                l.y.abs() < HALF_WIDTH + r && l.z.abs() < HALF_HEIGHT + r
            }
    }
}

fn portal_quad() -> Mesh {
    // In portal local space: X out of the surface, Y left, Z up.
    let (w, h) = (HALF_WIDTH, HALF_HEIGHT);
    let x = 0.4;
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(
            Mesh::ATTRIBUTE_POSITION,
            vec![[x, w, -h], [x, -w, -h], [x, -w, h], [x, w, h]],
        )
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
    image.texture_descriptor.usage =
        TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST | TextureUsages::RENDER_ATTACHMENT;
    image
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
    mut sounds: MessageWriter<crate::audio::SoundRequest>,
) {
    for req in requests.read() {
        let rotation = req.frame.rotation();
        let open = if req.slot == 0 { "Portal.open_blue" } else { "Portal.open_red" };
        sounds.write(crate::audio::SoundRequest::at(open, req.frame.center));
        if let Some(e) = portals.entities[req.slot]
            && let Ok(mut p) = existing.get_mut(e)
        {
            sounds.write(crate::audio::SoundRequest::at("Portal.fizzle_moved", p.frame.center));
            p.frame = req.frame;
            p.opened_at = time.elapsed_secs_f64();
            if let Ok(mut t) = transforms.get_mut(e) {
                *t = Transform::from_translation(req.frame.center).with_rotation(rotation);
            }
            continue;
        }
        let (width, height) = windows.single().map(|w| (w.physical_width(), w.physical_height())).unwrap_or((1600, 900));
        let texture = images.add(view_texture(width, height));
        let color = colors.0[req.slot];
        let material = materials.add(PortalMaterial {
            params: PortalParams { color: color.extend(4.0), state: Vec4::ZERO },
            view: Some(texture.clone()),
        });
        let (base, exposure) = main
            .single()
            .ok()
            .and_then(|(p, e)| match p {
                Projection::Perspective(pp) => Some((pp.clone(), *e)),
                _ => None,
            })
            .unwrap_or_default();
        let camera = commands
            .spawn((
                Name::new(format!("portal {} view", req.slot)),
                Camera3d::default(),
                Camera { order: -10 + req.slot as isize, is_active: false, clear_color: ClearColorConfig::Custom(Color::BLACK), ..default() },
                RenderTarget::Image(texture.clone().into()),
                Hdr,
                Tonemapping::None,
                bevy::render::view::DebandDither::Disabled,
                exposure,
                Msaa::Off,
                bevy::pbr::ScreenSpaceTransmission::default(),
                Projection::custom(PortalProjection::new(base)),
                RenderLayers::from_layers(&[0, crate::world::sky::SKY2D_LAYER]),
                Transform::default(),
            ))
            .id();
        let surface = commands
            .spawn((
                Mesh3d(meshes.add(portal_quad())),
                MeshMaterial3d(material.clone()),
                RenderLayers::layer(PORTAL_LAYER),
                Transform::IDENTITY,
            ))
            .id();
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
                },
                Transform::from_translation(req.frame.center).with_rotation(rotation),
                Visibility::Visible,
            ))
            .add_child(surface)
            .id();
        portals.entities[req.slot] = Some(e);
        sounds.write(crate::audio::SoundRequest::on("Portal.ambient_loop", e));
    }
}

fn fizzle_portals(
    mut commands: Commands,
    mut requests: MessageReader<FizzlePortals>,
    mut portals: ResMut<Portals>,
    query: Query<&Portal>,
    mut sounds: MessageWriter<crate::audio::SoundRequest>,
) {
    for req in requests.read() {
        for slot in 0..2 {
            if req.slot.is_some_and(|s| s != slot) {
                continue;
            }
            if let Some(e) = portals.entities[slot].take() {
                if let Ok(p) = query.get(e) {
                    commands.entity(p.camera).despawn();
                    let close = if slot == 0 { "Portal.close_blue" } else { "Portal.close_red" };
                    sounds.write(crate::audio::SoundRequest::at(close, p.frame.center));
                }
                commands.entity(e).despawn();
            }
        }
    }
}

/// Portal entities are map objects and vanish with the map; forget them.
pub(crate) fn clear_on_map_load(mut loaded: MessageReader<MapLoaded>, mut portals: ResMut<Portals>, mut commands: Commands, query: Query<&Portal>) {
    if loaded.read().last().is_none() {
        return;
    }
    for e in portals.entities.iter_mut().filter_map(|e| e.take()) {
        if let Ok(p) = query.get(e) {
            commands.entity(p.camera).despawn();
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
    let linked = portals.entities.iter().all(|e| e.is_some_and(|e| query.get(e).is_ok()));
    for p in portals.entities.iter().flatten().filter_map(|e| query.get(*e).ok()) {
        if let Some(mut m) = materials.get_mut(&p.material) {
            let open = ((now - p.opened_at) / 0.25).clamp(0.0, 1.0) as f32;
            m.params.state = Vec4::new(if linked { 1.0 } else { 0.0 }, now as f32, open, 0.0);
        }
        if let Ok(mut c) = cameras.get_mut(p.camera) {
            c.is_active = linked;
        }
    }
}

fn linked_frames(portals: &Portals, query: &Query<&Portal>) -> Option<[PortalFrame; 2]> {
    let a = query.get(portals.entities[0]?).ok()?;
    let b = query.get(portals.entities[1]?).ok()?;
    Some([a.frame, b.frame])
}

fn update_holes(portals: Res<Portals>, query: Query<&Portal>, mut holes: ResMut<PortalHoles>) {
    let new = match linked_frames(&portals, &query) {
        Some(frames) => frames.iter().map(|f| PortalHole::new(*f)).collect(),
        None => Vec::new(),
    };
    let same = new.len() == holes.holes.len() && new.iter().zip(&holes.holes).all(|(a, b)| a.frame == b.frame);
    if !same {
        holes.holes = new;
    }
}

/// Places each portal's camera where the player's view would be if it were
/// looking out of the linked portal, clipped at that portal's surface.
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
        let Ok((mut t, mut proj, mut exposure)) = cameras.get_mut(p.camera) else { continue };
        let main_affine = main_t.compute_affine();
        let virt = m * main_affine;
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

/// Ticks after a teleport during which the player cannot teleport again,
/// so being pushed out of an obstruction cannot bounce them straight back.
const TELEPORT_COOLDOWN_TICKS: u32 = 6;

/// Moves the player through a linked portal when their centre crosses it.
fn teleport_player(
    portals: Res<Portals>,
    query: Query<&Portal>,
    physics: crate::collision::Physics,
    mut players: Query<(Entity, &mut Player)>,
    mut input: ResMut<PlayerInput>,
    mut cooldown: Local<u32>,
    mut sounds: MessageWriter<crate::audio::SoundRequest>,
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
        if !(lb.x >= 0.0 && la.x < 0.0) {
            continue;
        }
        // Where the path crosses the surface must be inside the opening.
        let t = lb.x / (lb.x - la.x);
        let cross = lb.lerp(la, t);
        if !from.in_opening(cross, 0.0) {
            continue;
        }
        let m = transfer(&from, &to);
        let new_center = m.transform_point3(after);
        p.state.origin = new_center - half;
        // If something sits right in front of the exit, step out along the
        // exit's facing rather than back into it.
        for step in 0..12 {
            let at = p.state.origin + to.forward * (step as f32 * 4.0);
            let tr = physics.trace(at, at, mins, maxs, crate::collision::mask::PLAYER_SOLID, &[pe]);
            if !tr.start_solid {
                p.state.origin = at;
                break;
            }
        }
        *cooldown = TELEPORT_COOLDOWN_TICKS;
        sounds.write(crate::audio::SoundRequest::on("PortalPlayer.EnterPortal", pe));
        sounds.write(crate::audio::SoundRequest::on("PortalPlayer.ExitPortal", pe));
        p.previous_origin = p.state.origin;
        p.state.velocity = m.transform_vector3(p.state.velocity);
        p.state.on_ground = false;
        p.state.ground_entity = None;
        p.ground_ref = None;
        // Turn the view with the portal and level the roll out.
        let forward = m.transform_vector3(angles_forward(input.input.angles)).normalize_or(Vec3::X);
        let up = (m.transform_vector3(Vec3::Z) - forward * forward.dot(m.transform_vector3(Vec3::Z))).normalize_or(Vec3::Z);
        let rot = Quat::from_mat3(&Mat3::from_cols(forward, up.cross(forward), up));
        let a = quat_to_angles(rot);
        input.input.angles = Vec3::new(a.x.clamp(-89.0, 89.0), a.y, 0.0);
        info!("teleported through portal {slot}");
        break;
    }
}

/// Command line: whether portals render at all (debugging).
pub fn portals_enabled(args: &Args) -> bool {
    !args.no_portals
}
