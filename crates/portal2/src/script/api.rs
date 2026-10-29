//! Global script functions: I/O, time, randomness, includes, scenes,
//! traces and the helpers the original hosting layer defines.

use bevy::ecs::system::SystemState;
use bevy::prelude::*;
use squirrel::value::NativeFn;
use squirrel::{Ctx, Error, Result, Value, Vm};

use super::{ScriptClasses, entity, vector};
use crate::clock::GameClock;
use crate::entities::io::{EventQueue, EventTarget};
use crate::entities::{EntityRegistry, MapEntity};

/// Persistent GLaDOS dialogue flags (`AddGladosSpokenFlags`), kept across
/// maps.
#[derive(Resource, Default, serde::Serialize, serde::Deserialize)]
pub struct GladosSpokenFlags(pub [u32; 4]);

/// The VM and the world a native runs with.
pub fn parts<'a>(ctx: &'a mut Ctx) -> (&'a mut Vm, &'a mut World) {
    let vm = &mut *ctx.vm;
    let world = ctx.host.downcast_mut::<World>().expect("script host is the world");
    (vm, world)
}

pub fn arg(args: &[Value], i: usize) -> Value {
    args.get(i).cloned().unwrap_or_default()
}

pub fn arg_string(ctx: &mut Ctx, args: &[Value], i: usize) -> Result<String> {
    match args.get(i) {
        Some(Value::Str(s)) => Ok(s.to_string_lossy()),
        Some(Value::Null) | None => Ok(String::new()),
        Some(other) => Ok(ctx.to_string(other)?.to_string_lossy()),
    }
}

pub fn arg_f32(args: &[Value], i: usize) -> f32 {
    args.get(i).and_then(Value::as_f32).unwrap_or(0.0)
}

pub fn arg_vec(vm: &Vm, args: &[Value], i: usize) -> Result<Vec3> {
    args.get(i).and_then(|v| vector::read(vm, v)).ok_or_else(|| Error::runtime(format!("parameter {i} has an invalid type ; expected: 'Vector'")))
}

pub fn make_vec(vm: &mut Vm, world: &World, v: Vec3) -> Value {
    vector::make(vm, world.resource::<ScriptClasses>().vector, v)
}

pub fn register(vm: &mut Vm) -> ScriptClasses {
    let vector = vector::register(vm);
    let classes = entity::register(vm, vector);
    let root = vm.root_value();
    let globals: &[(&str, NativeFn, i32)] = &[
        ("printl", printl, 2),
        ("Msg", msg, 2),
        ("Assert", assert, -2),
        ("Time", time, 1),
        ("FrameTime", frame_time, 1),
        ("DoEntFire", do_ent_fire, -3),
        ("EntFire", ent_fire, -3),
        ("EntFireByHandle", ent_fire_by_handle, -3),
        ("DoUniqueString", unique_string, -1),
        ("UniqueString", unique_string, -1),
        ("DoIncludeScript", do_include_script, -2),
        ("IncludeScript", include_script, -2),
        ("CreateSceneEntity", create_scene_entity, 2),
        ("TraceLine", trace_line, 4),
        ("SendToConsole", send_to_console, 2),
        ("GetMapName", get_map_name, 1),
        ("ShowMessage", show_message, 2),
        ("GetDeveloperLevel", get_developer_level, 1),
        ("SetDucking", ignore, -1),
        ("RecordAchievementEvent", record_achievement_event, -2),
        ("CreateProp", create_prop, -3),
        ("DebugDrawLine", ignore, -1),
        ("DebugDrawBox", ignore, -1),
        ("LoopSinglePlayerMaps", false_fn, 1),
        ("GetMapIndexInPlayOrder", minus_one, 1),
        ("GetNumMapsPlayed", zero, 1),
        ("SetMapAsPlayed", ignore, -1),
        ("RequestMapRating", ignore, -1),
        ("GetPlayer", get_player, 1),
        ("AddGladosSpokenFlags", add_glados_spoken_flags, 3),
        ("GetGladosSpokenFlags", get_glados_spoken_flags, 2),
        ("RandomInt", random_int, 3),
        ("RandomFloat", random_float, 3),
        ("IsMultiplayer", false_fn, 1),
        ("IsLocalSplitScreen", false_fn, 1),
        ("GetBluePlayerIndex", one, 1),
        ("GetOrangePlayerIndex", two, 1),
        ("__DumpScope", dump_scope, 3),
        // Portal 2 additions.
        ("PrecacheMovie", ignore, 2),
        ("GivePlayerPortalgun", give_portalgun, 1),
        ("UpgradePlayerPortalgun", upgrade_portalgun, 1),
        ("UpgradePlayerPotatogun", upgrade_portalgun, 1),
        ("TryDLC1InstalledOrCatch", yes, 1),
        ("GetPlayerSilenceDuration", zero_float, 2),
        ("GetCoopSectionIndex", zero, 1),
        ("GetCoopBranchLevelIndex", zero, 2),
        ("GetHighestActiveBranch", zero, 1),
        ("AddBranchLevelName", ignore, 3),
        ("MarkMapComplete", ignore, 2),
        ("IsLevelComplete", false_fn, 3),
        ("IsPlayerLevelComplete", false_fn, 4),
        ("AddCoopCreditsName", ignore, 2),
        ("ScriptSteamShowURL", false_fn, 2),
        ("ScriptShowHudMessageAll", ignore, 3),
    ];
    for (name, f, n) in globals {
        vm.set_native(&root, name, *f, *n);
    }
    classes
}

