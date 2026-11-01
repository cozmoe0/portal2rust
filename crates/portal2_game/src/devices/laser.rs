//! Lasers: emitters cast beams that pass through glass and portals, bounce
//! out of reflection cubes, run through relays, power catchers and targets,
//! burn the player and destroy turrets.
//!
//! The client draws each stretch of the beam as an engine beam: a strip
//! facing the view, `sprites/purplelaser1` 32 units wide (a narrow hot core
//! in a wide red glow), or for lethal lasers `sprites/laserbeam` 2 units
//! wide, tinted (100, 255, 100).
//!
//! Each laser (the emitter's, and each one a reflection cube sends on) hums
//! `Laser.BeamLoop` (`LaserGreen.BeamLoop` for lethal ones) from the point
//! of its beam nearest the player. A catcher that powers up plays
//! `prop_laser_catcher.poweron` and loops `prop_laser_catcher.powerloop`,
//! showing its lit skin; powering down plays `prop_laser_catcher.poweroff`
//! (server.dll `sub_103C61F0` / `sub_103C62C0`).

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, MeshVertexBufferLayoutRef, PrimitiveTopology};
use bevy::pbr::{MaterialPipeline, MaterialPipelineKey};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError};
use bevy::shader::ShaderRef;

use super::{BeamSegment, linked_frames, trace_beam, with_ancestors};
use crate::entities::SpawnHandlers;
use crate::entities::io::{FireOutput, InputCall, InputHandlers};
use crate::entities::spawner::{ModelOptions, attach_studio_model};
use crate::player::Player;
use crate::player::health::DamagePlayer;
use crate::portals::{Portal, Portals};
use portal2_audio::SoundRequest;
use portal2_core::entity::MapEntity;
use portal2_core::sets::PlayerMoveSet;
use portal2_core::vfs::GameFs;
use portal2_render::models::StudioAnimator;
use portal2_render::textures::TextureCache;
use portal2_world::MapObject;
use portal2_world::collision::{Collider as TraceCollider, Physics};
use source_formats::bsp::contents;

/// Lasers pass through glass and grates.
pub const LASER_MASK: u32 = contents::SOLID | contents::MOVEABLE | contents::MONSTER;
const LASER_SHADER: &str = "embedded://portal2_render/shaders/laser.wesl";
/// Beam material and width of ordinary lasers (`C_PortalLaser`).
const BEAM_MATERIAL: &str = "sprites/purplelaser1";
const BEAM_WIDTH: f32 = 32.0;
/// Lethal lasers draw a thin green beam instead.
const LETHAL_BEAM_MATERIAL: &str = "sprites/laserbeam";
const LETHAL_BEAM_WIDTH: f32 = 2.0;
const LETHAL_BEAM_COLOR: [f32; 3] = [100.0, 255.0, 100.0];
/// Beam damage per second while it touches the player.
const BURN_DPS: f32 = 60.0;
/// How strongly the beam pushes the player away (units/s added per hit).
const BURN_PUSH: f32 = 120.0;

pub struct LaserPlugin;

impl Plugin for LaserPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<LaserBeamMaterial>::default())
            .add_message::<LaserHitTurret>()
            .add_systems(FixedUpdate, update_lasers.after(PlayerMoveSet))
            .add_systems(Update, (draw_beams, hum_beams));
        let world = app.world_mut();
        {
            let mut s = world.resource_mut::<SpawnHandlers>();
            s.register("env_portal_laser", spawn_emitter);
            s.register("prop_laser_catcher", spawn_receiver);
            s.register("prop_laser_relay", spawn_receiver);
            s.register("point_laser_target", spawn_receiver);
        }
        world.resource_mut::<InputHandlers>().register("env_portal_laser", emitter_input);
    }
}

#[derive(Component, Default, serde::Serialize, serde::Deserialize)]
pub struct LaserEmitter {
    pub on: bool,
    /// Muzzle in the emitter's local space and the local beam direction.
    pub muzzle: Vec3,
    pub muzzle_dir: Vec3,
    pub lethal: bool,
    #[serde(skip)]
    pub segments: Vec<BeamSegment>,
    #[serde(skip)]
    beams: Vec<Entity>,
    /// Where each laser of the chain hums from.
    #[serde(skip)]
    hums: Vec<Entity>,
}

/// Catchers, relays and invisible laser targets.
#[derive(Component, Default, serde::Serialize, serde::Deserialize)]
pub struct LaserReceiver {
    pub powered: bool,
    hit: bool,
    /// Ticks since the beam last touched it (smooths flicker).
    unhit_ticks: u32,
    /// The state its skin and power loop show (none yet after a spawn or
    /// a restore).
    #[serde(skip)]
    shown: Option<bool>,
}

