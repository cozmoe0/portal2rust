//! Logic entities: map start triggers, global states, relays, timers,
//! branches, comparisons, counters, cases, instance I/O proxies and teleport
//! points.

use std::collections::HashMap;

use bevy::prelude::*;

use super::io::{EventQueue, EventTarget, FireOutput, InputCall, InputHandlers, fire_output};
use super::{EntityRegistry, MapEntity, SpawnHandlers, despawn_entity, world_transform};
use portal2_core::clock::GameClock;
use portal2_core::sets::ThinkSet;
use portal2_world::{CurrentMap, MapLoadType};

pub fn register(app: &mut App) {
    app.init_resource::<GlobalStates>().add_systems(FixedUpdate, (fire_relay_spawn_outputs, tick_timers.in_set(ThinkSet)));
    let world = app.world_mut();
    {
        let mut spawn = world.resource_mut::<SpawnHandlers>();
        spawn.register("logic_auto", spawn_auto);
        spawn.register("env_global", spawn_global);
        spawn.register("logic_relay", spawn_relay);
        spawn.register("logic_timer", spawn_timer);
        spawn.register("logic_branch", spawn_branch);
        spawn.register("logic_branch_listener", spawn_branch_listener);
        spawn.register("logic_compare", spawn_compare);
        spawn.register("math_counter", spawn_counter);
        spawn.register("point_teleport", spawn_point_teleport);
        spawn.register("logic_random_outputs", spawn_random_outputs);
        spawn.register("logic_register_activator", spawn_register_activator);
        // Data-only classes: other systems read their keyvalues (trigger
        // filters, teleport destinations, portal placement volumes,
        // visibility brushes, the player start).
        for class in [
            "info_target",
            "info_teleport_destination",
            "path_corner",
            "info_player_start",
            "filter_activator_class",
            "filter_activator_name",
            "filter_activator_model",
            "filter_multi",
            "filter_player_held",
            "filter_damage_type",
            "func_illusionary",
            "func_areaportal",
            "info_lighting_relative",
        ] {
            spawn.register(class, |_, _| {});
        }
    }
    let mut h = world.resource_mut::<InputHandlers>();
    h.register("logic_auto", auto_input);
    h.register("point_servercommand", command_input);
    h.register("point_clientcommand", command_input);
    h.register("env_global", global_input);
    h.register("logic_relay", relay_input);
    h.register("logic_timer", timer_input);
    h.register("logic_branch", branch_input);
    h.register("logic_branch_listener", branch_listener_input);
    h.register("logic_compare", compare_input);
    h.register("math_counter", counter_input);
    h.register("logic_case", case_input);
    h.register("func_instance_io_proxy", proxy_input);
    h.register("point_teleport", teleport_input);
    h.register("logic_random_outputs", random_outputs_input);
    h.register("logic_register_activator", register_activator_input);
    // Classes whose effects are not simulated yet; their inputs are accepted
    // so I/O chains continue. Area portals only cull drawing in the engine
    // (everything is drawn here); AI relationships only aim turrets at
    // things other than the player (`sp_a2_bts4`'s test dummy).
    for class in [
        "logic_achievement",
        "env_spark",
        "info_overlay_accessor",
        "move_rope",
        "keyframe_rope",
        "func_areaportal",
        "ai_relationship",
        "npc_enemyfinder",
    ] {
        h.accept_rest(class);
    }
}

fn param_f32(call: &InputCall) -> f32 {
    source_formats::kv::parse_f32_lenient(&call.param)
}

fn curtime(world: &World) -> f64 {
    world.resource::<GameClock>().curtime
}

// ------------------------------------------------------------------ logic_auto

/// logic_auto thinks this long after the map activates, then fires.
const AUTO_THINK_DELAY: f32 = 0.2;
/// spawnflags: remove after firing.
const SF_AUTO_FIRE_ONCE: u32 = 1;

fn spawn_auto(world: &mut World, e: Entity) {
    world.resource_mut::<EventQueue>().push(AUTO_THINK_DELAY, EventTarget::Entity(e), "__autothink", "", None, None);
}

