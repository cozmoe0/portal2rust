//! Particle effects: the systems of the `.pcf` files in the particle
//! manifest, simulated and drawn the way the game's particle library does
//! (`docs/re/particles.md`).
//!
//! Effects are entities with a [`ParticleEffect`]: `info_particle_system`
//! from maps (`effect_name`, `start_active`, `cpoint1`..`cpoint63` naming
//! entities for the control points), or ones code spawns with
//! [`spawn_effect`]. Control point 0 is the entity itself. Each running
//! effect owns a [`sim::Collection`] (with its children) and one mesh per
//! renderer, rebuilt every frame.

pub mod ar2_explosion;
pub mod defs;
pub mod hooks;
pub mod render;
pub mod sim;

use std::collections::HashMap;

use bevy::camera::visibility::{NoFrustumCulling, RenderLayers};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use source_formats::pcf::{self, Pcf};

use crate::camera::MainCamera;
use crate::collision::Physics;
use crate::entities::io::{InputCall, InputHandlers};
use crate::entities::{EntityRegistry, MapEntity, SpawnHandlers};
use crate::render::textures::TextureCache;
use crate::vfs::GameFs;
use crate::world::{CurrentMap, MapLoaded, MapObject};
use defs::{Def, DefRef};
use render::{Batch, Looks, ParticleMaterial, Seen};
use sim::{Collection, ControlPoint, Env, MAX_CONTROL_POINTS};

/// Every particle system the manifest's files define, by lowercase name.
#[derive(Resource, Default)]
pub struct ParticleLibrary {
    pub defs: Vec<DefRef>,
    pub by_name: HashMap<String, usize>,
}

impl sim::Library for ParticleLibrary {
    fn def(&self, index: usize) -> Option<DefRef> {
        self.defs.get(index).cloned()
    }
}

impl ParticleLibrary {
    /// Loads the files `particles/particles_manifest.txt` lists, in order;
    /// the first definition of a name wins.
    pub fn load(fs: &GameFs) -> Self {
        let mut lib = Self::default();
        let manifest = fs.read_string(pcf::MANIFEST_PATH).map(|t| pcf::parse_manifest(&t)).unwrap_or_default();
        let mut seen_files = Vec::new();
        for entry in manifest {
            if seen_files.contains(&entry.path) {
                continue;
            }
            seen_files.push(entry.path.clone());
            let Some(data) = fs.read(&entry.path) else { continue };
            let file = match Pcf::parse(&data) {
                Ok(f) => f,
                Err(e) => {
                    warn!("{}: {e}", entry.path);
                    continue;
                }
            };
            let base = lib.defs.len();
            for d in &file.systems {
                let def = Def::compile(d, |i| (i < file.systems.len()).then_some(base + i));
                let index = lib.defs.len();
                lib.by_name.entry(d.name.to_ascii_lowercase()).or_insert(index);
                lib.defs.push(DefRef::new(def));
            }
        }
        lib
    }

    pub fn find(&self, name: &str) -> Option<DefRef> {
        self.by_name.get(&name.trim().to_ascii_lowercase()).and_then(|&i| self.defs.get(i).cloned())
    }
}

/// A particle effect on an entity.
#[derive(Component, Clone, Debug, Serialize, Deserialize)]
pub struct ParticleEffect {
    pub effect: String,
    pub active: bool,
    /// Control points taken from other entities, by name.
    pub cp_names: Vec<(usize, String)>,
    /// Control points code sets directly (world positions).
    pub cp_values: Vec<(usize, Vec3)>,
    /// Remove the entity once the effect has run its course.
    pub one_shot: bool,
    /// Start again whenever the effect has run its course.
    pub repeat: bool,
    #[serde(skip)]
    pub collection: Option<Box<Collection>>,
    #[serde(skip)]
    meshes: Vec<Entity>,
    #[serde(skip)]
    end_cap: bool,
    #[serde(skip)]
    warned: bool,
}

impl ParticleEffect {
    pub fn new(effect: &str, active: bool) -> Self {
        Self {
            effect: effect.trim().to_owned(),
            active,
            cp_names: Vec::new(),
            cp_values: Vec::new(),
            one_shot: false,
            repeat: false,
            collection: None,
            meshes: Vec::new(),
            end_cap: false,
            warned: false,
        }
    }

