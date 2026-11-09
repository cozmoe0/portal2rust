//! Portals: placement, linked views rendered through each opening, moving
//! the player through, and walking into openings cut in walls.

pub mod ghosts;
pub mod gun;
pub mod placement;
pub mod projection;

use bevy::asset::RenderAssetUsages;
use bevy::camera::{CameraProjection, Exposure, Hdr, RenderTarget, Viewport, visibility::RenderLayers};
use bevy::mesh::{Indices, MeshVertexBufferLayoutRef, PrimitiveTopology};
use bevy::pbr::{MaterialPipeline, MaterialPipelineKey};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, Extent3d, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError, TextureDimension, TextureFormat,
    TextureUsages,
};
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
use portal2_world::{CurrentMap, MapLoaded, MapObject};
use projection::PortalProjection;

pub const PORTAL_SHADER: &str = "embedded://portal2_render/shaders/portal.wesl";
/// Modules `portal.wesl` imports for the visual overhaul's fire.
const PORTAL_FX_MODULES: [&str; 2] = ["embedded://portal2_render/shaders/portal_fx.wesl", "embedded://portal2_render/shaders/noise.wesl"];

pub struct PortalsPlugin;

impl Plugin for PortalsPlugin {
    fn build(&self, app: &mut App) {
        {
            let mut console = app.world_mut().resource_mut::<portal2_core::console::Console>();
            // Resolution of the views through the portals, as a fraction of
            // the window's, and how many levels of portals seen in portals
            // are drawn (the game's `r_portal_stencil_depth`: 2). The
            // browser build's dynamic quality lowers them under load.
            console.register_cvar("r_portal_scale", "1", 0);
            console.register_cvar("r_portal_depth", "2", 0);
        }
        app.add_plugins(MaterialPlugin::<PortalMaterial>::default())
            .init_resource::<Portals>()
            .init_resource::<PortalHoles>()
            .init_resource::<PortalColors>()
            .init_resource::<PortalEnvironment>()
            .init_resource::<RestingTests>()
            .add_systems(Startup, load_fx_modules)
            .add_message::<PlacePortal>()
            .add_message::<FizzlePortals>()
            .add_message::<PortalFx>()
            .add_systems(
                Update,
                (apply_placements, test_resting_surfaces, fizzle_portals, clear_on_map_load, animate_portals, update_holes).chain(),
            )
            .add_systems(FixedUpdate, (teleport_player, portal_sounds).chain().after(PlayerMoveSet))
            .add_systems(
                PostUpdate,
                (resize_portal_views, update_portal_cameras, update_fix_meshes)
                    .chain()
                    .after(portal2_core::sets::PlayerCameraSet)
                    .after(portal2_world::camera::sync_camera_transform)
                    // The views' projections (with the exit's clip plane in
                    // view space) are worked out from this frame's cameras,
                    // not left a frame behind them.
                    .before(bevy::camera::CameraUpdateSystems)
                    .before(TransformSystems::Propagate),
            )
            .add_plugins((gun::GunPlugin, ghosts::GhostsPlugin));
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
    /// x: rim brightness multiplier (the visual overhaul's HDR glow, 1
    /// otherwise); yz: the pixel size of the target the surface is drawn
    /// into, which the view texture covers at its own (possibly lower)
    /// resolution.
    pub glow: Vec4,
    /// The overhaul's fire only: how far the quad reaches past the
    /// portal's rectangle on the left, right, bottom and top, in texture
    /// coordinates (see [`glow_margins`]).
    pub shape: Vec4,
}

/// Pipeline state of a [`PortalMaterial`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PortalMaterialKey {
    /// The visual overhaul's fire (`FX` shader def, premultiplied blending).
    pub fx: bool,
}

impl From<&PortalMaterial> for PortalMaterialKey {
    fn from(m: &PortalMaterial) -> Self {
        Self { fx: m.fx }
    }
}

/// A portal's opening and rim (the game's `PortalRefract` stages, see
/// `docs/re/portals.md`): the linked view inside the hole, a fiery rim from
/// the noise texture through the portal's colour gradient, and the same
/// fire filling the hole while the portal shows static. With the visual
/// overhaul's `r_fx_portals` (`fx`), procedural fire and smoke instead
/// (`portal_fx.wesl`).
#[derive(Asset, TypePath, AsBindGroup, Clone, Debug)]
#[bind_group_data(PortalMaterialKey)]
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
    /// Draw the visual overhaul's fire.
    pub fx: bool,
}

impl Material for PortalMaterial {
    fn fragment_shader() -> ShaderRef {
        PORTAL_SHADER.into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        // The fire adds its light over the wall and hides it only where
        // smoke or the hole covers it.
        if self.fx { AlphaMode::Premultiplied } else { AlphaMode::Blend }
    }

    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        if key.bind_group_data.fx
            && let Some(fragment) = descriptor.fragment.as_mut()
        {
            fragment.shader_defs.push("FX".into());
        }
        Ok(())
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
    pub inner_texture: Handle<Image>,
    /// Past the last level the portal is drawn full of static.
    pub end_material: Handle<PortalMaterial>,
    /// The opening's three surfaces (in the window, the outer view and the
    /// inner view), drawn with the plain quad or, for the visual overhaul's
    /// fire, `fx_quad`, which reaches past the rectangle by `margins` (the
    /// flames and the glow around the hole lie there).
    pub surfaces: [Entity; 3],
    pub quad: Handle<Mesh>,
    pub fx_quad: Handle<Mesh>,
    pub margins: [f32; 4],
    /// Fills the part of the opening the main view's near plane cuts away
    /// (the game's render fix mesh), in the portal's own space.
    pub fix: Entity,
    pub fix_mesh: Handle<Mesh>,
}

/// The pair of portals (slot 0 and 1).
#[derive(Resource, Default)]
pub struct Portals {
    pub entities: [Option<Entity>; 2],
}

