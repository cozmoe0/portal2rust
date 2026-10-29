//! Entity templates: `point_template` keeps the map data of the entities it
//! names instead of spawning them, and `ForceSpawn` (on the template or an
//! `env_entity_maker`) creates copies. Unless the template preserves names,
//! each copy's names get an `&NNNN` suffix and every reference between the
//! copies (keyvalues, output targets) is renamed to match.

use std::collections::HashSet;

use bevy::prelude::*;

use super::io::{InputCall, InputHandlers, fire_output};
use super::{EntityRegistry, MapEntity, SpawnHandlers, world_transform};
use portal2_core::coords::{angles_to_quat, quat_to_angles};

pub fn register(app: &mut App) {
    app.init_resource::<TemplateCounter>();
    let world = app.world_mut();
    {
        let mut s = world.resource_mut::<SpawnHandlers>();
        s.register("point_template", |_, _| {});
        s.register("env_entity_maker", |_, _| {});
    }
    let mut h = world.resource_mut::<InputHandlers>();
    h.register("point_template", template_input);
    h.register("env_entity_maker", maker_input);
}

/// point_template spawnflags.
const SF_DONT_REMOVE: u32 = 1;
const SF_PRESERVE_NAMES: u32 = 2;

/// The entities a point_template creates, as map keyvalue pairs.
#[derive(Component, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Template {
    pub entries: Vec<Vec<(String, String)>>,
    pub origin: Vec3,
    pub preserve_names: bool,
}

#[derive(Resource, Default, serde::Serialize, serde::Deserialize)]
struct TemplateCounter(u32);

fn template_names(me: &MapEntity) -> Vec<String> {
    (1..=16).filter_map(|i| me.get(&format!("template{i:02}")).map(str::to_ascii_lowercase)).filter(|s| !s.is_empty()).collect()
}

/// Runs after map entities exist and before their classes set up: moves
/// each template's entities into its data and removes them from the map.
/// Returns the entities removed.
pub fn capture(world: &mut World, map_pairs: &[Vec<(String, String)>], spawned: &[(Entity, String)]) -> HashSet<Entity> {
    let mut removed = HashSet::new();
    let templates: Vec<Entity> = spawned.iter().filter(|(_, c)| c == "point_template").map(|(e, _)| *e).collect();
    for t in templates {
        let Some(me) = world.get::<MapEntity>(t).cloned() else { continue };
        let names: HashSet<String> = template_names(&me).into_iter().collect();
        let keep = me.has_flag(SF_DONT_REMOVE);
        let mut entries = Vec::new();
        for (e, _) in spawned {
            let Some(m) = world.get::<MapEntity>(*e) else { continue };
            if !m.targetname.as_ref().is_some_and(|n| names.contains(n)) {
                continue;
            }
            if let Some(pairs) = map_pairs.get(m.index) {
                entries.push(pairs.clone());
            }
            if !keep {
                removed.insert(*e);
            }
        }
        let origin = me.origin();
        world.entity_mut(t).insert(Template { entries, origin, preserve_names: me.has_flag(SF_PRESERVE_NAMES) });
    }
    for e in &removed {
        world.resource_mut::<EntityRegistry>().remove(*e);
        world.entity_mut(*e).despawn();
    }
    removed
}

fn get<'a>(pairs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    pairs.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.as_str())
}

fn set(pairs: &mut Vec<(String, String)>, key: &str, value: String) {
    match pairs.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(key)) {
        Some(kv) => kv.1 = value,
        None => pairs.push((key.to_owned(), value)),
    }
}

/// Renames template members in one copy of the template.
fn fixup(entries: &mut [Vec<(String, String)>], suffix: &str) {
    let names: HashSet<String> =
        entries.iter().filter_map(|p| get(p, "targetname").map(str::to_ascii_lowercase)).filter(|n| !n.is_empty()).collect();
    let rename = |v: &str| -> Option<String> {
        let (head, tail) = match v.find([',', '\x1b']) {
            Some(i) => (&v[..i], &v[i..]),
            None => (v, ""),
        };
        names.contains(&head.to_ascii_lowercase()).then(|| format!("{head}{suffix}{tail}"))
    };
    for pairs in entries.iter_mut() {
        for (_, v) in pairs.iter_mut() {
            if let Some(new) = rename(v) {
                *v = new;
            }
        }
    }
}