    /// Stops emitting; particles already out finish their lives.
    pub fn stop(&mut self) {
        self.active = false;
        if let Some(c) = &mut self.collection {
            c.stop_emission();
        }
    }
}

impl crate::save::Restore for ParticleEffect {
    fn keep(&mut self, live: Self) {
        self.meshes = live.meshes;
    }
}

pub fn register_save(r: &mut crate::save::SaveRegistry) {
    r.component::<ParticleEffect>("particle_effect");
}

/// A renderer's mesh, owned by an effect entity.
#[derive(Component)]
struct ParticleMesh {
    owner: Entity,
}

/// Spawns an effect at `transform` (Source coordinates); with `one_shot`
/// the entity goes away once the effect is over.
pub fn spawn_effect(commands: &mut Commands, name: &str, transform: Transform, one_shot: bool) -> Entity {
    let mut effect = ParticleEffect::new(name, true);
    effect.one_shot = one_shot;
    commands.spawn((Name::new(format!("particles {name}")), transform, Visibility::default(), effect, MapObject)).id()
}

pub struct ParticlePlugin;

impl Plugin for ParticlePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((MaterialPlugin::<ParticleMaterial>::default(), hooks::HooksPlugin))
            .init_resource::<ParticleLibrary>()
            .init_resource::<Looks>()
            .add_systems(Startup, load_library)
            .add_systems(Update, reset_looks.run_if(on_message::<MapLoaded>))
            .add_systems(PostUpdate, (simulate, draw, cleanup).chain().after(TransformSystems::Propagate));
        let world = app.world_mut();
        world.resource_mut::<SpawnHandlers>().register("info_particle_system", spawn_info);
        world.resource_mut::<InputHandlers>().register("info_particle_system", effect_input);
        ar2_explosion::register(app);
    }
}

fn load_library(fs: Res<GameFs>, mut library: ResMut<ParticleLibrary>) {
    let start = std::time::Instant::now();
    *library = ParticleLibrary::load(&fs);
    info!("particles: {} systems in {:?}", library.defs.len(), start.elapsed());
}

/// Particle materials hold texture handles from the texture cache, which a
/// map load clears.
fn reset_looks(mut looks: ResMut<Looks>) {
    looks.clear();
}

fn spawn_info(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e).cloned() else { return };
    let mut effect = ParticleEffect::new(me.get("effect_name").unwrap_or(""), me.get_bool("start_active"));
    for i in 1..MAX_CONTROL_POINTS {
        if let Some(name) = me.get(&format!("cpoint{i}")).filter(|n| !n.trim().is_empty()) {
            effect.cp_names.push((i, name.trim().to_ascii_lowercase()));
        }
    }
    world.entity_mut(e).insert(effect);
}

fn effect_input(world: &mut World, call: &InputCall) -> bool {
    let Some(mut fx) = world.get_mut::<ParticleEffect>(call.target) else { return false };
    match call.input.as_str() {
        "start" => {
            if !fx.active || fx.collection.as_ref().is_some_and(|c| !c.emitting) {
                fx.collection = None;
                fx.end_cap = false;
            }
            fx.active = true;
        }
        "stop" => fx.stop(),
        "stopplayendcap" => {
            fx.active = false;
            fx.end_cap = true;
        }
        "destroyimmediately" => {
            fx.active = false;
            fx.collection = None;
        }
        _ => return false,
    }
    true
}

/// World queries for the simulation.
struct World2<'a, 'w, 's> {
    physics: &'a Physics<'w, 's>,
    map: Option<&'a CurrentMap>,
}

impl Env for World2<'_, '_, '_> {
    fn trace(&self, start: Vec3, end: Vec3) -> Option<(f32, Vec3)> {
        let t = self.physics.trace(start, end, Vec3::ZERO, Vec3::ZERO, crate::collision::mask::SOLID_ALL, &[]);
        (t.fraction < 1.0 && !t.start_solid).then_some((t.fraction, t.normal))
    }

    fn light(&self, at: Vec3) -> Vec3 {
        self.map
            .and_then(|m| m.bsp.ambient_cube(at))
            .map(|c| c.iter().copied().sum::<Vec3>() / 6.0)
            .unwrap_or(Vec3::splat(0.5))
    }
}

