//! Entity input/output: output connections, the timed event queue and input
//! dispatch.
//!
//! Outputs fire connections into a queue ordered by fire time. Each frame an
//! exclusive system services every event that is due, calling the input
//! handlers registered for the target's class with full world access, so a
//! chain of zero-delay connections resolves within the same frame.

use std::collections::HashMap;

use bevy::prelude::*;

use super::{EntityRegistry, MapEntity};

/// One output connection: when `output` fires, send `input(param)` to
/// `target` after `delay` seconds, at most `times` times (-1 = unlimited).
#[derive(Clone, Debug)]
pub struct Connection {
    pub output: String,
    pub target: String,
    pub input: String,
    pub param: String,
    pub delay: f32,
    pub times: i32,
}

impl Connection {
    /// Parses `target<sep>input<sep>param<sep>delay<sep>times` where the
    /// separator is ESC (compiled maps) or a comma.
    pub fn parse(output: &str, value: &str) -> Option<Self> {
        let parts: Vec<&str> = if value.contains('\x1b') { value.split('\x1b').collect() } else { value.split(',').collect() };
        if parts.len() < 2 {
            return None;
        }
        Some(Self {
            output: output.to_ascii_lowercase(),
            target: parts[0].trim().to_ascii_lowercase(),
            input: parts[1].trim().to_ascii_lowercase(),
            param: parts.get(2).map(|s| s.to_string()).unwrap_or_default(),
            delay: parts.get(3).map(|s| source_formats::kv::parse_f32_lenient(s)).unwrap_or(0.0),
            times: parts.get(4).map(|s| source_formats::kv::parse_i32_lenient(s)).unwrap_or(-1),
        })
    }

    /// `AddOutput` syntax: `Output target:input:param:delay:times`.
    pub fn parse_add_output(spec: &str) -> Option<Self> {
        let (output, rest) = spec.trim().split_once(char::is_whitespace)?;
        let parts: Vec<&str> = rest.trim().split(':').collect();
        if parts.len() < 2 {
            return None;
        }
        Some(Self {
            output: output.to_ascii_lowercase(),
            target: parts[0].trim().to_ascii_lowercase(),
            input: parts[1].trim().to_ascii_lowercase(),
            param: parts.get(2).map(|s| s.to_string()).unwrap_or_default(),
            delay: parts.get(3).map(|s| source_formats::kv::parse_f32_lenient(s)).unwrap_or(0.0),
            times: parts.get(4).map(|s| source_formats::kv::parse_i32_lenient(s)).unwrap_or(-1),
        })
    }
}

#[derive(Component, Default, Clone, Debug)]
pub struct Outputs {
    pub connections: Vec<Connection>,
}

/// What an event is aimed at.
#[derive(Clone, Debug)]
pub enum EventTarget {
    Name(String),
    Entity(Entity),
}

#[derive(Clone, Debug)]
pub struct QueuedEvent {
    pub time: f64,
    serial: u64,
    pub target: EventTarget,
    pub input: String,
    pub param: String,
    pub activator: Option<Entity>,
    pub caller: Option<Entity>,
}

#[derive(Resource, Default)]
pub struct EventQueue {
    events: Vec<QueuedEvent>,
    serial: u64,
    /// Current game time (seconds), updated each frame before servicing.
    pub now: f64,
}

impl EventQueue {
    pub fn push(
        &mut self,
        delay: f32,
        target: EventTarget,
        input: &str,
        param: &str,
        activator: Option<Entity>,
        caller: Option<Entity>,
    ) {
        self.serial += 1;
        self.events.push(QueuedEvent {
            time: self.now + delay.max(0.0) as f64,
            serial: self.serial,
            target,
            input: input.to_ascii_lowercase(),
            param: param.to_owned(),
            activator,
            caller,
        });
    }

    /// Removes pending events aimed at `entity` (CancelPending).
    pub fn cancel_from(&mut self, caller: Entity) {
        self.events.retain(|e| e.caller != Some(caller));
    }