/// Spawns one copy of a template. `place` maps template-space placements
/// (origin, angles) to world placements.
fn instantiate(world: &mut World, template: Entity, place: impl Fn(Vec3, Vec3) -> (Vec3, Vec3)) -> Vec<Entity> {
    let Some(t) = world.get::<Template>(template).cloned() else { return Vec::new() };
    let mut entries = t.entries.clone();
    if !t.preserve_names {
        let mut counter = world.resource_mut::<TemplateCounter>();
        let suffix = format!("&{:04}", counter.0);
        counter.0 += 1;
        fixup(&mut entries, &suffix);
    }
    for pairs in entries.iter_mut() {
        let origin = get(pairs, "origin").and_then(source_formats::kv::parse_vec3).unwrap_or(Vec3::ZERO);
        let angles = get(pairs, "angles").and_then(source_formats::kv::parse_vec3).unwrap_or(Vec3::ZERO);
        let (o, a) = place(origin, angles);
        set(pairs, "origin", format!("{} {} {}", o.x, o.y, o.z));
        set(pairs, "angles", format!("{} {} {}", a.x, a.y, a.z));
    }
    super::spawn_from_pairs(world, &entries)
}

fn template_input(world: &mut World, call: &InputCall) -> bool {
    if call.input != "forcespawn" {
        return false;
    }
    let spawned = instantiate(world, call.target, |o, a| (o, a));
    fire_output(world, call.target, "OnEntitySpawned", spawned.first().copied().or(call.activator), None);
    true
}

/// Spawns the maker's template around `at` (world transform of the spot).
fn spawn_at(world: &mut World, maker: Entity, at: GlobalTransform) -> Vec<Entity> {
    let Some(me) = world.get::<MapEntity>(maker).cloned() else { return Vec::new() };
    let Some(name) = me.get("entitytemplate").map(str::to_ascii_lowercase) else { return Vec::new() };
    let Some(template) = world.resource::<EntityRegistry>().first_named(&name) else { return Vec::new() };
    let Some(t) = world.get::<Template>(template).cloned() else { return Vec::new() };
    let (_, rot, pos) = at.to_scale_rotation_translation();
    let spawned = instantiate(world, template, |o, a| {
        let world_pos = pos + rot * (o - t.origin);
        let world_rot = rot * angles_to_quat(a);
        (world_pos, quat_to_angles(world_rot))
    });
    // Optional launch velocity for physics props.
    let speed = me.get_f32("postspawnspeed").unwrap_or(0.0);
    if speed > 0.0 {
        let dir = rot * portal2_core::coords::angles_forward(me.get_vec3("postspawndirection").unwrap_or(Vec3::ZERO));
        for e in &spawned {
            if let Some(mut v) = world.get_mut::<avian3d::prelude::LinearVelocity>(*e) {
                v.0 = dir * speed;
            }
        }
    }
    spawned
}

fn maker_input(world: &mut World, call: &InputCall) -> bool {
    let at = match call.input.as_str() {
        "forcespawn" => world_transform(world, call.target),
        "forcespawnatentityorigin" => {
            let name = call.param.trim().to_ascii_lowercase();
            match super::io::resolve_targets(world, &name, call.activator, Some(call.target)).first() {
                Some(e) => world_transform(world, *e),
                None => return true,
            }
        }
        _ => return false,
    };
    let spawned = spawn_at(world, call.target, at);
    let output = if spawned.is_empty() { "OnEntityFailedSpawn" } else { "OnEntitySpawned" };
    fire_output(world, call.target, output, spawned.first().copied().or(call.activator), None);
    true
}

impl portal2_core::save::Restore for Template {}

impl portal2_core::save::Restore for TemplateCounter {}

pub fn register_save_resources(r: &mut portal2_core::save::SaveRegistry) {
    r.resource::<TemplateCounter>("template_counter");
}
