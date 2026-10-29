//! Lesson definitions from the game's lesson scripts and the small command
//! language their event handlers use (`"icon_target ?set" "entity
//! hint_target"`, `"local_player is" "player userid"`, `"void scope in"`
//! ...). A handler runs its commands in order against a lesson's variables;
//! a failed test fails the handler unless it sits inside a scope, which is
//! then skipped to its end.

use std::sync::Arc;

use bevy::prelude::*;
use source_formats::kv::KeyValues;

/// A value carried by a game event.
#[derive(Clone, Debug, PartialEq)]
pub enum EventValue {
    Int(i32),
    Float(f32),
    Str(String),
    /// Players (`userid`) and entities (entity indices) alike.
    Entity(Option<Entity>),
}

/// A named game event with its fields, as the server and client code fire
/// them; the game instructor listens.
#[derive(Message, Clone, Debug)]
pub struct GameEvent {
    pub name: String,
    pub fields: Vec<(String, EventValue)>,
}

impl GameEvent {
    pub fn new(name: &str) -> Self {
        Self { name: name.to_owned(), fields: Vec::new() }
    }

    pub fn with(mut self, key: &str, value: EventValue) -> Self {
        self.fields.push((key.to_owned(), value));
        self
    }

    pub fn get(&self, key: &str) -> Option<&EventValue> {
        self.fields.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v)
    }
}

/// The variables of a lesson instance (the root lesson's settings are the
/// defaults).
#[derive(Clone, Debug)]
pub struct LessonVars {
    pub icon_target: Option<Entity>,
    pub entity1: Option<Entity>,
    pub entity2: Option<Entity>,
    pub string1: String,
    pub string2: String,
    pub integer1: i32,
    pub integer2: i32,
    pub float1: f32,
    pub float2: f32,
    pub timeout: f32,
    pub onscreen_icon: String,
    pub offscreen_icon: String,
    pub caption: String,
    pub caption_color: [u8; 3],
    pub icon_offset: f32,
    pub range: f32,
    pub flags: u32,
    pub binding: String,
    pub gamepad_binding: String,
    pub allow_nodraw_target: bool,
    pub no_offscreen: bool,
    pub force_caption: bool,
    pub replace_key: String,
    pub visible: bool,
}

impl Default for LessonVars {
    fn default() -> Self {
        Self {
            icon_target: None,
            entity1: None,
            entity2: None,
            string1: String::new(),
            string2: String::new(),
            integer1: 0,
            integer2: 0,
            float1: 0.0,
            float2: 0.0,
            timeout: 0.0,
            onscreen_icon: "icon_tip".into(),
            offscreen_icon: "icon_tip".into(),
            caption: String::new(),
            caption_color: [255, 255, 255],
            icon_offset: 0.0,
            range: 0.0,
            flags: 0,
            binding: String::new(),
            gamepad_binding: String::new(),
            allow_nodraw_target: true,
            no_offscreen: false,
            force_caption: false,
            replace_key: String::new(),
            visible: true,
        }
    }
}

/// `"r,g,b"` (commas or spaces).
fn parse_color(s: &str) -> [u8; 3] {
    let v: Vec<u8> =
        s.split([',', ' ']).filter(|p| !p.is_empty()).filter_map(|p| p.trim().parse::<i32>().ok()).map(|n| n.clamp(0, 255) as u8).collect();
    if v.len() >= 3 { [v[0], v[1], v[2]] } else { [255, 255, 255] }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Set,
    Is,
    Less,
    Greater,
    Add,
    Subtract,
    Multiply,
    Divide,
}

#[derive(Clone, Debug)]
enum Command {
    ScopeIn,
    ScopeOut,
    Close,
    /// `void is multiplayer` (negated with `!is`).
    Multiplayer {
        negate: bool,
    },
    Var {
        var: String,
        /// Accessor on an entity variable (`classname`, `team`).
        member: Option<String>,
        op: Op,
        negate: bool,
        /// `?set`: only when the parameter exists.
        optional: bool,
        ty: String,
        param: String,
    },
}