fn auto_input(world: &mut World, call: &InputCall) -> bool {
    if call.input != "__autothink" {
        return false;
    }
    let e = call.target;
    let Some(me) = world.get::<MapEntity>(e) else { return true };
    // Optionally gated on a global state being on.
    if let Some(state) = me.get("globalstate").filter(|s| !s.is_empty())
        && world.resource::<GlobalStates>().get(state) != Some(GlobalState::On)
    {
        return true;
    }
    let remove = me.has_flag(SF_AUTO_FIRE_ONCE);
    let output = match world.get_resource::<CurrentMap>().map(|m| m.load_type).unwrap_or_default() {
        MapLoadType::NewGame => "OnNewGame",
        MapLoadType::LoadGame => "OnLoadGame",
        MapLoadType::Transition => "OnMapTransition",
        MapLoadType::Background => "OnBackgroundMap",
    };
    fire_output(world, e, output, None, None);
    fire_output(world, e, "OnMapSpawn", None, None);
    if remove {
        despawn_entity(world, e);
    }
    true
}

// ------------------------------------------- point_servercommand / clientcommand

/// `Command`: runs the parameter as console text.
fn command_input(world: &mut World, call: &InputCall) -> bool {
    if call.input != "command" {
        return false;
    }
    portal2_core::console::queue_trusted(world, &call.param);
    true
}

// ------------------------------------------------------------------ env_global

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GlobalState {
    Off,
    On,
    Dead,
}

/// Named global states; they outlive map changes.
#[derive(Resource, Default, serde::Serialize, serde::Deserialize)]
pub struct GlobalStates {
    states: HashMap<String, (GlobalState, i32)>,
}

impl GlobalStates {
    pub fn get(&self, name: &str) -> Option<GlobalState> {
        self.states.get(&name.to_ascii_lowercase()).map(|s| s.0)
    }

    pub fn set(&mut self, name: &str, state: GlobalState) {
        self.states.entry(name.to_ascii_lowercase()).or_insert((state, 0)).0 = state;
    }

    pub fn counter(&self, name: &str) -> i32 {
        self.states.get(&name.to_ascii_lowercase()).map(|s| s.1).unwrap_or(0)
    }

    pub fn set_counter(&mut self, name: &str, value: i32) {
        self.states.entry(name.to_ascii_lowercase()).or_insert((GlobalState::Off, 0)).1 = value;
    }
}

/// spawnflags: create the state with its initial value if it does not exist.
const SF_GLOBAL_SET: u32 = 1;

fn global_name(world: &World, e: Entity) -> Option<String> {
    world.get::<MapEntity>(e).and_then(|m| m.get("globalstate")).filter(|s| !s.is_empty()).map(str::to_owned)
}

fn spawn_global(world: &mut World, e: Entity) {
    let Some(me) = world.get::<MapEntity>(e).cloned() else { return };
    let Some(name) = global_name(world, e) else { return };
    if !me.has_flag(SF_GLOBAL_SET) || world.resource::<GlobalStates>().get(&name).is_some() {
        return;
    }
    let state = match me.get_i32("initialstate").unwrap_or(0) {
        1 => GlobalState::On,
        2 => GlobalState::Dead,
        _ => GlobalState::Off,
    };
    let mut states = world.resource_mut::<GlobalStates>();
    states.set(&name, state);
    if let Some(c) = me.get_i32("counter").filter(|c| *c != 0) {
        states.set_counter(&name, c);
    }
}

fn global_input(world: &mut World, call: &InputCall) -> bool {
    let Some(name) = global_name(world, call.target) else { return true };
    let p = source_formats::kv::parse_i32_lenient(&call.param);
    if call.input == "getcounter" {
        let c = world.resource::<GlobalStates>().counter(&name).to_string();
        fire_output(world, call.target, "OutCounter", call.activator, Some(&c));
        return true;
    }
    let mut states = world.resource_mut::<GlobalStates>();
    match call.input.as_str() {
        "turnon" => states.set(&name, GlobalState::On),
        "turnoff" => states.set(&name, GlobalState::Off),
        "remove" => states.set(&name, GlobalState::Dead),
        "toggle" => {
            let next = if states.get(&name) == Some(GlobalState::On) { GlobalState::Off } else { GlobalState::On };
            states.set(&name, next);
        }
        "setcounter" => states.set_counter(&name, p),
        "addtocounter" => {
            let c = states.counter(&name);
            states.set_counter(&name, c + p);
        }
        _ => return false,
    }
    true
}

