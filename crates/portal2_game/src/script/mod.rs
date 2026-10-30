//! Map scripting: one Squirrel VM per map running the game's vscripts.
//!
//! Follows the original hosting (see `docs/re/vscript.md`): `mapspawn.nut`
//! runs when the VM starts; entities with a `vscripts` keyvalue get a scope
//! (a table in the root table, delegating to it) and run their files before
//! their own spawn code; `OnPostSpawn` / `Precache` call chains; think
//! functions every 0.1 s; `activator` / `caller` and `Input<Name>` hooks
//! around inputs; `RunScriptCode` and friends.

pub mod api;
pub mod entity;
pub mod save;
pub mod vector;

use std::any::Any;

use bevy::prelude::*;
use squirrel::{ObjRef, Value, Vm};

use crate::entities::io::{EventQueue, EventTarget, InputCall, InputHandlers};
use portal2_core::clock::GameClock;
use portal2_core::entity::MapEntity;
use portal2_core::sets::ThinkSet;
use portal2_core::vfs::GameFs;

/// `sv_script_think_interval`.
pub const THINK_INTERVAL: f64 = 0.1;

pub struct ScriptPlugin;

impl Plugin for ScriptPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ScriptHandles>()
            .init_resource::<api::GladosSpokenFlags>()
            .add_systems(FixedUpdate, (bind_player_global, run_thinks).chain().in_set(ThinkSet))
            .add_systems(FixedLast, collect_garbage.after(portal2_core::sets::IoSet));
        app.world_mut().resource_mut::<InputHandlers>().register_generic(script_inputs);
    }
}

/// The current map's VM.
#[derive(Resource)]
pub struct ScriptVm {
    pub vm: Vm,
}

/// Script classes, for creating handles and vectors from natives.
#[derive(Resource, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct ScriptClasses {
    pub vector: ObjRef,
    pub entity: ObjRef,
    pub animating: ObjRef,
    pub flex: ObjRef,
    pub player: ObjRef,
    pub portal_player: ObjRef,
    pub scene: ObjRef,
    pub camera: ObjRef,
    pub linked_door: ObjRef,
}

/// One script instance per entity, so handles compare equal.
#[derive(Resource, Default)]
pub struct ScriptHandles {
    pub by_entity: std::collections::HashMap<Entity, Value>,
}

/// Script state of an entity that has a scope.
#[derive(Component)]
pub struct ScriptScope {
    pub scope: Value,
    pub id: String,
    pub think: Option<String>,
    pub next_think: f64,
    pub post_spawn: Vec<Value>,
    pub precache: Vec<Value>,
}

/// Prints script output to the console.
fn print_to_console(host: &mut dyn Any, text: &str) {
    if let Some(world) = host.downcast_mut::<World>() {
        let mut console = world.resource_mut::<portal2_core::console::Console>();
        for line in text.trim_end_matches('\n').split('\n') {
            console.print(line.to_owned());
        }
    }
}

/// Reports a script error the way the original does.
pub fn report(world: &mut World, e: &squirrel::Error) {
    let mut console = world.resource_mut::<portal2_core::console::Console>();
    match e {
        squirrel::Error::Compile { .. } => console.print(format!("Script compile error: {e}")),
        squirrel::Error::Runtime { message, stack, .. } => {
            console.print(format!("AN ERROR HAS OCCURED [{message}]"));
            console.print("CALLSTACK".to_owned());
            for s in stack {
                console.print(s.clone());
            }
        }
    }
}

/// Runs `f` with the VM taken out of the world (natives get the world).
/// None when no VM exists or it is already in use.
pub fn with_vm<R>(world: &mut World, f: impl FnOnce(&mut World, &mut Vm) -> R) -> Option<R> {
    if !world.contains_resource::<ScriptVm>() {
        return None;
    }
    Some(world.resource_scope(|world, mut svm: Mut<ScriptVm>| f(world, &mut svm.vm)))
}

/// Calls a script value, reporting errors to the console.
pub fn call(world: &mut World, func: &Value, this: &Value, args: &[Value]) -> Option<Value> {
    with_vm(world, |world, vm| match vm.call(world, func, this, args) {
        Ok(v) => Some(v),
        Err(e) => {
            report(world, &e);
            None
        }
    })
    .flatten()
}

/// Source text of a script under `scripts/vscripts`, adding `.nut`.
pub fn load_script(world: &World, name: &str) -> Option<Vec<u8>> {
    let name = name.trim().replace('\\', "/");
    let file = if name.to_ascii_lowercase().ends_with(".nut") { name } else { format!("{name}.nut") };
    world.resource::<GameFs>().read(&format!("scripts/vscripts/{file}"))
}