/// A laser touched a turret this tick.
#[derive(Message, Clone, Copy, Debug)]
pub struct LaserHitTurret(pub Entity);

fn spawn_emitter(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e).cloned() else { return };
    let model = me.studio_model().and_then(|p| attach_studio_model(world, e, &p, ModelOptions::default()));
    // Prefer a model attachment named for the laser; fall back to the front.
    let (muzzle, muzzle_dir) = model
        .as_ref()
        .and_then(|m| {
            let mdl = &m.studio.mdl;
            let i = mdl
                .attachments
                .iter()
                .position(|a| a.name.to_ascii_lowercase().contains("laser"))
                .or_else(|| (!mdl.attachments.is_empty()).then_some(0))?;
            let a = &mdl.attachments[i];
            let bone = m.default_pose.get(a.bone.max(0) as usize).copied().unwrap_or(Mat4::IDENTITY);
            let world = bone * a.local;
            Some((world.transform_point3(Vec3::ZERO), world.transform_vector3(Vec3::X).normalize_or(Vec3::X)))
        })
        .unwrap_or((Vec3::X * 16.0, Vec3::X));
    let on = me.get_i32("startstate").unwrap_or(0) == 0;
    debug!("laser emitter {e}: on {on}, muzzle {muzzle:?} dir {muzzle_dir:?}");
    world.entity_mut(e).insert(LaserEmitter { on, muzzle, muzzle_dir, lethal: me.get_bool("lethaldamage"), ..default() });
}

/// A catcher's unlit and lit skins: clean 0 and 1, rusted (`SkinType`) 2
/// and 3. Relays have one skin.
fn catcher_skins(me: &MapEntity) -> Option<(i32, i32)> {
    (me.classname == "prop_laser_catcher").then(|| if me.get_bool("skintype") { (2, 3) } else { (0, 1) })
}

fn spawn_receiver(world: &mut World, e: Entity) {
    // A catcher starts unlit; its state turns the lit skin on.
    let Some(me) = world.get_mut::<MapEntity>(e).map(|mut me| {
        if let Some((off, _)) = catcher_skins(&me) {
            me.set("skin", &off.to_string());
        }
        me.clone()
    }) else {
        return;
    };
    if let Some(path) = me.studio_model() {
        let skin = me.get_i32("skin").unwrap_or(0).max(0) as usize;
        attach_studio_model(world, e, &path, ModelOptions { skin, ..default() });
    }
    world.entity_mut(e).insert(LaserReceiver::default());
}

fn emitter_input(world: &mut World, call: &InputCall) -> bool {
    let Some(mut l) = world.get_mut::<LaserEmitter>(call.target) else { return false };
    match call.input.as_str() {
        "turnon" => l.on = true,
        "turnoff" => l.on = false,
        "toggle" => l.on = !l.on,
        _ => return false,
    }
    true
}

fn segment_point_distance(seg: &BeamSegment, p: Vec3) -> (f32, Vec3) {
    let d = seg.end - seg.start;
    let t = ((p - seg.start).dot(d) / d.length_squared().max(1e-6)).clamp(0.0, 1.0);
    let closest = seg.start + d * t;
    (closest.distance(p), closest)
}