// ----------------------------------------------------------------- logic_relay

#[derive(Component, serde::Serialize, serde::Deserialize)]
pub struct LogicRelay {
    pub enabled: bool,
}

/// A relay that has yet to fire `OnSpawn`. Not saved: a restored relay fires
/// whatever `OnSpawn` connections it still has (one-shot ones that already
/// went out are gone from its outputs).
#[derive(Component)]
struct PendingSpawnOutput;

fn spawn_relay(world: &mut World, e: Entity) {
    let enabled = !world.get::<MapEntity>(e).is_some_and(|m| m.get_bool("startdisabled"));
    world.entity_mut(e).insert((LogicRelay { enabled }, PendingSpawnOutput));
}

/// `OnSpawn` goes out once the map (or the restored game) has spawned, as the
/// original's relay fires it from a think just after activating; a relay set
/// to fire only once then removes itself. Maps use it to set up animated
/// props, such as hanging wall panels on the arms that move them.
fn fire_relay_spawn_outputs(world: &mut World, mut pending: Local<QueryState<Entity, With<PendingSpawnOutput>>>) {
    let relays: Vec<Entity> = pending.iter(world).collect();
    for e in relays {
        world.entity_mut(e).remove::<PendingSpawnOutput>();
        let fires = world.get::<super::io::Outputs>(e).is_some_and(|o| o.connections.iter().any(|c| c.output == "onspawn"));
        if !fires {
            continue;
        }
        fire_output(world, e, "OnSpawn", None, None);
        if world.get::<MapEntity>(e).is_some_and(|m| m.has_flag(1)) {
            despawn_entity(world, e);
        }
    }
}

fn relay_input(world: &mut World, call: &InputCall) -> bool {
    let e = call.target;
    match call.input.as_str() {
        "trigger" => {
            if world.get::<LogicRelay>(e).is_some_and(|r| r.enabled) {
                fire_output(world, e, "OnTrigger", call.activator, None);
                if world.get::<MapEntity>(e).is_some_and(|m| m.has_flag(1)) {
                    despawn_entity(world, e);
                }
            }
        }
        "enable" | "disable" | "toggle" => {
            if let Some(mut r) = world.get_mut::<LogicRelay>(e) {
                r.enabled = match call.input.as_str() {
                    "enable" => true,
                    "disable" => false,
                    _ => !r.enabled,
                };
            }
        }
        "cancelpending" => world.resource_mut::<super::io::EventQueue>().cancel_from(e),
        "enablerefire" => {}
        _ => return false,
    }
    true
}

// -------------------------------------------------------- logic_random_outputs

#[derive(Component, serde::Serialize, serde::Deserialize)]
struct RandomOutputs {
    enabled: bool,
}

fn spawn_random_outputs(world: &mut World, e: Entity) {
    let enabled = !world.get::<MapEntity>(e).is_some_and(|m| m.get_bool("startdisabled"));
    world.entity_mut(e).insert(RandomOutputs { enabled });
    fire_output(world, e, "OnSpawn", None, None);
}

/// `Trigger` fires each of `OnTrigger1..8` with its own chance; with
/// spawnflag 1 the entity then removes itself.
fn random_outputs_input(world: &mut World, call: &InputCall) -> bool {
    let e = call.target;
    match call.input.as_str() {
        "trigger" => {
            if !world.get::<RandomOutputs>(e).is_some_and(|r| r.enabled) {
                return true;
            }
            for i in 1..=8 {
                let chance = world.get::<MapEntity>(e).and_then(|m| m.get_f32(&format!("ontriggerchance{i}"))).unwrap_or(1.0);
                if rand_unit() <= chance {
                    fire_output(world, e, &format!("OnTrigger{i}"), call.activator, None);
                }
            }
            if world.get::<MapEntity>(e).is_some_and(|m| m.has_flag(1)) {
                despawn_entity(world, e);
            }
        }
        "enable" | "disable" | "toggle" => {
            if let Some(mut r) = world.get_mut::<RandomOutputs>(e) {
                r.enabled = match call.input.as_str() {
                    "enable" => true,
                    "disable" => false,
                    _ => !r.enabled,
                };
            }
        }
        "cancelpending" => world.resource_mut::<super::io::EventQueue>().cancel_from(e),
        _ => return false,
    }
    true
}

