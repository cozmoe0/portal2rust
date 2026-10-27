//! Map entities: spawning, naming, parenting and the shared input handlers.
//! Class behaviour lives in the submodules.

pub mod io;
pub mod logic;
pub mod movers;
pub mod props;
pub mod spawner;
pub mod triggers;

use std::collections::HashMap;
use std::sync::Arc;

use bevy::prelude::*;
use source_formats::fgd::{Fgd, FgdClass};

use crate::coords::angles_to_quat;
use crate::vfs::GameFs;
use crate::world::{CurrentMap, MapLoaded, MapObject};
use io::{Connection, EventQueue, FireOutput, InputCall, InputHandlers, IoDebug, Outputs, fire_output};

pub struct EntitiesPlugin;

impl Plugin for EntitiesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<EntityRegistry>()
            .init_resource::<EventQueue>()
            .init_resource::<InputHandlers>()
            .init_resource::<SpawnHandlers>()
            .init_resource::<IoDebug>()
            .add_message::<FireOutput>()
            .add_systems(Startup, load_entity_definitions)
            .add_systems(PreUpdate, spawn_on_map_load.in_set(EntitySpawnSet).after(crate::world::LoadMapSet))
            .add_systems(Update, io::service_events.in_set(IoSet));
        app.world_mut().resource_mut::<InputHandlers>().register_generic(generic_inputs);
        logic::register(app);
        movers::register(app);
        props::register(app);
        triggers::register(app);
    }
}

/// Spawning of map entities; collision is rebuilt before it.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct EntitySpawnSet;

/// Systems that fire outputs should run before this set to have them serviced
/// in the same frame.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct IoSet;

/// Common data of every map entity.
#[derive(Component, Clone, Debug)]
pub struct MapEntity {
    /// Index in the BSP entity lump (usize::MAX for runtime spawned entities).
    pub index: usize,
    pub classname: String,
    pub targetname: Option<String>,
    /// Keyvalues with lowercase keys, outputs excluded.
    pub keyvalues: Vec<(String, String)>,
    /// Class defaults from the installed entity definitions.
    pub defaults: Option<Arc<FgdClass>>,
}

impl MapEntity {
    pub fn get(&self, key: &str) -> Option<&str> {
        let key = key.to_ascii_lowercase();
        self.keyvalues
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.as_str())
            .or_else(|| self.defaults.as_ref().and_then(|d| d.defaults.get(&key)).map(|s| s.as_str()))
    }

    pub fn get_f32(&self, key: &str) -> Option<f32> {
        self.get(key).map(source_formats::kv::parse_f32_lenient)
    }

    pub fn get_i32(&self, key: &str) -> Option<i32> {
        self.get(key).map(source_formats::kv::parse_i32_lenient)
    }

    pub fn get_bool(&self, key: &str) -> bool {
        self.get_i32(key).unwrap_or(0) != 0
    }

    pub fn get_vec3(&self, key: &str) -> Option<Vec3> {
        self.get(key).and_then(source_formats::kv::parse_vec3)
    }

    pub fn spawnflags(&self) -> u32 {
        match self.keyvalues.iter().find(|(k, _)| k == "spawnflags") {
            Some((_, v)) => source_formats::kv::parse_i32_lenient(v) as u32,
            None => self.defaults.as_ref().map(|d| d.default_spawnflags).unwrap_or(0),
        }
    }

    pub fn has_flag(&self, bit: u32) -> bool {
        self.spawnflags() & bit != 0
    }

    pub fn origin(&self) -> Vec3 {
        self.get_vec3("origin").unwrap_or(Vec3::ZERO)
    }

    pub fn angles(&self) -> Vec3 {
        let mut a = self.get_vec3("angles").unwrap_or(Vec3::ZERO);
        if let Some(p) = self.keyvalues.iter().find(|(k, _)| k == "pitch") {
            a.x = source_formats::kv::parse_f32_lenient(&p.1);
        }
        a
    }

    /// Brush model index for `model "*N"` entities.
    pub fn brush_model(&self) -> Option<usize> {
        self.get("model").and_then(|m| m.strip_prefix('*')).and_then(|m| m.parse().ok())
    }

    /// Studio model path from the keyvalue or the class's editor model.
    pub fn studio_model(&self) -> Option<String> {
        self.get("model")
            .filter(|m| m.to_ascii_lowercase().ends_with(".mdl"))
            .map(str::to_owned)
            .or_else(|| self.defaults.as_ref().and_then(|d| d.studio_model.clone()))
    }

    pub fn set(&mut self, key: &str, value: &str) {
        let key = key.to_ascii_lowercase();
        match self.keyvalues.iter_mut().find(|(k, _)| *k == key) {
            Some(kv) => kv.1 = value.to_owned(),
            None => self.keyvalues.push((key, value.to_owned())),
        }
    }
}

