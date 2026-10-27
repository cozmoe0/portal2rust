//! The script VM in saved games: the heap as a VM snapshot (entity handles
//! written as save ids), the host's class references, and each entity's
//! scope and think state.

use std::collections::HashMap;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use squirrel::Value;
use squirrel::snapshot::{HostValue, VmSnapshot};

use super::entity::ENTITY_TAG;
use super::{ScriptClasses, ScriptHandles, ScriptScope, ScriptVm};
use crate::save::ids::NO_ID;
use crate::save::{decode, encode};

#[derive(Serialize, Deserialize)]
struct SavedScripts {
    vm: VmSnapshot,
    classes: ScriptClasses,
}

pub fn capture(world: &World, ids: &HashMap<Entity, u32>) -> Result<Option<Vec<u8>>, String> {
    let Some(svm) = world.get_resource::<ScriptVm>() else { return Ok(None) };
    let classes = *world.resource::<ScriptClasses>();
    let vm = svm.vm.snapshot(&mut |tag, user| {
        if tag != ENTITY_TAG {
            return user;
        }
        Entity::try_from_bits(user).and_then(|e| ids.get(&e)).map_or(NO_ID as u64, |&id| id as u64)
    })?;
    Ok(Some(encode(&SavedScripts { vm, classes })?))
}

/// Puts the saved VM into the fresh one (made by `init_vm`) and finds the
/// entity handles again.
pub fn restore(world: &mut World, bytes: &[u8], by_id: &HashMap<u32, Entity>) -> Result<(), String> {
    let saved: SavedScripts = decode(bytes)?;
    let mut svm = world.remove_resource::<ScriptVm>().ok_or("no script VM")?;
    let result = svm.vm.restore(&saved.vm, &mut |tag, user| {
        if tag != ENTITY_TAG {
            return user;
        }
        u32::try_from(user).ok().and_then(|id| by_id.get(&id)).unwrap_or(&Entity::PLACEHOLDER).to_bits()
    });
    let handles: Vec<(Entity, Value)> = svm
        .vm
        .instances_with_tag(ENTITY_TAG)
        .into_iter()
        .filter_map(|(r, user)| Some((Entity::try_from_bits(user)?, Value::Instance(r))))
        .filter(|(e, _)| world.get_entity(*e).is_ok())
        .collect();
    world.insert_resource(svm);
    result?;
    world.insert_resource(saved.classes);
    let mut h = world.resource_mut::<ScriptHandles>();
    h.by_entity.clear();
    h.by_entity.extend(handles);
    Ok(())
}

#[derive(Serialize, Deserialize)]
struct ScopeSave {
    scope: HostValue,
    id: String,
    think: Option<String>,
    next_think: f64,
    post_spawn: Vec<HostValue>,
    precache: Vec<HostValue>,
}

pub fn save_scope(world: &World, e: Entity) -> Option<Result<Vec<u8>, String>> {
    let s = world.get::<ScriptScope>(e)?;
    Some(encode(&ScopeSave {
        scope: HostValue::of(&s.scope),
        id: s.id.clone(),
        think: s.think.clone(),
        next_think: s.next_think,
        post_spawn: s.post_spawn.iter().map(HostValue::of).collect(),
        precache: s.precache.iter().map(HostValue::of).collect(),
    }))
}

pub fn load_scope(world: &mut World, e: Entity, bytes: &[u8]) -> Result<(), String> {
    let s: ScopeSave = decode(bytes)?;
    world.entity_mut(e).insert(ScriptScope {
        scope: s.scope.value(),
        id: s.id,
        think: s.think,
        next_think: s.next_think,
        post_spawn: s.post_spawn.iter().map(HostValue::value).collect(),
        precache: s.precache.iter().map(HostValue::value).collect(),
    });
    Ok(())
}