// ---------------------------------------------------- logic_register_activator

/// Keeps an entity to send later messages with it as the activator.
#[derive(Component, serde::Serialize, serde::Deserialize)]
struct RegisterActivator {
    enabled: bool,
    #[serde(with = "portal2_core::save::ids::opt_entity")]
    registered: Option<Entity>,
}

fn spawn_register_activator(world: &mut World, e: Entity) {
    let enabled = !world.get::<MapEntity>(e).is_some_and(|m| m.get_bool("startdisabled"));
    world.entity_mut(e).insert(RegisterActivator { enabled, registered: None });
}

fn register_activator_input(world: &mut World, call: &InputCall) -> bool {
    let e = call.target;
    let input = call.input.as_str();
    if input == "registerentity" {
        let name = call.param.trim().to_ascii_lowercase();
        let found = if name.is_empty() {
            call.activator
        } else {
            super::io::resolve_targets(world, &name, call.activator, call.caller).into_iter().next()
        };
        if let Some(mut r) = world.get_mut::<RegisterActivator>(e) {
            r.registered = found;
        }
    } else if let Some(n) = input.strip_prefix("fireregisteredasactivator") {
        let Some(r) = world.get::<RegisterActivator>(e) else { return true };
        if let (true, Some(registered)) = (r.enabled, r.registered)
            && world.get_entity(registered).is_ok()
        {
            fire_output(world, e, &format!("OnRegisteredActivate{n}"), Some(registered), None);
        }
    } else if matches!(input, "enable" | "disable" | "toggle") {
        if let Some(mut r) = world.get_mut::<RegisterActivator>(e) {
            r.enabled = match input {
                "enable" => true,
                "disable" => false,
                _ => !r.enabled,
            };
        }
    } else {
        return false;
    }
    true
}

// ----------------------------------------------------------------- logic_timer

#[derive(Component, serde::Serialize, serde::Deserialize)]
pub struct LogicTimer {
    pub enabled: bool,
    pub refire: f32,
    pub random: bool,
    pub lower: f32,
    pub upper: f32,
    pub next: f64,
    pub oscillator: bool,
    pub high: bool,
}

impl LogicTimer {
    fn interval(&self) -> f32 {
        if self.random {
            let r = rand_unit();
            self.lower + (self.upper - self.lower).max(0.0) * r
        } else {
            self.refire.max(0.01)
        }
    }
}

/// Small xorshift generator; gameplay randomness does not need a crate.
fn rand_unit() -> f32 {
    use std::sync::atomic::{AtomicU32, Ordering};
    static STATE: AtomicU32 = AtomicU32::new(0x9E37_79B9);
    let mut x = STATE.load(Ordering::Relaxed);
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    STATE.store(x, Ordering::Relaxed);
    (x >> 8) as f32 / (1u32 << 24) as f32
}

fn spawn_timer(world: &mut World, e: Entity) {
    let now = curtime(world);
    let Some(m) = world.get::<MapEntity>(e) else { return };
    let mut t = LogicTimer {
        enabled: !m.get_bool("startdisabled"),
        refire: m.get_f32("refiretime").unwrap_or(1.0),
        random: m.get_bool("userandomtime"),
        lower: m.get_f32("lowerrandombound").unwrap_or(0.0),
        upper: m.get_f32("upperrandombound").unwrap_or(1.0),
        next: 0.0,
        oscillator: m.has_flag(1),
        high: false,
    };
    t.next = now + t.interval() as f64;
    world.entity_mut(e).insert(t);
}

fn tick_timers(clock: Res<GameClock>, mut timers: Query<(Entity, &mut LogicTimer)>, mut out: MessageWriter<FireOutput>) {
    let now = clock.curtime;
    for (e, mut t) in &mut timers {
        if !t.enabled || !clock.due(t.next) {
            continue;
        }
        if t.oscillator {
            t.high = !t.high;
            out.write(FireOutput::new(e, if t.high { "OnTimerHigh" } else { "OnTimerLow" }));
        } else {
            out.write(FireOutput::new(e, "OnTimer"));
        }
        t.next = now + t.interval() as f64;
    }
}