fn update_lasers(
    time: Res<Time<Fixed>>,
    physics: Physics,
    portals: Res<Portals>,
    portal_query: Query<&Portal>,
    mut emitters: Query<(Entity, &mut LaserEmitter, &GlobalTransform)>,
    mut receivers: Query<(Entity, &mut LaserReceiver, &GlobalTransform, &MapEntity, Option<&mut StudioAnimator>)>,
    cubes: Query<(&MapEntity, &GlobalTransform, &TraceCollider)>,
    players: Query<(Entity, &Player)>,
    parents: Query<&ChildOf>,
    mut damage: MessageWriter<DamagePlayer>,
    mut turret_hits: MessageWriter<LaserHitTurret>,
    mut out: MessageWriter<FireOutput>,
    (mut sounds, mut queue): (MessageWriter<SoundRequest>, ResMut<crate::entities::io::EventQueue>),
) {
    let frames = linked_frames(&portals, &portal_query);
    let dt = time.delta_secs();
    for (_, mut r, _, _, _) in &mut receivers {
        r.hit = false;
    }
    let player = players.single().ok();
    for (e, mut laser, gt) in &mut emitters {
        if !laser.on {
            laser.segments.clear();
            continue;
        }
        let affine = gt.affine();
        let start = affine.transform_point3(laser.muzzle);
        let dir = affine.transform_vector3(laser.muzzle_dir);
        let redirect = |hit: Entity| -> Option<(Vec3, Vec3)> {
            let (me, t, col) = cubes.get(hit).ok()?;
            match me.classname.as_str() {
                // Reflection cubes re-emit the beam along their facing.
                "prop_weighted_cube" if me.get_i32("cubetype") == Some(2) => {
                    let tr = t.compute_transform();
                    let center = tr.translation + tr.rotation * ((col.shape.mins + col.shape.maxs) * 0.5);
                    Some((center, tr.rotation * Vec3::X))
                }
                // Relays let the beam run on through them.
                "prop_laser_relay" => {
                    let tr = t.compute_transform();
                    Some((tr.translation + dir * 24.0, dir))
                }
                _ => None,
            }
        };
        let ignore = with_ancestors(e, &parents);
        let segments = trace_beam(&physics, frames, start, dir, LASER_MASK, &ignore, 16, redirect);
        // Receivers hit by the beam's ends, and targets along its path.
        for seg in &segments {
            if let Some(hit) = seg.hit_entity
                && let Ok((_, mut r, _, me, _)) = receivers.get_mut(hit)
                && me.classname != "point_laser_target"
            {
                r.hit = true;
            }
            if let Some(hit) = seg.hit_entity
                && cubes.get(hit).is_ok_and(|(m, _, _)| m.classname == "npc_portal_turret_floor")
            {
                turret_hits.write(LaserHitTurret(hit));
            }
            for (_, mut r, rt, me, _) in &mut receivers {
                if me.classname == "point_laser_target" && segment_point_distance(seg, rt.translation()).0 < 12.0 {
                    r.hit = true;
                }
            }
            if let Some((_, p)) = player
                && !p.noclip
            {
                let (mins, maxs) = p.state.hull();
                let center = p.state.origin + (mins + maxs) * 0.5;
                // Distance from the beam to the player's vertical axis.
                let (dist, closest) = segment_point_distance(seg, center);
                if dist < 20.0 && (closest.z - center.z).abs() < (maxs.z - mins.z) * 0.5 {
                    let away = (center - closest).with_z(0.0).normalize_or(-seg.dir.with_z(0.0).normalize_or(Vec3::X));
                    let amount = if laser.lethal { 1000.0 } else { BURN_DPS * dt };
                    damage.write(DamagePlayer { amount, push: away * BURN_PUSH * dt * 10.0, source: Some(e) });
                }
            }
        }
        let moved =
            segments.len() != laser.segments.len() || segments.iter().zip(&laser.segments).any(|(a, b)| a.end.distance(b.end) > 1.0);
        if moved {
            let first = physics.trace(start, start + dir * 4.0, Vec3::ZERO, Vec3::ZERO, LASER_MASK, &ignore);
            debug!(
                "laser {e} from {:?} dir {:?} (start solid {} contents {:#x} entity {:?} surface {}) path: {:?}",
                start.round(),
                dir,
                first.start_solid,
                first.contents,
                first.entity.and_then(|h| {
                    cubes
                        .get(h)
                        .ok()
                        .map(|(m, t, _)| (h, m.classname.clone(), t.translation().round(), parents.get(h).ok().map(|p| p.parent())))
                }),
                first.surface,
                segments.iter().map(|s| (s.end.round(), s.hit_entity, s.through_portal)).collect::<Vec<_>>()
            );
        }
        laser.segments = segments;
    }
    for (re, mut r, _, me, anim) in &mut receivers {
        if r.hit {
            r.unhit_ticks = 0;
        } else {
            r.unhit_ticks += 1;
        }
        let powered = r.hit || (r.powered && r.unhit_ticks < 3);
        if powered != r.powered {
            r.powered = powered;
            out.write(FireOutput::new(re, if powered { "OnPowered" } else { "OnUnpowered" }));
            if me.classname != "point_laser_target" {
                if let Some(mut a) = anim {
                    a.set_sequence(if powered { "spin" } else { "idle" });
                }
                if powered {
                    sounds.write(SoundRequest::on("prop_laser_catcher.poweron", re));
                    sounds.write(SoundRequest::stop("prop_laser_catcher.poweroff", re));
                } else {
                    sounds.write(SoundRequest::on("prop_laser_catcher.poweroff", re));
                }
            }
        }
        // The power loop and lit skin follow the state, also for a catcher
        // a restored game left powered.
        if me.classname != "point_laser_target" && r.shown != Some(r.powered) {
            if r.powered {
                sounds.write(SoundRequest::on("prop_laser_catcher.powerloop", re));
            } else if r.shown.is_some() {
                sounds.write(SoundRequest::stop("prop_laser_catcher.powerloop", re));
            }
            if let Some((off, on)) = catcher_skins(me) {
                let skin = if r.powered { on } else { off };
                if me.get_i32("skin") != Some(skin) {
                    queue.push(0.0, crate::entities::io::EventTarget::Entity(re), "skin", &skin.to_string(), None, None);
                }
            }
            r.shown = Some(r.powered);
        }
    }
}