    fn pop_due(&mut self) -> Option<QueuedEvent> {
        let idx = self
            .events
            .iter()
            .enumerate()
            .filter(|(_, e)| e.time <= self.now + 1e-6)
            .min_by(|a, b| a.1.time.total_cmp(&b.1.time).then(a.1.serial.cmp(&b.1.serial)))
            .map(|(i, _)| i)?;
        Some(self.events.swap_remove(idx))
    }

    pub fn clear(&mut self) {
        self.events.clear();
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

/// Arguments of one input delivery.
#[derive(Clone, Debug)]
pub struct InputCall {
    pub target: Entity,
    pub input: String,
    pub param: String,
    pub activator: Option<Entity>,
    pub caller: Option<Entity>,
}

/// Handler signature: return `true` when the input was consumed.
pub type InputHandler = fn(&mut World, &InputCall) -> bool;

#[derive(Resource, Default)]
pub struct InputHandlers {
    by_class: HashMap<String, Vec<InputHandler>>,
    generic: Vec<InputHandler>,
    /// Classes whose remaining inputs are accepted without effect.
    quiet: std::collections::HashSet<String>,
}

impl InputHandlers {
    pub fn register(&mut self, classname: &str, handler: InputHandler) {
        self.by_class.entry(classname.to_ascii_lowercase()).or_default().push(handler);
    }

    pub fn register_generic(&mut self, handler: InputHandler) {
        self.generic.push(handler);
    }

    /// Inputs of `classname` that neither its handlers nor the generic ones
    /// take are accepted without effect (behaviour not simulated yet).
    pub fn accept_rest(&mut self, classname: &str) {
        self.quiet.insert(classname.to_ascii_lowercase());
    }

    pub fn handles(&self, classname: &str) -> bool {
        let class = classname.to_ascii_lowercase();
        self.by_class.contains_key(&class) || self.quiet.contains(&class)
    }
}

/// Request to fire a named output of an entity (usable from ordinary systems).
#[derive(Message, Clone, Debug)]
pub struct FireOutput {
    pub entity: Entity,
    pub output: String,
    pub activator: Option<Entity>,
    /// Replaces the parameter of connections that left it empty (e.g. OutValue).
    pub value: Option<String>,
}

impl FireOutput {
    pub fn new(entity: Entity, output: &str) -> Self {
        Self { entity, output: output.to_owned(), activator: None, value: None }
    }

    pub fn with_activator(mut self, activator: Option<Entity>) -> Self {
        self.activator = activator;
        self
    }

    pub fn with_value(mut self, value: impl Into<String>) -> Self {
        self.value = Some(value.into());
        self
    }
}

/// Fires an output from inside an exclusive context (input handlers).
pub fn fire_output(world: &mut World, entity: Entity, output: &str, activator: Option<Entity>, value: Option<&str>) {
    let output = output.to_ascii_lowercase();
    let Some(mut outputs) = world.get_mut::<Outputs>(entity) else { return };
    let mut fired = Vec::new();
    for c in outputs.connections.iter_mut().filter(|c| c.output == output) {
        if c.times == 0 {
            continue;
        }
        if c.times > 0 {
            c.times -= 1;
        }
        fired.push(c.clone());
    }
    outputs.connections.retain(|c| c.times != 0);
    if fired.is_empty() {
        return;
    }
    let debug = world.get_resource::<IoDebug>().is_some_and(|d| d.0);
    let mut queue = world.resource_mut::<EventQueue>();
    for c in fired {
        let param = match value {
            Some(v) if c.param.is_empty() => v.to_owned(),
            _ => c.param.clone(),
        };
        if debug {
            info!("io: {output} -> {}.{}({param}) +{}s", c.target, c.input, c.delay);
        }
        queue.push(c.delay, EventTarget::Name(c.target.clone()), &c.input, &param, activator, Some(entity));
    }
}

/// Enables I/O tracing in the log (`--io-debug`).
#[derive(Resource, Default)]
pub struct IoDebug(pub bool);

/// Resolves an output target name to entities, following Source's rules:
/// special `!names`, trailing `*` wildcards, then targetname, then classname.
pub fn resolve_targets(world: &World, name: &str, activator: Option<Entity>, caller: Option<Entity>) -> Vec<Entity> {
    let registry = world.resource::<EntityRegistry>();
    match name {
        "!activator" => return activator.into_iter().collect(),
        "!caller" | "!self" => return caller.into_iter().collect(),
        "!player" | "!pvsplayer" | "!picker" => return registry.player.into_iter().collect(),
        _ => {}
    }
    if let Some(prefix) = name.strip_suffix('*') {
        let mut out: Vec<Entity> = registry
            .by_name
            .iter()
            .filter(|(k, _)| k.starts_with(prefix))
            .flat_map(|(_, v)| v.iter().copied())
            .collect();
        out.sort();
        return out;
    }
    if let Some(list) = registry.by_name.get(name) {
        return list.clone();
    }
    registry.by_class.get(name).cloned().unwrap_or_default()
}

/// Services every due event; handlers may queue more zero-delay events,
/// which are serviced in the same pass (bounded to avoid infinite loops).
pub fn service_events(world: &mut World) {
    let now = world.resource::<crate::clock::GameClock>().curtime;
    world.resource_mut::<EventQueue>().now = now;

    // Outputs requested by ordinary systems.
    let requests: Vec<FireOutput> = world.resource_mut::<Messages<FireOutput>>().drain().collect();
    for r in requests {
        fire_output(world, r.entity, &r.output, r.activator, r.value.as_deref());
    }

    for _ in 0..10_000 {
        let Some(event) = world.resource_mut::<EventQueue>().pop_due() else { break };
        let targets = match &event.target {
            EventTarget::Entity(e) => vec![*e],
            EventTarget::Name(n) => resolve_targets(world, n, event.activator, event.caller),
        };
        for target in targets {
            if world.get_entity(target).is_err() {
                continue;
            }
            let call = InputCall {
                target,
                input: event.input.clone(),
                param: event.param.clone(),
                activator: event.activator,
                caller: event.caller,
            };
            dispatch_input(world, &call);
        }
    }
}

pub fn dispatch_input(world: &mut World, call: &InputCall) {
    // Scripted entities see `activator` / `caller` and may veto the input
    // with an `Input<Name>` function.
    let scripted = world.get::<crate::script::ScriptScope>(call.target).is_some();
    if scripted && !crate::script::begin_input(world, call) {
        crate::script::end_input(world, call.target);
        return;
    }
    dispatch_to_handlers(world, call);
    if scripted {
        crate::script::end_input(world, call.target);
    }
}

fn dispatch_to_handlers(world: &mut World, call: &InputCall) {
    // The player has no map entity; its inputs go to the "player" class.
    let class = match world.get::<MapEntity>(call.target) {
        Some(m) => m.classname.clone(),
        None if world.resource::<EntityRegistry>().player == Some(call.target) => "player".to_owned(),
        None => String::new(),
    };
    let (class_handlers, generic, quiet) = {
        let handlers = world.resource::<InputHandlers>();
        (handlers.by_class.get(&class).cloned().unwrap_or_default(), handlers.generic.clone(), handlers.quiet.contains(&class))
    };
    for h in class_handlers {
        if h(world, call) {
            return;
        }
    }
    for h in generic {
        if h(world, call) {
            return;
        }
    }
    if quiet {
        return;
    }
    if world.get_resource::<IoDebug>().is_some_and(|d| d.0) {
        debug!("io: unhandled input {} on {class}", call.input);
    }
    if let Some(mut stats) = world.get_resource_mut::<IoStats>() {
        *stats.unhandled.entry((class, call.input.clone())).or_default() += 1;
    }
}

/// Inputs no handler accepted, by (class, input), for diagnostics.
#[derive(Resource, Default)]
pub struct IoStats {
    pub unhandled: HashMap<(String, String), u32>,
}
