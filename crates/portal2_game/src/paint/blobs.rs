//! Paint sprayers (`info_paint_sprayer`) and the blobs they shoot. Blobs
//! fall under gravity with a little drag, 60 steps a second, and paint a
//! sphere where they hit (the world and any brush entity it reaches), or
//! slide along the surface first when they streak. Each sprayer draws its
//! live blobs as small spheres lit by the ambient light at the nozzle.
//! Rules from the game's (docs/re/paint.md).

use bevy::asset::RenderAssetUsages;
use bevy::math::Affine3A;
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
use portal2_render::textures::TextureCache;
use portal2_world::CurrentMap;
use portal2_world::collision::{Physics, mask};
use portal2_world::paint_map::{PaintMap, PaintType, SplatSettings};

pub const BLOB_SHADER: &str = "embedded://portal2_game/paint/paint_blob.wesl";
const TICK: f32 = 1.0 / 60.0;
/// Drawn radius of a blob per unit of its `radius` (the game renders blobs
/// as one merged liquid surface; spheres of this size stand in).
const DRAW_SCALE: f32 = 8.0;
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
            .add_systems(PostUpdate, draw_blobs);
        let world = app.world_mut();
        world.resource_mut::<SpawnHandlers>().register("info_paint_sprayer", spawn_sprayer);
        world.resource_mut::<InputHandlers>().register("info_paint_sprayer", sprayer_input);
        world.resource_mut::<InputHandlers>().register("paint_sphere", paint_sphere_input);
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
    /// Ambient cube at the nozzle (+X, -X, +Y, -Y, +Z, -Z) for drawing.
    #[serde(skip)]
    ambient: [Vec3; 6],
    #[serde(skip)]
    draw: Option<Entity>,
    /// Blobs in the drawn mesh.
    #[serde(skip)]
    drawn: usize,
}

impl portal2_core::save::Restore for PaintSprayer {
    fn keep(&mut self, live: Self) {
        self.ambient = live.ambient;
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
    /// A short streaking blob of a paint bomb's splash; it only draws.
    pub fn splash(pos: Vec3, vel: Vec3, paint: PaintType, owner: Entity) -> Self {
        Self {
            pos,
            vel,
            paint,
            sprayer: owner,
            start_radius: 0.7,
            end_radius: 0.5,
            grow_time: 0.3,
            age: 0.0,
            streak_time: 0.15,
            streak_dampen: 650.0,
            streak_normal: None,
            last_paint: Vec3::splat(f32::MAX),
            draw_only: true,
            silent: true,
        }
    }

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
}

fn clear_on_map_load(mut loaded: MessageReader<portal2_world::MapLoaded>, mut blobs: ResMut<Blobs>) {
    if loaded.read().last().is_some() {
        blobs.list.clear();
    }
}

fn spawn_sprayer(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e).cloned() else { return };
    let ambient = world
        .get_resource::<CurrentMap>()
        .map(|m| portal2_render::models::lighting::compute(&m.bsp, me.origin()).ambient)
        .unwrap_or([Vec3::splat(0.2); 6]);
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
        ambient,
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
fn paint_impact(
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
    mut blobs: ResMut<Blobs>,
    map: Option<ResMut<PaintMap>>,
    physics: Physics,
    console: Res<Console>,
    current: Option<Res<CurrentMap>>,
    brushes: Query<(&MapEntity, &GlobalTransform)>,
    (mut sounds, mut painted): (MessageWriter<SoundRequest>, MessageWriter<super::props::PaintEntity>),
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
    blobs.noise_budget = (blobs.noise_budget + NOISY_PER_SECOND * TICK).min(NOISY_PER_SECOND);
    let mut noise_budget = blobs.noise_budget;
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
    blobs.list = list;
}

#[derive(Clone, Copy, Debug, Default, ShaderType)]
pub struct BlobParams {
    /// Ambient light from +X, -X, +Y, -Y, +Z, -Z.
    pub ambient: [Vec4; 6],
}

/// Shiny paint blobs; colours come from the vertices.
#[derive(Asset, TypePath, AsBindGroup, Clone, Debug)]
pub struct PaintBlobMaterial {
    #[uniform(0)]
    pub params: BlobParams,
    #[texture(1, dimension = "cube")]
    #[sampler(2)]
    pub envmap: Option<Handle<Image>>,
}

impl Material for PaintBlobMaterial {
    fn fragment_shader() -> ShaderRef {
        BLOB_SHADER.into()
    }