fn control_point(gt: &GlobalTransform) -> ControlPoint {
    let (_, rot, pos) = gt.to_scale_rotation_translation();
    let mut cp = ControlPoint { position: pos, prev_position: pos, ..Default::default() };
    cp.set_rotation(rot);
    cp
}

#[allow(clippy::too_many_arguments)]
fn simulate(
    time: Res<Time>,
    library: Res<ParticleLibrary>,
    physics: Physics,
    map: Option<Res<CurrentMap>>,
    registry: Res<EntityRegistry>,
    transforms: Query<&GlobalTransform>,
    mut effects: Query<(Entity, &GlobalTransform, &mut ParticleEffect)>,
    mut looks: ResMut<Looks>,
    fs: Res<GameFs>,
    mut textures: ResMut<TextureCache>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<ParticleMaterial>>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    let env = World2 { physics: &physics, map: map.as_deref() };
    for (e, gt, mut fx) in &mut effects {
        let fx = &mut *fx;
        if fx.collection.is_none() {
            if !fx.active {
                continue;
            }
            let Some(def) = library.find(&fx.effect) else {
                if !fx.warned {
                    fx.warned = true;
                    debug!("particles: no system named {:?}", fx.effect);
                }
                if fx.one_shot {
                    commands.entity(e).despawn();
                }
                continue;
            };
            let look = looks.get(&def.material, &fs, &mut textures, &mut images, &mut materials);
            if !fx.warned {
                fx.warned = true;
                let skipped = def.unsupported();
                if !skipped.is_empty() {
                    debug!("particles: {} runs without {skipped:?}", def.name);
                }
            }
            let seed = ((e.to_bits() as u32).wrapping_mul(2_654_435_761)) >> 20;
            let mut c = Collection::new(def, library.as_ref(), seed);
            c.sequence_frames = look.sheet.frame_counts();
            let cp0 = control_point(gt);
            c.set_cp(0, cp0);
            fx.collection = Some(Box::new(c));
        }
        let Some(c) = fx.collection.as_mut() else { continue };
        // Control points follow their entities.
        let mut cp0 = control_point(gt);
        cp0.prev_position = c.cps[0].position;
        c.set_cp(0, cp0);
        for (i, name) in &fx.cp_names {
            let target = registry.first_named(name);
            if let Some(t) = target.and_then(|t| transforms.get(t).ok()) {
                let mut cp = control_point(t);
                cp.prev_position = c.cps[*i].position;
                c.set_cp(*i, cp);
            }
        }
        for (i, p) in &fx.cp_values {
            let mut cp = c.cps[*i];
            cp.prev_position = cp.position;
            cp.position = *p;
            c.set_cp(*i, cp);
        }
        if fx.end_cap {
            fx.end_cap = false;
            c.start_end_cap(library.as_ref());
        }
        c.simulate(dt, &env, library.as_ref());
        if c.finished() && fx.active && fx.repeat {
            fx.collection = None;
        } else if c.finished() && !fx.active {
            fx.collection = None;
            if fx.one_shot {
                commands.entity(e).despawn();
            }
        } else if fx.one_shot && c.finished() {
            commands.entity(e).despawn();
        }
    }
}