#[derive(Clone, Copy, Debug, Default, ShaderType)]
pub struct LaserParams {
    pub color: Vec4,
    pub shape: Vec4,
    pub transform: Vec4,
}

#[derive(Asset, TypePath, AsBindGroup, Clone, Debug)]
pub struct LaserBeamMaterial {
    #[uniform(0)]
    pub params: LaserParams,
    #[texture(1)]
    #[sampler(2)]
    pub base: Option<Handle<Image>>,
    #[texture(3)]
    #[sampler(4)]
    pub base2: Option<Handle<Image>>,
}

impl Material for LaserBeamMaterial {
    fn vertex_shader() -> ShaderRef {
        LASER_SHADER.into()
    }

    fn fragment_shader() -> ShaderRef {
        LASER_SHADER.into()
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
        descriptor.primitive.cull_mode = None;
        if let Some(depth) = descriptor.depth_stencil.as_mut() {
            depth.depth_write_enabled = Some(false);
        }
        Ok(())
    }
}

pub(crate) fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// The beam material for a laser beam VMT: its base texture and, for
/// UnlitTwoTexture, the second texture with its transform and the
/// LinearRamp or TextureScroll and UniformNoise or Sine proxies (a Sine
/// this fast flickers like the noise).
pub(crate) fn beam_material(
    fs: &GameFs,
    textures: &mut TextureCache,
    images: &mut Assets<Image>,
    name: &str,
    width: f32,
    color: Vec3,
) -> LaserBeamMaterial {
    let vmt = portal2_render::materials::load_vmt(fs, &format!("materials/{name}.vmt"));
    let mut texture =
        |param: &str| vmt.as_ref().and_then(|v| v.texture_path(param)).and_then(|p| textures.load(fs, images, &p, true)).map(|t| t.handle);
    let base = texture("$basetexture");
    let base2 = texture("$texture2");
    let proxy = |name: &str| vmt.as_ref().and_then(|v| v.proxies.iter().find(|p| p.key.eq_ignore_ascii_case(name)));
    let noise = proxy("uniformnoise")
        .map(|p| (p.get_f32("minval").unwrap_or(0.0), p.get_f32("maxval").unwrap_or(1.0)))
        .or_else(|| proxy("sine").map(|p| (p.get_f32("sinemin").unwrap_or(0.0), p.get_f32("sinemax").unwrap_or(1.0))));
    let scale = vmt.as_ref().and_then(|v| v.get_f32("$t2scale")).unwrap_or(1.0);
    let rotation = vmt.as_ref().and_then(|v| v.get_f32("$t2rot")).unwrap_or(0.0).to_radians();
    let rate = proxy("linearramp")
        .and_then(|p| p.get_f32("rate"))
        .or_else(|| proxy("texturescroll").and_then(|p| p.get_f32("texturescrollrate")))
        .unwrap_or(0.0);
    let (lo, hi) = noise.unwrap_or((1.0, 1.0));
    LaserBeamMaterial {
        params: LaserParams {
            color: color.extend(if noise.is_some() { 1.0 } else { 0.0 }),
            shape: Vec4::new(width * 0.5, lo, hi, if base2.is_some() { 1.0 } else { 0.0 }),
            transform: Vec4::new(scale, rotation, rate, 0.0),
        },
        base,
        base2,
    }
}

/// Strip from (0, -1) to (1, 1): x along the beam, y across it.
pub(crate) fn beam_quad() -> Mesh {
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, vec![[0.0, -1.0, 0.0], [0.0, 1.0, 0.0], [1.0, 1.0, 0.0], [1.0, -1.0, 0.0]])
        .with_inserted_indices(Indices::U16(vec![0, 1, 2, 0, 2, 3]))
}

/// A beam stretch's placement: at its start, local X along it and scaled
/// to its length (the shader turns it to face the view).
pub(crate) fn beam_transform(start: Vec3, end: Vec3) -> Transform {
    let d = end - start;
    let len = d.length().max(0.01);
    Transform { translation: start, rotation: Quat::from_rotation_arc(Vec3::X, d / len), scale: Vec3::new(len, 1.0, 1.0) }
}