fn timer_input(world: &mut World, call: &InputCall) -> bool {
    let e = call.target;
    let now = curtime(world);
    let fire_now = call.input == "firetimer";
    let Some(mut t) = world.get_mut::<LogicTimer>(e) else { return false };
    match call.input.as_str() {
        "enable" => {
            if !t.enabled {
                t.enabled = true;
                t.next = now + t.interval() as f64;
            }
        }
        "disable" => t.enabled = false,
        "toggle" => {
            t.enabled = !t.enabled;
            t.next = now + t.interval() as f64;
        }
        "refiretime" => {
            t.refire = param_f32(call);
            t.next = now + t.interval() as f64;
        }
        "resettimer" => t.next = now + t.interval() as f64,
        "lowerrandombound" => t.lower = param_f32(call),
        "upperrandombound" => t.upper = param_f32(call),
        "addtotimer" => t.next += param_f32(call) as f64,
        "subtractfromtimer" => t.next -= param_f32(call) as f64,
        "firetimer" => t.next = now + t.interval() as f64,
        _ => return false,
    }
    if fire_now {
        fire_output(world, e, "OnTimer", call.activator, None);
    }
    true
}

// ---------------------------------------------------------------- logic_branch

#[derive(Component, serde::Serialize, serde::Deserialize)]
pub struct LogicBranch {
    pub value: bool,
}

fn spawn_branch(world: &mut World, e: Entity) {
    let value = world.get::<MapEntity>(e).is_some_and(|m| m.get_bool("initialvalue"));
    world.entity_mut(e).insert(LogicBranch { value });
}

fn branch_input(world: &mut World, call: &InputCall) -> bool {
    let e = call.target;
    let Some(current) = world.get::<LogicBranch>(e).map(|b| b.value) else { return false };
    let (new_value, test) = match call.input.as_str() {
        "setvalue" => (source_formats::kv::parse_i32_lenient(&call.param) != 0, false),
        "setvaluetest" => (source_formats::kv::parse_i32_lenient(&call.param) != 0, true),
        "toggle" => (!current, false),
        "toggletest" => (!current, true),
        "test" => (current, true),
        _ => return false,
    };
    if let Some(mut b) = world.get_mut::<LogicBranch>(e) {
        b.value = new_value;
    }
    if test {
        fire_output(world, e, if new_value { "OnTrue" } else { "OnFalse" }, call.activator, None);
    }
    if new_value != current {
        notify_branch_listeners(world, e, call.activator);
    }
    true
}

#[derive(Component, serde::Serialize, serde::Deserialize)]
pub struct BranchListener {
    pub branches: Vec<String>,
    pub last: Option<u8>,
}

fn spawn_branch_listener(world: &mut World, e: Entity) {
    let Some(m) = world.get::<MapEntity>(e) else { return };
    let branches =
        (1..=16).filter_map(|i| m.get(&format!("branch{i:02}")).map(|s| s.to_ascii_lowercase())).filter(|s| !s.is_empty()).collect();
    world.entity_mut(e).insert(BranchListener { branches, last: None });
}

/// 0 = all false, 1 = mixed, 2 = all true.
fn listener_state(world: &World, listener: &BranchListener) -> Option<u8> {
    let mut any_true = false;
    let mut any_false = false;
    for name in &listener.branches {
        for b in super::io::resolve_targets(world, name, None, None) {
            match world.get::<LogicBranch>(b) {
                Some(br) if br.value => any_true = true,
                Some(_) => any_false = true,
                None => {}
            }
        }
    }
    match (any_true, any_false) {
        (true, false) => Some(2),
        (false, true) => Some(0),
        (true, true) => Some(1),
        (false, false) => None,
    }
}

fn evaluate_listener(world: &mut World, e: Entity, force: bool, activator: Option<Entity>) {
    let Some(listener) = world.get::<BranchListener>(e) else { return };
    let state = listener_state(world, listener);
    let last = listener.last;
    let Some(state) = state else { return };
    if !force && last == Some(state) {
        return;
    }
    if let Some(mut l) = world.get_mut::<BranchListener>(e) {
        l.last = Some(state);
    }
    let output = match state {
        0 => "OnAllFalse",
        2 => "OnAllTrue",
        _ => "OnMixed",
    };
    fire_output(world, e, output, activator, None);
}

