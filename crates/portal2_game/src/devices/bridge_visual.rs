//! How light bridges look, after `C_ProjectedWallEntity` (client.dll):
//!
//! - Each straight piece draws one quad on its top face with
//!   `effects/projected_wall` (a `SolidEnergy` material) and a rail on each
//!   side edge, a 12-unit tall quad with `effects/projected_wall_rail`
//!   (`sub_103AADB0`); `shaders/bridge.wesl` has the details.
//! - The drawn length grows by `cl_projected_wall_projection_speed` (150)
//!   every client think, 16 ms apart (`sub_103A4800`), so a bridge visibly
//!   shoots out of its projector or portal; the texture stays anchored to
//!   the moving front, as the game draws it. Only the picture grows: the
//!   bridge holds things up at its full length straight away.
//! - `projected_wall_impact` sparks run across each end that touches a
//!   portal or a wall, 3.125 units in from it, turned like the piece, with
//!   control point 1 on the portal (or 512 units above a wall end)
//!   (`sub_103A3A80`).
//!
//! - Through linked portals the bridge reads as one: each piece's texture,
//!   strands and pulses are laid along the whole path from the projector
//!   (the texture anchored to the projection front, as the game's), only
//!   the first piece glows at its emitter, and where a piece meets a portal
//!   it is drawn a little past the portal's plane, for the view through the
//!   other portal to show, while from the room's side the strip between the
//!   plane and the portal's quad is left to that view: every point of the
//!   bridge is drawn exactly once, whichever is drawn first.
//!
//! With the visual overhaul on, the shader adds its extras (HDR edges,
//! pulses, end glows, the projection front, ripples around the player's feet
//! and props resting on the bridge) and each piece lights its surroundings
//! in blue: a tube light along each rail and a wide one along the surface,
//! plus a glow at the projection front while the bridge shoots out.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, MeshVertexBufferLayoutRef, PrimitiveTopology};
use bevy::pbr::{MaterialPipeline, MaterialPipelineKey};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError};
use bevy::shader::ShaderRef;

use super::bridge::{BridgePiece, LightBridge, PortalPlane, THICKNESS, WIDTH, piece_basis};
use crate::entities::props::PhysicsProp;
use crate::player::Player;
use crate::visuals::fx::lights::{FxLightData, FxLightShape, FxLights, light_flags};
use crate::visuals::fx::{FxSettings, features};
use portal2_core::vfs::GameFs;
use portal2_particles::ParticleEffect;
use portal2_render::textures::TextureCache;

const SHADER: &str = "embedded://portal2_render/shaders/bridge.wesl";
const SURFACE_MATERIAL: &str = "materials/effects/projected_wall.vmt";
const RAIL_MATERIAL: &str = "materials/effects/projected_wall_rail.vmt";
/// Drawn length gained per second: 150 units per 16 ms think.
const PROJECTION_SPEED: f32 = 150.0 / 0.016;
/// How long the projection front keeps glowing once the bridge is out.
const FRONT_FADE: f32 = 0.35;
const RAIL_HEIGHT: f32 = 12.0;
const IMPACT_EFFECT: &str = "projected_wall_impact";
const IMPACT_INSET: f32 = 3.125;
/// A piece that ran this far met nothing.
const OPEN_END: f32 = 16000.0;
/// Props whose centre is this close above the surface leave a ripple.
const CONTACT_HEIGHT: f32 = 36.0;
/// How far a piece is drawn past a portal it meets: past where the view
/// through the other portal clips (`portals::CLIP_BEHIND`).
const PAST_PORTAL: f32 = crate::portals::CLIP_BEHIND + 0.1;
/// The overhaul's light from each piece: a tube along each glowing rail
/// (their pale cyan, with highlights that run up ribbed walls and across
/// wet floors) and a wide one along the deeper blue surface between them,
/// so the bridge washes the walls, floors and props around it in its
/// colour; a faint blue glow in the air over it; and a glow at the front
/// while it projects.
const RAIL_LIGHT_COLOR: Vec3 = Vec3::new(0.26, 0.66, 1.0);
const RAIL_LIGHT_INTENSITY: f32 = 160.0;
/// The rails' highlights are their own light, brighter than their diffuse
/// glow, so walls and floors catch crisp reflections without being washed
/// out.
const RAIL_HIGHLIGHT_INTENSITY: f32 = 320.0;
const RAIL_LIGHT_RADIUS: f32 = 20.0;
const RAIL_LIGHT_RANGE: f32 = 300.0;
const SURFACE_LIGHT_COLOR: Vec3 = Vec3::new(0.12, 0.42, 1.0);
const SURFACE_LIGHT_INTENSITY: f32 = 230.0;
const SURFACE_LIGHT_RADIUS: f32 = 44.0;
const SURFACE_LIGHT_RANGE: f32 = 280.0;
const HAZE_COLOR: Vec3 = Vec3::new(0.15, 0.48, 1.0);
const HAZE_INTENSITY: f32 = 2200.0;
const HAZE_RADIUS: f32 = 40.0;
const HAZE_HEIGHT: f32 = 10.0;
const HAZE_RANGE: f32 = 260.0;
const FRONT_LIGHT_INTENSITY: f32 = 700.0;
const FRONT_LIGHT_RANGE: f32 = 220.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, ShaderType)]
pub struct BridgeParams {
    /// x: shown length, y: width, z: 0 surface / 1 rail, w: 1 with the overhaul.
    pub shape: Vec4,
    /// x: distance from the projector to the piece's start, y: projection
    /// front brightness, z: 1 for an emitter glow at the start, w: 1 when
    /// the far end meets a surface.
    pub path: Vec4,
    /// x along, y left of centre, z strength (0 for none).
    pub contacts: [Vec4; 4],
    /// x: where the drawn quad starts, units along from the piece's start
    /// (negative when drawn past the portal it leaves), y: shown length,
    /// z: the projection front's distance along the whole path.
    pub extent: Vec4,
    /// The planes (xyz facing, w offset) of the portals the piece leaves
    /// and enters; zero for none.
    pub leave: Vec4,
    pub enter: Vec4,
}

