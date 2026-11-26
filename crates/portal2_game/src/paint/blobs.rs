//! Paint sprayers (`info_paint_sprayer`) and the blobs they shoot. Blobs
//! fall under gravity with a little drag, 60 steps a second, and paint a
//! sphere where they hit (the world and any brush entity it reaches), or
//! slide along the surface first when they streak. Each sprayer draws its
//! live blobs as one merged liquid surface (`super::surface`), lit as a
//! model at its `light_position_name` (`paint_blob.wesl`); bombs draw the
//! same way, from twenty blobs bouncing about inside them. Rules from the
//! game's (docs/re/paint.md).

use bevy::asset::RenderAssetUsages;
use bevy::math::Affine3A;
use bevy::platform::collections::HashMap;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::{Material, MaterialPlugin};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;

use crate::entities::SpawnHandlers;
use crate::entities::io::{InputCall, InputHandlers};
use portal2_audio::SoundRequest;
use portal2_core::console::Console;
use portal2_core::entity::MapEntity;
use portal2_core::sets::ThinkSet;
use portal2_core::vfs::GameFs;
use portal2_render::models::lighting::MAX_MODEL_LIGHTS;
use portal2_render::source_material::ModelLight;
use portal2_render::textures::TextureCache;
use portal2_world::CurrentMap;
use portal2_world::collision::portal::{HALF_HEIGHT, HALF_WIDTH, PortalFrame, transfer};
use portal2_world::collision::{Physics, mask};
use portal2_world::paint_map::{PaintMap, PaintType, SplatSettings};

pub const BLOB_SHADER: &str = "embedded://portal2_game/paint/paint_blob.wesl";
const TICK: f32 = 1.0 / 60.0;
/// Blobs this far below the world are gone.
const KILL_DEPTH: f32 = -32768.0;
const MAX_LIFETIME: f32 = 30.0;
/// At most this many impacts a second make a sound (`max_noisy_blobs_per_second`).
const NOISY_PER_SECOND: f32 = 5.0;

pub struct BlobPlugin;

impl Plugin for BlobPlugin {
    fn build(&self, app: &mut App) {
        bevy::asset::embedded_asset!(app, "paint_blob.wesl");
        app.add_plugins(MaterialPlugin::<PaintBlobMaterial>::default())
            .init_resource::<Blobs>()
            .add_systems(PreUpdate, clear_on_map_load.after(portal2_core::sets::LoadMapSet))
            .add_systems(FixedUpdate, (spray, move_blobs).chain().in_set(ThinkSet))
            
            // Drawn where the bombs are now, not a frame behind.
            .add_systems(PostUpdate, draw_blobs.after(TransformSystems::Propagate));
        let world = app.world_mut();
        world.resource_mut::<SpawnHandlers>().register("info_paint_sprayer", spawn_sprayer);
        world.resource_mut::<InputHandlers>().register("info_paint_sprayer", sprayer_input);
        world.resource_mut::<InputHandlers>().register("paint_sphere", paint_sphere_input);
        world.resource_mut::<InputHandlers>().register_generic(remove_paint_input);
    }
}

#[derive(Clone, Copy, Debug, Default, serde::Serialize, serde::Deserialize)]
struct Range {
    min: f32,
    max: f32,
}

impl Range {
    fn of(me: &MapEntity, min: &str, max: &str, default: f32) -> Self {
        let lo = me.get_f32(min).unwrap_or(default);
        Self { min: lo, max: me.get_f32(max).unwrap_or(lo) }
    }

    fn pick(&self, rng: &mut Rng) -> f32 {
        self.min + (self.max - self.min) * rng.next()
    }
}

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
struct Rng(u32);

impl Rng {
    fn next(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        (x >> 8) as f32 / (1u32 << 24) as f32
    }
}

#[derive(Component, serde::Serialize, serde::Deserialize)]
pub struct PaintSprayer {
    pub active: bool,
    pub paint: PaintType,
    blobs_per_second: f32,
    speed: Range,
    spread_angle: f32,
    spread_radius: f32,
    streak_percentage: f32,
    streak_time: Range,
    streak_dampen: Range,
    start_radius: Range,
    end_radius: Range,
    grow_time: Range,
    max_blobs: usize,
    silent: bool,
    /// `drawonly`: blobs that paint nothing.
    draw_only: bool,
    accumulated: f32,
    rng: Rng,
    /// Light the blobs are drawn in.
    #[serde(skip)]
    lighting: BlobLighting,
    #[serde(skip)]
    draw: Option<Entity>,
    /// Blobs in the drawn mesh.
    #[serde(skip)]
    drawn: usize,
}

impl portal2_core::save::Restore for PaintSprayer {
    fn keep(&mut self, live: Self) {
        self.lighting = live.lighting;
        self.draw = live.draw;
        self.drawn = live.drawn;
    }
}

pub fn register_save(r: &mut portal2_core::save::SaveRegistry) {
    r.component::<PaintSprayer>("paint_sprayer");
    r.custom_resource("paint_map", save_paint, load_paint);
}

fn save_paint(world: &World) -> Option<Result<Vec<u8>, String>> {
    world.get_resource::<PaintMap>().map(|m| portal2_core::save::encode(&m.snapshot()))
}