fn notify_branch_listeners(world: &mut World, branch: Entity, activator: Option<Entity>) {
    let Some(name) = world.get::<MapEntity>(branch).and_then(|m| m.targetname.clone()) else { return };
    let listeners: Vec<Entity> = world.resource::<EntityRegistry>().by_class.get("logic_branch_listener").cloned().unwrap_or_default();
    for l in listeners {
        let watches = world.get::<BranchListener>(l).is_some_and(|bl| {
            bl.branches.iter().any(|b| match b.strip_suffix('*') {
                Some(prefix) => name.starts_with(prefix),
                None => *b == name,
            })
        });
        if watches {
            evaluate_listener(world, l, false, activator);
        }
    }
}

fn branch_listener_input(world: &mut World, call: &InputCall) -> bool {
    match call.input.as_str() {
        "test" => evaluate_listener(world, call.target, true, call.activator),
        "_ontrue" | "_onfalse" => evaluate_listener(world, call.target, false, call.activator),
        _ => return false,
    }
    true
}

// --------------------------------------------------------------- logic_compare

#[derive(Component, serde::Serialize, serde::Deserialize)]
pub struct LogicCompare {
    pub value: f32,
    pub compare: f32,
}

fn spawn_compare(world: &mut World, e: Entity) {
    let Some(m) = world.get::<MapEntity>(e) else { return };
    let c = LogicCompare { value: m.get_f32("initialvalue").unwrap_or(0.0), compare: m.get_f32("comparevalue").unwrap_or(0.0) };
    world.entity_mut(e).insert(c);
}

fn compare_input(world: &mut World, call: &InputCall) -> bool {
    let e = call.target;
    let v = param_f32(call);
    let do_compare = {
        let Some(mut c) = world.get_mut::<LogicCompare>(e) else { return false };
        match call.input.as_str() {
            "setvalue" => {
                c.value = v;
                false
            }
            "setvaluecompare" => {
                c.value = v;
                true
            }
            "setcomparevalue" => {
                c.compare = v;
                false
            }
            "compare" => true,
            _ => return false,
        }
    };
    if do_compare && let Some(c) = world.get::<LogicCompare>(e) {
        let (value, compare) = (c.value, c.compare);
        let s = value.to_string();
        if value < compare {
            fire_output(world, e, "OnLessThan", call.activator, Some(&s));
        }
        if (value - compare).abs() < f32::EPSILON {
            fire_output(world, e, "OnEqualTo", call.activator, Some(&s));
        } else {
            fire_output(world, e, "OnNotEqualTo", call.activator, Some(&s));
        }
        if value > compare {
            fire_output(world, e, "OnGreaterThan", call.activator, Some(&s));
        }
    }
    true
}

// ---------------------------------------------------------------- math_counter

#[derive(Component, serde::Serialize, serde::Deserialize)]
pub struct MathCounter {
    pub value: f32,
    pub min: f32,
    pub max: f32,
    pub enabled: bool,
}

fn spawn_counter(world: &mut World, e: Entity) {
    let Some(m) = world.get::<MapEntity>(e) else { return };
    let c = MathCounter {
        value: m.get_f32("startvalue").unwrap_or(0.0),
        min: m.get_f32("min").unwrap_or(0.0),
        max: m.get_f32("max").unwrap_or(0.0),
        enabled: !m.get_bool("startdisabled"),
    };
    world.entity_mut(e).insert(c);
}

fn format_value(v: f32) -> String {
    if v.fract() == 0.0 { format!("{}", v as i64) } else { v.to_string() }
}

fn counter_set(world: &mut World, e: Entity, new_value: f32, fire: bool, activator: Option<Entity>) {
    let Some(mut c) = world.get_mut::<MathCounter>(e) else { return };
    let old = c.value;
    let mut v = new_value;
    let bounded = c.max != c.min;
    if bounded {
        v = v.clamp(c.min, c.max);
    }
    c.value = v;
    let (min, max) = (c.min, c.max);
    if !fire {
        return;
    }
    let s = format_value(v);
    fire_output(world, e, "OutValue", activator, Some(&s));
    if bounded {
        if v >= max && old < max {
            fire_output(world, e, "OnHitMax", activator, None);
        }
        if v <= min && old > min {
            fire_output(world, e, "OnHitMin", activator, None);
        }
        if old >= max && v < max {
            fire_output(world, e, "OnChangedFromMax", activator, None);
        }
        if old <= min && v > min {
            fire_output(world, e, "OnChangedFromMin", activator, None);
        }
    }
}