fn plane_params(plane: Option<PortalPlane>) -> Vec4 {
    plane.map_or(Vec4::ZERO, |p| p.normal.extend(p.normal.dot(p.point)))
}

/// `effects/projected_wall`'s three textures and the rail's, sampled with
/// the base texture's sampler.
#[derive(Asset, TypePath, AsBindGroup, Clone, Debug)]
pub struct BridgeMaterial {
    #[uniform(0)]
    pub params: BridgeParams,
    #[texture(1)]
    #[sampler(2)]
    pub base: Option<Handle<Image>>,
    #[texture(3)]
    pub detail1: Option<Handle<Image>>,
    #[texture(4)]
    pub detail2: Option<Handle<Image>>,
    #[texture(5)]
    pub rail: Option<Handle<Image>>,
}

impl Material for BridgeMaterial {
    fn fragment_shader() -> ShaderRef {
        SHADER.into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Add
    }

    fn enable_prepass() -> bool {
        false
    }

    fn enable_shadows() -> bool {
        false
    }

    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        // `$nocull`: seen from above and below.
        descriptor.primitive.cull_mode = None;
        if let Some(depth) = descriptor.depth_stencil.as_mut() {
            depth.depth_write_enabled = Some(false);
        }
        Ok(())
    }
}

/// What a bridge shows: how far it has projected and the drawn parts of
/// each piece.
#[derive(Component, Default)]
pub struct BridgeVisual {
    shown: f32,
    front: f32,
    pieces: Vec<BridgePiece>,
    parts: Vec<PiecePart>,
}

struct PiecePart {
    piece: Entity,
    surface: Entity,
    rails: [Entity; 2],
    material: Handle<BridgeMaterial>,
    rail_material: Handle<BridgeMaterial>,
    /// Distance from the projector to the piece's start.
    start: f32,
    len: f32,
    center: Vec3,
    rotation: Quat,
    shown: f32,
    /// Whether the quads have been sized for `shown` yet.
    placed: bool,
    /// Sparks still to fire at the start (leaving a portal) and the end,
    /// when the projection gets there: control point 1 for each.
    start_impact: Option<Vec3>,
    end_impact: Option<Vec3>,
    leaves: Option<PortalPlane>,
    enters: Option<PortalPlane>,
}

impl PiecePart {
    fn world_start(&self) -> Vec3 {
        self.center - self.rotation * Vec3::X * (self.len * 0.5)
    }
}