    fn enable_prepass() -> bool {
        false
    }

    fn enable_shadows() -> bool {
        false
    }
}

/// Unit sphere: an icosahedron subdivided once.
fn unit_sphere() -> (Vec<Vec3>, Vec<u32>) {
    let t = (1.0 + 5f32.sqrt()) / 2.0;
    let mut verts: Vec<Vec3> = [
        (-1.0, t, 0.0),
        (1.0, t, 0.0),
        (-1.0, -t, 0.0),
        (1.0, -t, 0.0),
        (0.0, -1.0, t),
        (0.0, 1.0, t),
        (0.0, -1.0, -t),
        (0.0, 1.0, -t),
        (t, 0.0, -1.0),
        (t, 0.0, 1.0),
        (-t, 0.0, -1.0),
        (-t, 0.0, 1.0),
    ]
    .iter()
    .map(|&(x, y, z)| Vec3::new(x, y, z).normalize())
    .collect();
    let faces: [[u32; 3]; 20] = [
        [0, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ];
    let mut mid = std::collections::HashMap::new();
    let mut split = |a: u32, b: u32, verts: &mut Vec<Vec3>| -> u32 {
        *mid.entry((a.min(b), a.max(b))).or_insert_with(|| {
            verts.push(((verts[a as usize] + verts[b as usize]) * 0.5).normalize());
            verts.len() as u32 - 1
        })
    };
    let mut indices = Vec::with_capacity(80 * 3);
    for [a, b, c] in faces {
        let ab = split(a, b, &mut verts);
        let bc = split(b, c, &mut verts);
        let ca = split(c, a, &mut verts);
        indices.extend([a, ab, ca, b, bc, ab, c, ca, bc, ab, bc, ca]);
    }
    (verts, indices)
}

/// A sphere to draw: centre, radius, linear colour.
type Sphere = (Vec3, f32, [f32; 4]);

fn spheres_mesh(spheres: &[Sphere], unit: &[Vec3], tris: &[u32]) -> Mesh {
    let mut positions = Vec::with_capacity(spheres.len() * unit.len());
    let mut normals = Vec::with_capacity(spheres.len() * unit.len());
    let mut colors = Vec::with_capacity(spheres.len() * unit.len());
    let mut indices = Vec::with_capacity(spheres.len() * tris.len());
    for &(center, r, color) in spheres {
        let base = positions.len() as u32;
        for v in unit {
            positions.push((center + *v * r).to_array());
            normals.push(v.to_array());
            colors.push(color);
        }
        indices.extend(tris.iter().map(|i| base + i));
    }
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

/// The linear colour of every paint type, read from the console once a
/// frame rather than for every blob.
struct Palette([[f32; 4]; 5]);

impl Palette {
    fn new(console: &Console) -> Self {
        Self([PaintType::Bounce, PaintType::Reflect, PaintType::Speed, PaintType::Portal, PaintType::Erase].map(|t| {
            let c = t.color(console);
            Color::srgb_u8(c[0], c[1], c[2]).to_linear().to_f32_array()
        }))
    }

    fn get(&self, paint: PaintType) -> [f32; 4] {
        self.0[paint as usize]
    }
}

fn blob_sphere(b: &Blob, palette: &Palette) -> Sphere {
    (b.pos, b.radius() * DRAW_SCALE, palette.get(b.paint))
}

/// A drawn group of spheres: its entity and how many it showed last.
#[derive(Default)]
struct DrawSlot {
    entity: Option<Entity>,
    drawn: usize,
}

/// Blobs no sprayer owns (bomb splashes) and the bomb balls, drawn
/// together, lit by the ambient light where the first of them is.
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
    unit: &'a [Vec3],
    tris: &'a [u32],
}

impl DrawTargets<'_> {
    /// Puts `spheres` into the slot's mesh, spawning its entity on first use.
    fn show(
        &mut self,
        slot: &mut DrawSlot,
        spheres: &[Sphere],
        commands: &mut Commands,
        handles: &Query<&Mesh3d>,
        make_material: impl FnOnce(&mut Assets<PaintBlobMaterial>) -> Handle<PaintBlobMaterial>,
    ) {
        if spheres.is_empty() && slot.drawn == 0 {
            return;
        }
        let mesh = spheres_mesh(spheres, self.unit, self.tris);
        match slot.entity.and_then(|d| handles.get(d).ok()) {
            Some(handle) => {
                if let Some(mut m) = self.meshes.get_mut(&handle.0) {
                    *m = mesh;
                }
            }
            None => {
                if spheres.is_empty() {
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
        slot.drawn = spheres.len();
    }
}

fn ambient_params(ambient: [Vec3; 6]) -> BlobParams {
    BlobParams { ambient: ambient.map(|a| a.extend(1.0)) }
}

/// One mesh per sprayer holding its blobs, and one for everything else,
/// rebuilt every frame.
fn draw_blobs(
    mut commands: Commands,
    blobs: Res<Blobs>,
    mut sprayers: Query<(Entity, &mut PaintSprayer)>,
    bombs: Query<(Entity, &super::bomb::PaintBomb, &GlobalTransform)>,
    (mut meshes, mut materials, handles): (ResMut<Assets<Mesh>>, ResMut<Assets<PaintBlobMaterial>>, Query<&Mesh3d>),
    (console, time, current): (Res<Console>, Res<Time>, Option<Res<CurrentMap>>),
    (fs, mut textures, mut images): (Res<GameFs>, ResMut<TextureCache>, ResMut<Assets<Image>>),
    (mut sphere, mut free): (Local<Option<(Vec<Vec3>, Vec<u32>)>>, Local<FreeDraw>),
) {
    let (unit, tris) = sphere.get_or_insert_with(unit_sphere);
    let envmap = textures.load(&fs, &mut images, "materials/paint/paint_envmap_hdr.vtf", false).map(|t| t.handle);
    let mut targets = DrawTargets { meshes: &mut meshes, materials: &mut materials, unit, tris };
    let palette = Palette::new(&console);
    let mut owned = std::collections::HashSet::new();
    for (e, mut s) in &mut sprayers {
        owned.insert(e);
        let spheres: Vec<Sphere> = blobs.list.iter().filter(|b| b.sprayer == e).map(|b| blob_sphere(b, &palette)).collect();
        let ambient = s.ambient;
        let mut slot = DrawSlot { entity: s.draw, drawn: s.drawn };
        let env = envmap.clone();
        targets.show(&mut slot, &spheres, &mut commands, &handles, |m| {
            m.add(PaintBlobMaterial { params: ambient_params(ambient), envmap: env })
        });
        s.draw = slot.entity;
        s.drawn = slot.drawn;
    }
    let mut spheres: Vec<Sphere> = blobs.list.iter().filter(|b| !owned.contains(&b.sprayer)).map(|b| blob_sphere(b, &palette)).collect();
    let t = time.elapsed_secs();
    for (e, bomb, gt) in &bombs {
        let color = palette.get(bomb.paint);
        for o in super::bomb::cluster_offsets(e.index_u32(), t) {
            spheres.push((gt.translation() + o, 0.7 * DRAW_SCALE, color));
        }
    }
    // Light the free group where it is, now and then.
    let mut relight = None;
    if free.relight_in == 0
        && let Some(first) = spheres.first()
        && let Some(map) = current.as_ref()
    {
        relight = Some(portal2_render::models::lighting::compute(&map.bsp, first.0).ambient);
        free.relight_in = 30;
    }
    free.relight_in = free.relight_in.saturating_sub(1);
    if let (Some(ambient), Some(m)) = (relight, free.material.as_ref())
        && let Some(mut mat) = targets.materials.get_mut(m)
    {
        mat.params = ambient_params(ambient);
    }
    let FreeDraw { slot, material, .. } = &mut *free;
    targets.show(slot, &spheres, &mut commands, &handles, |m| {
        let h = m.add(PaintBlobMaterial { params: ambient_params(relight.unwrap_or([Vec3::splat(0.2); 6])), envmap });
        *material = Some(h.clone());
        h
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sphere_is_closed_and_unit() {
        let (v, i) = unit_sphere();
        assert_eq!(v.len(), 42);
        assert_eq!(i.len(), 80 * 3);
        assert!(v.iter().all(|p| (p.length() - 1.0).abs() < 1e-5));
    }

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
