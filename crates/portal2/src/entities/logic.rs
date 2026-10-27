//! Logic entities: relays, timers, branches, comparisons, counters, cases,
//! instance I/O proxies and teleport points.

use bevy::prelude::*;

use super::io::{FireOutput, InputCall, InputHandlers, fire_output};
use super::{EntityRegistry, IoSet, MapEntity, SpawnHandlers, despawn_entity, world_transform};

pub fn register(app: &mut App) {
    app.add_systems(Update, tick_timers.before(IoSet));
    let world = app.world_mut();
    {
        let mut spawn = world.resource_mut::<SpawnHandlers>();
        spawn.register("logic_relay", spawn_relay);
        spawn.register("logic_timer", spawn_timer);
        spawn.register("logic_branch", spawn_branch);
        spawn.register("logic_branch_listener", spawn_branch_listener);
        spawn.register("logic_compare", spawn_compare);
        spawn.register("math_counter", spawn_counter);
    }
    let mut h = world.resource_mut::<InputHandlers>();
    h.register("logic_relay", relay_input);
    h.register("logic_timer", timer_input);
    h.register("logic_branch", branch_input);
    h.register("logic_branch_listener", branch_listener_input);
    h.register("logic_compare", compare_input);
    h.register("math_counter", counter_input);
    h.register("logic_case", case_input);
    h.register("func_instance_io_proxy", proxy_input);
    h.register("logic_choreographed_scene", scene_input);
    h.register("point_teleport", teleport_input);
    // Classes whose effects are not simulated yet; their inputs are accepted
    // so I/O chains continue.
    for class in [
        "env_fade",
        "env_shake",
        "game_text",
        "point_clientcommand",
        "point_servercommand",
        "logic_achievement",
        "logic_autosave",
        "env_texturetoggle",
        "ambient_generic",
        "logic_script",
        "env_soundscape",
        "env_fog_controller",
        "env_tonemap_controller",
        "light",
        "light_spot",
        "env_projectedtexture",
        "env_spark",
        "env_ar2explosion",
        "info_overlay_accessor",
        "vgui_movie_display",
        "vgui_screen",
        "point_viewcontrol",
        "env_instructor_hint",
        "move_rope",
        "keyframe_rope",
    ] {
        h.register(class, accept_all);
    }
}

fn accept_all(_world: &mut World, _call: &InputCall) -> bool {
    true
}

fn param_f32(call: &InputCall) -> f32 {
    source_formats::kv::parse_f32_lenient(&call.param)
}

// ----------------------------------------------------------------- logic_relay

#[derive(Component)]
pub struct LogicRelay {
    pub enabled: bool,
}

fn spawn_relay(world: &mut World, e: Entity) {
    let enabled = !world.get::<MapEntity>(e).is_some_and(|m| m.get_bool("startdisabled"));
    world.entity_mut(e).insert(LogicRelay { enabled });
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

// ----------------------------------------------------------------- logic_timer

#[derive(Component)]
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
    let now = world.resource::<Time>().elapsed_secs_f64();
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

fn tick_timers(time: Res<Time>, mut timers: Query<(Entity, &mut LogicTimer)>, mut out: MessageWriter<FireOutput>) {
    let now = time.elapsed_secs_f64();
    for (e, mut t) in &mut timers {
        if !t.enabled || now < t.next {
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
    let now = world.resource::<Time>().elapsed_secs_f64();
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

#[derive(Component)]
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

#[derive(Component)]
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
    let listeners: Vec<Entity> = world
        .resource::<EntityRegistry>()
        .by_class
        .get("logic_branch_listener")
        .cloned()
        .unwrap_or_default();
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

#[derive(Component)]
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

#[derive(Component)]
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
        "pickrandom" | "pickrandomshuffle" => {
            let filled: Vec<usize> = cases.iter().filter(|(_, v)| !v.is_empty()).map(|(i, _)| *i).collect();
            let pool: Vec<usize> = if filled.is_empty() { (1..=16).collect() } else { filled };
            let i = pool[((rand_unit() * pool.len() as f32) as usize).min(pool.len() - 1)];
            fire_output(world, e, &format!("OnCase{i:02}"), call.activator, None);
            true
        }
        _ => false,
    }
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

// --------------------------------------------------- logic_choreographed_scene

/// Scenes are not played yet: report a start and a completion shortly after
/// so dependent logic keeps running.
fn scene_input(world: &mut World, call: &InputCall) -> bool {
    match call.input.as_str() {
        "start" => {
            fire_output(world, call.target, "OnStart", call.activator, None);
            let mut queue = world.resource_mut::<super::io::EventQueue>();
            queue.push(
                1.0,
                super::io::EventTarget::Entity(call.target),
                "__scenecomplete",
                "",
                call.activator,
                Some(call.target),
            );
        }
        "__scenecomplete" => fire_output(world, call.target, "OnCompletion", call.activator, None),
        "pause" | "resume" | "cancel" | "reload" | "startwaitingforactor" | "stopwaitingforactor" | "setbreakpoint" => {}
        _ => return false,
    }
    true
}

// --------------------------------------------------------------- point_teleport

fn teleport_input(world: &mut World, call: &InputCall) -> bool {
    if call.input != "teleport" {
        return false;
    }
    let e = call.target;
    let Some(target) = world.get::<MapEntity>(e).and_then(|m| m.get("target").map(|s| s.to_ascii_lowercase())) else {
        return true;
    };
    let dest = world_transform(world, e).compute_transform();
    for t in super::io::resolve_targets(world, &target, call.activator, Some(e)) {
        if let Some(mut tr) = world.get_mut::<Transform>(t) {
            tr.translation = dest.translation;
            tr.rotation = dest.rotation;
        }
    }
    true
}