/// Meshes and the textured materials every piece copies.
#[derive(Clone)]
pub(super) struct BridgeAssets {
    surface: Handle<Mesh>,
    rail: Handle<Mesh>,
    material: BridgeMaterial,
}

impl BridgeAssets {
    fn load(fs: &GameFs, textures: &mut TextureCache, images: &mut Assets<Image>, meshes: &mut Assets<Mesh>) -> Self {
        let surface_vmt = portal2_render::materials::load_vmt(fs, SURFACE_MATERIAL);
        let rail_vmt = portal2_render::materials::load_vmt(fs, RAIL_MATERIAL);
        let mut texture = |vmt: &Option<source_formats::vmt::Vmt>, param: &str| {
            let path = vmt.as_ref().and_then(|v| v.texture_path(param));
            if path.is_none() {
                warn!("light bridge: no {param} in its material");
            }
            path.and_then(|p| textures.load(fs, images, &p, true)).map(|t| t.handle)
        };
        let material = BridgeMaterial {
            params: BridgeParams::default(),
            base: texture(&surface_vmt, "$basetexture"),
            detail1: texture(&surface_vmt, "$detail1"),
            detail2: texture(&surface_vmt, "$detail2"),
            rail: texture(&rail_vmt, "$basetexture"),
        };
        Self { surface: meshes.add(quad(false)), rail: meshes.add(quad(true)), material }
    }
}

/// A unit quad centred on the origin: on the XY plane for the surface (U
/// across from the right edge, V along X from the start), on the XZ plane
/// for a rail (U down from the top).
fn quad(rail: bool) -> Mesh {
    let corners = [(-0.5f32, -0.5f32), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)];
    let mut positions = Vec::new();
    let mut uvs = Vec::new();
    for (x, s) in corners {
        if rail {
            positions.push([x, 0.0, s]);
            uvs.push([0.5 - s, x + 0.5]);
        } else {
            positions.push([x, s, 0.0]);
            uvs.push([s + 0.5, x + 0.5]);
        }
    }
    let normal = if rail { [0.0, 1.0, 0.0] } else { [0.0, 0.0, 1.0] };
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![normal; 4])
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
        .with_inserted_indices(Indices::U32(vec![0, 1, 2, 0, 2, 3]))
}

/// Path length two piece lists share from the projector: the bridge only
/// re-projects past the point where it changed.
fn common_length(old: &[BridgePiece], new: &[BridgePiece]) -> f32 {
    let mut total = 0.0;
    for (a, b) in old.iter().zip(new) {
        let (da, db) = (a.end - a.start, b.end - b.start);
        let same_ray = a.start.distance(b.start) < 0.5 && da.normalize_or_zero().dot(db.normalize_or_zero()) > 0.9999;
        if !same_ray {
            break;
        }
        if a.end.distance(b.end) < 0.5 {
            total += da.length();
            continue;
        }
        total += da.length().min(db.length());
        break;
    }
    total
}