fn load_paint(world: &mut World, bytes: &[u8]) -> Result<(), String> {
    let saved: Vec<(usize, Vec<u8>)> = portal2_core::save::decode(bytes)?;
    if let Some(mut map) = world.get_resource_mut::<PaintMap>() {
        map.restore(&saved);
    }
    world.resource_mut::<Blobs>().list.clear();
    Ok(())
}

pub struct Blob {
    pub pos: Vec3,
    pub vel: Vec3,
    pub paint: PaintType,
    sprayer: Entity,
    start_radius: f32,
    end_radius: f32,
    grow_time: f32,
    age: f32,
    streak_time: f32,
    streak_dampen: f32,
    /// Sliding along a surface (its inward normal).
    streak_normal: Option<Vec3>,
    last_paint: Vec3,
    draw_only: bool,
    silent: bool,
}

impl Blob {
    pub fn radius(&self) -> f32 {
        let t = if self.grow_time > 0.0 { (self.age / self.grow_time).min(1.0) } else { 1.0 };
        self.start_radius + (self.end_radius - self.start_radius) * t
    }
}

/// Live blobs of the map.
#[derive(Resource, Default)]
pub struct Blobs {
    pub list: Vec<Blob>,
    noise_budget: f32,
    /// Each sprayer's recent splash effects: where, and how long they
    /// still count for grouping.
    splashes: HashMap<Entity, Vec<(Vec3, f32)>>,
}

/// The splash effect a blob of `paint` makes where it lands
/// (`paint_splat_bounce_01`, `_stick_01`, `_speed_01`; portal paint and
/// erasers use `paint_splat_erase_01`).
fn splat_effect(paint: PaintType) -> &'static str {
    match paint {
        PaintType::Bounce => "paint_splat_bounce_01",
        PaintType::Reflect => "paint_splat_stick_01",
        PaintType::Speed => "paint_splat_speed_01",
        PaintType::Portal | PaintType::Erase => "paint_splat_erase_01",
    }
}

/// Whether a landing blob gets its own splash (the client's grouping,
/// `cl_group_paint_impact_effects`): always while its sprayer has fewer than
/// `paint_min_impact_particles` recent splashes, never at
/// `paint_max_impact_particles`, and in between only at least
/// `paint_impact_particles_distance_threshold` from each recent one.
fn splash_allowed(recent: &[(Vec3, f32)], at: Vec3, console: &Console) -> bool {
    if !console.bool("cl_group_paint_impact_effects") {
        return true;
    }
    let n = recent.len() as i32;
    if n < console.i32("paint_min_impact_particles") {
        return true;
    }
    if n >= console.i32("paint_max_impact_particles") {
        return false;
    }
    let d = console.f32("paint_impact_particles_distance_threshold");
    recent.iter().all(|(p, _)| p.distance_squared(at) >= d * d)
}

fn clear_on_map_load(mut loaded: MessageReader<portal2_world::MapLoaded>, mut blobs: ResMut<Blobs>) {
    if loaded.read().last().is_some() {
        blobs.splashes.clear();
        blobs.list.clear();
    }
}

/// Light a group of blobs is drawn in: the ambient cube and strongest few
/// world lights at a point, as for a model.
#[derive(Clone, Copy)]
struct BlobLighting {
    ambient: [Vec3; 6],
    lights: [ModelLight; MAX_MODEL_LIGHTS],
    count: usize,
}

impl Default for BlobLighting {
    fn default() -> Self {
        Self { ambient: [Vec3::splat(0.2); 6], lights: [ModelLight::default(); MAX_MODEL_LIGHTS], count: 0 }
    }
}

impl BlobLighting {
    fn at(bsp: &source_formats::bsp::Bsp, p: Vec3) -> Self {
        let m = portal2_render::models::lighting::compute(bsp, p);
        let mut lights = [ModelLight::default(); MAX_MODEL_LIGHTS];
        let count = m.lights.len().min(MAX_MODEL_LIGHTS);
        lights[..count].copy_from_slice(&m.lights[..count]);
        Self { ambient: m.ambient, lights, count }
    }
}