struct BeamAssets {
    mesh: Handle<Mesh>,
    normal: Handle<LaserBeamMaterial>,
    lethal: Handle<LaserBeamMaterial>,
}

/// Keeps one beam per stretch of each laser.
fn draw_beams(
    mut commands: Commands,
    mut emitters: Query<&mut LaserEmitter>,
    mut transforms: Query<&mut Transform>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<LaserBeamMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut textures: ResMut<TextureCache>,
    fs: Res<GameFs>,
    mut shared: Local<Option<BeamAssets>>,
) {
    let assets = shared.get_or_insert_with(|| {
        let lethal_color = Vec3::from_array(LETHAL_BEAM_COLOR.map(|c| srgb_to_linear(c / 255.0)));
        BeamAssets {
            mesh: meshes.add(beam_quad()),
            normal: materials.add(beam_material(&fs, &mut textures, &mut images, BEAM_MATERIAL, BEAM_WIDTH, Vec3::ONE)),
            lethal: materials.add(beam_material(&fs, &mut textures, &mut images, LETHAL_BEAM_MATERIAL, LETHAL_BEAM_WIDTH, lethal_color)),
        }
    });
    for mut laser in &mut emitters {
        let material = if laser.lethal { assets.lethal.clone() } else { assets.normal.clone() };
        while laser.beams.len() < laser.segments.len() {
            let e = commands
                .spawn((
                    Mesh3d(assets.mesh.clone()),
                    MeshMaterial3d(material.clone()),
                    Transform::default(),
                    // The strip is placed in the shader; its mesh bounds say nothing.
                    bevy::camera::visibility::NoFrustumCulling,
                    MapObject,
                ))
                .id();
            laser.beams.push(e);
        }
        while laser.beams.len() > laser.segments.len() {
            if let Some(e) = laser.beams.pop() {
                commands.entity(e).despawn();
            }
        }
        for (seg, &beam) in laser.segments.iter().zip(&laser.beams) {
            if let Ok(mut t) = transforms.get_mut(beam) {
                *t = beam_transform(seg.start, seg.end);
            }
        }
    }
}

/// The stretches of a laser chain that belong to one laser: a reflection
/// cube (or relay) the beam runs into starts the next.
fn lasers_of(segments: &[BeamSegment]) -> Vec<&[BeamSegment]> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, s) in segments.iter().enumerate() {
        if s.hit_entity.is_some() && i + 1 < segments.len() {
            out.push(&segments[start..=i]);
            start = i + 1;
        }
    }
    if start < segments.len() {
        out.push(&segments[start..]);
    }
    out
}

/// Keeps one humming source per laser at the point of its beam nearest the
/// player.
fn hum_beams(
    mut commands: Commands,
    mut emitters: Query<&mut LaserEmitter>,
    mut transforms: Query<&mut Transform, Without<Player>>,
    players: Query<&Player>,
    mut sounds: MessageWriter<SoundRequest>,
) {
    let Ok(player) = players.single() else { return };
    let ear = player.state.origin + Vec3::Z * player.state.eye_offset();
    for mut laser in &mut emitters {
        let name = if laser.lethal { "LaserGreen.BeamLoop" } else { "Laser.BeamLoop" };
        let points: Vec<Vec3> = if laser.on {
            lasers_of(&laser.segments)
                .into_iter()
                .filter_map(|l| l.iter().map(|s| segment_point_distance(s, ear)).min_by(|a, b| a.0.total_cmp(&b.0)).map(|(_, p)| p))
                .collect()
        } else {
            Vec::new()
        };
        while laser.hums.len() > points.len() {
            if let Some(h) = laser.hums.pop() {
                sounds.write(SoundRequest::stop(name, h));
                commands.entity(h).despawn();
            }
        }
        for (i, p) in points.into_iter().enumerate() {
            match laser.hums.get(i) {
                Some(&h) => {
                    if let Ok(mut t) = transforms.get_mut(h) {
                        t.translation = p;
                    }
                }
                None => {
                    let h = commands.spawn((Name::new("laser hum"), Transform::from_translation(p), MapObject)).id();
                    sounds.write(SoundRequest::on(name, h));
                    laser.hums.push(h);
                }
            }
        }
    }
}

impl portal2_core::save::Restore for LaserEmitter {
    fn keep(&mut self, live: Self) {
        self.segments = live.segments;
        self.beams = live.beams;
        self.hums = live.hums;
    }
}

impl portal2_core::save::Restore for LaserReceiver {}