/// Keeps every bridge's drawn parts in step with its pieces and projection.
pub(super) fn draw_bridges(
    mut commands: Commands,
    time: Res<Time>,
    settings: Res<FxSettings>,
    fs: Res<GameFs>,
    mut bridges: Query<(Entity, &LightBridge, Option<&mut BridgeVisual>)>,
    mut transforms: Query<&mut Transform>,
    mut materials: ResMut<Assets<BridgeMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut textures: ResMut<TextureCache>,
    players: Query<&Player>,
    props: Query<&GlobalTransform, With<PhysicsProp>>,
    mut shared: Local<Option<BridgeAssets>>,
) {
    let dt = time.delta_secs();
    let fx = settings.enabled();
    for (e, bridge, visual) in &mut bridges {
        let Some(mut visual) = visual else {
            commands.entity(e).insert(BridgeVisual::default());
            continue;
        };
        let built: Vec<Entity> = visual.parts.iter().map(|p| p.piece).collect();
        if built != bridge.entities() || visual.pieces != bridge.pieces {
            let assets = shared.get_or_insert_with(|| BridgeAssets::load(&fs, &mut textures, &mut images, &mut meshes)).clone();
            let keep = common_length(&visual.pieces, &bridge.pieces);
            visual.shown = visual.shown.min(keep);
            visual.pieces = bridge.pieces.clone();
            visual.parts = build_parts(&mut commands, &mut materials, &assets, bridge);
        }
        let total: f32 = visual.parts.iter().map(|p| p.len).sum();
        if visual.shown < total {
            visual.shown = (visual.shown + PROJECTION_SPEED * dt).min(total);
            visual.front = 1.0;
        } else {
            visual.front = (visual.front - dt / FRONT_FADE).max(0.0);
        }
        let (shown, front) = (visual.shown, visual.front);
        let count = visual.parts.len();
        for (i, part) in visual.parts.iter_mut().enumerate() {
            let disp = (shown - part.start).clamp(0.0, part.len);
            let moved = disp != part.shown || !part.placed;
            part.shown = disp;
            if disp > 0.0
                && let Some(cp1) = part.start_impact.take()
            {
                spawn_impact(&mut commands, part.piece, -part.len * 0.5 + IMPACT_INSET, cp1);
            }
            if disp >= part.len
                && let Some(cp1) = part.end_impact.take()
            {
                spawn_impact(&mut commands, part.piece, part.len * 0.5 - IMPACT_INSET, cp1);
            }
            // The quads cover the drawn length from the piece's start, and
            // a little past the portals it meets once drawn up to them.
            let from = if disp > 0.0 && part.leaves.is_some() { -PAST_PORTAL } else { 0.0 };
            let to = disp + if disp >= part.len && part.enters.is_some() { PAST_PORTAL } else { 0.0 };
            if moved {
                let x = -part.len * 0.5 + (from + to) * 0.5;
                let length = (to - from).max(0.01);
                // (Parts spawned this frame exist from the next one.)
                if let Ok(mut t) = transforms.get_mut(part.surface) {
                    *t = Transform::from_xyz(x, 0.0, THICKNESS * 0.5).with_scale(Vec3::new(length, WIDTH, 1.0));
                    part.placed = true;
                }
                for (rail, side) in part.rails.iter().zip([0.5, -0.5]) {
                    if let Ok(mut t) = transforms.get_mut(*rail) {
                        *t = Transform::from_xyz(x, WIDTH * side, 0.0).with_scale(Vec3::new(length, 1.0, RAIL_HEIGHT));
                    }
                }
            }
            let last = i + 1 == count;
            let reaching = disp > 0.0 && disp < part.len - 0.5;
            let front_glow = if reaching { 1.0 } else if last { front } else { 0.0 };
            let meets_surface = last && part.len < OPEN_END;
            let contacts = if fx { contacts_on(part, &players, &props) } else { [Vec4::ZERO; 4] };
            // Only the first piece glows where it leaves its emitter.
            let emitter = if i == 0 { 1.0 } else { 0.0 };
            let params = BridgeParams {
                shape: Vec4::new((to - from).max(0.01), WIDTH, 0.0, if fx { 1.0 } else { 0.0 }),
                path: Vec4::new(part.start, front_glow, emitter, if meets_surface { 1.0 } else { 0.0 }),
                contacts,
                extent: Vec4::new(from, disp, shown, 0.0),
                leave: plane_params(part.leaves),
                enter: plane_params(part.enters),
            };
            set_params(&mut materials, &part.material, params);
            let rail = BridgeParams { shape: Vec4::new(params.shape.x, WIDTH, 1.0, params.shape.w), contacts: [Vec4::ZERO; 4], ..params };
            set_params(&mut materials, &part.rail_material, rail);
        }
    }
}

fn set_params(materials: &mut Assets<BridgeMaterial>, handle: &Handle<BridgeMaterial>, params: BridgeParams) {
    // Only touch the asset when something changed (each change re-uploads it).
    if materials.get(handle).is_some_and(|m| m.params != params)
        && let Some(mut m) = materials.get_mut(handle)
    {
        m.params = params;
    }
}