fn spawn_sprayer(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e).cloned() else { return };
    // The game lights a sprayer's blobs where its `light_position_name`
    // entity is (the client's lighting origin, `m_vLightPosition`), at the
    // nozzle without one.
    let light_at = me
        .get("light_position_name")
        .filter(|n| !n.is_empty())
        .and_then(|n| world.resource::<portal2_core::entity::EntityRegistry>().first_named(n))
        .and_then(|t| world.get::<MapEntity>(t).map(|m| m.origin()))
        .unwrap_or_else(|| me.origin());
    let lighting = world.get_resource::<CurrentMap>().map(|m| BlobLighting::at(&m.bsp, light_at)).unwrap_or_default();
    world.entity_mut(e).insert(PaintSprayer {
        active: me.get_bool("start_active"),
        paint: PaintType::from_index(me.get_i32("painttype").unwrap_or(0)).unwrap_or(PaintType::Bounce),
        blobs_per_second: me.get_f32("blobs_per_second").unwrap_or(1.0),
        speed: Range::of(&me, "min_speed", "max_speed", 100.0),
        spread_angle: me.get_f32("blob_spread_angle").unwrap_or(0.0).clamp(0.0, 89.0),
        spread_radius: me.get_f32("blob_spread_radius").unwrap_or(0.0).max(0.0),
        streak_percentage: me.get_f32("blob_streak_percentage").unwrap_or(0.0),
        streak_time: Range::of(&me, "min_streak_time", "max_streak_time", 0.2),
        streak_dampen: Range::of(&me, "min_streak_speed_dampen", "max_streak_speed_dampen", 500.0),
        start_radius: Range::of(&me, "start_radius_min", "start_radius_max", 0.5),
        end_radius: Range::of(&me, "end_radius_min", "end_radius_max", 0.7),
        grow_time: Range::of(&me, "radius_grow_time_min", "radius_grow_time_max", 1.0),
        max_blobs: me.get_i32("maxblobcount").unwrap_or(250).max(0) as usize,
        silent: me.get_bool("silent"),
        draw_only: me.get_bool("drawonly"),
        accumulated: 0.0,
        rng: Rng(0x9e37_79b9 ^ (me.index as u32).wrapping_mul(0x85eb_ca6b) | 1),
        lighting,
        draw: None,
        drawn: 0,
    });
}

fn sprayer_input(world: &mut World, call: &InputCall) -> bool {
    let Some(mut s) = world.get_mut::<PaintSprayer>(call.target) else { return false };
    match call.input.to_ascii_lowercase().as_str() {
        "start" => s.active = true,
        "stop" => s.active = false,
        "changepainttype" => {
            if let Some(t) = PaintType::from_index(source_formats::kv::parse_i32_lenient(&call.param)) {
                s.paint = t;
            }
        }
        _ => return false,
    }
    true
}

/// `paint_sphere`'s `Paint`: paints its sphere at once (`radius`, coat
/// `alpha_percent`, `paint_type`) on the world and the brush entities it
/// reaches.
fn paint_sphere_input(world: &mut World, call: &InputCall) -> bool {
    if !call.input.eq_ignore_ascii_case("paint") {
        return false;
    }
    let Some(me) = world.get::<MapEntity>(call.target).cloned() else { return false };
    let center = world.get::<GlobalTransform>(call.target).map(|g| g.translation()).unwrap_or_else(|| me.origin());
    let radius = me.get_f32("radius").unwrap_or(60.0);
    let coat = me.get_f32("alpha_percent").unwrap_or(1.0);
    let paint = PaintType::from_index(me.get_i32("paint_type").unwrap_or(0)).unwrap_or(PaintType::Bounce);
    let settings = SplatSettings::from_console(world.resource::<Console>());
    let mut brushes = world.query::<(&MapEntity, &GlobalTransform)>();
    let models: Vec<(usize, Affine3A)> =
        brushes.iter(world).filter_map(|(me, gt)| me.brush_model().filter(|&m| m > 0).map(|m| (m, gt.affine()))).collect();
    let bsp = world.get_resource::<CurrentMap>().map(|m| m.bsp.clone());
    let Some(mut map) = world.get_resource_mut::<PaintMap>() else { return true };
    map.paint_sphere(0, center, radius, coat, paint, &settings);
    if let Some(bsp) = bsp {
        for (model, affine) in models {
            let Some(bm) = bsp.models.get(model) else { continue };
            let local = affine.inverse().transform_point3(center);
            if local.clamp(bm.mins, bm.maxs).distance_squared(local) <= radius * radius {
                map.paint_sphere(model, local, radius, coat, paint, &settings);
            }
        }
    }
    true
}

/// Any brush entity's `RemovePaint`: all paint on its model goes
/// (CBaseEntity::InputRemovePaint, the engine's `RemoveAllPaint`).
/// sp_a3_crazy_box's cleansing gel floor is washed this way when the gel
/// comes back on.
fn remove_paint_input(world: &mut World, call: &InputCall) -> bool {
    if call.input != "removepaint" {
        return false;
    }
    let Some(model) = world.get::<MapEntity>(call.target).and_then(|m| m.brush_model()).filter(|&m| m > 0) else { return true };
    if let Some(mut map) = world.get_resource_mut::<PaintMap>() {
        map.clear_model(model);
    }
    true
}

/// Emits a blob every `1 / blobs_per_second` while active and under the
/// sprayer's and the map's blob limits.
fn spray(mut sprayers: Query<(Entity, &mut PaintSprayer, &GlobalTransform)>, mut blobs: ResMut<Blobs>, map: Option<Res<PaintMap>>) {
    let Some(map) = map else { return };
    for (e, mut s, gt) in &mut sprayers {
        if !s.active || s.blobs_per_second <= 0.0 {
            s.accumulated = 0.0;
            continue;
        }
        s.accumulated += TICK;
        let interval = 1.0 / s.blobs_per_second;
        while s.accumulated >= interval {
            s.accumulated -= interval;
            let mine = blobs.list.iter().filter(|b| b.sprayer == e).count();
            if mine >= s.max_blobs || blobs.list.len() >= map.max_blobs {
                continue;
            }
            let blob = new_blob(&mut s, e, gt);
            blobs.list.push(blob);
        }
    }
}