fn parse_command(key: &str, value: &str) -> Option<Command> {
    let key = key.trim().to_ascii_lowercase();
    let words: Vec<&str> = key.split_whitespace().collect();
    if words.first() == Some(&"void") {
        return match &words[1..] {
            ["scope", "in"] => Some(Command::ScopeIn),
            ["scope", "out"] => Some(Command::ScopeOut),
            ["close"] => Some(Command::Close),
            ["is", "multiplayer"] => Some(Command::Multiplayer { negate: false }),
            ["!is", "multiplayer"] => Some(Command::Multiplayer { negate: true }),
            _ => None,
        };
    }
    let (var, member, op_word) = match words.as_slice() {
        [var, op] => (*var, None, *op),
        [var, member, op] => (*var, Some(member.to_string()), *op),
        _ => return None,
    };
    let (negate, optional, op_name) = match op_word.chars().next() {
        Some('!') => (true, false, &op_word[1..]),
        Some('?') => (false, true, &op_word[1..]),
        _ => (false, false, op_word),
    };
    let op = match op_name {
        "set" => Op::Set,
        "is" => Op::Is,
        "less" => Op::Less,
        "greater" => Op::Greater,
        "add" => Op::Add,
        "subtract" => Op::Subtract,
        "multiply" => Op::Multiply,
        "divide" => Op::Divide,
        _ => return None,
    };
    let mut parts = value.trim().splitn(2, char::is_whitespace);
    let ty = parts.next().unwrap_or("").to_ascii_lowercase();
    let param = parts.next().unwrap_or("").trim().to_owned();
    Some(Command::Var { var: var.to_owned(), member, op, negate, optional, ty, param })
}

/// A list of commands run when an event arrives (or each frame for
/// `update`, or after opening for `onopen`).
#[derive(Clone, Debug)]
pub struct Handler {
    pub event: String,
    commands: Vec<Command>,
}

impl Handler {
    fn parse(kv: &KeyValues) -> Self {
        let commands = kv.children().iter().filter_map(|c| c.as_str().and_then(|v| parse_command(&c.key, v))).collect();
        Self { event: kv.key.to_ascii_lowercase(), commands }
    }
}

/// A root lesson from the scripts.
#[derive(Debug)]
pub struct LessonDef {
    pub name: String,
    pub priority: i32,
    /// 0 several open, 1 one open, 2 replace same name and key, 3 several
    /// open but one shown.
    pub instance_type: i32,
    pub fixed_instances_max: i32,
    pub replace_only_when_stopped: bool,
    pub success_limit: i32,
    pub display_limit: i32,
    pub once_learned_never_open: bool,
    pub defaults: LessonVars,
    pub open: Vec<Handler>,
    pub close: Vec<Handler>,
    pub success: Vec<Handler>,
    pub onopen: Vec<Handler>,
    pub update: Vec<Handler>,
}

impl LessonDef {
    fn parse(kv: &KeyValues, convar: &dyn Fn(&str) -> Option<f32>) -> Self {
        let mut d = LessonVars::default();
        let s = |k: &str| kv.get_str(k).map(str::to_owned);
        let num = |k: &str| {
            kv.get_str(k).map(|v| match v.trim().strip_prefix("convar ") {
                Some(name) => convar(name.trim()).unwrap_or(0.0),
                None => source_formats::kv::parse_f32_lenient(v),
            })
        };
        let flag = |k: &str| num(k).is_some_and(|v| v != 0.0);
        if let Some(v) = num("timeout") {
            d.timeout = v;
        }
        if let Some(v) = s("onscreen_icon") {
            d.onscreen_icon = v;
        }
        if let Some(v) = s("offscreen_icon") {
            d.offscreen_icon = v;
        }
        if let Some(v) = s("caption") {
            d.caption = v;
        }
        if let Some(v) = s("caption_color") {
            d.caption_color = parse_color(&v);
        }
        if let Some(v) = num("icon_offset") {
            d.icon_offset = v;
        }
        if let Some(v) = num("range") {
            d.range = v;
        }
        if let Some(v) = num("flags") {
            d.flags = v as u32;
        }
        if let Some(v) = s("binding") {
            d.binding = v;
        }
        if let Some(v) = s("gamepad_binding") {
            d.gamepad_binding = v;
        }
        if kv.get("allow_nodraw_target").is_some() {
            d.allow_nodraw_target = flag("allow_nodraw_target");
        }
        d.no_offscreen = flag("no_offscreen");
        d.force_caption = flag("force_caption");
        if let Some(v) = s("replace_key") {
            d.replace_key = v;
        }
        if kv.get("visible").is_some() {
            d.visible = flag("visible");
        }
        if let Some(v) = num("integer1") {
            d.integer1 = v as i32;
        }
        if let Some(v) = num("integer2") {
            d.integer2 = v as i32;
        }
        let handlers =
            |k: &str| kv.get(k).map(|b| b.children().iter().filter(|c| c.is_block()).map(Handler::parse).collect()).unwrap_or_default();
        Self {
            name: kv.key.clone(),
            priority: num("priority").unwrap_or(0.0) as i32,
            instance_type: num("instance_type").unwrap_or(0.0) as i32,
            fixed_instances_max: num("fixed_instances_max").map(|v| v as i32).unwrap_or(1),
            replace_only_when_stopped: flag("replace_only_when_stopped"),
            success_limit: num("success_limit").unwrap_or(0.0) as i32,
            display_limit: num("display_limit").unwrap_or(0.0) as i32,
            once_learned_never_open: flag("once_learned_never_open"),
            defaults: d,
            open: handlers("open"),
            close: handlers("close"),
            success: handlers("success"),
            onopen: handlers("onopen"),
            update: handlers("update"),
        }
    }
}