fn counter_input(world: &mut World, call: &InputCall) -> bool {
    let e = call.target;
    let p = param_f32(call);
    let Some(c) = world.get::<MathCounter>(e) else { return false };
    let (value, enabled) = (c.value, c.enabled);
    match call.input.as_str() {
        "enable" | "disable" => {
            if let Some(mut c) = world.get_mut::<MathCounter>(e) {
                c.enabled = call.input == "enable";
            }
        }
        "getvalue" => {
            let s = format_value(value);
            fire_output(world, e, "OnGetValue", call.activator, Some(&s));
        }
        "setmaxvalue" | "setminvalue" => {
            if let Some(mut c) = world.get_mut::<MathCounter>(e) {
                if call.input == "setmaxvalue" { c.max = p } else { c.min = p }
            }
        }
        "sethitmax" | "sethitmin" => {
            if let Some(mut c) = world.get_mut::<MathCounter>(e) {
                if call.input == "sethitmax" { c.max = p } else { c.min = p }
            }
        }
        _ if !enabled => {}
        "add" => counter_set(world, e, value + p, true, call.activator),
        "subtract" => counter_set(world, e, value - p, true, call.activator),
        "multiply" => counter_set(world, e, value * p, true, call.activator),
        "divide" => {
            if p != 0.0 {
                counter_set(world, e, value / p, true, call.activator)
            }
        }
        "setvalue" => counter_set(world, e, p, true, call.activator),
        "setvaluenofire" => counter_set(world, e, p, false, call.activator),
        _ => return false,
    }
    true
}

// ------------------------------------------------------------------ logic_case

fn case_input(world: &mut World, call: &InputCall) -> bool {
    let e = call.target;
    let Some(m) = world.get::<MapEntity>(e) else { return false };
    let cases: Vec<(usize, String)> = (1..=16).filter_map(|i| m.get(&format!("case{i:02}")).map(|v| (i, v.to_owned()))).collect();
    match call.input.as_str() {
        "invalue" => {
            match cases.iter().find(|(_, v)| v.eq_ignore_ascii_case(call.param.trim())) {
                Some((i, _)) => fire_output(world, e, &format!("OnCase{i:02}"), call.activator, Some(&call.param)),
                None => fire_output(world, e, "OnDefault", call.activator, Some(&call.param)),
            }
            true
        }
        // Random picks choose among the cases with an output connected
        // (`CLogicCase::BuildCaseMap`), whatever their values: most cases
        // in `sp_a2_bts4`'s turret lines have no value at all.
        "pickrandom" => {
            let pool = connected_cases(world, e);
            if !pool.is_empty() {
                let i = pool[pick(pool.len())];
                fire_output(world, e, &format!("OnCase{i:02}"), call.activator, None);
            }
            true
        }
        // Every connected case once, in random order, before any repeats;
        // a new round does not open with the case that closed the last.
        "pickrandomshuffle" => {
            let mut shuffle = world.get::<CaseShuffle>(e).cloned().unwrap_or_default();
            if shuffle.left.is_empty() {
                shuffle.left = connected_cases(world, e);
                if shuffle.left.is_empty() {
                    return true;
                }
            }
            let mut index = pick(shuffle.left.len());
            if shuffle.left.len() > 1 && Some(shuffle.left[index]) == shuffle.last {
                index = (index + 1) % shuffle.left.len();
            }
            let i = shuffle.left.swap_remove(index);
            shuffle.last = Some(i);
            world.entity_mut(e).insert(shuffle);
            fire_output(world, e, &format!("OnCase{i:02}"), call.activator, None);
            true
        }
        _ => false,
    }
}

/// What is left of a `logic_case`'s shuffled round.
#[derive(Component, Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct CaseShuffle {
    left: Vec<u8>,
    last: Option<u8>,
}

impl portal2_core::save::Restore for CaseShuffle {}