fn new_blob(s: &mut PaintSprayer, e: Entity, gt: &GlobalTransform) -> Blob {
    let rot = gt.rotation();
    let (forward, left, up) = (rot * Vec3::X, rot * Vec3::Y, rot * Vec3::Z);
    let origin = gt.translation();
    let mut rng = s.rng;
    // A point on the nozzle's disc, aimed away from a cone apex behind it.
    let r = s.spread_radius * rng.next();
    let theta = rng.next() * std::f32::consts::TAU;
    let offset = (left * theta.cos() + up * theta.sin()) * r;
    let pos = origin + offset;
    let angle = s.spread_angle.to_radians();
    let dir = if angle > 0.0 && s.spread_radius > 0.0 {
        let apex = origin - forward * (s.spread_radius / angle.tan());
        (pos - apex).normalize_or(forward)
    } else if angle > 0.0 {
        let spread = angle * rng.next();
        let around = rng.next() * std::f32::consts::TAU;
        (forward * spread.cos() + (left * around.cos() + up * around.sin()) * spread.sin()).normalize()
    } else {
        forward
    };
    let speed = s.speed.pick(&mut rng);
    let streaks = s.streak_percentage > 0.0 && s.streak_percentage >= rng.next() * 100.0;
    let blob = Blob {
        pos,
        vel: dir * speed,
        paint: s.paint,
        sprayer: e,
        start_radius: s.start_radius.pick(&mut rng),
        end_radius: s.end_radius.pick(&mut rng),
        grow_time: s.grow_time.pick(&mut rng),
        age: 0.0,
        streak_time: if streaks { s.streak_time.pick(&mut rng) } else { 0.0 },
        streak_dampen: s.streak_dampen.pick(&mut rng),
        streak_normal: None,
        last_paint: Vec3::splat(f32::MAX),
        draw_only: s.draw_only,
        silent: s.silent,
    };
    s.rng = rng;
    blob
}

/// Where a blob paints: a sphere just off the surface, on the world and
/// on every brush entity it reaches.
pub(super) fn paint_impact(
    map: &mut PaintMap,
    brushes: &Query<(&MapEntity, &GlobalTransform)>,
    bsp: Option<&source_formats::bsp::Bsp>,
    point: Vec3,
    normal: Vec3,
    paint: PaintType,
    settings: &SplatSettings,
    radius: f32,
    coat: f32,
) {
    let center = point + normal * 0.1;
    map.paint_sphere(0, center, radius, coat, paint, settings);
    let Some(bsp) = bsp else { return };
    for (me, gt) in brushes {
        let Some(model) = me.brush_model().filter(|&m| m > 0) else { continue };
        let Some(bm) = bsp.models.get(model) else { continue };
        let local = gt.affine().inverse().transform_point3(center);
        if local.clamp(bm.mins, bm.maxs).distance_squared(local) > radius * radius {
            continue;
        }
        map.paint_sphere(model, local, radius, coat, paint, settings);
    }
}