/// Builds every running effect's meshes.
#[allow(clippy::too_many_arguments)]
fn draw(
    library: Res<ParticleLibrary>,
    mut effects: Query<(Entity, &mut ParticleEffect)>,
    main: Query<&GlobalTransform, With<MainCamera>>,
    mut looks: ResMut<Looks>,
    fs: Res<GameFs>,
    mut textures: ResMut<TextureCache>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<ParticleMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut parts: Query<(&Mesh3d, &mut MeshMaterial3d<ParticleMaterial>, &mut Transform, &mut Visibility), With<ParticleMesh>>,
    physics: Physics,
    mut commands: Commands,
) {
    let _ = &library;
    let eye_tf = main.single().ok();
    let eye = eye_tf.map(|t| t.translation()).unwrap_or_default();
    // Share of a proxy sphere the view sees: its centre and four points
    // around it across the view.
    let visible = |centre: Vec3, radius: f32| -> f32 {
        let Some(t) = eye_tf else { return 1.0 };
        let (right, up) = (Vec3::from(t.right()), Vec3::from(t.up()));
        let pts = [Vec3::ZERO, right * radius, -right * radius, up * radius, -up * radius];
        let seen = pts
            .iter()
            .filter(|&&o| {
                let tr = physics.trace(eye, centre + o, Vec3::ZERO, Vec3::ZERO, crate::collision::mask::SOLID_ALL, &[]);
                !tr.hit() || tr.fraction > 0.999
            })
            .count();
        seen as f32 / pts.len() as f32
    };
    for (owner, mut fx) in &mut effects {
        let fx = &mut *fx;
        let mut slot = 0usize;
        if let Some(c) = fx.collection.as_deref() {
            let mut stack: Vec<&Collection> = vec![c];
            while let Some(c) = stack.pop() {
                stack.extend(c.children.iter().map(|ch| &ch.collection));
                if c.time < 0.0 || c.def.renderers.is_empty() {
                    continue;
                }
                let look = looks.get(&c.def.material, &fs, &mut textures, &mut images, &mut materials);
                if look.hidden {
                    continue;
                }
                let origin = c.cps[0].position;
                let far = origin.distance(eye) > c.def.max_draw_distance;
                for r in 0..c.def.renderers.len() {
                    // Renderers fade like any operator; far systems are not drawn.
                    let strength = if far { None } else { c.strength(&c.def.renderers[r].common) };
                    let mut seen = match &c.def.renderers[r].visibility {
                        Some(v) => {
                            let cp = c.cps[v.cp.clamp(0, 63) as usize].position;
                            render::visibility_scale(v, visible(cp, v.radius))
                        }
                        None => Seen::default(),
                    };
                    seen.alpha *= strength.unwrap_or(0.0);
                    let mut batch = Batch::default();
                    if strength.is_some() {
                        render::build(&mut batch, c, r, &look, origin, eye, seen);
                    }
                    if slot == fx.meshes.len() {
                        let mesh = meshes.add(render::empty_mesh());
                        // View model effects draw with the first-person weapon.
                        let layer = if c.def.view_model {
                            RenderLayers::layer(crate::portals::gun::VIEWMODEL_LAYER)
                        } else {
                            RenderLayers::layer(0)
                        };
                        let id = commands
                            .spawn((
                                Name::new("particle mesh"),
                                Mesh3d(mesh),
                                MeshMaterial3d(look.material.clone()),
                                Transform::from_translation(origin),
                                Visibility::Hidden,
                                NoFrustumCulling,
                                layer,
                                ParticleMesh { owner },
                                MapObject,
                            ))
                            .id();
                        fx.meshes.push(id);
                        // Filled next frame, once the entity exists.
                        slot += 1;
                        continue;
                    }
                    let Ok((mesh, mut mat, mut tf, mut vis)) = parts.get_mut(fx.meshes[slot]) else {
                        slot += 1;
                        continue;
                    };
                    slot += 1;
                    let empty = batch.is_empty();
                    if !empty && let Some(mut m) = meshes.get_mut(&mesh.0) {
                        batch.write(&mut m);
                    }
                    if mat.0 != look.material {
                        mat.0 = look.material.clone();
                    }
                    tf.translation = origin;
                    let want = if empty { Visibility::Hidden } else { Visibility::Inherited };
                    if *vis != want {
                        *vis = want;
                    }
                }
            }
        }
        // Unused meshes hide.
        for &m in &fx.meshes[slot.min(fx.meshes.len())..] {
            if let Ok((.., mut vis)) = parts.get_mut(m)
                && *vis != Visibility::Hidden
            {
                *vis = Visibility::Hidden;
            }
        }
    }
}

/// Removes meshes whose effect is gone.
fn cleanup(parts: Query<(Entity, &ParticleMesh)>, owners: Query<(), With<ParticleEffect>>, mut commands: Commands) {
    for (e, p) in &parts {
        if owners.get(p.owner).is_err() {
            commands.entity(e).despawn();
        }
    }
}