/// Lookup tables for output targets.
#[derive(Resource, Default)]
pub struct EntityRegistry {
    pub by_name: HashMap<String, Vec<Entity>>,
    pub by_class: HashMap<String, Vec<Entity>>,
    pub player: Option<Entity>,
}

impl EntityRegistry {
    pub fn insert(&mut self, entity: Entity, classname: &str, targetname: Option<&str>) {
        self.by_class.entry(classname.to_owned()).or_default().push(entity);
        if let Some(n) = targetname {
            self.by_name.entry(n.to_owned()).or_default().push(entity);
        }
    }

    pub fn remove(&mut self, entity: Entity) {
        for list in self.by_name.values_mut().chain(self.by_class.values_mut()) {
            list.retain(|e| *e != entity);
        }
    }

    pub fn rename(&mut self, entity: Entity, old: Option<&str>, new: Option<&str>) {
        if let Some(o) = old
            && let Some(list) = self.by_name.get_mut(o)
        {
            list.retain(|e| *e != entity);
        }
        if let Some(n) = new {
            self.by_name.entry(n.to_owned()).or_default().push(entity);
        }
    }

    pub fn first_named(&self, name: &str) -> Option<Entity> {
        self.by_name.get(&name.to_ascii_lowercase()).and_then(|l| l.first().copied())
    }

    pub fn clear(&mut self) {
        self.by_name.clear();
        self.by_class.clear();
        self.player = None;
    }
}

/// Installed entity definitions (`bin/portal2.fgd` and its includes).
#[derive(Resource, Default)]
pub struct EntityDefinitions {
    fgd: Fgd,
    resolved: HashMap<String, Option<Arc<FgdClass>>>,
}

impl EntityDefinitions {
    pub fn class(&mut self, classname: &str) -> Option<Arc<FgdClass>> {
        let key = classname.to_ascii_lowercase();
        if let Some(c) = self.resolved.get(&key) {
            return c.clone();
        }
        let c = self.fgd.resolve(&key).map(Arc::new);
        self.resolved.insert(key, c.clone());
        c
    }
}

fn load_entity_definitions(mut commands: Commands, fs: Res<GameFs>) {
    let bin = fs.root.join("bin");
    let read = |name: &str| std::fs::read(bin.join(name)).ok().map(|b| source_formats::text::decode_text(&b));
    let fgd = match read("portal2.fgd") {
        Some(text) => Fgd::parse(&text, &mut |name| read(name)),
        None => {
            warn!("entity definitions not found in {}; class defaults unavailable", bin.display());
            Fgd::default()
        }
    };
    info!("entity definitions: {} classes", fgd.classes.len());
    commands.insert_resource(EntityDefinitions { fgd, resolved: HashMap::new() });
}

/// Per-class spawn functions, run after the common components exist.
pub type SpawnHandler = fn(&mut World, Entity);

#[derive(Resource, Default)]
pub struct SpawnHandlers {
    by_class: HashMap<String, SpawnHandler>,
}

impl SpawnHandlers {
    pub fn register(&mut self, classname: &str, handler: SpawnHandler) {
        self.by_class.insert(classname.to_ascii_lowercase(), handler);
    }
}

/// Classes that exist only at compile time or are handled elsewhere.
fn skip_class(class: &str) -> bool {
    matches!(
        class,
        "worldspawn" | "func_detail" | "info_node" | "info_node_hint" | "info_node_air" | "func_viscluster" | "info_lighting"
    )
}