/// Spawns the drawn parts and end sparks under each piece's entity.
fn build_parts(commands: &mut Commands, materials: &mut Assets<BridgeMaterial>, assets: &BridgeAssets, bridge: &LightBridge) -> Vec<PiecePart> {
    let mut parts = Vec::new();
    let mut start = 0.0;
    let count = bridge.pieces.len();
    debug!("light bridge pieces: {:?}", bridge.pieces);
    for (i, (piece, &entity)) in bridge.pieces.iter().zip(bridge.entities()).enumerate() {
        let (center, len, rotation) = piece_basis(piece);
        let material = materials.add(assets.material.clone());
        let rail_material = materials.add(assets.material.clone());
        // Sized by `draw_bridges` from the next frame on.
        let unsized_ = Transform::from_scale(Vec3::splat(0.001));
        let surface = commands
            .spawn((Name::new("light bridge surface"), Mesh3d(assets.surface.clone()), MeshMaterial3d(material.clone()), unsized_))
            .id();
        let rails = [0, 1].map(|_| {
            commands
                .spawn((Name::new("light bridge rail"), Mesh3d(assets.rail.clone()), MeshMaterial3d(rail_material.clone()), unsized_))
                .id()
        });
        commands.entity(entity).add_children(&[surface, rails[0], rails[1]]);
        // Sparks where the piece leaves a portal and wherever it ends, with
        // control point 1 on the portal or 512 units above a wall end.
        let start_impact = (i > 0).then_some(piece.start);
        let end_impact = (len < OPEN_END).then(|| if i + 1 < count { piece.end } else { piece.end + Vec3::Z * 512.0 });
        parts.push(PiecePart {
            piece: entity,
            surface,
            rails,
            material,
            rail_material,
            start,
            len,
            center,
            rotation,
            shown: 0.0,
            placed: false,
            start_impact,
            end_impact,
            leaves: piece.leaves,
            enters: piece.enters,
        });
        start += len;
    }
    parts
}

fn spawn_impact(commands: &mut Commands, piece: Entity, x: f32, cp1: Vec3) {
    let mut effect = ParticleEffect::new(IMPACT_EFFECT, true);
    effect.cp_values.push((1, cp1));
    effect.one_shot = true;
    // Just above the surface: the sprites' visibility test traces to their
    // origin, which the bridge's own collision would hide.
    let z = THICKNESS * 0.5 + 0.5;
    let mut effect_entity = commands.spawn((Name::new("light bridge impact"), Transform::from_xyz(x, 0.0, z), Visibility::default(), effect));
    effect_entity.insert(ChildOf(piece));
}

/// The player's feet and props near the surface, strongest first, in the
/// piece's terms (units along from its start, units left of centre).
fn contacts_on(part: &PiecePart, players: &Query<&Player>, props: &Query<&GlobalTransform, With<PhysicsProp>>) -> [Vec4; 4] {
    let to_local = |p: Vec3| part.rotation.inverse() * (p - part.center);
    let inside = |l: Vec3| l.x.abs() < part.len * 0.5 && l.x + part.len * 0.5 < part.shown && l.y.abs() < WIDTH * 0.5 + 8.0;
    let mut found: Vec<Vec4> = Vec::new();
    for player in players {
        let s = &player.state;
        if s.on_ground && s.ground_entity == Some(part.piece) {
            let l = to_local(s.origin);
            if inside(l) {
                let pace = (s.velocity.truncate().length() / 175.0).min(1.0);
                found.push(Vec4::new(l.x + part.len * 0.5, l.y, 0.7 + 0.3 * pace, 0.0));
            }
        }
    }
    for gt in props {
        let l = to_local(gt.translation());
        let height = l.z - THICKNESS * 0.5;
        if inside(l) && (0.0..CONTACT_HEIGHT).contains(&height) {
            found.push(Vec4::new(l.x + part.len * 0.5, l.y, 1.0 - height / CONTACT_HEIGHT, 0.0));
        }
    }
    found.sort_by(|a, b| b.z.total_cmp(&a.z));
    let mut out = [Vec4::ZERO; 4];
    for (slot, c) in out.iter_mut().zip(found) {
        *slot = c;
    }
    out
}