/// Parses every lesson of a script (`"instructor_lessons" { ... }`).
pub fn parse_lessons(text: &str, convar: &dyn Fn(&str) -> Option<f32>) -> Vec<Arc<LessonDef>> {
    let doc = source_formats::kv::parse_document(text);
    doc.roots.iter().flat_map(|r| r.children().iter()).filter(|l| l.is_block()).map(|l| Arc::new(LessonDef::parse(l, convar))).collect()
}

/// What a script command can see besides the lesson: the event, the local
/// player and entity class names.
pub struct ScriptEnv<'a> {
    pub event: Option<&'a GameEvent>,
    pub player: Option<Entity>,
    pub classname: &'a dyn Fn(Entity) -> Option<String>,
}

/// Outcome of running a handler.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Passed,
    Failed,
    /// `void close` ran.
    Close,
}

#[derive(Clone, Debug, PartialEq)]
enum Val {
    Void,
    Int(i32),
    Float(f32),
    Str(String),
    Entity(Option<Entity>),
}

impl Val {
    fn float(&self) -> f32 {
        match self {
            Val::Int(i) => *i as f32,
            Val::Float(f) => *f,
            Val::Str(s) => source_formats::kv::parse_f32_lenient(s),
            _ => 0.0,
        }
    }

    fn string(&self) -> String {
        match self {
            Val::Str(s) => s.clone(),
            Val::Int(i) => i.to_string(),
            Val::Float(f) => f.to_string(),
            _ => String::new(),
        }
    }

    fn entity(&self) -> Option<Entity> {
        match self {
            Val::Entity(e) => *e,
            _ => None,
        }
    }
}

fn from_event(v: &EventValue) -> Val {
    match v {
        EventValue::Int(i) => Val::Int(*i),
        EventValue::Float(f) => Val::Float(*f),
        EventValue::Str(s) => Val::Str(s.clone()),
        EventValue::Entity(e) => Val::Entity(*e),
    }
}

/// Converts to the parameter's declared type.
fn coerce(v: Val, ty: &str) -> Val {
    match ty {
        "int" | "short" | "long" | "byte" | "bool" => Val::Int(match v {
            Val::Int(i) => i,
            other => other.float() as i32,
        }),
        "float" => Val::Float(v.float()),
        "string" => Val::Str(v.string()),
        "player" | "entity" => Val::Entity(v.entity()),
        _ => v,
    }
}

fn read_var(vars: &LessonVars, name: &str, env: &ScriptEnv) -> Option<Val> {
    Some(match name {
        "local_player" => Val::Entity(env.player),
        "icon_target" => Val::Entity(vars.icon_target),
        "entity1" => Val::Entity(vars.entity1),
        "entity2" => Val::Entity(vars.entity2),
        "string1" => Val::Str(vars.string1.clone()),
        "string2" => Val::Str(vars.string2.clone()),
        "integer1" => Val::Int(vars.integer1),
        "integer2" => Val::Int(vars.integer2),
        "float1" => Val::Float(vars.float1),
        "float2" => Val::Float(vars.float2),
        "timeout" => Val::Float(vars.timeout),
        "onscreen_icon" => Val::Str(vars.onscreen_icon.clone()),
        "offscreen_icon" => Val::Str(vars.offscreen_icon.clone()),
        "caption" => Val::Str(vars.caption.clone()),
        "icon_offset" => Val::Float(vars.icon_offset),
        "range" => Val::Float(vars.range),
        "flags" => Val::Int(vars.flags as i32),
        "binding" => Val::Str(vars.binding.clone()),
        "gamepad_binding" => Val::Str(vars.gamepad_binding.clone()),
        "replace_key" => Val::Str(vars.replace_key.clone()),
        "visible" => Val::Int(vars.visible as i32),
        _ => return None,
    })
}