/// Cases (1 to 16) of a `logic_case` with an `OnCaseNN` output connected.
fn connected_cases(world: &World, e: Entity) -> Vec<u8> {
    let Some(outputs) = world.get::<super::io::Outputs>(e) else { return Vec::new() };
    (1..=16u8).filter(|i| outputs.connections.iter().any(|c| c.output.eq_ignore_ascii_case(&format!("oncase{i:02}")))).collect()
}

/// A random index below `n` (non-zero).
fn pick(n: usize) -> usize {
    ((rand_unit() * n as f32) as usize).min(n - 1)
}

// ------------------------------------------------------ func_instance_io_proxy

fn proxy_input(world: &mut World, call: &InputCall) -> bool {
    if call.input.starts_with("onproxyrelay") {
        fire_output(world, call.target, &call.input, call.activator, Some(&call.param));
        return true;
    }
    if call.input == "proxyrelay" {
        let output = if call.param.trim().is_empty() { "OnProxyRelay1".to_owned() } else { call.param.trim().to_owned() };
        fire_output(world, call.target, &output, call.activator, None);
        return true;
    }
    false
}

// --------------------------------------------------------------- point_teleport

/// Where a point_teleport sends its target: its own placement when the map
/// started, or with spawnflag 1 the target's starting placement.
#[derive(Component, Clone, Copy, serde::Serialize, serde::Deserialize)]
struct TeleportHome {
    origin: Vec3,
    angles: Vec3,
}

/// point_teleport: send the target back to where it started.
const SF_TELEPORT_HOME: u32 = 1;

fn spawn_point_teleport(world: &mut World, e: Entity) {
    let Some(t) = world.get::<Transform>(e).copied() else { return };
    let home = TeleportHome { origin: t.translation, angles: portal2_core::coords::quat_to_angles(t.rotation) };
    world.entity_mut(e).insert(home);
}

/// `Teleport` (to the saved placement), `TeleportEntity` (the parameter
/// names the entity) and `TeleportToCurrentPos`. The first entity found is
/// moved; its velocity is kept, and entities with a parent are refused.
fn teleport_input(world: &mut World, call: &InputCall) -> bool {
    let e = call.target;
    let Some(me) = world.get::<MapEntity>(e) else { return false };
    let own_target = me.get("target").map(str::to_ascii_lowercase).unwrap_or_default();
    let teleport_home = me.spawnflags() & SF_TELEPORT_HOME != 0;
    let (target, to_saved) = match call.input.as_str() {
        "teleport" => (own_target, true),
        "teleportentity" => (call.param.trim().to_ascii_lowercase(), true),
        "teleporttocurrentpos" => (own_target, false),
        _ => return false,
    };
    let Some(t) = super::io::resolve_targets(world, &target, call.activator, Some(e)).into_iter().next() else { return true };
    if world.get::<ChildOf>(t).is_some() {
        warn!("point_teleport: can't teleport {target} as it has a parent");
        return true;
    }
    let (origin, angles) = if !to_saved {
        let here = world_transform(world, e).compute_transform();
        (here.translation, portal2_core::coords::quat_to_angles(here.rotation))
    } else {
        let start = if teleport_home { world.get::<MapEntity>(t).map(|m| (m.origin(), m.angles())) } else { None };
        match (start, world.get::<TeleportHome>(e)) {
            (Some(s), _) => s,
            (None, Some(h)) => (h.origin, h.angles),
            (None, None) => return true,
        }
    };
    super::teleport_entity(world, t, origin, Some(angles), None);
    true
}

impl portal2_core::save::Restore for LogicRelay {}

impl portal2_core::save::Restore for LogicTimer {}

impl portal2_core::save::Restore for MathCounter {}

impl portal2_core::save::Restore for LogicBranch {}

impl portal2_core::save::Restore for LogicCompare {}

impl portal2_core::save::Restore for BranchListener {}

impl portal2_core::save::Restore for RandomOutputs {}

impl portal2_core::save::Restore for RegisterActivator {}

impl portal2_core::save::Restore for TeleportHome {}

impl portal2_core::save::Restore for GlobalStates {}

pub fn register_save(r: &mut portal2_core::save::SaveRegistry) {
    r.component::<RandomOutputs>("logic_random_outputs");
    r.component::<RegisterActivator>("logic_register_activator");
    r.component::<TeleportHome>("teleport_home");
    r.component::<CaseShuffle>("case_shuffle");
}