/// When each portal next checks that the surface behind it is still there
/// (the game's `TestRestingSurfaceThink`), by slot.
#[derive(Resource, Default)]
pub struct RestingTests {
    next: [Option<f64>; 2],
}

/// Seconds between resting surface checks.
const RESTING_TEST_INTERVAL: f64 = 0.1;

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

/// Particle effects of portal shots and portals (drawn by `portal2_particles`).
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

/// How far in front of the portal's plane its quad is drawn.
pub const QUAD_OFFSET: f32 = 0.4;
/// How far behind a portal's plane its linked view, and the models cut off
/// at it, still draw. Past the quad's offset: what lies between the plane
/// and the quad is hidden behind the quad, so the view through the other
/// portal must show it, or a model partly through would have a seam there.
pub const CLIP_BEHIND: f32 = QUAD_OFFSET + 0.1;

fn portal_quad() -> Mesh {
    portal_quad_with_margins([0.0; 4])
}

/// The portal's quad grown past its rectangle by `margins` units (left,
/// right, bottom, top). Texture coordinates run 0..1 across the rectangle
/// whatever the margins (as the render fix mesh's, `fix_mesh_from`), and on
/// past it into them.
fn portal_quad_with_margins(margins: [f32; 4]) -> Mesh {
    // In portal local space: X out of the surface, Y left, Z up.
    let [left, right, bottom, top] = margins;
    let (l, r, b, t) = (HALF_WIDTH + left, -(HALF_WIDTH + right), -(HALF_HEIGHT + bottom), HALF_HEIGHT + top);
    let x = QUAD_OFFSET;
    let corners = [[x, l, b], [x, r, b], [x, r, t], [x, l, t]];
    let uvs: Vec<[f32; 2]> = corners.iter().map(|c| quad_uv(c[1], c[2])).collect();
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, corners.to_vec())
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[1.0, 0.0, 0.0]; 4])
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
        // Counter-clockwise seen from the front (+X), where +Y is screen right.
        .with_inserted_indices(Indices::U32(vec![0, 2, 1, 0, 3, 2]))
}

/// The quad's texture coordinates at a point `left` and `up` of the centre.
fn quad_uv(left: f32, up: f32) -> [f32; 2] {
    [0.5 - left / (2.0 * HALF_WIDTH), 0.5 - up / (2.0 * HALF_HEIGHT)]
}

/// Most the overhaul's quad reaches past the portal's rectangle (units).
pub const GLOW_MARGIN: f32 = 14.0;

/// How far past each side of a portal (left, right, bottom, top) the
/// surface it sits on goes on, up to [`GLOW_MARGIN`]: the overhaul's
/// flames and glow reach that far. A side whose surface ends at the edge
/// (the portal beside an outside corner) gets less, so nothing glows in
/// the air past it; a wall standing at the edge (an inside corner) counts
/// as surface, as the quad going into it is hidden behind it. Short traces
/// through the plane at three points along each side, at the full margin
/// and at half of it.
fn glow_margins(physics: &portal2_world::collision::Physics, frame: &PortalFrame) -> [f32; 4] {
    use source_formats::bsp::contents;
    let mask = contents::SOLID | contents::WINDOW | contents::GRATE | contents::MOVEABLE;
    let surface_at = |left: f32, up: f32| {
        let start = frame.to_world(Vec3::new(1.0, left, up));
        let end = frame.to_world(Vec3::new(-1.0, left, up));
        let tr = physics.trace(start, end, Vec3::ZERO, Vec3::ZERO, mask, &[]);
        tr.start_solid || tr.fraction < 1.0
    };
    margins_from(|side, along, out| {
        let (w, h) = (HALF_WIDTH, HALF_HEIGHT);
        match side {
            0 => surface_at(w + out, along * h),
            1 => surface_at(-(w + out), along * h),
            2 => surface_at(along * w, -(h + out)),
            _ => surface_at(along * w, h + out),
        }
    })
}

/// Each side's margin from `surface(side, along, out)`: whether there is
/// surface `out` units past side `side` (left, right, bottom, top) at
/// `along` (-1..1) of the way along it.
fn margins_from(mut surface: impl FnMut(usize, f32, f32) -> bool) -> [f32; 4] {
    std::array::from_fn(|side| {
        [GLOW_MARGIN, GLOW_MARGIN * 0.5]
            .into_iter()
            .find(|&out| [-0.6, 0.0, 0.6].into_iter().all(|along| surface(side, along, out)))
            .unwrap_or(0.0)
    })
}