fn write_var(vars: &mut LessonVars, name: &str, v: Val) {
    match name {
        "icon_target" => vars.icon_target = v.entity(),
        "entity1" => vars.entity1 = v.entity(),
        "entity2" => vars.entity2 = v.entity(),
        "string1" => vars.string1 = v.string(),
        "string2" => vars.string2 = v.string(),
        "integer1" => vars.integer1 = v.float() as i32,
        "integer2" => vars.integer2 = v.float() as i32,
        "float1" => vars.float1 = v.float(),
        "float2" => vars.float2 = v.float(),
        "timeout" => vars.timeout = v.float(),
        "onscreen_icon" => vars.onscreen_icon = v.string(),
        "offscreen_icon" => vars.offscreen_icon = v.string(),
        "caption" => vars.caption = v.string(),
        "caption_color" => vars.caption_color = parse_color(&v.string()),
        "icon_offset" => vars.icon_offset = v.float(),
        "range" => vars.range = v.float(),
        "flags" => vars.flags = v.float() as u32,
        "binding" => vars.binding = v.string(),
        "gamepad_binding" => vars.gamepad_binding = v.string(),
        "allow_nodraw_target" => vars.allow_nodraw_target = v.float() != 0.0,
        "no_offscreen" => vars.no_offscreen = v.float() != 0.0,
        "force_caption" => vars.force_caption = v.float() != 0.0,
        "replace_key" => vars.replace_key = v.string(),
        "visible" => vars.visible = v.float() != 0.0,
        _ => {}
    }
}

/// The parameter's value: a lesson variable, an event field, `NULL` /
/// `local_player`, or the literal. `None` when an event field is named but
/// the event lacks it (or there is no event).
fn param_value(ty: &str, param: &str, vars: &LessonVars, env: &ScriptEnv) -> Option<Val> {
    if ty == "void" {
        return Some(Val::Void);
    }
    let lower = param.to_ascii_lowercase();
    if let Some(v) = read_var(vars, &lower, env) {
        return Some(coerce(v, ty));
    }
    if let Some(v) = env.event.and_then(|e| e.get(&lower)) {
        return Some(coerce(from_event(v), ty));
    }
    match (ty, lower.as_str()) {
        ("player" | "entity", "null") => Some(Val::Entity(None)),
        ("player" | "entity", _) => None,
        ("string", _) => Some(Val::Str(param.to_owned())),
        ("float", _) => Some(Val::Float(source_formats::kv::parse_f32_lenient(param))),
        (_, _) if param.trim().parse::<f32>().is_ok() => Some(coerce(Val::Float(source_formats::kv::parse_f32_lenient(param)), ty)),
        // A named field the event does not carry.
        _ => None,
    }
}

fn equal(a: &Val, b: &Val) -> bool {
    match (a, b) {
        (Val::Entity(x), Val::Entity(y)) => x == y,
        (Val::Str(x), Val::Str(y)) => x.eq_ignore_ascii_case(y),
        (Val::Void, _) | (_, Val::Void) => false,
        (x, y) => (x.float() - y.float()).abs() < 1e-4,
    }
}