fn move_blobs(
    mut commands: Commands,
    mut blobs: ResMut<Blobs>,
    map: Option<ResMut<PaintMap>>,
    physics: Physics,
    console: Res<Console>,
    current: Option<Res<CurrentMap>>,
    brushes: Query<(&MapEntity, &GlobalTransform)>,
    (mut sounds, mut painted): (MessageWriter<SoundRequest>, MessageWriter<super::props::PaintEntity>),
    (portals, portal_query): (Res<crate::portals::Portals>, Query<&crate::portals::Portal>),
) {
    let Some(mut map) = map else {
        blobs.list.clear();
        return;
    };
    if blobs.list.is_empty() {
        return;
    }
    let gravity = console.f32("sv_gravity") * console.f32("paintblob_gravity_scale");
    let drag = console.f32("paintblob_air_drag");
    let streak_cos = console.f32("paintblob_streak_angle_threshold").to_radians().cos();
    let settings = SplatSettings::from_console(&console);
    let coat = console.f32("sv_paint_alpha_coat");
    let paint_radius = console.f32("sv_paint_surface_sphere_radius");
    let erase_radius = console.f32("sv_erase_surface_sphere_radius");
    let bsp = current.as_ref().map(|m| m.bsp.as_ref());
    let frames = crate::portals::linked_frames(&portals, &portal_query);
    blobs.noise_budget = (blobs.noise_budget + NOISY_PER_SECOND * TICK).min(NOISY_PER_SECOND);
    let mut noise_budget = blobs.noise_budget;
    let mut splashes = std::mem::take(&mut blobs.splashes);
    for recent in splashes.values_mut() {
        recent.retain_mut(|(_, left)| {
            *left -= TICK;
            *left > 0.0
        });
    }
    splashes.retain(|_, recent| !recent.is_empty());
    let draw_splashes = console.bool("draw_paint_splat_particles");
    let splash_time = console.f32("paint_impact_particles_duration");
    let mut list = std::mem::take(&mut blobs.list);
    list.retain_mut(|b| {
        b.age += TICK;
        if b.age > MAX_LIFETIME || b.pos.z < KILL_DEPTH {
            return false;
        }
        let radius = if b.paint == PaintType::Erase { erase_radius } else { paint_radius };
        if let Some(inward) = b.streak_normal {
            // Sliding: slow down, keep to the surface, paint as it goes.
            b.streak_time -= TICK;
            let speed = b.vel.length() - b.streak_dampen * TICK;
            if b.streak_time <= 0.0 || speed <= 0.0 {
                return false;
            }
            b.vel = b.vel.normalize_or_zero() * speed;
            let next = b.pos + b.vel * TICK;
            let tr = physics.trace(next, next + inward * 20.0, Vec3::ZERO, Vec3::ZERO, mask::SOLID_BRUSH_ONLY, &[]);
            if !tr.hit() {
                return false;
            }
            b.pos = tr.end - inward * 0.5;
            if !b.draw_only && b.last_paint.distance_squared(tr.end) > 25.0 {
                b.last_paint = tr.end;
                paint_impact(&mut map, &brushes, bsp, tr.end, tr.normal, b.paint, &settings, radius, coat);
            }
            return true;
        }
        let v0 = b.vel;
        let v1 = v0 - Vec3::Z * gravity * TICK;
        let next = b.pos + (v0 + v1) * 0.5 * TICK;
        b.vel = v1 * (1.0 - drag * TICK);
        // Into a linked portal's opening: on out of the other one.
        if let Some(frames) = &frames
            && let Some(slot) = (0..2).find(|&slot| {
                let f = &frames[slot];
                let (lb, la) = (f.to_local(b.pos), f.to_local(next));
                lb.x >= 0.0 && la.x < 0.0 && f.in_opening(lb.lerp(la, lb.x / (lb.x - la.x)), 0.0)
            })
        {
            // Nothing in the way up to just in front of the opening.
            let f = &frames[slot];
            let (lb, la) = (f.to_local(b.pos), f.to_local(next));
            let crossing = b.pos.lerp(next, lb.x / (lb.x - la.x)) + f.forward * 0.5;
            if !physics.trace(b.pos, crossing, Vec3::ZERO, Vec3::ZERO, mask::SOLID_ALL, &[]).hit() {
                let m = transfer(f, &frames[1 - slot]);
                b.pos = m.transform_point3(next);
                b.vel = m.transform_vector3(b.vel);
                trace!("paint blob went through portal {slot}: now at {:.0?} moving {:.0?}", b.pos, b.vel);
                return true;
            }
        }
        let tr = physics.trace(b.pos, next, Vec3::ZERO, Vec3::ZERO, mask::SOLID_ALL, &[]);
        if !tr.hit() {
            b.pos = next;
            return true;
        }
        let n = tr.normal;
        // Brushes and the world take paint on their faces; anything else
        // (props) is painted as a whole.
        let on_entity = tr.entity.filter(|&e| brushes.get(e).map(|(me, _)| me.brush_model().is_none()).unwrap_or(true));
        if !b.draw_only {
            match on_entity {
                Some(entity) => {
                    painted.write(super::props::PaintEntity { entity, paint: b.paint });
                }
                None => paint_impact(&mut map, &brushes, bsp, tr.end, n, b.paint, &settings, radius, coat),
            }
        }
        if !b.silent && noise_budget >= 1.0 {
            noise_budget -= 1.0;
            sounds.write(SoundRequest::at("PaintBlob.Impact", tr.end));
        }
        let recent = splashes.entry(b.sprayer).or_default();
        if splash_allowed(recent, tr.end, &console) {
            if draw_splashes {
                let at = Transform::from_translation(tr.end).with_rotation(Quat::from_rotation_arc(Vec3::X, -n));
                portal2_particles::spawn_effect(&mut commands, splat_effect(b.paint), at, true);
            }
            recent.push((tr.end, splash_time));
        }
        // Streak along walls, or along anything hit at a glancing angle.
        let tangent = b.vel - n * b.vel.dot(n);
        let glancing = b.vel.normalize_or_zero().dot(tangent.normalize_or_zero()) >= streak_cos;
        if b.streak_time > 0.0 && (n.z.abs() < 0.5 || glancing) {
            b.vel = tangent;
            b.pos = tr.end + n * 0.5;
            b.streak_normal = Some(-n);
            b.last_paint = tr.end;
            return true;
        }
        false
    });
    blobs.noise_budget = noise_budget;
    blobs.splashes = splashes;
    blobs.list = list;
}

#[derive(Clone, Copy, Debug, Default, ShaderType)]
pub struct BlobParams {
    /// Ambient light from +X, -X, +Y, -Y, +Z, -Z.
    pub ambient: [Vec4; 6],
    /// The linked portals, three rows each: centre (w 1 when linked),
    /// forward (w half width), up (w half height). The shader drops what
    /// lies behind an opening.
    pub portals: [Vec4; 6],
    pub lights: [ModelLight; MAX_MODEL_LIGHTS],
    /// x: lights in use.
    pub light_count: Vec4,
}

impl BlobParams {
    fn new(lighting: &BlobLighting, portals: [Vec4; 6]) -> Self {
        Self {
            ambient: lighting.ambient.map(|a| a.extend(1.0)),
            portals,
            lights: lighting.lights,
            light_count: Vec4::new(lighting.count as f32, 0.0, 0.0, 0.0),
        }
    }
}

