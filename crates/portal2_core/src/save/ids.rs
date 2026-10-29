//! Stable entity ids for saved games, and serde helpers that write entity
//! references as those ids.
//!
//! Map entities keep their index in the map's entity list; entities made
//! while playing get ids from [`RUNTIME_IDS`] up. While a save is written or
//! read, a thread-local table translates between ids and the live entities,
//! so components can name other entities in their saved form
//! (`#[serde(with = "crate::save::ids::entity")]` and friends).

use std::cell::RefCell;
use std::collections::HashMap;

use bevy::prelude::*;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// First id of entities made after the map spawned.
pub const RUNTIME_IDS: u32 = 1 << 20;
/// The player.
pub const PLAYER_ID: u32 = u32::MAX - 1;
/// An entity that is not part of the save.
pub const NO_ID: u32 = u32::MAX;

/// An entity's id in saved games.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SaveId(pub u32);

/// Next id for entities made while playing.
#[derive(Resource)]
pub struct NextSaveId(pub u32);

impl Default for NextSaveId {
    fn default() -> Self {
        Self(RUNTIME_IDS)
    }
}

/// Id for an entity being created: its map index, or the next runtime id.
pub fn assign(world: &mut World, index: usize) -> SaveId {
    if index < RUNTIME_IDS as usize {
        return SaveId(index as u32);
    }
    let mut next = world.get_resource_or_init::<NextSaveId>();
    let id = next.0;
    next.0 += 1;
    SaveId(id)
}

enum Table {
    None,
    Saving(HashMap<Entity, u32>),
    Loading(HashMap<u32, Entity>),
}

thread_local! {
    static TABLE: RefCell<Table> = const { RefCell::new(Table::None) };
}

/// Runs `f` with entity references written as ids from `ids`.
pub fn saving<R>(ids: HashMap<Entity, u32>, f: impl FnOnce() -> R) -> R {
    TABLE.with(|t| *t.borrow_mut() = Table::Saving(ids));
    let r = f();
    TABLE.with(|t| *t.borrow_mut() = Table::None);
    r
}

/// Runs `f` with ids read back as the entities in `entities`.
pub fn loading<R>(entities: HashMap<u32, Entity>, f: impl FnOnce() -> R) -> R {
    TABLE.with(|t| *t.borrow_mut() = Table::Loading(entities));
    let r = f();
    TABLE.with(|t| *t.borrow_mut() = Table::None);
    r
}

pub fn id_of(e: Entity) -> u32 {
    TABLE.with(|t| match &*t.borrow() {
        Table::Saving(ids) => ids.get(&e).copied().unwrap_or(NO_ID),
        _ => NO_ID,
    })
}

pub fn entity_of(id: u32) -> Option<Entity> {
    TABLE.with(|t| match &*t.borrow() {
        Table::Loading(map) => map.get(&id).copied(),
        _ => None,
    })
}

/// `Entity` fields; an entity missing on load becomes `Entity::PLACEHOLDER`.
pub mod entity {
    use super::*;

    pub fn serialize<S: Serializer>(e: &Entity, s: S) -> Result<S::Ok, S::Error> {
        id_of(*e).serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Entity, D::Error> {
        Ok(entity_of(u32::deserialize(d)?).unwrap_or(Entity::PLACEHOLDER))
    }
}

/// `Option<Entity>` fields; references to entities not in the save are lost.
pub mod opt_entity {
    use super::*;

    pub fn serialize<S: Serializer>(e: &Option<Entity>, s: S) -> Result<S::Ok, S::Error> {
        e.map(id_of).filter(|&i| i != NO_ID).serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Entity>, D::Error> {
        Ok(Option::<u32>::deserialize(d)?.and_then(entity_of))
    }
}

/// `Vec<Entity>` fields; entities not in the save are dropped.
pub mod entities {
    use super::*;

    pub fn serialize<S: Serializer>(list: &[Entity], s: S) -> Result<S::Ok, S::Error> {
        list.iter().map(|e| id_of(*e)).filter(|&i| i != NO_ID).collect::<Vec<_>>().serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Entity>, D::Error> {
        Ok(Vec::<u32>::deserialize(d)?.into_iter().filter_map(entity_of).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize, Deserialize)]
    struct Refs {
        #[serde(with = "entity")]
        one: Entity,
        #[serde(with = "opt_entity")]
        maybe: Option<Entity>,
        #[serde(with = "entities")]
        many: Vec<Entity>,
    }

    #[test]
    fn references_become_ids_and_back() {
        let mut world = World::new();
        let (a, b, gone) = (world.spawn_empty().id(), world.spawn_empty().id(), world.spawn_empty().id());
        let refs = Refs { one: a, maybe: Some(gone), many: vec![b, gone, a] };
        let bytes = saving(HashMap::from([(a, 7), (b, 9)]), || crate::save::encode(&refs)).unwrap();
        let (a2, b2) = (world.spawn_empty().id(), world.spawn_empty().id());
        let back: Refs = loading(HashMap::from([(7, a2), (9, b2)]), || crate::save::decode(&bytes)).unwrap();
        assert_eq!(back.one, a2);
        assert_eq!(back.maybe, None, "entities outside the save are dropped");
        assert_eq!(back.many, vec![b2, a2]);
    }
}