/// The overhaul's light from each shown piece: its rails, its surface, and
/// the projection front while it grows.
pub(super) fn bridge_lights(settings: Res<FxSettings>, bridges: Query<&BridgeVisual>, mut lights: ResMut<FxLights>) {
    if !settings.has(features::LIGHTS) {
        return;
    }
    let tube = |start: Vec3, end: Vec3, facing: Vec3, color: Vec3, radius: f32, range: f32, flags: u32| FxLightData {
        position: start,
        shape: FxLightShape::Line { end, facing },
        color,
        range,
        flags,
        radius,
        priority: 1.5,
        shadow: false,
    };
    for visual in &bridges {
        let count = visual.parts.len();
        for (i, part) in visual.parts.iter().enumerate() {
            if part.shown < 8.0 {
                continue;
            }
            let start = part.world_start();
            let along = part.rotation * Vec3::X * part.shown;
            let side = part.rotation * Vec3::Y * (WIDTH * 0.5);
            let up = part.rotation * Vec3::Z * (THICKNESS * 0.5 + 1.0);
            let both = light_flags::DIFFUSE | light_flags::SPECULAR;
            // A bridge to walk on lights the walls beside it and what stands
            // on it, and only a little of the floor beneath, which shows
            // through it and would wash out its blue. One standing as a wall
            // lights both ways.
            let normal = part.rotation * Vec3::Z;
            let facing = if normal.z > 0.7 { normal } else { Vec3::ZERO };
            for s in [side, -side] {
                let a = start + s + up;
                let (glow, highlight) = (RAIL_LIGHT_COLOR * RAIL_LIGHT_INTENSITY, RAIL_LIGHT_COLOR * RAIL_HIGHLIGHT_INTENSITY);
                lights.frame.push(tube(a, a + along, facing, glow, RAIL_LIGHT_RADIUS, RAIL_LIGHT_RANGE, light_flags::DIFFUSE));
                lights.frame.push(tube(a, a + along, facing, highlight, RAIL_LIGHT_RADIUS, RAIL_LIGHT_RANGE, light_flags::SPECULAR));
            }
            // The surface is a broad emitter, diffuse only (a highlight on
            // the floor seen through it would wash it out).
            lights.frame.push(tube(
                start,
                start + along,
                facing,
                SURFACE_LIGHT_COLOR * SURFACE_LIGHT_INTENSITY,
                SURFACE_LIGHT_RADIUS,
                SURFACE_LIGHT_RANGE,
                light_flags::DIFFUSE,
            ));
            // The air over it glows (only where the overhaul's volumetrics
            // run; the light itself adds nothing to surfaces).
            let lift = part.rotation * Vec3::Z * HAZE_HEIGHT;
            lights.frame.push(FxLightData {
                priority: 2.5,
                ..tube(start + lift, start + along + lift, Vec3::ZERO, HAZE_COLOR * HAZE_INTENSITY, HAZE_RADIUS, HAZE_RANGE, light_flags::VOLUMETRIC)
            });
            // The front of a bridge still shooting out.
            let reaching = part.shown < part.len - 0.5;
            let glow = if reaching {
                1.0
            } else if i + 1 == count {
                visual.front
            } else {
                0.0
            };
            if glow > 0.01 {
                lights.frame.push(FxLightData {
                    position: start + along + up * 4.0,
                    shape: FxLightShape::Point { attenuation: Vec3::new(1600.0, 0.0, 1.0) },
                    color: RAIL_LIGHT_COLOR * FRONT_LIGHT_INTENSITY * glow,
                    range: FRONT_LIGHT_RANGE,
                    flags: both,
                    radius: 24.0,
                    priority: 2.0,
                    shadow: false,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn piece(start: [f32; 3], end: [f32; 3]) -> BridgePiece {
        BridgePiece::new(Vec3::from(start), Vec3::from(end), Vec3::Z)
    }

    #[test]
    fn reprojects_only_past_the_change() {
        let a = piece([0.0, 0.0, 0.0], [500.0, 0.0, 0.0]);
        let b = piece([0.0, 100.0, 0.0], [0.0, 400.0, 0.0]);
        // Unchanged: all of it stays.
        assert_eq!(common_length(&[a, b], &[a, b]), 800.0);
        // The first piece grows (a portal moved away): the old length stays.
        assert_eq!(common_length(&[a], &[piece([0.0, 0.0, 0.0], [700.0, 0.0, 0.0])]), 500.0);
        // It shrinks (something blocks it): only the new length stays.
        assert_eq!(common_length(&[a, b], &[piece([0.0, 0.0, 0.0], [200.0, 0.0, 0.0])]), 200.0);
        // The second piece leaves elsewhere: the first stays whole.
        assert_eq!(common_length(&[a, b], &[a, piece([0.0, -100.0, 0.0], [0.0, -300.0, 0.0])]), 500.0);
        // A projector that turned re-projects from the start.
        assert_eq!(common_length(&[a], &[piece([0.0, 0.0, 0.0], [0.0, 500.0, 0.0])]), 0.0);
        assert_eq!(common_length(&[], &[a]), 0.0);
    }
}