/// Paint blobs (`paintblob`); each paint's base colour and opacity come
/// from the vertices.
#[derive(Asset, TypePath, AsBindGroup, Clone, Debug)]
pub struct PaintBlobMaterial {
    #[uniform(0)]
    pub params: BlobParams,
    #[texture(1, dimension = "cube")]
    #[sampler(2)]
    pub envmap: Option<Handle<Image>>,
    #[texture(3)]
    #[sampler(4)]
    pub normal_map: Option<Handle<Image>>,
}

/// The textures every blob material shares.
#[derive(Clone)]
struct BlobTextures {
    envmap: Option<Handle<Image>>,
    normal_map: Option<Handle<Image>>,
}

impl BlobTextures {
    fn material(&self, params: BlobParams) -> PaintBlobMaterial {
        PaintBlobMaterial { params, envmap: self.envmap.clone(), normal_map: self.normal_map.clone() }
    }
}

impl Material for PaintBlobMaterial {
    fn fragment_shader() -> ShaderRef {
        BLOB_SHADER.into()
    }

    /// Erase paint shows the scene behind it, which puts blobs in the
    /// transmissive pass after a copy of the opaque one.
    fn reads_view_transmission_texture(&self) -> bool {
        true
    }

    fn enable_prepass() -> bool {
        false
    }

    fn enable_shadows() -> bool {
        false
    }
}

fn surface_mesh(particles: &[super::surface::Particle], params: &super::surface::SurfaceParams) -> Mesh {
    let m = super::surface::build(particles, params);
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, m.positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, m.normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, m.colors);
    mesh.insert_indices(Indices::U32(m.indices));
    mesh
}

/// Erase blobs' base colour: blob_surface_erase.vtf, a flat 4x4 texture
/// (`erase_color` is for painted surfaces and is fully clear).
const ERASE_BLOB_COLOR: [u8; 3] = [191, 198, 211];

/// Each paint type's blob colour (linear rgb; alpha 1 for opaque paint, 0
/// for erase paint, which `paint_blob.wesl` draws as water), read from the
/// console once a frame rather than for every blob.
struct Palette([[f32; 4]; 5]);

impl Palette {
    fn new(console: &Console) -> Self {
        Self([PaintType::Bounce, PaintType::Reflect, PaintType::Speed, PaintType::Portal, PaintType::Erase].map(|t| {
            let c = if t == PaintType::Erase { ERASE_BLOB_COLOR } else { t.color(console)[..3].try_into().unwrap() };
            let rgb = Color::srgb_u8(c[0], c[1], c[2]).to_linear();
            [rgb.red, rgb.green, rgb.blue, if t == PaintType::Erase { 0.0 } else { 1.0 }]
        }))
    }

    fn get(&self, paint: PaintType) -> [f32; 4] {
        self.0[paint as usize]
    }
}

/// A blob for the surface; a streaking blob thins to
/// `paintblob_radius_while_streaking`.
fn blob_particle(b: &Blob, palette: &Palette, streak_radius: f32) -> super::surface::Particle {
    let radius = if b.streak_normal.is_some() { b.radius().min(streak_radius) } else { b.radius() };
    super::surface::Particle { center: b.pos, radius, color: palette.get(b.paint) }
}

/// The blobs a paint bomb is drawn with (C_PropPaintBomb): they start on a
/// sphere of `paintbomb_draw_sphere_radius` around the bomb, heading inward
/// at `paintbomb_draw_blob_speed_min`..`max`, ride along with it, and turn
/// back whenever they leave the sphere moving outward, stepped
/// `paintblob_update_per_second` times a second. Their radii are
/// `paintbomb_draw_min_blob_radius`..`max`.
struct BombBlobs {
    last_center: Vec3,
    blobs: Vec<(Vec3, Vec3, f32)>,
    pending: f32,
}

impl BombBlobs {
    fn new(center: Vec3, seed: u32, console: &Console) -> Self {
        let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9) | 1);
        let n = console.i32("paintbomb_draw_num_paint_blobs").max(0) as usize;
        let sphere = console.f32("paintbomb_draw_sphere_radius");
        let speed = Range { min: console.f32("paintbomb_draw_blob_speed_min"), max: console.f32("paintbomb_draw_blob_speed_max") };
        let radius = Range { min: console.f32("paintbomb_draw_min_blob_radius"), max: console.f32("paintbomb_draw_max_blob_radius") };
        let blobs = (0..n)
            .map(|_| {
                let d = Vec3::new(rng.next() * 2.0 - 1.0, rng.next() * 2.0 - 1.0, rng.next() * 2.0 - 1.0).normalize_or(Vec3::Z);
                (center + d * sphere, -d * speed.pick(&mut rng), radius.pick(&mut rng))
            })
            .collect();
        Self { last_center: center, blobs, pending: 0.0 }
    }

    fn advance(&mut self, center: Vec3, dt: f32, console: &Console) {
        let step = 1.0 / console.f32("paintblob_update_per_second").max(1.0);
        let sphere2 = console.f32("paintbomb_draw_sphere_radius").powi(2);
        self.pending += dt;
        while self.pending > 0.0 {
            let h = step.min(self.pending);
            let moved = center - self.last_center;
            for (p, v, _) in &mut self.blobs {
                *p += moved;
                if p.distance_squared(center) > sphere2 && v.dot(center - *p) < 0.0 {
                    *v = -*v;
                }
                *p += *v * h;
            }
            self.last_center = center;
            self.pending -= h;
        }
    }
}

