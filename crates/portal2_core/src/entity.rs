//! What every map entity has: its class, name and keyvalues, plus the
//! lookup tables that find entities by name and class.

use std::collections::HashMap;
use std::sync::Arc;

use bevy::prelude::*;
use source_formats::fgd::FgdClass;

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

    /// Forgets map entities; the player carries over between maps.
    pub fn clear(&mut self) {
        self.by_name.clear();
        self.by_class.clear();
    }
}

/// Bounds of a brush entity's model in its own space; sounds attached to the
/// entity come from their centre.
#[derive(Component, Clone, Copy, Debug)]
pub struct BrushBounds {
    pub mins: Vec3,
    pub maxs: Vec3,
}