/// Compiles and runs a script file in `scope`. False when missing or failing.
pub fn run_file(world: &mut World, name: &str, scope: &Value) -> bool {
    let Some(src) = load_script(world, name) else {
        let msg = format!("Script not found (scripts/vscripts/{name})");
        world.resource_mut::<portal2_core::console::Console>().print(msg);
        return false;
    };
    run_source(world, &src, name, scope)
}

pub fn run_source(world: &mut World, src: &[u8], name: &str, scope: &Value) -> bool {
    with_vm(world, |world, vm| match vm.run_script(world, src, name, scope) {
        Ok(_) => true,
        Err(e) => {
            report(world, &e);
            false
        }
    })
    .unwrap_or(false)
}

/// Starts a fresh VM for the map: natives, helpers, then `mapspawn.nut`.
pub fn init_for_map(world: &mut World) {
    init_vm(world);
    let root = with_vm(world, |_, vm| vm.root_value()).expect("vm");
    if load_script(world, "mapspawn").is_some() {
        run_file(world, "mapspawn", &root);
    }
}

/// A fresh VM with the game's API and no scripts run (a restored game puts
/// its saved VM into it).
pub fn init_vm(world: &mut World) {
    world.remove_resource::<ScriptVm>();
    world.resource_mut::<ScriptHandles>().by_entity.clear();
    let mut vm = Vm::new();
    vm.print = print_to_console;
    let classes = api::register(&mut vm);
    world.insert_resource(classes);
    world.insert_resource(ScriptVm { vm });
}

/// The handle (script instance) of an entity.
pub fn handle(world: &mut World, e: Entity) -> Value {
    if let Some(h) = world.resource::<ScriptHandles>().by_entity.get(&e) {
        return h.clone();
    }
    let classes = *world.resource::<ScriptClasses>();
    let class = entity::class_for(world, e, &classes);
    let Some(h) = with_vm(world, |_, vm| vm.new_instance(class, e.to_bits())) else { return Value::Null };
    world.resource_mut::<ScriptHandles>().by_entity.insert(e, h.clone());
    h
}

/// Handle for natives (VM already borrowed).
pub fn handle_in(world: &mut World, vm: &mut Vm, e: Entity) -> Value {
    if let Some(h) = world.resource::<ScriptHandles>().by_entity.get(&e) {
        return h.clone();
    }
    let classes = *world.resource::<ScriptClasses>();
    let class = entity::class_for(world, e, &classes);
    let h = vm.new_instance(class, e.to_bits());
    world.resource_mut::<ScriptHandles>().by_entity.insert(e, h.clone());
    h
}

/// Unique script id: `_<serial>_<name or class>`.
fn script_id(world: &World, e: Entity) -> String {
    let me = world.get::<MapEntity>(e);
    let base = me.and_then(|m| m.targetname.clone()).unwrap_or_else(|| me.map(|m| m.classname.clone()).unwrap_or_default());
    format!("_{:08x}_{}", e.index_u32(), base)
}

/// Gets or creates the entity's scope (`ValidateScriptScope`).
pub fn validate_scope(world: &mut World, vm: &mut Vm, e: Entity) -> Value {
    if let Some(s) = world.get::<ScriptScope>(e) {
        return s.scope.clone();
    }
    let id = script_id(world, e);
    let root = vm.root_value();
    let scope = vm.new_table();
    vm.raw_set(&scope, Value::str("__vname"), Value::str(&id));
    vm.raw_set(&scope, Value::str("__vrefs"), Value::Int(1));
    if let Value::Table(r) = root {
        vm.set_delegate(&scope, Some(r));
    }
    vm.raw_set(&root, Value::str(&id), scope.clone());
    let h = handle_in(world, vm, e);
    vm.raw_set(&scope, Value::str("self"), h);
    let think = world.get::<MapEntity>(e).and_then(|m| m.get("thinkfunction")).filter(|s| !s.is_empty()).map(str::to_owned);
    world.entity_mut(e).insert(ScriptScope {
        scope: scope.clone(),
        id,
        think,
        next_think: f64::MAX,
        post_spawn: Vec::new(),
        precache: Vec::new(),
    });
    scope
}

pub fn scope_of(world: &World, e: Entity) -> Option<Value> {
    world.get::<ScriptScope>(e).map(|s| s.scope.clone())
}