/// A drawn group of blobs: its entity and how many it showed last.
#[derive(Default)]
struct DrawSlot {
    entity: Option<Entity>,
    drawn: usize,
}

/// Blobs no sprayer owns (bomb splashes) and the bomb balls, drawn
/// together, lit where the first of them is.
#[derive(Default)]
struct FreeDraw {
    slot: DrawSlot,
    material: Option<Handle<PaintBlobMaterial>>,
    relight_in: u32,
}

/// Mesh and material stores the slots draw into.
struct DrawTargets<'a> {
    meshes: &'a mut Assets<Mesh>,
    materials: &'a mut Assets<PaintBlobMaterial>,
}

impl DrawTargets<'_> {
    /// Puts a group's surface (built from `count` blobs) into the slot's
    /// mesh, spawning its entity on first use.
    fn show(
        &mut self,
        slot: &mut DrawSlot,
        mesh: Option<Mesh>,
        count: usize,
        commands: &mut Commands,
        handles: &Query<&Mesh3d>,
        make_material: impl FnOnce(&mut Assets<PaintBlobMaterial>) -> Handle<PaintBlobMaterial>,
    ) {
        let Some(mesh) = mesh else { return };
        match slot.entity.and_then(|d| handles.get(d).ok()) {
            Some(handle) => {
                if let Some(mut m) = self.meshes.get_mut(&handle.0) {
                    *m = mesh;
                }
            }
            None => {
                if count == 0 {
                    return;
                }
                let material = make_material(self.materials);
                let id = commands
                    .spawn((
                        Name::new("paint blobs"),
                        portal2_world::MapObject,
                        Mesh3d(self.meshes.add(mesh)),
                        MeshMaterial3d(material),
                        Transform::IDENTITY,
                        bevy::camera::visibility::NoFrustumCulling,
                    ))
                    .id();
                slot.entity = Some(id);
            }
        }
        slot.drawn = count;
    }
}

fn portal_rows(frames: Option<&[PortalFrame; 2]>) -> [Vec4; 6] {
    let mut rows = [Vec4::ZERO; 6];
    if let Some(frames) = frames {
        for (i, f) in frames.iter().enumerate() {
            rows[i * 3] = f.center.extend(1.0);
            rows[i * 3 + 1] = f.forward.extend(HALF_WIDTH);
            rows[i * 3 + 2] = f.up.extend(HALF_HEIGHT);
        }
    }
    rows
}

/// Blobs over a linked portal's opening, near its plane, copied through to
/// the other portal. With the shader dropping what lies behind each
/// opening, the surface flows out of the exit as it goes into the entrance
/// (the game draws models partly through a portal the same way, see
/// `crate::portals::ghosts`), instead of jumping across when a blob or bomb
/// is moved over.
fn through_portals(particles: &mut Vec<super::surface::Particle>, frames: &[PortalFrame; 2], surface: &super::surface::SurfaceParams) {
    let n = particles.len();
    for slot in 0..2 {
        let (from, to) = (frames[slot], frames[1 - slot]);
        let m = transfer(&from, &to);
        for i in 0..n {
            let p = particles[i];
            let reach = p.radius.clamp(0.001, 1.0) * surface.box_width * surface.render_radius * 2.2 + surface.cell;
            let l = from.to_local(p.center);
            if l.x < reach && l.x > -reach && l.y.abs() < HALF_WIDTH && l.z.abs() < HALF_HEIGHT {
                particles.push(super::surface::Particle { center: m.transform_point3(p.center), ..p });
            }
        }
    }
}