/// Margins in units as the quad's texture coordinates, for the shader.
fn margins_uv(m: [f32; 4]) -> Vec4 {
    Vec4::new(m[0] / (2.0 * HALF_WIDTH), m[1] / (2.0 * HALF_WIDTH), m[2] / (2.0 * HALF_HEIGHT), m[3] / (2.0 * HALF_HEIGHT))
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

/// `r_portal_scale`: the views' resolution as a fraction of the window's.
fn portal_scale(console: &portal2_core::console::Console) -> f32 {
    let s = console.f32("r_portal_scale");
    if s > 0.0 { s.clamp(0.25, 1.0) } else { 1.0 }
}

/// Pixel size of the portal views for a window of `screen` pixels.
fn portal_view_size(screen: UVec2, scale: f32) -> UVec2 {
    (screen.as_vec2() * scale).ceil().as_uvec2().clamp(UVec2::ONE, screen.max(UVec2::ONE))
}

/// A map-placed portal moves onto the surface behind it, 1/32 unit out, as
/// the game's placement check does (`sp_a1_intro1`'s orange portal is
/// placed 2 units in front of its wall and ends up on it).
fn settle_on_surface(physics: &portal2_world::collision::Physics, ignore: &[Entity], frame: &mut PortalFrame) {
    let start = frame.center + frame.forward;
    let tr = physics.trace(start, start - frame.forward * 17.0, Vec3::ZERO, Vec3::ZERO, placement::SHOT_MASK, ignore);
    if tr.fraction < 1.0 && !tr.start_solid && tr.normal.dot(frame.forward) > 0.99 {
        frame.center = tr.end + frame.forward / 32.0;
    }
}

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
    (fs, mut textures, physics, entities, mut resting): (
        Res<portal2_core::vfs::GameFs>,
        ResMut<portal2_render::textures::TextureCache>,
        portal2_world::collision::Physics,
        Query<(Entity, &portal2_core::entity::MapEntity)>,
        ResMut<RestingTests>,
    ),
) {
    for req in requests.read() {
        let mut req = *req;
        resting.next[req.slot] = Some(time.elapsed_secs_f64() + RESTING_TEST_INTERVAL);
        if req.settle {
            let ignore: Vec<Entity> = entities.iter().filter(|(_, me)| placement::shot_ignores(&me.classname)).map(|(e, _)| e).collect();
            settle_on_surface(&physics, &ignore, &mut req.frame);
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
            p.margins = glow_margins(&physics, &req.frame);
            if let Some(mut mesh) = meshes.get_mut(&p.fx_quad) {
                *mesh = portal_quad_with_margins(p.margins);
            }
            for handle in [&p.material, &p.inner_material, &p.end_material] {
                if let Some(mut m) = materials.get_mut(handle) {
                    m.params.shape = margins_uv(p.margins);
                }
            }
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
        let margins = glow_margins(&physics, &req.frame);
        let base_material = PortalMaterial {
            params: PortalParams { color: color.extend(4.0), state: Vec4::ZERO, glow: Vec4::ONE, shape: margins_uv(margins) },
            view: Some(texture.clone()),
            noise: textures.load(&fs, &mut images, "materials/models/portals/noise-blur-256x256.vtf", false).map(|t| t.handle),
            gradient: textures.load(&fs, &mut images, &gradient, true).map(|t| t.handle),
            // `visuals::fx::portal_look` turns it on with the overhaul.
            fx: false,
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
        let fx_quad = meshes.add(portal_quad_with_margins(margins));
        let mut surface_on = |layer: usize, material: &Handle<PortalMaterial>| {
            commands.spawn((Mesh3d(quad.clone()), MeshMaterial3d(material.clone()), RenderLayers::layer(layer), Transform::IDENTITY)).id()
        };
        let surface = surface_on(PORTAL_LAYER, &material);
        let inner_surface = surface_on(PORTAL_INNER_LAYER, &inner_material);
        let end_surface = surface_on(PORTAL_END_LAYER, &end_material);
        let fix_mesh = meshes.add(fix_mesh_from(&[]));
        let fix = commands
            .spawn((
                Mesh3d(fix_mesh.clone()),
                MeshMaterial3d(material.clone()),
                RenderLayers::layer(PORTAL_LAYER),
                Transform::IDENTITY,
                Visibility::Hidden,
                bevy::camera::visibility::NoFrustumCulling,
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
                    inner_camera,
                    inner_material,
                    inner_texture,
                    end_material,
                    surfaces: [surface, inner_surface, end_surface],
                    quad,
                    fx_quad,
                    margins,
                    fix,
                    fix_mesh,
                },
                Transform::from_translation(req.frame.center).with_rotation(rotation),
                Visibility::Visible,
            ))
            .add_children(&[surface, inner_surface, end_surface, fix])
            .id();
        portals.entities[req.slot] = Some(e);
        sounds.write(portal2_audio::SoundRequest::on("Portal.ambient_loop", e));
    }
}

/// A portal whose surface is gone from behind any corner of its opening
/// closes. Traces run from `CORNER_INSET` inside each corner to 1 unit behind
/// the plane; the check repeats while a corner rests on an entity (which
/// may move away or be removed), and stops once all rest on the world.
fn test_resting_surfaces(
    time: Res<Time>,
    portals: Res<Portals>,
    query: Query<&Portal>,
    physics: portal2_world::collision::Physics,
    mut tests: ResMut<RestingTests>,
    mut fizzle: MessageWriter<FizzlePortals>,
) {
    use source_formats::bsp::contents;
    let now = time.elapsed_secs_f64();
    let mask = contents::SOLID | contents::WINDOW | contents::GRATE | contents::MOVEABLE;
    for slot in 0..2 {
        let Some(at) = tests.next[slot] else { continue };
        if now < at {
            continue;
        }
        let Some(p) = portals.entities[slot].and_then(|e| query.get(e).ok()) else {
            tests.next[slot] = None;
            continue;
        };
        let f = p.frame;
        let mut on_entity = false;
        let mut removed = false;
        for (left, up) in [(1.0, 1.0), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)] {
            let inset = placement::CORNER_INSET;
            let corner = f.to_world(Vec3::new(0.0, left * (HALF_WIDTH - inset), up * (HALF_HEIGHT - inset)));
            let tr = physics.trace(corner, corner - f.forward, Vec3::ZERO, Vec3::ZERO, mask, &[]);
            if tr.fraction >= 1.0 && !tr.start_solid {
                removed = true;
                break;
            }
            on_entity |= tr.entity.is_some();
        }
        if removed {
            info!("portal {slot}: surface removed from behind it");
            fizzle.write(FizzlePortals { slot: Some(slot) });
            tests.next[slot] = None;
        } else {
            tests.next[slot] = on_entity.then_some(now + RESTING_TEST_INTERVAL);
        }
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

/// Keeps the overhaul fire's shader modules loaded, so `portal.wesl`'s
/// imports resolve.
#[derive(Resource)]
#[allow(dead_code)]
struct PortalFxModules(Vec<Handle<Shader>>);

fn load_fx_modules(mut commands: Commands, assets: Res<AssetServer>) {
    commands.insert_resource(PortalFxModules(PORTAL_FX_MODULES.iter().map(|m| assets.load(*m)).collect()));
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
    console: Res<portal2_core::console::Console>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let screen = windows.single().map(|w| UVec2::new(w.physical_width(), w.physical_height())).unwrap_or(UVec2::ONE).max(UVec2::ONE);
    let view = portal_view_size(screen, portal_scale(&console));
    // With one level only, a portal seen inside a portal shows static.
    let inner_views = console.i32("r_portal_depth") >= 2;
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
        // The shader takes the time from the view, so a portal's materials
        // stay unchanged (and are not prepared again) once it has opened.
        let state = Vec4::new(if linked { 1.0 } else { 0.0 }, 0.0, open, 1.0 - static_amount);
        let end_state = Vec4::new(0.0, 0.0, open, 0.0);
        let inner_state = if inner_views { state } else { end_state };
        // The outer surface is drawn in the window, the inner one in the
        // outer view.
        for (material, state, target) in
            [(&p.material, state, screen), (&p.inner_material, inner_state, view), (&p.end_material, end_state, view)]
        {
            let target = target.as_vec2();
            if materials.get(material).is_some_and(|m| m.params.state != state || m.params.glow.yz() != target)
                && let Some(mut m) = materials.get_mut(material)
            {
                m.params.state = state;
                m.params.glow.y = target.x;
                m.params.glow.z = target.y;
            }
        }
        for camera in [p.camera, p.inner_camera] {
            if let Ok(mut c) = cameras.get_mut(camera)
                && c.is_active != linked
            {
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

/// Keeps the portal views the size of the window (times `r_portal_scale`):
/// they are sampled in screen space with the main view's projection.
fn resize_portal_views(
    windows: Query<&Window, With<PrimaryWindow>>,
    portals: Res<Portals>,
    query: Query<&Portal>,
    mut images: ResMut<Assets<Image>>,
    console: Res<portal2_core::console::Console>,
) {
    let Ok(window) = windows.single() else { return };
    let screen = UVec2::new(window.physical_width(), window.physical_height()).max(UVec2::ONE);
    let view = portal_view_size(screen, portal_scale(&console));
    let size = Extent3d { width: view.x, height: view.y, depth_or_array_layers: 1 };
    for p in portals.entities.iter().flatten().filter_map(|e| query.get(*e).ok()) {
        for handle in [&p.texture, &p.inner_texture] {
            if images.get(handle).is_some_and(|i| i.texture_descriptor.size != size)
                && let Some(mut image) = images.get_mut(handle)
            {
                image.resize(size);
            }
        }
    }
}

/// Places each portal's camera where the player's view would be if it were
/// looking out of the linked portal, clipped at that portal's surface, and
/// its inner camera one more trip through (the portal seen again inside
/// its own view, looking out of the same exit).
///
/// A view is only ever seen on its portal, so like the game (which stencils
/// each portal's view into the pixels of its opening and skips portals
/// outside the PVS) each camera draws only the rectangle its portal covers
/// on screen, and not at all while that is empty: the portal is off screen,
/// faces away, or lies in no cluster the eye can see. An inner view draws
/// where its portal shows inside the outer view's rectangle.
fn update_portal_cameras(
    portals: Res<Portals>,
    query: Query<&Portal>,
    main: Query<(&Transform, &Projection, &Exposure), (With<MainCamera>, Without<Portal>)>,
    mut cameras: Query<(&mut Camera, &mut Transform, &mut Projection, &mut Exposure), (Without<MainCamera>, Without<Portal>)>,
    windows: Query<&Window, With<PrimaryWindow>>,
    map: Option<Res<CurrentMap>>,
    mut pvs: Local<PvsCache>,
    console: Res<portal2_core::console::Console>,
) {
    let Some(frames) = linked_frames(&portals, &query) else { return };
    let Ok((main_t, main_proj, main_exposure)) = main.single() else { return };
    let Projection::Perspective(base) = main_proj else { return };
    let Ok(window) = windows.single() else { return };
    let screen = UVec2::new(window.physical_width(), window.physical_height()).max(UVec2::ONE);
    // The views' own pixels: rectangles found on screen are scaled to them.
    let view_size = portal_view_size(screen, portal_scale(&console));
    let inner_views = console.i32("r_portal_depth") >= 2;
    let eye = main_t.compute_affine();
    let full_clip_from_view = base.get_clip_from_view();
    let main_clip_from_world = full_clip_from_view * eye.inverse();
    let bsp = map.as_ref().map(|m| &*m.bsp);
    pvs.track(bsp);
    let eye_clusters = bsp.map(|b| vec![cluster_at(b, main_t.translation)]).unwrap_or_default();
    let portal_clusters: [Vec<i32>; 2] = frames.map(|f| bsp.map(|b| clusters_in_front(b, &f)).unwrap_or_default());
    for slot in 0..2 {
        let Some(p) = portals.entities[slot].and_then(|e| query.get(e).ok()) else { continue };
        let (from, to) = (frames[slot], frames[1 - slot]);
        let m = transfer(&from, &to);
        let outer = m * eye;
        // Where the portal shows in the main view: its quad while it faces
        // the eye, and the near-plane fix while that is drawn.
        let mut seen = Vec::new();
        if from.to_local(main_t.translation).x > QUAD_OFFSET {
            seen.extend(clip_to_front(&quad_corners(&from), main_clip_from_world));
        }
        let fix = near_plane_behind(&from, main_t, base);
        seen.extend(fix.iter().map(|l| main_clip_from_world * from.to_world(*l).extend(1.0)));
        // The eye passing through may be past the wall the portal is on.
        let visible = !fix.is_empty() || bsp.is_none_or(|b| pvs.sees(b, &eye_clusters, &portal_clusters[slot]));
        let outer_rect = visible.then(|| pixel_rect(&seen, screen)).flatten();
        // The portal again inside its own view, in front of the exit's clip
        // plane, seen from where the exit looks out.
        let inner_rect = outer_rect.filter(|_| inner_views).and_then(|outer_rect| {
            if from.to_local(outer.translation.into()).x <= QUAD_OFFSET {
                return None;
            }
            let corners = quad_corners(&from);
            if corners.iter().all(|c| to.to_local(*c).x < -CLIP_BEHIND) {
                return None;
            }
            if !bsp.is_none_or(|b| pvs.sees(b, &portal_clusters[1 - slot], &portal_clusters[slot])) {
                return None;
            }
            let points = clip_to_front(&corners, full_clip_from_view * outer.inverse());
            pixel_rect(&points, screen).map(|r| r.intersect(outer_rect)).filter(|r| !r.is_empty())
        });
        for (camera, virt, rect) in [(p.camera, outer, outer_rect), (p.inner_camera, m * outer, inner_rect)] {
            let Ok((mut cam, mut t, mut proj, mut exposure)) = cameras.get_mut(camera) else { continue };
            let Some(rect) = rect.and_then(|r| scale_rect(r, screen, view_size)) else {
                cam.is_active = false;
                continue;
            };
            cam.is_active = true;
            let full = rect.min == UVec2::ZERO && rect.max == view_size;
            cam.viewport = (!full).then(|| Viewport { physical_position: rect.min, physical_size: rect.size(), ..default() });
            *t = Transform::from_matrix(virt.into());
            *exposure = *main_exposure;
            // Exit plane in view space, keeping what lies in front of it. An
            // eye less than the bias in front of it would leave the plane
            // behind the camera (and the wall behind the exit in view), so
            // the plane then passes just in front of the eye instead.
            let plane_world = to.forward.extend(-to.plane_dist() + CLIP_BEHIND);
            let world_from_view = Mat4::from(virt);
            let mut plane_view = world_from_view.transpose() * plane_world;
            plane_view.w = plane_view.w.min(-0.01);
            if let Projection::Custom(custom) = proj.as_mut()
                && let Some(pp) = custom.get_mut::<PortalProjection>()
            {
                pp.base = base.clone();
                pp.clip_plane = Some(plane_view);
                pp.cull_plane = Some(plane_world);
                // From the viewport itself, so that the projection covers
                // exactly the pixels drawn.
                pp.crop = (!full).then(|| {
                    let ndc =
                        |p: UVec2| Vec2::new(p.x as f32 / view_size.x as f32 * 2.0 - 1.0, 1.0 - p.y as f32 / view_size.y as f32 * 2.0);
                    Rect::from_corners(ndc(rect.min), ndc(rect.max))
                });
            }
        }
    }
}

/// Pixels kept around a portal's rectangle on screen, as a fraction of the
/// window's larger side: refraction, the volumetric light's upsampling and
/// filtering read a little around each pixel they draw.
const RECT_MARGIN: f32 = 0.03;

/// A rectangle of window pixels as the pixels of a view `view` in size
/// covering the same window (rounded outwards); None when empty.
fn scale_rect(rect: URect, screen: UVec2, view: UVec2) -> Option<URect> {
    if view == screen {
        return Some(rect);
    }
    let k = view.as_vec2() / screen.as_vec2();
    let min = (rect.min.as_vec2() * k).floor().as_uvec2().min(view);
    let max = (rect.max.as_vec2() * k).ceil().as_uvec2().min(view);
    let r = URect::from_corners(min, max);
    (!r.is_empty()).then_some(r)
}

/// The corners of a portal's quad.
fn quad_corners(frame: &PortalFrame) -> [Vec3; 4] {
    [(1.0, 1.0), (-1.0, 1.0), (-1.0, -1.0), (1.0, -1.0)]
        .map(|(y, z)| frame.to_world(Vec3::new(QUAD_OFFSET, y * HALF_WIDTH, z * HALF_HEIGHT)))
}

/// A convex polygon in clip space, cut where it passes behind the eye.
fn clip_to_front(polygon: &[Vec3], clip_from_world: Mat4) -> Vec<Vec4> {
    const MIN_W: f32 = 1e-3;
    let clip: Vec<Vec4> = polygon.iter().map(|p| clip_from_world * p.extend(1.0)).collect();
    let mut out = Vec::with_capacity(clip.len() + 1);
    for (i, &a) in clip.iter().enumerate() {
        let b = clip[(i + 1) % clip.len()];
        if a.w >= MIN_W {
            out.push(a);
        }
        if (a.w >= MIN_W) != (b.w >= MIN_W) {
            out.push(a + (b - a) * ((MIN_W - a.w) / (b.w - a.w)));
        }
    }
    out
}

/// The window's pixels covering points in clip space (all in front of the
/// eye), with the margin; None when that is off screen.
fn pixel_rect(points: &[Vec4], screen: UVec2) -> Option<URect> {
    if points.is_empty() {
        return None;
    }
    let (mut lo, mut hi) = (Vec2::MAX, Vec2::MIN);
    for p in points {
        let ndc = p.truncate().truncate() / p.w;
        lo = lo.min(ndc);
        hi = hi.max(ndc);
    }
    if hi.x < -1.0 || hi.y < -1.0 || lo.x > 1.0 || lo.y > 1.0 {
        return None;
    }
    let size = screen.as_vec2();
    let margin = RECT_MARGIN * size.max_element();
    // Pixels run down from the top.
    let min = ((Vec2::new(lo.x, -hi.y) * 0.5 + 0.5) * size - margin).floor().clamp(Vec2::ZERO, size);
    let max = ((Vec2::new(hi.x, -lo.y) * 0.5 + 0.5) * size + margin).ceil().clamp(Vec2::ZERO, size);
    let rect = URect::from_corners(min.as_uvec2(), max.as_uvec2());
    (!rect.is_empty()).then_some(rect)
}

/// The cluster of the leaf at a point (-1 in solid or outside the map).
fn cluster_at(bsp: &source_formats::bsp::Bsp, point: Vec3) -> i32 {
    bsp.leafs.get(bsp.find_leaf(point)).map_or(-1, |l| l.cluster as i32)
}

/// The clusters a portal's opening looks into: those just in front of its
/// centre and corners.
fn clusters_in_front(bsp: &source_formats::bsp::Bsp, frame: &PortalFrame) -> Vec<i32> {
    let mut clusters: Vec<i32> = [(0.0, 0.0), (1.0, 1.0), (-1.0, 1.0), (-1.0, -1.0), (1.0, -1.0)]
        .iter()
        .map(|(y, z)| cluster_at(bsp, frame.to_world(Vec3::new(2.0, y * (HALF_WIDTH - 2.0), z * (HALF_HEIGHT - 2.0)))))
        .collect();
    clusters.sort_unstable();
    clusters.dedup();
    clusters
}

/// Decompressed PVS rows of the current map, by cluster.
#[derive(Default)]
struct PvsCache {
    map: usize,
    rows: std::collections::HashMap<i32, Vec<u8>>,
}

impl PvsCache {
    fn track(&mut self, bsp: Option<&source_formats::bsp::Bsp>) {
        let id = bsp.map_or(0, |b| b as *const _ as usize);
        if id != self.map {
            self.map = id;
            self.rows.clear();
        }
    }

    /// Whether any of the `to` clusters is potentially visible from any of
    /// the `from` clusters (always, for a point in solid or a map without
    /// visibility data).
    fn sees(&mut self, bsp: &source_formats::bsp::Bsp, from: &[i32], to: &[i32]) -> bool {
        let Some(vis) = &bsp.visibility else { return true };
        if from.iter().chain(to).any(|&c| c < 0) {
            return true;
        }
        from.iter().any(|&f| {
            let row = self.rows.entry(f).or_insert_with(|| vis.pvs(f as usize));
            row.is_empty() || to.iter().any(|&t| row.get((t / 8) as usize).is_some_and(|b| b & (1 << (t % 8)) != 0))
        })
    }
}

/// The game's render fix: where the main view's near plane lies behind a
/// portal inside its opening, the opening's quad is cut away and the wall
/// behind would show (walking slowly through, or with the eye carried
/// through ahead of the body); that piece of the near plane, just beyond
/// it, is drawn with the portal's material instead.
fn update_fix_meshes(
    portals: Res<Portals>,
    query: Query<&Portal>,
    main: Query<(&Transform, &Projection), (With<MainCamera>, Without<Portal>)>,
    mut visibility: Query<&mut Visibility>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let linked = linked_frames(&portals, &query).is_some();
    let main = main.single().ok().and_then(|(t, p)| match p {
        Projection::Perspective(pp) => Some((*t, pp.clone())),
        _ => None,
    });
    for p in portals.entities.iter().flatten().filter_map(|e| query.get(*e).ok()) {
        let polygon = match (&main, linked) {
            (Some((t, pp)), true) => near_plane_behind(&p.frame, t, pp),
            _ => Vec::new(),
        };
        if let Ok(mut v) = visibility.get_mut(p.fix) {
            let want = if polygon.len() >= 3 { Visibility::Inherited } else { Visibility::Hidden };
            if *v != want {
                *v = want;
            }
        }
        if polygon.len() >= 3
            && let Some(mut mesh) = meshes.get_mut(&p.fix_mesh)
        {
            *mesh = fix_mesh_from(&polygon);
        }
    }
}

/// The part of the camera's near plane (pushed a hair further out) behind
/// the portal's quad and inside its opening, in the portal's space. Empty
/// unless the eye or its near plane reaches in front of the quad: a camera
/// wholly behind it (one the view was not carried through for) is behind
/// the wall, however it lines up with the opening.
fn near_plane_behind(frame: &PortalFrame, camera: &Transform, pp: &PerspectiveProjection) -> Vec<Vec3> {
    let near = pp.near * 1.01;
    let hy = near * (pp.fov * 0.5).tan();
    let hx = hy * pp.aspect_ratio;
    let (right, up, forward) = (camera.rotation * Vec3::X, camera.rotation * Vec3::Y, camera.rotation * Vec3::NEG_Z);
    let centre = camera.translation + forward * near;
    let mut poly: Vec<Vec3> = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
        .iter()
        .map(|(sx, sy)| frame.to_local(centre + right * (hx * sx) + up * (hy * sy)))
        .collect();
    if frame.to_local(camera.translation).x <= QUAD_OFFSET && poly.iter().all(|l| l.x <= QUAD_OFFSET) {
        return Vec::new();
    }
    // Keep x <= the quad's plane and |y|, |z| within the opening.
    let planes =
        [(Vec3::X, QUAD_OFFSET), (Vec3::Y, HALF_WIDTH), (Vec3::NEG_Y, HALF_WIDTH), (Vec3::Z, HALF_HEIGHT), (Vec3::NEG_Z, HALF_HEIGHT)];
    for (n, d) in planes {
        poly = clip_polygon(&poly, n, d);
        if poly.len() < 3 {
            return Vec::new();
        }
    }
    poly
}

/// Keeps the part of a convex polygon where `n · p <= d`.
fn clip_polygon(poly: &[Vec3], n: Vec3, d: f32) -> Vec<Vec3> {
    let mut out = Vec::with_capacity(poly.len() + 1);
    for (i, &a) in poly.iter().enumerate() {
        let b = poly[(i + 1) % poly.len()];
        let (da, db) = (n.dot(a) - d, n.dot(b) - d);
        if da <= 0.0 {
            out.push(a);
        }
        if (da <= 0.0) != (db <= 0.0) {
            out.push(a + (b - a) * (da / (da - db)));
        }
    }
    out
}

/// A fan over a convex polygon in portal space, with the quad's texture
/// coordinates of each point projected onto the opening.
fn fix_mesh_from(polygon: &[Vec3]) -> Mesh {
    let points: Vec<Vec3> = if polygon.len() >= 3 { polygon.to_vec() } else { vec![Vec3::ZERO; 3] };
    let uvs: Vec<[f32; 2]> = points.iter().map(|p| quad_uv(p.y, p.z)).collect();
    let mut indices = Vec::with_capacity((points.len() - 2) * 6);
    for i in 1..points.len() as u32 - 1 {
        // Both windings: the camera may see it from either side.
        indices.extend([0, i, i + 1, 0, i + 1, i]);
    }
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, points.iter().map(|p| p.to_array()).collect::<Vec<_>>())
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[1.0, 0.0, 0.0]; points.len()])
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
        .with_inserted_indices(Indices::U32(indices))
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
fn teleport_player(
    time: Res<Time<Fixed>>,
    portals: Res<Portals>,
    query: Query<&Portal>,
    physics: portal2_world::collision::Physics,
    mut players: Query<(Entity, &mut Player)>,
    mut input: ResMut<PlayerInput>,
    mut cooldown: Local<u32>,
    mut env: ResMut<PortalEnvironment>,
    mut holding: ResMut<crate::physics::carry::Holding>,
    mut props: Query<crate::physics::portal_travel::TravelBody, With<crate::entities::props::PhysicsProp>>,
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
        // What the player carries comes through with them (falling into a
        // floor portal with a cube held out over the floor beside it),
        // unless it is already on the far side.
        if let Some(held) = holding.entity
            && holding.portal.is_none()
            && let Ok(body) = props.get_mut(held)
        {
            crate::physics::portal_travel::carry_through(body, &m, &to, holding.radius);
            holding.clear = None;
        } else {
            holding.player_crossed(slot);
        }
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
    env.slot = frames
        .and_then(|f| env.slot.filter(|&s| f[s].touches_box(center, half)).or_else(|| (0..2).find(|&s| f[s].touches_box(center, half))));
    let speed = p.state.velocity.length();
    let mut play = |name: &str| {
        let mut r = portal2_audio::SoundRequest::on(name, pe);
        r.params.volume = Some((0.3 + speed * 0.00075).min(1.0));
        r.params.pitch = Some((80.0 + speed * 0.03).trunc());
        sounds.write(r);
    };
    let touching = |scale: f32| frames.zip(env.slot).is_some_and(|(f, s)| f[s].touches_box(center, half * scale));
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

    fn mesh_uvs(mesh: &Mesh) -> Vec<[f32; 2]> {
        match mesh.attribute(Mesh::ATTRIBUTE_UV_0) {
            Some(bevy::mesh::VertexAttributeValues::Float32x2(v)) => v.clone(),
            _ => panic!("no uvs"),
        }
    }

    /// The plain quad keeps the game's corners and texture coordinates; the
    /// grown one maps its corners as the render fix mesh maps points, so the
    /// shaders see one coordinate system on both.
    #[test]
    fn grown_quad_shares_the_fix_mesh_coordinates() {
        assert_eq!(mesh_uvs(&portal_quad()), vec![[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]]);
        let margins = [14.0, 0.0, 7.0, 14.0];
        let grown = mesh_uvs(&portal_quad_with_margins(margins));
        let [left, right, bottom, top] = margins;
        let corners = [
            Vec3::new(QUAD_OFFSET, HALF_WIDTH + left, -(HALF_HEIGHT + bottom)),
            Vec3::new(QUAD_OFFSET, -(HALF_WIDTH + right), -(HALF_HEIGHT + bottom)),
            Vec3::new(QUAD_OFFSET, -(HALF_WIDTH + right), HALF_HEIGHT + top),
        ];
        let fix = mesh_uvs(&fix_mesh_from(&corners));
        for (a, b) in grown.iter().zip(&fix) {
            assert!((a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6, "{a:?} {b:?}");
        }
        // The shader's margins, in texture coordinates, reach the corners.
        let uv = margins_uv(margins);
        assert!((grown[0][0] + uv.x).abs() < 1e-6 && (grown[1][0] - 1.0 - uv.y).abs() < 1e-6);
        assert!((grown[0][1] - 1.0 - uv.z).abs() < 1e-6 && (grown[2][1] + uv.w).abs() < 1e-6);
    }

    /// A side with surface all along it gets the whole margin, one whose
    /// surface ends within half of it none, one in between half.
    #[test]
    fn glow_margins_stop_where_the_surface_does() {
        let m = margins_from(|side, along, out| match side {
            0 => true,
            1 => out <= GLOW_MARGIN * 0.5,
            2 => false,
            // Ends short only near one end of the side.
            _ => along < 0.5 || out < 1.0,
        });
        assert_eq!(m, [GLOW_MARGIN, GLOW_MARGIN * 0.5, 0.0, 0.0]);
    }

    /// A portal ahead covers its own rectangle (with the margin), one behind
    /// the eye none, and one passing beside the eye the window's side.
    #[test]
    fn portal_rectangles_on_screen() {
        let screen = UVec2::new(1600, 900);
        let proj = PerspectiveProjection { fov: 1.2, aspect_ratio: 16.0 / 9.0, near: 4.0, ..default() };
        // Looking down -Z from the origin.
        let clip_from_world = proj.get_clip_from_view();
        let ahead = PortalFrame { center: Vec3::new(0.0, 0.0, -400.0), forward: Vec3::Z, up: Vec3::Y };
        let rect = pixel_rect(&clip_to_front(&quad_corners(&ahead), clip_from_world), screen).unwrap();
        let margin = (RECT_MARGIN * 1600.0) as u32;
        assert!(rect.min.x > 650 && rect.max.x < 950 && rect.min.y > 300 && rect.max.y < 600, "{rect:?}");
        assert!(rect.width() > 2 * margin && rect.height() > 2 * margin);
        let behind = PortalFrame { center: Vec3::new(0.0, 0.0, 400.0), forward: Vec3::NEG_Z, up: Vec3::Y };
        assert_eq!(pixel_rect(&clip_to_front(&quad_corners(&behind), clip_from_world), screen), None);
        // A wall portal running from beside the eye to ahead of it.
        let beside = PortalFrame { center: Vec3::new(20.0, 0.0, -10.0), forward: Vec3::NEG_X, up: Vec3::Y };
        let rect = pixel_rect(&clip_to_front(&quad_corners(&beside), clip_from_world), screen).unwrap();
        assert_eq!(rect.max.x, 1600);
    }

    /// Walking into a portal, the near plane passing the quad gets the fix
    /// where it is inside the opening. Standing behind the wall, lined up
    /// with the opening, gets none, however far back.
    #[test]
    fn near_plane_fix_only_from_in_front() {
        let pp = PerspectiveProjection { fov: 1.2, aspect_ratio: 16.0 / 9.0, near: 4.0, ..default() };
        let frame = PortalFrame { center: Vec3::ZERO, forward: Vec3::X, up: Vec3::Z };
        let into = Transform::from_xyz(2.0, 0.0, 10.0).looking_to(Vec3::NEG_X, Vec3::Z);
        assert!(near_plane_behind(&frame, &into, &pp).len() >= 3);
        for depth in [20.0, 60.0, 150.0] {
            for dir in [Vec3::X, Vec3::NEG_X, Vec3::Y] {
                let behind = Transform::from_xyz(-depth, 0.0, 10.0).looking_to(dir, Vec3::Z);
                assert!(near_plane_behind(&frame, &behind, &pp).is_empty(), "{depth} {dir}");
            }
        }
    }

    #[test]
    fn floor_portal_pushes_the_player_out_at_300() {
        let floor = PortalFrame { center: Vec3::ZERO, forward: Vec3::Z, up: Vec3::X };
        assert_eq!(min_exit_speed(&floor, Vec3::new(0.0, 0.0, 20.0), 36.0, 600.0), 300.0);
        let wall = PortalFrame { center: Vec3::ZERO, forward: Vec3::X, up: Vec3::Z };
        assert_eq!(min_exit_speed(&wall, Vec3::new(20.0, 0.0, 0.0), 36.0, 600.0), f32::MIN);
    }

    /// A portal on a brush entity is checked again while the brush is
    /// there, and closes once the brush stops colliding (disabled, moved
    /// away or removed).
    /// Traced against a panel that ends two units past the portal's right
    /// edge, that side gets no margin and the others the whole of it.
    #[test]
    fn glow_margins_traced_on_a_panel() {
        use portal2_world::collision::volume::ConvexVolume;
        use portal2_world::collision::{Collider, Physics, Shape};
        use source_formats::bsp::contents;

        #[derive(Resource, Default)]
        struct Found([f32; 4]);
        // Facing -X with Z up, the portal's left is -Y.
        let frame = PortalFrame { center: Vec3::new(-1.0 / 32.0, 0.0, 0.0), forward: Vec3::NEG_X, up: Vec3::Z };
        let mut app = App::new();
        app.init_resource::<CollisionWorld>().init_resource::<Found>().add_systems(
            Update,
            move |physics: Physics, mut found: ResMut<Found>| found.0 = glow_margins(&physics, &frame),
        );
        let panel =
            ConvexVolume::from_box(Vec3::new(0.0, -100.0, -100.0), Vec3::new(8.0, HALF_WIDTH + 2.0, 100.0), contents::SOLID, 0);
        app.world_mut().spawn((Collider::new(std::sync::Arc::new(Shape::new(vec![panel])), true, true), GlobalTransform::IDENTITY));
        app.update();
        assert_eq!(app.world().resource::<Found>().0, [GLOW_MARGIN, 0.0, GLOW_MARGIN, GLOW_MARGIN]);
    }

    #[test]
    fn portal_closes_when_its_brush_goes() {
        use portal2_world::collision::volume::ConvexVolume;
        use portal2_world::collision::{Collider, Shape};
        use source_formats::bsp::contents;

        let mut app = App::new();
        app.add_message::<FizzlePortals>()
            .init_resource::<Time>()
            .init_resource::<CollisionWorld>()
            .init_resource::<RestingTests>()
            .init_resource::<Portals>()
            .add_systems(Update, test_resting_surfaces);
        let panel = ConvexVolume::from_box(Vec3::new(0.0, -100.0, -100.0), Vec3::new(8.0, 100.0, 100.0), contents::SOLID, 0);
        let brush = app
            .world_mut()
            .spawn((Collider::new(std::sync::Arc::new(Shape::new(vec![panel])), true, true), GlobalTransform::IDENTITY))
            .id();
        let portal = app
            .world_mut()
            .spawn(Portal {
                slot: 0,
                frame: PortalFrame { center: Vec3::new(-1.0 / 32.0, 0.0, 0.0), forward: Vec3::NEG_X, up: Vec3::Z },
                surface: Entity::PLACEHOLDER,
                camera: Entity::PLACEHOLDER,
                material: Handle::default(),
                texture: Handle::default(),
                opened_at: 0.0,
                inner_camera: Entity::PLACEHOLDER,
                inner_material: Handle::default(),
                inner_texture: Handle::default(),
                end_material: Handle::default(),
                surfaces: [Entity::PLACEHOLDER; 3],
                quad: Handle::default(),
                fx_quad: Handle::default(),
                margins: [0.0; 4],
                fix: Entity::PLACEHOLDER,
                fix_mesh: Handle::default(),
            })
            .id();
        app.world_mut().resource_mut::<Portals>().entities[0] = Some(portal);
        let closed = |app: &mut App| app.world_mut().resource_mut::<Messages<FizzlePortals>>().drain().count();

        app.world_mut().resource_mut::<RestingTests>().next[0] = Some(0.0);
        app.update();
        assert_eq!(closed(&mut app), 0);
        assert_eq!(app.world().resource::<RestingTests>().next[0], Some(RESTING_TEST_INTERVAL), "resting on an entity: checked again");

        app.world_mut().get_mut::<Collider>(brush).unwrap().enabled = false;
        app.world_mut().resource_mut::<RestingTests>().next[0] = Some(0.0);
        app.update();
        assert_eq!(closed(&mut app), 1);
        assert_eq!(app.world().resource::<RestingTests>().next[0], None);
    }
}