/// After a script file ran: append `OnPostSpawn` / `Precache` to the
/// entity's call chains when the scope defines them.
fn collect_hooks(world: &mut World, vm: &mut Vm, e: Entity, scope: &Value) {
    let post = vm.raw_get(scope, &Value::str("OnPostSpawn")).filter(|v| matches!(v, Value::Closure(_) | Value::Native(_)));
    let pre = vm.raw_get(scope, &Value::str("Precache")).filter(|v| matches!(v, Value::Closure(_) | Value::Native(_)));
    if let Some(mut s) = world.get_mut::<ScriptScope>(e) {
        s.post_spawn.extend(post);
        s.precache.extend(pre);
    }
}

/// Runs an entity's `vscripts` (before its spawn code) and its Precache
/// chain. Returns whether the entity has scripts.
pub fn run_entity_scripts(world: &mut World, e: Entity) -> bool {
    let Some(files) = world.get::<MapEntity>(e).and_then(|m| m.get("vscripts")).filter(|s| !s.trim().is_empty()).map(str::to_owned) else {
        return false;
    };
    let Some(scope) = with_vm(world, |world, vm| validate_scope(world, vm, e)) else { return false };
    let is_world = world.get::<MapEntity>(e).is_some_and(|m| m.classname == "worldspawn");
    for file in files.split_whitespace() {
        let target = if is_world { with_vm(world, |_, vm| vm.root_value()).unwrap_or_default() } else { scope.clone() };
        run_file(world, file, &target);
        with_vm(world, |world, vm| collect_hooks(world, vm, e, &scope));
    }
    let now = world.resource::<GameClock>().curtime;
    if let Some(mut s) = world.get_mut::<ScriptScope>(e)
        && s.think.is_some()
    {
        s.next_think = now + THINK_INTERVAL;
    }
    dispatch_chain(world, e, false);
    true
}

/// Calls a hook chain from the newest entry to the oldest, stopping when
/// one returns false.
pub fn dispatch_chain(world: &mut World, e: Entity, post_spawn: bool) -> bool {
    let Some(s) = world.get::<ScriptScope>(e) else { return true };
    let chain = if post_spawn { s.post_spawn.clone() } else { s.precache.clone() };
    let scope = s.scope.clone();
    for f in chain.iter().rev() {
        match call_owned(world, e, f, &scope) {
            Some(Value::Bool(false)) => return false,
            Some(v) if !v.is_null() && !v.truthy() && !matches!(v, Value::Int(_) | Value::Float(_)) => return false,
            _ => {}
        }
    }
    true
}

/// Queues the post-spawn dispatch like the original (no delay, the entity
/// as activator and caller).
pub fn queue_post_spawn(world: &mut World, e: Entity) {
    if world.get::<ScriptScope>(e).is_none() {
        return;
    }
    world.resource_mut::<EventQueue>().push(0.0, EventTarget::Entity(e), "callscriptfunction", "DispatchOnPostSpawn", Some(e), Some(e));
}

/// Calls a function of the entity's scope by name.
pub fn call_scope_function(world: &mut World, e: Entity, name: &str) -> Option<Value> {
    if name == "DispatchOnPostSpawn" {
        return Some(Value::Bool(dispatch_chain(world, e, true)));
    }
    if name == "DispatchPrecache" {
        return Some(Value::Bool(dispatch_chain(world, e, false)));
    }
    let scope = scope_of(world, e)?;
    let f = with_vm(world, |world, vm| vm.get(world, &scope, &Value::str(name), false).ok().flatten()).flatten()?;
    call_owned(world, e, &f, &scope)
}

/// Sets `activator` and `caller` (in the root table, as the original does)
/// and runs the target's `Input<Name>` hook. Returns false when the hook
/// vetoes the input.
pub fn begin_input(world: &mut World, call_info: &InputCall) -> bool {
    let Some(scope) = scope_of(world, call_info.target) else { return true };
    let activator = call_info.activator.map(|a| handle(world, a)).unwrap_or_default();
    let caller = call_info.caller.map(|c| handle(world, c)).unwrap_or_default();
    let hook = with_vm(world, |_, vm| {
        let root = vm.root_value();
        vm.raw_set(&root, Value::str("activator"), activator);
        vm.raw_set(&root, Value::str("caller"), caller);
        // Inputs are stored lowercase; hooks are matched ignoring case.
        let wanted = format!("input{}", call_info.input);
        let Value::Table(t) = &scope else { return None };
        vm.heap
            .table(*t)
            .map
            .iter()
            .find(|(k, v)| {
                matches!(v, Value::Closure(_) | Value::Native(_))
                    && k.as_str().is_some_and(|s| s.to_string_lossy().eq_ignore_ascii_case(&wanted))
            })
            .map(|(_, v)| v.clone())
    })
    .flatten();
    match hook {
        Some(f) => !matches!(call_owned(world, call_info.target, &f, &scope), Some(Value::Bool(false))),
        None => true,
    }
}