/// One mesh per sprayer holding its blobs, and one for everything else,
/// rebuilt every frame.
fn draw_blobs(
    mut commands: Commands,
    blobs: Res<Blobs>,
    mut sprayers: Query<(Entity, &mut PaintSprayer)>,
    bombs: Query<(Entity, &super::bomb::PaintBomb, &GlobalTransform)>,
    (mut meshes, mut materials, handles, material_handles): (
        ResMut<Assets<Mesh>>,
        ResMut<Assets<PaintBlobMaterial>>,
        Query<&Mesh3d>,
        Query<&MeshMaterial3d<PaintBlobMaterial>>,
    ),
    (console, time, current): (Res<Console>, Res<Time>, Option<Res<CurrentMap>>),
    (fs, mut textures, mut images): (Res<GameFs>, ResMut<TextureCache>, ResMut<Assets<Image>>),
    (mut bomb_blobs, mut free): (Local<HashMap<Entity, BombBlobs>>, Local<FreeDraw>),
    (portals, portal_query): (Res<crate::portals::Portals>, Query<&crate::portals::Portal>),
) {
    let frames = crate::portals::linked_frames(&portals, &portal_query);
    let rows = portal_rows(frames.as_ref());
    let shared = BlobTextures {
        envmap: textures.load(&fs, &mut images, "materials/paint/paint_envmap_hdr.vtf", false).map(|t| t.handle),
        normal_map: textures.load(&fs, &mut images, "materials/paintblobs/blob_surface_normal.vtf", false).map(|t| t.handle),
    };
    let surface = super::surface::SurfaceParams::from_console(&console);
    let palette = Palette::new(&console);
    let streak_radius = console.f32("paintblob_radius_while_streaking");
    // Each sprayer's blobs, then everything else (bomb splashes, bombs).
    let mut owned = std::collections::HashSet::new();
    let mut groups: Vec<(Vec<super::surface::Particle>, usize)> = Vec::new();
    for (e, s) in &sprayers {
        owned.insert(e);
        groups.push((blobs.list.iter().filter(|b| b.sprayer == e).map(|b| blob_particle(b, &palette, streak_radius)).collect(), s.drawn));
    }
    let mut spheres: Vec<super::surface::Particle> =
        blobs.list.iter().filter(|b| !owned.contains(&b.sprayer)).map(|b| blob_particle(b, &palette, streak_radius)).collect();
    let dt = time.delta_secs();
    bomb_blobs.retain(|e, _| bombs.contains(*e));
    if console.bool("cl_draw_paint_bomb_with_blobs") {
        for (e, bomb, gt) in &bombs {
            let center = gt.translation();
            let state = bomb_blobs.entry(e).or_insert_with(|| BombBlobs::new(center, e.index_u32(), &console));
            state.advance(center, dt, &console);
            let color = palette.get(bomb.paint);
            spheres.extend(state.blobs.iter().map(|&(p, _, r)| super::surface::Particle { center: p, radius: r, color }));
        }
    }
    groups.push((spheres, free.slot.drawn));
    if let Some(frames) = &frames {
        for (particles, _) in &mut groups {
            through_portals(particles, frames, &surface);
        }
    }
    // The surfaces are built side by side; a group with nothing to show,
    // and nothing shown before, is skipped.
    let built: Vec<Option<Mesh>> = bevy::tasks::ComputeTaskPool::get().scope(|scope| {
        for (particles, drawn) in &groups {
            scope.spawn(async move { (!particles.is_empty() || *drawn > 0).then(|| surface_mesh(particles, &surface)) });
        }
    });
    let mut built = built.into_iter();
    let mut counts = groups.iter().map(|(p, _)| p.len());
    let mut targets = DrawTargets { meshes: &mut meshes, materials: &mut materials };
    for (_, mut s) in &mut sprayers {
        let (mesh, count) = (built.next().flatten(), counts.next().unwrap_or(0));
        let lighting = s.lighting;
        let mut slot = DrawSlot { entity: s.draw, drawn: s.drawn };
        targets.show(&mut slot, mesh, count, &mut commands, &handles, |m| m.add(shared.material(BlobParams::new(&lighting, rows))));
        set_portals(&mut targets, slot.entity, &material_handles, rows);
        s.draw = slot.entity;
        s.drawn = slot.drawn;
    }
    let (mesh, count) = (built.next().flatten(), counts.next().unwrap_or(0));
    // Light the free group where it is, now and then.
    let mut relight = None;
    if free.relight_in == 0
        && let Some(first) = groups.last().and_then(|(p, _)| p.first())
        && let Some(map) = current.as_ref()
    {
        relight = Some(BlobLighting::at(&map.bsp, first.center));
        free.relight_in = 30;
    }
    free.relight_in = free.relight_in.saturating_sub(1);
    if let (Some(lighting), Some(m)) = (relight.as_ref(), free.material.as_ref())
        && let Some(mut mat) = targets.materials.get_mut(m)
    {
        mat.params = BlobParams::new(lighting, rows);
    }
    let FreeDraw { slot, material, .. } = &mut *free;
    targets.show(slot, mesh, count, &mut commands, &handles, |m| {
        let h = m.add(shared.material(BlobParams::new(&relight.unwrap_or_default(), rows)));
        *material = Some(h.clone());
        h
    });
    set_portals(&mut targets, slot.entity, &material_handles, rows);
}

/// Keeps a group's material told where the portals are.
fn set_portals(targets: &mut DrawTargets, entity: Option<Entity>, handles: &Query<&MeshMaterial3d<PaintBlobMaterial>>, rows: [Vec4; 6]) {
    let Some(handle) = entity.and_then(|e| handles.get(e).ok()) else { return };
    if targets.materials.get(&handle.0).is_some_and(|m| m.params.portals != rows)
        && let Some(mut m) = targets.materials.get_mut(&handle.0)
    {
        m.params.portals = rows;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn radius_grows_to_end_radius() {
        let b = Blob {
            pos: Vec3::ZERO,
            vel: Vec3::ZERO,
            paint: PaintType::Bounce,
            sprayer: Entity::PLACEHOLDER,
            start_radius: 0.5,
            end_radius: 0.7,
            grow_time: 1.0,
            age: 0.5,
            streak_time: 0.0,
            streak_dampen: 0.0,
            streak_normal: None,
            last_paint: Vec3::ZERO,
            draw_only: false,
            silent: true,
        };
        assert!((b.radius() - 0.6).abs() < 1e-6);
    }
}