fn spawn_on_map_load(world: &mut World) {
    let loaded: Vec<MapLoaded> = world.resource_mut::<Messages<MapLoaded>>().drain().collect();
    if loaded.is_empty() {
        return;
    }
    let Some(map) = world.get_resource::<CurrentMap>().cloned() else { return };
    world.resource_mut::<EntityRegistry>().clear();
    world.resource_mut::<EventQueue>().clear();

    let started = std::time::Instant::now();
    let mut spawned: Vec<(Entity, String)> = Vec::new();
    for (index, ent) in map.bsp.entities.iter().enumerate() {
        let classname = ent.classname().to_ascii_lowercase();
        if skip_class(&classname) {
            continue;
        }
        let defaults = world.resource_mut::<EntityDefinitions>().class(&classname);
        let mut keyvalues = Vec::new();
        let mut outputs = Outputs::default();
        for (k, v) in &ent.pairs {
            let is_output = v.contains('\x1b') || (k.len() > 2 && k[..2].eq_ignore_ascii_case("on") && v.matches(',').count() >= 4);
            if is_output {
                if let Some(c) = Connection::parse(k, v) {
                    outputs.connections.push(c);
                }
            } else {
                keyvalues.push((k.to_ascii_lowercase(), v.clone()));
            }
        }
        let targetname = ent.targetname().map(|s| s.to_ascii_lowercase());
        let me = MapEntity { index, classname: classname.clone(), targetname: targetname.clone(), keyvalues, defaults };
        let transform = Transform::from_translation(me.origin()).with_rotation(angles_to_quat(me.angles()));
        let name = format!("{classname} {}", targetname.as_deref().unwrap_or(""));
        let e = world.spawn((Name::new(name), me, outputs, transform, Visibility::Visible, MapObject)).id();
        world.resource_mut::<EntityRegistry>().insert(e, &classname, targetname.as_deref());
        spawned.push((e, classname));
    }

    // Class setup.
    for (e, class) in &spawned {
        let handler = world.resource::<SpawnHandlers>().by_class.get(class).copied();
        spawner::attach_brush_model(world, *e);
        if let Some(h) = handler {
            h(world, *e);
        } else if world.get::<MapEntity>(*e).is_some_and(|m| m.studio_model().is_some()) && class.starts_with("prop_") {
            props::spawn_generic_prop(world, *e);
        }
        spawner::attach_collider(world, *e);
    }

    // Parenting: keep each child's world placement.
    for (e, _) in &spawned {
        let Some(parent_name) = world.get::<MapEntity>(*e).and_then(|m| m.get("parentname").map(str::to_owned)) else {
            continue;
        };
        let (name, _attachment) = parent_name.split_once(',').unwrap_or((&parent_name, ""));
        set_parent(world, *e, &name.to_ascii_lowercase());
    }
    movers::apply_start_positions(world, spawned.iter().map(|(e, _)| *e));

    props::spawn_static_props(world);

    // Debug spawns requested on the command line.
    let requests = world.resource::<crate::args::Args>().spawn.clone();
    for req in requests {
        let mut parts = req.split('@');
        let (Some(class), Some(at)) = (parts.next(), parts.next()) else { continue };
        let origin = at.replace(',', " ");
        let mut keyvalues = vec![("origin", origin.as_str())];
        let extra = parts.next().unwrap_or("");
        keyvalues.extend(extra.split(';').filter_map(|kv| kv.split_once('=')));
        spawn_runtime_entity(world, class, &keyvalues);
    }

    // Debug inputs: `class.input[=param][@delay]`.
    let fires = world.resource::<crate::args::Args>().fire.clone();
    for spec in &fires {
        let (head, delay) = match spec.rsplit_once('@') {
            Some((h, d)) => (h, d.parse().unwrap_or(0.0)),
            None => (spec.as_str(), 0.0),
        };
        let (head, param) = head.split_once('=').unwrap_or((head, ""));
        let Some((class, input)) = head.split_once('.') else { continue };
        let targets = world.resource::<EntityRegistry>().by_class.get(class).cloned().unwrap_or_default();
        let mut queue = world.resource_mut::<io::EventQueue>();
        for t in targets {
            queue.push(delay, io::EventTarget::Entity(t), input, param, None, None);
        }
    }

    let count = spawned.len();
    // logic_auto fires once everything exists.
    let autos: Vec<Entity> = world.resource::<EntityRegistry>().by_class.get("logic_auto").cloned().unwrap_or_default();
    for a in autos {
        fire_output(world, a, "OnMapSpawn", None, None);
        fire_output(world, a, "OnNewGame", None, None);
        if world.get::<MapEntity>(a).is_some_and(|m| m.has_flag(1)) {
            despawn_entity(world, a);
        }
    }
    info!("spawned {count} entities in {:?}", started.elapsed());
}

/// Creates an entity at runtime as if it had been in the map, running the
/// class's spawn handler and attaching collision.
pub fn spawn_runtime_entity(world: &mut World, classname: &str, keyvalues: &[(&str, &str)]) -> Entity {
    let classname = classname.to_ascii_lowercase();
    let defaults = world.resource_mut::<EntityDefinitions>().class(&classname);
    let keyvalues: Vec<(String, String)> =
        keyvalues.iter().map(|(k, v)| (k.to_ascii_lowercase(), v.to_string())).collect();
    let me = MapEntity { index: usize::MAX, classname: classname.clone(), targetname: None, keyvalues, defaults };
    let transform = Transform::from_translation(me.origin()).with_rotation(angles_to_quat(me.angles()));
    let e = world
        .spawn((Name::new(format!("{classname} (spawned)")), me, Outputs::default(), transform, Visibility::Visible, MapObject))
        .id();
    world.resource_mut::<EntityRegistry>().insert(e, &classname, None);
    let handler = world.resource::<SpawnHandlers>().by_class.get(&classname).copied();
    spawner::attach_brush_model(world, e);
    match handler {
        Some(h) => h(world, e),
        None if classname.starts_with("prop_") => props::spawn_generic_prop(world, e),
        None => {}
    }
    spawner::attach_collider(world, e);
    movers::apply_start_positions(world, [e]);
    info!("spawned {classname} at {:?}", world.get::<Transform>(e).map(|t| t.translation));
    e
}