fn ignore(_ctx: &mut Ctx, _args: &[Value]) -> Result<Value> {
    Ok(Value::Null)
}

fn false_fn(_ctx: &mut Ctx, _args: &[Value]) -> Result<Value> {
    Ok(Value::Bool(false))
}

fn zero(_ctx: &mut Ctx, _args: &[Value]) -> Result<Value> {
    Ok(Value::Int(0))
}

/// Natives that only report success (`TryDLC1InstalledOrCatch`: the DLC is
/// part of the installed game, so the call never throws).
fn yes(_ctx: &mut Ctx, _args: &[Value]) -> Result<Value> {
    Ok(Value::Bool(true))
}

fn one(_ctx: &mut Ctx, _args: &[Value]) -> Result<Value> {
    Ok(Value::Int(1))
}

fn two(_ctx: &mut Ctx, _args: &[Value]) -> Result<Value> {
    Ok(Value::Int(2))
}

fn minus_one(_ctx: &mut Ctx, _args: &[Value]) -> Result<Value> {
    Ok(Value::Int(-1))
}

fn zero_float(_ctx: &mut Ctx, _args: &[Value]) -> Result<Value> {
    Ok(Value::Float(0.0))
}

fn give_portalgun(ctx: &mut Ctx, _args: &[Value]) -> Result<Value> {
    let (_, world) = parts(ctx);
    crate::portals::gun::enable_portals(world, [true, false]);
    Ok(Value::Null)
}

fn upgrade_portalgun(ctx: &mut Ctx, _args: &[Value]) -> Result<Value> {
    let (_, world) = parts(ctx);
    crate::portals::gun::enable_portals(world, [true, true]);
    Ok(Value::Null)
}

fn printl(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let text = ctx.to_string(&arg(args, 1))?.to_string_lossy();
    (ctx.vm.print)(ctx.host, &format!("{text}\n"));
    Ok(Value::Null)
}

fn msg(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let text = ctx.to_string(&arg(args, 1))?.to_string_lossy();
    (ctx.vm.print)(ctx.host, &text);
    Ok(Value::Null)
}

fn assert(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    if arg(args, 1).truthy() {
        return Ok(Value::Null);
    }
    match args.get(2).filter(|v| !v.is_null()) {
        Some(m) => {
            let m = ctx.to_string(m)?.to_string_lossy();
            Err(Error::runtime(format!("Assertion failed: {m}")))
        }
        None => Err(Error::runtime("Assertion failed")),
    }
}

fn time(ctx: &mut Ctx, _args: &[Value]) -> Result<Value> {
    let (_, world) = parts(ctx);
    Ok(Value::Float(world.resource::<GameClock>().curtime as f32))
}

fn frame_time(ctx: &mut Ctx, _args: &[Value]) -> Result<Value> {
    let (_, world) = parts(ctx);
    Ok(Value::Float(world.resource::<GameClock>().frametime()))
}

/// Entity of an optional handle argument.
fn opt_entity(vm: &Vm, world: &World, v: &Value) -> Option<Entity> {
    entity::entity_of(vm, world, v)
}

/// `DoEntFire(target, action, value, delay, activator, caller)`
fn do_ent_fire(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let target = arg_string(ctx, args, 1)?.to_ascii_lowercase();
    let action = arg_string(ctx, args, 2)?;
    let value = arg_string(ctx, args, 3)?;
    let delay = arg_f32(args, 4);
    let (vm, world) = parts(ctx);
    let activator = opt_entity(vm, world, &arg(args, 5));
    let caller = opt_entity(vm, world, &arg(args, 6));
    world.resource_mut::<EventQueue>().push(delay, EventTarget::Name(target), &action, &value, activator, caller);
    Ok(Value::Null)
}