/// Clears `activator` and `caller` after an input.
pub fn end_input(world: &mut World, target: Entity) {
    if world.get::<ScriptScope>(target).is_none() {
        return;
    }
    with_vm(world, |_, vm| {
        if let Value::Table(root) = vm.root_value() {
            vm.heap.table_mut(root).map.remove(&Value::str("activator"));
            vm.heap.table_mut(root).map.remove(&Value::str("caller"));
        }
    });
}

/// Calls a scope function the way entity code does: `owninginstance` (in
/// the root table) is the entity for the duration of the call.
pub fn call_owned(world: &mut World, e: Entity, f: &Value, scope: &Value) -> Option<Value> {
    let me = handle(world, e);
    with_vm(world, |_, vm| {
        let root = vm.root_value();
        vm.raw_set(&root, Value::str("owninginstance"), me);
    });
    let r = call(world, f, scope, &[]);
    with_vm(world, |_, vm| {
        if let Value::Table(root) = vm.root_value() {
            vm.heap.table_mut(root).map.remove(&Value::str("owninginstance"));
        }
    });
    r
}

/// Runs code in an entity's scope, creating the scope when needed.
pub fn run_code(world: &mut World, e: Entity, code: &str, name: &str) -> bool {
    let Some(scope) = with_vm(world, |world, vm| validate_scope(world, vm, e)) else { return false };
    run_source(world, code.as_bytes(), name, &scope)
}

/// RunScriptCode, RunScriptFile and CallScriptFunction on any entity.
fn script_inputs(world: &mut World, c: &InputCall) -> bool {
    match c.input.as_str() {
        "runscriptcode" => {
            let code = c.param.clone();
            run_code(world, c.target, &code, "InputRunScript");
            true
        }
        "runscriptfile" => {
            let Some(scope) = with_vm(world, |world, vm| validate_scope(world, vm, c.target)) else { return true };
            let file = c.param.trim().to_owned();
            run_file(world, &file, &scope);
            true
        }
        "callscriptfunction" => {
            if world.get::<ScriptScope>(c.target).is_some() {
                let name = c.param.trim().to_owned();
                call_scope_function(world, c.target, &name);
            }
            true
        }
        _ => false,
    }
}

/// The single-player VM has a global `player` (the player's handle).
fn bind_player_global(world: &mut World) {
    let Some(p) = world.resource::<portal2_core::entity::EntityRegistry>().player else { return };
    let h = handle(world, p);
    with_vm(world, |_, vm| {
        let root = vm.root_value();
        if !vm.raw_get(&root, &Value::str("player")).is_some_and(|v| v.raw_eq(&h)) {
            vm.raw_set(&root, Value::str("player"), h);
        }
    });
}

/// Think functions of scripted entities.
fn run_thinks(world: &mut World) {
    let clock = *world.resource::<GameClock>();
    let mut due: Vec<(Entity, String)> = Vec::new();
    let mut q = world.query::<(Entity, &ScriptScope)>();
    for (e, s) in q.iter(world) {
        if let Some(t) = &s.think
            && clock.due(s.next_think)
        {
            due.push((e, t.clone()));
        }
    }
    for (e, name) in due {
        let Some(scope) = scope_of(world, e) else { continue };
        let f = with_vm(world, |world, vm| vm.get(world, &scope, &Value::str(&name), false).ok().flatten()).flatten();
        let result = f.and_then(|f| call_owned(world, e, &f, &scope));
        let Some(mut s) = world.get_mut::<ScriptScope>(e) else { continue };
        match result {
            Some(v) => {
                let interval = v.as_f32().map(|x| x as f64).unwrap_or(THINK_INTERVAL);
                s.next_think = clock.curtime + interval;
            }
            None => {
                warn!("{} FAILED to call script think function {name}!", s.id);
                s.think = None;
            }
        }
    }
}

/// Frees unreachable script objects once a second.
fn collect_garbage(world: &mut World) {
    let tick = world.resource::<GameClock>().tick;
    if !tick.is_multiple_of(60) || !world.contains_resource::<ScriptVm>() {
        return;
    }
    let mut extra: Vec<Value> = world.resource::<ScriptHandles>().by_entity.values().cloned().collect();
    let mut q = world.query::<&ScriptScope>();
    for s in q.iter(world) {
        extra.push(s.scope.clone());
        extra.extend(s.post_spawn.iter().cloned());
        extra.extend(s.precache.iter().cloned());
    }
    world.resource_mut::<ScriptVm>().vm.collect_garbage_with(&extra);
}