/// Runs a handler's commands against `vars`.
pub fn run(handler: &Handler, vars: &mut LessonVars, env: &ScriptEnv) -> Outcome {
    let mut skipping = false;
    let mut depth = 0;
    for cmd in &handler.commands {
        if skipping {
            match cmd {
                Command::ScopeIn => depth += 1,
                Command::ScopeOut if depth == 0 => skipping = false,
                Command::ScopeOut => depth -= 1,
                _ => {}
            }
            continue;
        }
        let ok = match cmd {
            Command::ScopeIn | Command::ScopeOut => true,
            Command::Close => return Outcome::Close,
            // Single player only.
            Command::Multiplayer { negate } => *negate,
            Command::Var { var, member, op, negate, optional, ty, param } => match param_value(ty, param, vars, env) {
                // `?set` of a missing field or an entity the client does not
                // have does nothing; other commands with a missing field
                // leave the variable alone or fail.
                None | Some(Val::Entity(None)) if *optional => continue,
                None if *op != Op::Is && *op != Op::Less && *op != Op::Greater => continue,
                None => false,
                Some(value) => {
                    let current = read_var(vars, var, env).map(|v| match member.as_deref() {
                        Some("classname") => Val::Str(v.entity().and_then(|e| (env.classname)(e)).unwrap_or_default()),
                        // Teams only exist in co-op.
                        Some(_) => Val::Int(0),
                        None => v,
                    });
                    match op {
                        Op::Set => {
                            write_var(vars, var, value);
                            true
                        }
                        Op::Add | Op::Subtract | Op::Multiply | Op::Divide => {
                            let a = current.map(|c| c.float()).unwrap_or(0.0);
                            let b = value.float();
                            let r = match op {
                                Op::Add => a + b,
                                Op::Subtract => a - b,
                                Op::Multiply => a * b,
                                _ if b != 0.0 => a / b,
                                _ => a,
                            };
                            write_var(vars, var, Val::Float(r));
                            true
                        }
                        Op::Is | Op::Less | Op::Greater => current.is_some_and(|c| {
                            let r = match op {
                                Op::Is => equal(&c, &value),
                                Op::Less => c.float() < value.float(),
                                _ => c.float() > value.float(),
                            };
                            r != *negate
                        }),
                    }
                }
            },
        };
        if !ok {
            // A failed test inside a scope skips to the scope's end.
            if in_scope(handler, cmd) {
                skipping = true;
                depth = 0;
                continue;
            }
            return Outcome::Failed;
        }
    }
    Outcome::Passed
}

/// Whether `cmd` lies between a `scope in` and its `scope out`.
fn in_scope(handler: &Handler, cmd: &Command) -> bool {
    let mut depth = 0;
    for c in &handler.commands {
        if std::ptr::eq(c, cmd) {
            return depth > 0;
        }
        match c {
            Command::ScopeIn => depth += 1,
            Command::ScopeOut => depth -= 1,
            _ => {}
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    const SERVER_HINT: &str = r#""instructor_lessons"
    {
        "Serverside Hint"
        {
            "priority" "0"
            "instance_type" "2"
            "onscreen_icon" "icon_none"
            "caption" "No Caption Specified"
            "open"
            {
                "instructor_server_hint_create"
                {
                    "icon_target set" "player local_player"
                    "integer1 set" "bool hint_local_player_only"
                    "integer1 is" "int 0"
                    "string1 ?set" "string hint_name"
                    "icon_target ?set" "entity hint_target"
                    "caption ?set" "string hint_caption"
                    "void scope in" "void"
                        "local_player is" "player entity2"
                        "caption ?set" "string hint_activator_caption"
                        "void scope out" "void"
                    "caption_color ?set" "string hint_color"
                }
            }
            "close" { "instructor_server_hint_stop" { "string1 is" "string hint_name" } }
        }
    }"#;

    fn env<'a>(event: &'a GameEvent, player: Entity, names: &'a dyn Fn(Entity) -> Option<String>) -> ScriptEnv<'a> {
        ScriptEnv { event: Some(event), player: Some(player), classname: names }
    }

    #[test]
    fn server_hint_opens_and_closes_by_name() {
        let defs = parse_lessons(SERVER_HINT, &|_| None);
        let def = &defs[0];
        assert_eq!(def.instance_type, 2);
        let player = Entity::from_raw_u32(7).unwrap();
        let names = |_: Entity| None;
        let create = GameEvent::new("instructor_server_hint_create")
            .with("hint_name", EventValue::Str("hint_look_up".into()))
            .with("hint_caption", EventValue::Str("#Valve_Hint_Look_Up".into()))
            .with("hint_target", EventValue::Entity(Some(player)))
            .with("hint_local_player_only", EventValue::Int(0))
            .with("hint_color", EventValue::Str("255,255,255".into()));
        let mut vars = def.defaults.clone();
        assert_eq!(run(&def.open[0], &mut vars, &env(&create, player, &names)), Outcome::Passed);
        assert_eq!(vars.caption, "#Valve_Hint_Look_Up");
        assert_eq!(vars.string1, "hint_look_up");
        assert_eq!(vars.icon_target, Some(player));
        let stop_other = GameEvent::new("instructor_server_hint_stop").with("hint_name", EventValue::Str("other".into()));
        assert_eq!(run(&def.close[0], &mut vars, &env(&stop_other, player, &names)), Outcome::Failed);
        let stop = GameEvent::new("instructor_server_hint_stop").with("hint_name", EventValue::Str("HINT_LOOK_UP".into()));
        assert_eq!(run(&def.close[0], &mut vars, &env(&stop, player, &names)), Outcome::Passed);
    }
}