/// `EntFire(target, action, value = null, delay = 0.0, activator = null)`:
/// the calling scope's `self` is the caller and the default activator.
fn ent_fire(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let target = arg_string(ctx, args, 1)?.to_ascii_lowercase();
    let action = arg_string(ctx, args, 2)?;
    let value = match args.get(3) {
        Some(v) if v.truthy() => ctx.to_string(v)?.to_string_lossy(),
        _ => String::new(),
    };
    let delay = arg_f32(args, 4);
    let this = arg(args, 0);
    let (vm, world) = parts(ctx);
    let me = vm.get(world, &this, &Value::str("self"), true)?.and_then(|s| opt_entity(vm, world, &s));
    let activator = opt_entity(vm, world, &arg(args, 5)).or(me);
    world.resource_mut::<EventQueue>().push(delay, EventTarget::Name(target), &action, &value, activator, me);
    Ok(Value::Null)
}

/// `EntFireByHandle(target, action, value, delay, activator, caller)`
fn ent_fire_by_handle(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let action = arg_string(ctx, args, 2)?;
    let value = arg_string(ctx, args, 3)?;
    let delay = arg_f32(args, 4);
    let (vm, world) = parts(ctx);
    let Some(target) = opt_entity(vm, world, &arg(args, 1)) else { return Ok(Value::Null) };
    let activator = opt_entity(vm, world, &arg(args, 5));
    let caller = opt_entity(vm, world, &arg(args, 6));
    world.resource_mut::<EventQueue>().push(delay, EventTarget::Entity(target), &action, &value, activator, caller);
    Ok(Value::Null)
}

fn unique_string(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let root = arg_string(ctx, args, 1)?;
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    Ok(Value::str(&format!("_{n:x}_{root}")))
}

fn include_into(ctx: &mut Ctx, name: &str, scope: Value) -> Result<Value> {
    let (vm, world) = parts(ctx);
    let scope = if scope.is_null() { vm.root_value() } else { scope };
    let Some(src) = super::load_script(world, name) else {
        let msg = format!("Script not found (scripts/vscripts/{name})");
        world.resource_mut::<crate::console::Console>().print(msg);
        return Ok(Value::Bool(false));
    };
    match vm.run_script(world, &src, name, &scope) {
        Ok(_) => Ok(Value::Bool(true)),
        Err(e) => {
            super::report(world, &e);
            Ok(Value::Bool(false))
        }
    }
}

fn do_include_script(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let name = arg_string(ctx, args, 1)?;
    include_into(ctx, &name, arg(args, 2))
}

/// `IncludeScript(name, scope = null)`: the caller's `this` by default.
fn include_script(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let name = arg_string(ctx, args, 1)?;
    let scope = match arg(args, 2) {
        Value::Null => arg(args, 0),
        s => s,
    };
    include_into(ctx, &name, scope)
}

fn create_scene_entity(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let scene = arg_string(ctx, args, 1)?;
    let (vm, world) = parts(ctx);
    let e = crate::entities::spawn_runtime_entity(world, "logic_choreographed_scene", &[("scenefile", scene.as_str())]);
    Ok(super::handle_in(world, vm, e))
}

/// `TraceLine(start, end, ignore)`: fraction of the line that is clear.
fn trace_line(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let (vm, world) = parts(ctx);
    let start = arg_vec(vm, args, 1)?;
    let end = arg_vec(vm, args, 2)?;
    let ignore: Vec<Entity> = opt_entity(vm, world, &arg(args, 3)).into_iter().collect();
    let mut state: SystemState<crate::collision::Physics> = SystemState::new(world);
    let physics = state.get(world).expect("physics");
    let tr = physics.trace(start, end, Vec3::ZERO, Vec3::ZERO, crate::collision::mask::SOLID_ALL, &ignore);
    Ok(Value::Float(tr.fraction))
}

fn send_to_console(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let cmd = arg_string(ctx, args, 1)?;
    let (_, world) = parts(ctx);
    crate::console::queue_trusted(world, &cmd);
    Ok(Value::Null)
}

fn get_map_name(ctx: &mut Ctx, _args: &[Value]) -> Result<Value> {
    let (_, world) = parts(ctx);
    let name = world.get_resource::<crate::world::CurrentMap>().map(|m| m.name.clone()).unwrap_or_default();
    Ok(Value::str(&name))
}

fn show_message(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let text = arg_string(ctx, args, 1)?;
    let (_, world) = parts(ctx);
    world.resource_mut::<crate::console::Console>().print(format!("(message) {text}"));
    Ok(Value::Null)
}

fn get_developer_level(ctx: &mut Ctx, _args: &[Value]) -> Result<Value> {
    let (_, world) = parts(ctx);
    Ok(Value::Int(world.resource::<crate::console::Console>().i32("developer")))
}