/// Re-parents `child` under the first entity named `parent`, keeping its
/// current world transform.
pub fn set_parent(world: &mut World, child: Entity, parent: &str) {
    let Some(p) = world.resource::<EntityRegistry>().first_named(parent) else { return };
    if p == child {
        return;
    }
    let old_parent = parent_placement(world, child);
    let parent_world = world_transform(world, p);
    let child_world = world_transform(world, child);
    let local = parent_world.affine().inverse() * child_world.affine();
    let local = Transform::from_matrix(local.into());
    world.entity_mut(child).insert((ChildOf(p), local));
    movers::rebase(world, child, old_parent, parent_world.affine());
}

/// World placement of an entity's current parent (identity when unparented).
fn parent_placement(world: &World, e: Entity) -> bevy::math::Affine3A {
    world
        .get::<ChildOf>(e)
        .map(|c| world_transform(world, c.parent()).affine())
        .unwrap_or(bevy::math::Affine3A::IDENTITY)
}

/// World transform computed from the hierarchy (valid before propagation).
pub fn world_transform(world: &World, mut e: Entity) -> GlobalTransform {
    let mut t = world.get::<Transform>(e).copied().unwrap_or_default();
    while let Some(parent) = world.get::<ChildOf>(e).map(|c| c.parent()) {
        let pt = world.get::<Transform>(parent).copied().unwrap_or_default();
        t = pt.mul_transform(t);
        e = parent;
    }
    GlobalTransform::from(t)
}

pub fn despawn_entity(world: &mut World, e: Entity) {
    world.resource_mut::<EntityRegistry>().remove(e);
    if let Ok(ent) = world.get_entity_mut(e) {
        ent.despawn();
    }
}

/// Inputs every entity understands.
fn generic_inputs(world: &mut World, call: &InputCall) -> bool {
    let e = call.target;
    match call.input.as_str() {
        "kill" | "killhierarchy" => {
            despawn_entity(world, e);
            true
        }
        "addoutput" => {
            if let Some(c) = Connection::parse_add_output(&call.param).filter(|_| call.param.contains(':')) {
                if let Some(mut outs) = world.get_mut::<Outputs>(e) {
                    outs.connections.push(c);
                }
            } else if let Some((key, value)) = call.param.trim().split_once(char::is_whitespace) {
                let key = key.to_ascii_lowercase();
                let value = value.trim().to_owned();
                if key == "targetname" {
                    let old = world.get::<MapEntity>(e).and_then(|m| m.targetname.clone());
                    let new = value.to_ascii_lowercase();
                    world.resource_mut::<EntityRegistry>().rename(e, old.as_deref(), Some(&new));
                    if let Some(mut m) = world.get_mut::<MapEntity>(e) {
                        m.targetname = Some(new);
                    }
                }
                if let Some(mut m) = world.get_mut::<MapEntity>(e) {
                    m.set(&key, &value);
                }
            }
            true
        }
        "fireuser1" | "fireuser2" | "fireuser3" | "fireuser4" => {
            let n = &call.input[8..];
            fire_output(world, e, &format!("onuser{n}"), call.activator, None);
            true
        }
        "enabledraw" | "disabledraw" => {
            if let Some(mut v) = world.get_mut::<Visibility>(e) {
                *v = if call.input == "enabledraw" { Visibility::Inherited } else { Visibility::Hidden };
            }
            true
        }
        "setparent" => {
            let name = call.param.trim().to_ascii_lowercase();
            set_parent(world, e, &name);
            true
        }
        "clearparent" => {
            let old_parent = parent_placement(world, e);
            let t = world_transform(world, e).compute_transform();
            world.entity_mut(e).remove::<ChildOf>().insert(t);
            movers::rebase(world, e, old_parent, bevy::math::Affine3A::IDENTITY);
            true
        }
        "runscriptfile" | "runscriptcode" | "callscriptfunction" => {
            debug!("script input {} ignored until scripting is implemented", call.input);
            true
        }
        _ => false,
    }
}
