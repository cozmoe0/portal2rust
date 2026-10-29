//! What a saved game holds, as far as the engine crates need to know:
//! components and resources register under a name with [`SaveRegistry`],
//! and [`Restore`] puts back the fields a save skips. The save files
//! themselves (format, commands, autosaves) live in the game crate.
//!

pub mod ids;

use bevy::ecs::component::Mutable;
use bevy::prelude::*;
use serde::{Serialize, de::DeserializeOwned};

pub type ComponentSave = fn(&World, Entity) -> Option<Result<Vec<u8>, String>>;
pub type ComponentLoad = fn(&mut World, Entity, &[u8]) -> Result<(), String>;
pub type ResourceSave = fn(&World) -> Option<Result<Vec<u8>, String>>;
pub type ResourceLoad = fn(&mut World, &[u8]) -> Result<(), String>;

#[derive(Resource, Default)]
pub struct SaveRegistry {
    /// (name, save, load) of every saved component, in restore order.
    pub components: Vec<(&'static str, ComponentSave, ComponentLoad)>,
    /// (name, save, load) of every saved resource, in restore order.
    pub resources: Vec<(&'static str, ResourceSave, ResourceLoad)>,
}

/// Saved state replacing live state on load.
pub trait Restore: Sized {
    /// Called on the saved value with the live one (as the spawn code built
    /// it) just before the saved value replaces it: copy across the fields
    /// the save skips.
    fn keep(&mut self, _live: Self) {}
}

impl SaveRegistry {
    pub fn component<T>(&mut self, name: &'static str)
    where
        T: Component<Mutability = Mutable> + Serialize + DeserializeOwned + Restore,
    {
        self.components.push((name, save_component::<T>, load_component::<T>));
    }

    pub fn custom(&mut self, name: &'static str, save: ComponentSave, load: ComponentLoad) {
        self.components.push((name, save, load));
    }

    pub fn custom_resource(&mut self, name: &'static str, save: ResourceSave, load: ResourceLoad) {
        self.resources.push((name, save, load));
    }

    pub fn resource<T: Resource + Serialize + DeserializeOwned + Restore>(&mut self, name: &'static str) {
        self.resources.push((name, save_resource::<T>, load_resource::<T>));
    }
}

fn save_component<T: Component + Serialize>(world: &World, e: Entity) -> Option<Result<Vec<u8>, String>> {
    world.get::<T>(e).map(encode)
}

fn load_component<T>(world: &mut World, e: Entity, bytes: &[u8]) -> Result<(), String>
where
    T: Component<Mutability = Mutable> + DeserializeOwned + Restore,
{
    let mut saved: T = decode(bytes)?;
    let mut em = world.entity_mut(e);
    if let Some(live) = em.take::<T>() {
        saved.keep(live);
    }
    em.insert(saved);
    Ok(())
}

fn save_resource<T: Resource + Serialize>(world: &World) -> Option<Result<Vec<u8>, String>> {
    world.get_resource::<T>().map(encode)
}

fn load_resource<T: Resource + DeserializeOwned + Restore>(world: &mut World, bytes: &[u8]) -> Result<(), String> {
    let mut saved: T = decode(bytes)?;
    if let Some(live) = world.remove_resource::<T>() {
        saved.keep(live);
    }
    world.insert_resource(saved);
    Ok(())
}

/// Encodes a value the way saves store it (CBOR).
pub fn encode<T: Serialize>(v: &T) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    ciborium::into_writer(v, &mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

/// Decodes a value encoded with [`encode`].
pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, String> {
    ciborium::from_reader(bytes).map_err(|e| e.to_string())
}

/// An autosave asked for by an entity. `dangerous` seconds above zero make
/// it a dangerous autosave, skipped below `min_health` and kept only if the
/// player still has `commit_health` when the time is up.
#[derive(Message, Clone, Copy, Debug)]
pub struct AutosaveRequest {
    pub dangerous: f32,
    pub min_health: f32,
    pub commit_health: f32,
    /// Entity removed once it has saved (`trigger_autosave`).
    pub remove: Option<Entity>,
}