fn record_achievement_event(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let name = arg_string(ctx, args, 1)?;
    debug!("achievement event {name}");
    Ok(Value::Null)
}

/// `CreateProp(class, origin, model, animation)`
fn create_prop(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let class = arg_string(ctx, args, 1)?;
    let model = arg_string(ctx, args, 3)?;
    let (vm, world) = parts(ctx);
    let origin = arg_vec(vm, args, 2).unwrap_or(Vec3::ZERO);
    let at = format!("{} {} {}", origin.x, origin.y, origin.z);
    let e = crate::entities::spawn_runtime_entity(world, &class, &[("origin", at.as_str()), ("model", model.as_str())]);
    Ok(super::handle_in(world, vm, e))
}

fn get_player(ctx: &mut Ctx, _args: &[Value]) -> Result<Value> {
    let (vm, world) = parts(ctx);
    match world.resource::<EntityRegistry>().player {
        Some(p) => Ok(super::handle_in(world, vm, p)),
        None => Ok(Value::Null),
    }
}

fn add_glados_spoken_flags(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let set = args.get(1).and_then(Value::as_i32).unwrap_or(0).clamp(0, 3) as usize;
    let bits = args.get(2).and_then(Value::as_i32).unwrap_or(0) as u32;
    let (_, world) = parts(ctx);
    world.resource_mut::<GladosSpokenFlags>().0[set] |= bits;
    Ok(Value::Null)
}

fn get_glados_spoken_flags(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let set = args.get(1).and_then(Value::as_i32).unwrap_or(0).clamp(0, 3) as usize;
    let (_, world) = parts(ctx);
    Ok(Value::Int(world.resource::<GladosSpokenFlags>().0[set] as i32))
}

/// Script randomness (independent of the VM's `rand`).
fn next_random() -> u32 {
    use std::sync::atomic::{AtomicU32, Ordering};
    static STATE: AtomicU32 = AtomicU32::new(0x1234_5678);
    let mut x = STATE.load(Ordering::Relaxed);
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    STATE.store(x, Ordering::Relaxed);
    x
}

fn random_int(_ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let lo = args.get(1).and_then(Value::as_i32).unwrap_or(0);
    let hi = args.get(2).and_then(Value::as_i32).unwrap_or(0);
    if hi <= lo {
        return Ok(Value::Int(lo));
    }
    let span = (hi as i64 - lo as i64 + 1) as u64;
    Ok(Value::Int((lo as i64 + (next_random() as u64 % span) as i64) as i32))
}

fn random_float(_ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    let lo = arg_f32(args, 1);
    let hi = arg_f32(args, 2);
    let u = (next_random() >> 8) as f32 / (1u32 << 24) as f32;
    Ok(Value::Float(lo + (hi - lo) * u))
}

/// `__DumpScope(depth, table)`: prints a table's contents.
fn dump_scope(ctx: &mut Ctx, args: &[Value]) -> Result<Value> {
    fn dump(ctx: &mut Ctx, depth: usize, t: &Value, out: &mut String) -> Result<()> {
        let entries: Vec<(Value, Value)> = match t {
            Value::Table(r) => ctx.vm.heap.table(*r).map.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            Value::Array(r) => ctx.vm.heap.array(*r).iter().enumerate().map(|(i, v)| (Value::Int(i as i32), v.clone())).collect(),
            _ => return Ok(()),
        };
        for (k, v) in entries {
            out.push_str(&"   ".repeat(depth));
            out.push_str(&ctx.to_string(&k)?.to_string_lossy());
            match &v {
                Value::Table(_) | Value::Array(_) => {
                    out.push_str(if matches!(v, Value::Table(_)) { "(TABLE)\n" } else { "(ARRAY)\n" });
                    dump(ctx, depth + 1, &v, out)?;
                }
                Value::Str(s) => out.push_str(&format!(" = \"{}\"", s.to_string_lossy())),
                other => out.push_str(&format!(" = {}", ctx.to_string(other)?.to_string_lossy())),
            }
            out.push('\n');
        }
        Ok(())
    }
    let depth = args.get(1).and_then(Value::as_i32).unwrap_or(0).max(0) as usize;
    let mut out = String::new();
    dump(ctx, depth, &arg(args, 2), &mut out)?;
    (ctx.vm.print)(ctx.host, &out);
    Ok(Value::Null)
}

/// Name of a map entity for messages.
pub fn describe(world: &World, e: Entity) -> String {
    world.get::<MapEntity>(e).map(|m| format!("{} ({})", m.targetname.as_deref().unwrap_or(""), m.classname)).unwrap_or_default()
}

impl crate::save::Restore for GladosSpokenFlags {}
