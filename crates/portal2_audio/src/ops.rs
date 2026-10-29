//! Sound operator stacks: the small data-flow programs Portal 2's sound
//! entries use to compute volume, pitch, spatialisation and side effects.
//!
//! `scripts/sound_operator_stacks.txt` holds a library of start, update and
//! stop stacks. A sound entry's `operator_stacks` block names the stacks it
//! uses (`import_stack`) and may override parameters of the imported
//! operators by name or add new ones. Operators run in order; an input is a
//! literal or `@operator.output` of an earlier operator. Operator semantics
//! (defaults, formulas) are described in `docs/re/audio_runtime.md`.

use portal2_core::rng::rand_unit;
use std::collections::HashMap;

use bevy::prelude::*;
use source_formats::fs::GameFileSystem;
use source_formats::kv::{self, KeyValues};

use super::mixer::MixData;

/// Values are up to six floats: scalars use [0], vectors [0..3], speaker
/// arrays [0..6] (left front, right front, left rear, right rear, center,
/// LFE).
pub type V = [f32; 6];

pub const SPEAKERS: [&str; 6] = ["left_front", "right_front", "left_rear", "right_rear", "center", "lfe"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StackKind {
    Prestart,
    Start,
    Update,
    Stop,
}

impl StackKind {
    fn section(self) -> &'static str {
        match self {
            Self::Prestart => "prestart_stacks",
            Self::Start => "start_stacks",
            Self::Update => "update_stacks",
            Self::Stop => "stop_stacks",
        }
    }

    pub fn entry_key(self) -> &'static str {
        match self {
            Self::Prestart => "prestart_stack",
            Self::Start => "start_stack",
            Self::Update => "update_stack",
            Self::Stop => "stop_stack",
        }
    }
}

/// An operator definition as written: its name and key/value parameters in
/// order (later keys override earlier ones).
#[derive(Clone, Debug)]
struct OpDef {
    name: String,
    params: Vec<(String, String)>,
}

impl OpDef {
    fn set(&mut self, key: &str, value: &str) {
        if let Some(p) = self.params.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(key)) {
            p.1 = value.to_owned();
        } else {
            self.params.push((key.to_owned(), value.to_owned()));
        }
    }
}

/// The stack library, keyed by lowercase stack name per kind.
#[derive(Default)]
pub struct Library {
    stacks: HashMap<(u8, String), KeyValues>,
}

impl Library {
    pub fn load(fs: &GameFileSystem) -> Self {
        let mut lib = Self::default();
        let Some(text) = fs.read_string("scripts/sound_operator_stacks.txt") else {
            warn!("audio: scripts/sound_operator_stacks.txt missing");
            return lib;
        };
        let doc = kv::parse_document(&text);
        for root in &doc.roots {
            for kind in [StackKind::Prestart, StackKind::Start, StackKind::Update, StackKind::Stop] {
                if root.key.eq_ignore_ascii_case(kind.section()) {
                    for stack in root.children() {
                        lib.stacks.insert((kind as u8, stack.key.to_ascii_lowercase()), stack.clone());
                    }
                }
            }
        }
        lib
    }

    /// Expands a stack block (library stack or an entry's override block)
    /// into its final operator list.
    fn expand(&self, kind: StackKind, block: &KeyValues, out: &mut Vec<OpDef>, depth: u32) {
        if depth > 8 {
            return;
        }
        for child in block.children() {
            if child.key.eq_ignore_ascii_case("import_stack") {
                if let Some(name) = child.as_str()
                    && let Some(lib) = self.stacks.get(&(kind as u8, name.to_ascii_lowercase()))
                {
                    self.expand(kind, lib, out, depth + 1);
                }
                continue;
            }
            if !child.is_block() {
                continue;
            }
            let op = match out.iter_mut().position(|o| o.name.eq_ignore_ascii_case(&child.key)) {
                Some(i) => &mut out[i],
                None => {
                    out.push(OpDef { name: child.key.clone(), params: Vec::new() });
                    out.last_mut().unwrap()
                }
            };
            for p in child.children() {
                if let Some(v) = p.as_str() {
                    op.set(&p.key, v);
                }
            }
        }
    }

    /// The compiled stack an entry uses for `kind`: its own block when it
    /// has one, else the named library stack.
    pub fn compile(&self, kind: StackKind, entry_stacks: Option<&KeyValues>, fallback: Option<&str>) -> Option<Stack> {
        let mut defs = Vec::new();
        match entry_stacks.and_then(|s| s.get(kind.entry_key())) {
            Some(block) => self.expand(kind, block, &mut defs, 0),
            None => {
                let lib = self.stacks.get(&(kind as u8, fallback?.to_ascii_lowercase()))?;
                self.expand(kind, lib, &mut defs, 0);
            }
        }
        (!defs.is_empty()).then(|| Stack::build(&defs))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    SourceInfo,
    SourceDistance,
    MixGroup,
    Occlusion,
    Convar,
    Float,
    Remap,
    Func1,
    Random,
    Delta,
    Speakers,
    Falloff,
    DistantDsp,
    Spatialize,
    IterateMerge,
    Output,
    EntryTime,
    StopEntries,
    StartEntry,
    BlockEntries,
    MixLayer,
    ViewInfo,
    Facing,
    LogicSwitch,
    GetOpvar,
    SetOpvar,
    PrintFloat,
    SysTime,
    MapName,
    Platform,
    Dashboard,
    Accumulate,
    PosVec8,
    Unknown,
}

impl Kind {
    fn from_name(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "get_source_info" => Self::SourceInfo,
            "calc_source_distance" => Self::SourceDistance,
            "get_soundmixer" => Self::MixGroup,
            "calc_occlusion" => Self::Occlusion,
            "get_convar" => Self::Convar,
            "math_float" => Self::Float,
            "math_remap_float" => Self::Remap,
            "math_func1" => Self::Func1,
            "math_random" => Self::Random,
            "math_delta" => Self::Delta,
            "math_speakers" => Self::Speakers,
            "calc_falloff" => Self::Falloff,
            "calc_distant_dsp" => Self::DistantDsp,
            "calc_spatialize_speakers" => Self::Spatialize,
            "iterate_merge_speakers" => Self::IterateMerge,
            "sys_output" => Self::Output,
            "get_entry_time" => Self::EntryTime,
            "sys_stop_entries" => Self::StopEntries,
            "sys_start_entry" => Self::StartEntry,
            "sys_block_entries" => Self::BlockEntries,
            "sys_mixlayer" => Self::MixLayer,
            "game_view_info" => Self::ViewInfo,
            "calc_angles_facing" => Self::Facing,
            "math_logic_switch" => Self::LogicSwitch,
            "get_opvar_float" => Self::GetOpvar,
            "set_opvar_float" | "increment_opvar_float" => Self::SetOpvar,
            "util_print_float" => Self::PrintFloat,
            "get_sys_time" => Self::SysTime,
            "get_map_name" => Self::MapName,
            "sys_platform" => Self::Platform,
            "get_dashboard" => Self::Dashboard,
            "op_accumulate_ss_float" => Self::Accumulate,
            "util_pos_vec8" => Self::PosVec8,
            _ => Self::Unknown,
        }
    }

    /// Output names; the first is where unknown names resolve.
    fn outputs(self) -> &'static [&'static str] {
        match self {
            Self::SourceInfo => &[
                "output_position",
                "output_angles",
                "output_volume",
                "output_level",
                "output_pitch",
                "output_radius",
                "output_source_count",
                "output_entity_index",
            ],
            Self::MixGroup => &["output_volume", "output_level", "output_dsp"],
            Self::EntryTime => &["output_entry_elapsed", "output_sound_elapsed", "output_stop_elapsed", "output_sound_duration"],
            Self::StopEntries => &["output_entries_matching", "output_this_matches_index"],
            Self::GetOpvar => &["output", "output_opvar_exists"],
            Self::ViewInfo => &["output_position", "output_angles", "output_velocity", "output_velocity_vector"],
            Self::Facing => &["output"],
            _ => &["output"],
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Comp {
    Literal(f32),
    /// Operator index, output slot, component of that output.
    Ref(usize, usize, usize),
}

/// An input's components; unset ones take the operator's default.
type Input = [Option<Comp>; 6];

#[derive(Clone, Debug)]
struct Op {
    name: String,
    kind: Kind,
    /// Inputs by lowercase name; components set individually (`input1[2]`,
    /// speaker names) are merged into a literal.
    inputs: HashMap<String, Input>,
    strings: HashMap<String, String>,
    execute_once: bool,
}

impl Op {
    fn s(&self, key: &str) -> Option<&str> {
        self.strings.get(key).map(String::as_str)
    }

    fn flag(&self, key: &str, default: bool) -> bool {
        match self.s(key) {
            Some(v) if v.eq_ignore_ascii_case("true") || v == "1" => true,
            Some(v) if v.eq_ignore_ascii_case("false") || v == "0" => false,
            _ => default,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Stack {
    ops: Vec<Op>,
}

fn parse_literal(s: &str) -> V {
    let mut v = [0.0; 6];
    for (slot, part) in v.iter_mut().zip(s.split(|c: char| c.is_whitespace() || c == ',').filter(|p| !p.is_empty())) {
        *slot = kv::parse_f32_lenient(part);
    }
    v
}

impl Stack {
    fn build(defs: &[OpDef]) -> Self {
        let names: Vec<String> = defs.iter().map(|d| d.name.to_ascii_lowercase()).collect();
        let kinds: Vec<Kind> = defs
            .iter()
            .map(|d| Kind::from_name(d.params.iter().find(|(k, _)| k.eq_ignore_ascii_case("operator")).map_or("", |(_, v)| v)))
            .collect();
        let mut ops = Vec::with_capacity(defs.len());
        for (i, d) in defs.iter().enumerate() {
            let mut op =
                Op { name: names[i].clone(), kind: kinds[i], inputs: HashMap::new(), strings: HashMap::new(), execute_once: false };
            for (k, v) in &d.params {
                let key = k.to_ascii_lowercase();
                if key == "operator" {
                    continue;
                }
                if key == "execute_once" {
                    op.execute_once = v.eq_ignore_ascii_case("true") || v == "1";
                    continue;
                }
                let speaker = SPEAKERS.iter().position(|s| *s == key);
                if !key.starts_with("input") && speaker.is_none() {
                    op.strings.insert(key, v.clone());
                    continue;
                }
                // Components: `input2[*]` sets all of them from one value,
                // `input1[3]` and speaker names (which belong to `input1`)
                // set one, a plain key sets them component-wise.
                enum Target {
                    All,
                    Broadcast,
                    One(usize),
                }
                let (base, target) = match (speaker, key.find('[')) {
                    (Some(s), _) => ("input1".to_owned(), Target::One(s)),
                    (None, Some(b)) => {
                        let idx = key[b + 1..].trim_end_matches(']');
                        let t = if idx == "*" { Target::Broadcast } else { idx.parse().map_or(Target::Broadcast, Target::One) };
                        (key[..b].to_owned(), t)
                    }
                    (None, None) => (key.clone(), Target::All),
                };
                // A component value: literal, or (operator, output slot).
                let reference = v.trim().strip_prefix('@').map(|r| {
                    let (opname, out) = r.split_once('.').unwrap_or((r, "output"));
                    let opname = opname.to_ascii_lowercase();
                    names[..i].iter().rposition(|n| *n == opname).or_else(|| names.iter().position(|n| *n == opname)).map(|j| {
                        let slot = kinds[j].outputs().iter().position(|o| o.eq_ignore_ascii_case(out)).unwrap_or(0);
                        (j, slot)
                    })
                });
                let lit = parse_literal(v);
                let comp = |c: usize| -> Comp {
                    match reference {
                        Some(Some((j, slot))) => Comp::Ref(j, slot, c),
                        Some(None) => Comp::Literal(0.0),
                        None => Comp::Literal(lit[c]),
                    }
                };
                let entry = op.inputs.entry(base).or_insert([None; 6]);
                match target {
                    Target::All => (0..6).for_each(|c| entry[c] = Some(comp(c))),
                    Target::Broadcast => (0..6).for_each(|c| entry[c] = Some(comp(0))),
                    Target::One(c) if c < 6 => entry[c] = Some(comp(0)),
                    Target::One(_) => {}
                }
            }
            ops.push(op);
        }
        Self { ops }
    }

    pub fn instance(&self) -> Instance {
        Instance {
            outputs: self.ops.iter().map(|o| vec![[0.0; 6]; o.kind.outputs().len()]).collect(),
            ran: vec![false; self.ops.len()],
            scratch: vec![None; self.ops.len()],
        }
    }

    /// Whether the stack contains an operator of a kind (by operator name).
    pub fn has(&self, operator: &str) -> bool {
        let k = Kind::from_name(operator);
        self.ops.iter().any(|o| o.kind == k)
    }
}

/// Per-sound state of one stack.
#[derive(Clone, Debug)]
pub struct Instance {
    outputs: Vec<Vec<V>>,
    ran: Vec<bool>,
    scratch: Vec<Option<f32>>,
}

/// What the operators read from and write to the sound.
#[derive(Clone, Debug, Default)]
pub struct Chan {
    pub entry: String,
    pub entity_index: i32,
    pub has_entity: bool,
    pub slot: i32,
    pub origin: Vec3,
    pub angles: Vec3,
    pub volume: f32,
    pub level: f32,
    /// Ratio, 1 = normal.
    pub pitch: f32,
    pub entry_elapsed: f32,
    pub sound_elapsed: f32,
    pub stop_elapsed: f32,
    pub duration: f32,
    pub groups: Vec<usize>,
    pub occlusion: f32,
    // Outputs.
    pub speakers: Option<V>,
    pub out_pitch: Option<f32>,
    pub dsp: f32,
    pub delay: Option<f32>,
    pub stop_hold: Option<bool>,
    pub mixlayer_trigger: f32,
    pub block_start: bool,
    pub position: Option<Vec3>,
}

pub struct Listener {
    pub origin: Vec3,
    pub forward: Vec3,
    pub right: Vec3,
    pub velocity: Vec3,
}

/// What the stacks ask the sound system to do.
#[derive(Debug)]
pub enum Effect {
    Stop(Vec<u64>),
    Start { entry: String, delay: f32 },
    Block { entry: Option<String>, substring: bool, entity: Option<i32>, channel: Option<i32>, until: f64 },
    MixLayer { layer: String, group: String, field: String, value: f32 },
    Opvar(String, f32),
}

/// Another playing sound, for the entry matching operators.
pub struct Other {
    pub id: u64,
    pub entry: String,
    pub entity_index: i32,
    pub slot: i32,
    pub started: f64,
}

pub struct Env<'a> {
    pub listener: &'a Listener,
    pub mix: &'a MixData,
    pub cvar: &'a dyn Fn(&str) -> f32,
    pub opvars: &'a HashMap<String, f32>,
    pub others: &'a [Other],
    pub now: f64,
    pub map: &'a str,
    pub speaker_mode: i32,
    pub effects: Vec<Effect>,
}

fn remap(input: f32, in_min: f32, in_max: f32, map_min: f32, map_max: f32, clamp: bool, default_to_max: bool) -> f32 {
    let (mut a, mut b, mut lo, mut hi) = (in_min, in_max, map_min, map_max);
    if lo > hi && a != b {
        std::mem::swap(&mut lo, &mut hi);
        std::mem::swap(&mut a, &mut b);
    }
    let out = if a == b {
        if a > input {
            lo
        } else if input > b {
            hi
        } else if default_to_max {
            hi
        } else {
            lo
        }
    } else if lo == hi {
        lo
    } else if a >= input && b > a {
        lo
    } else if input >= b && b > a {
        hi
    } else if input >= a && a > b {
        lo
    } else if !(b < input || a <= b) {
        hi
    } else {
        lo + (input - a) / (b - a) * (hi - lo)
    };
    if !clamp {
        return out;
    }
    let (min, max) = if hi > lo { (lo, hi) } else { (hi, lo) };
    out.clamp(min, max)
}

/// Distance gain for a sound level, as the engine computes it (reference
/// level 60 dB at 36 units, foliage loss, near-field clamp and the
/// compression curve above 0.5).
pub fn falloff(distance: f32, level: f32, cvar: &dyn Fn(&str) -> f32) -> f32 {
    let refdist = cvar("snd_refdist").max(1e-3);
    let dist_mult = if level == 0.0 { 0.0 } else { 1000.0 / 10f32.powf(level * 0.05) / refdist };
    gain_from_mult(1.0, dist_mult, distance, cvar)
}

pub fn gain_from_mult(gain: f32, dist_mult: f32, dist: f32, cvar: &dyn Fn(&str) -> f32) -> f32 {
    let foliage = 10f32.powf(dist / 1200.0 * cvar("snd_foliage_db_loss") * 0.05);
    let relative = dist * dist_mult * foliage;
    let mut g = if relative <= 0.1 { gain * 10.0 } else { gain / relative };
    if g > 0.5 {
        let level = if dist_mult != 0.0 { (20.0 * (1000.0 / (dist_mult * cvar("snd_refdist"))).log10()) as i32 as f32 } else { 0.0 };
        let mut power = 2.5f32;
        if level > 90.0 {
            power = 2.5 - (level - 90.0) * 1.7 * 0.02;
        }
        let y = -1.0 / (0.5f32.powf(power) * -0.5);
        g = (1.0 - 1.0 / (g.powf(power) * y)) * cvar("snd_gain_max");
    }
    let min = cvar("snd_gain_min");
    if min > g {
        g = (2.0 - min * relative) * min;
        if g <= 0.0 {
            return 0.001;
        }
    }
    g
}

/// Folded elevation (0..90 degrees) and the mono amount it adds.
fn elevation_mono(pitch: f32, mono: f32) -> f32 {
    let mut p = pitch;
    if p < 0.0 {
        p += 360.0;
    }
    if p > 180.0 {
        p = 360.0 - p;
    }
    if p > 90.0 {
        p = 90.0 - (p - 90.0);
    }
    if p > 45.0 { ((p - 45.0) / 45.0 + mono).clamp(0.0, 1.0) } else { mono }
}

fn angle_between(a: f32, b: f32) -> f32 {
    let d = (a - b).abs();
    if d > 180.0 { 360.0 - d } else { d }
}

/// Speaker gains for a direction from the listener, as the PC sound devices
/// compute them. `mode` 0 = headphones, otherwise two speakers (four or 5.1
/// outputs fold into the front pair here). `mono` blends towards an even mix.
pub fn spatialize(listener: &Listener, dir: Vec3, mono: f32, rear_scale: f32, mode: i32) -> V {
    // Angles as VectorAngles gives them: yaw and pitch in 0..360 degrees.
    let wrap = |a: f32| if a < 0.0 { a + 360.0 } else { a };
    let dir = dir.normalize_or(Vec3::X);
    let src_yaw = wrap(dir.y.atan2(dir.x).to_degrees());
    let src_pitch = wrap((-dir.z).atan2(dir.truncate().length()).to_degrees());
    // Yaw is measured from the listener's right, so 90 is straight ahead.
    let right = Vec3::new(listener.right.x, listener.right.y, 0.0).normalize_or(Vec3::NEG_Y);
    let right_yaw = wrap(right.y.atan2(right.x).to_degrees());
    let mut rel = src_yaw - right_yaw;
    if rel < 0.0 {
        rel += 360.0;
    }
    let m = elevation_mono(src_pitch, mono);
    let mut out = [0.0; 6];
    if mode == 0 {
        let gain = |speaker: f32| {
            let g = 1.0 - (angle_between(rel, speaker) / 180.0).powf(1.5);
            g + (0.9 - g) * m
        };
        out[1] = gain(0.0).clamp(0.0, 1.0);
        out[0] = gain(180.0).clamp(0.0, 1.0);
        return out;
    }
    let speaker = |angle: f32| {
        let a = angle_between(rel, angle);
        if a < 90.0 { 1.0 - (a / 90.0).powf(1.5) } else { 0.0 }
    };
    let front_right = {
        let g = speaker(45.0);
        (0.9 - g) * m + g
    };
    let front_left = {
        let g = speaker(135.0);
        (0.9 - g) * m + g
    };
    let rear_right = speaker(315.0) * (1.0 - m);
    let rear_left = speaker(225.0) * (1.0 - m);
    out[1] = (front_right + rear_right * rear_scale).clamp(0.0, 1.0);
    out[0] = (front_left + rear_left * rear_scale).clamp(0.0, 1.0);
    out
}

impl Stack {
    /// Runs every operator once (an update, or a start/stop pass).
    pub fn run(&self, inst: &mut Instance, chan: &mut Chan, env: &mut Env) {
        for i in 0..self.ops.len() {
            let op = &self.ops[i];
            if op.execute_once && inst.ran[i] {
                continue;
            }
            let input = |name: &str, default: V, inst: &Instance| -> V {
                let Some(comps) = op.inputs.get(name) else { return default };
                std::array::from_fn(|c| match comps[c] {
                    Some(Comp::Literal(v)) => v,
                    Some(Comp::Ref(j, slot, k)) => inst.outputs[j][slot][k],
                    None => default[c],
                })
            };
            let f = |name: &str, default: f32, inst: &Instance| input(name, [default; 6], inst)[0];
            if f("input_execute", 1.0, inst) <= 0.0 {
                continue;
            }
            inst.ran[i] = true;
            let mut out = inst.outputs[i].clone();
            match op.kind {
                Kind::SourceInfo => {
                    let pos = chan.position.unwrap_or(chan.origin);
                    out[0] = [pos.x, pos.y, pos.z, 0.0, 0.0, 0.0];
                    out[1] = [chan.angles.x, chan.angles.y, chan.angles.z, 0.0, 0.0, 0.0];
                    out[2][0] = chan.volume;
                    out[3][0] = chan.level;
                    out[4][0] = chan.pitch;
                    out[5][0] = 0.0;
                    out[6][0] = 1.0;
                    out[7][0] = chan.entity_index as f32;
                }
                Kind::SourceDistance => {
                    let p = input("input_position", [0.0; 6], inst);
                    let pos = Vec3::new(p[0], p[1], p[2]);
                    let d = if op.flag("in2d", false) {
                        (pos.truncate() - env.listener.origin.truncate()).length()
                    } else {
                        pos.distance(env.listener.origin)
                    };
                    out[0][0] = d;
                }
                Kind::MixGroup => {
                    let group = op.s("mixgroup").and_then(|g| env.mix.group(g)).or_else(|| chan.groups.first().copied());
                    let (v, l, d) = group.map_or((1.0, 1.0, 1.0), |g| {
                        env.mix.values(g, (env.cvar)("snd_mixer_master_level"), (env.cvar)("snd_mixer_master_dsp"))
                    });
                    out[0][0] = v;
                    out[1][0] = l;
                    out[2][0] = d;
                }
                Kind::Occlusion => {
                    let scalar = f("input_scalar", 1.0, inst);
                    out[0][0] = 1.0 - (1.0 - chan.occlusion) * scalar.clamp(0.0, 1.0);
                }
                Kind::Convar => {
                    out[0][0] = op.s("convar").map_or(0.0, |c| (env.cvar)(c));
                }
                Kind::Float => {
                    let a = f("input1", 0.0, inst);
                    let b = f("input2", 0.0, inst);
                    out[0][0] = math_float(op.s("apply").unwrap_or("none"), a, b);
                }
                Kind::Remap => {
                    out[0][0] = remap(
                        f("input", 0.0, inst),
                        f("input_min", 0.0, inst),
                        f("input_max", 0.1, inst),
                        f("input_map_min", 0.0, inst),
                        f("input_map_max", 1.0, inst),
                        op.flag("clamp_range", true),
                        op.flag("default_to_max", true),
                    );
                }
                Kind::Func1 => {
                    let a = f("input1", 0.0, inst);
                    out[0][0] = match op.s("function").unwrap_or("none").to_ascii_lowercase().as_str() {
                        "abs" => a.abs(),
                        "sqrt" => a.max(0.0).sqrt(),
                        "pow2" => a * a,
                        "pow3" => a * a * a,
                        "pow4" => a * a * a * a,
                        "sin" => a.sin(),
                        "cos" => a.cos(),
                        "log10" => a.max(1e-9).log10(),
                        "ceil" => a.ceil(),
                        "floor" => a.floor(),
                        "round" => a.round(),
                        "normalize_angle" => a.rem_euclid(360.0),
                        _ => a,
                    };
                }
                Kind::Random => {
                    let lo = f("input_min", 0.0, inst);
                    let hi = f("input_max", 1.0, inst);
                    let mut r = lo + rand_unit() * (hi - lo);
                    if op.flag("round_to_int", false) {
                        r = r.round();
                    }
                    out[0][0] = r;
                }
                Kind::Delta => {
                    let x = f("input", 0.0, inst);
                    let prev = inst.scratch[i].unwrap_or(x);
                    inst.scratch[i] = Some(x);
                    out[0][0] = x - prev;
                }
                Kind::Speakers => {
                    let a = input("input1", [0.0; 6], inst);
                    let b = input("input2", [0.0; 6], inst);
                    let apply = op.s("apply").unwrap_or("none");
                    for s in 0..6 {
                        out[0][s] = math_float(apply, a[s], b[s]);
                    }
                }
                Kind::Falloff => {
                    out[0][0] = falloff(f("input_distance", 1.0, inst), f("input_level", 1.0, inst), env.cvar);
                }
                Kind::DistantDsp => {
                    // Fraction of the room DSP for a distant sound: grows with
                    // distance relative to the sound's reach.
                    let d = f("input_distance", 0.0, inst);
                    let level = f("input_level", 75.0, inst);
                    let reach = if level > 0.0 { 36.0 * 10f32.powf((level - 60.0) / 20.0) } else { 0.0 };
                    out[0][0] = if reach > 0.0 { (d / reach).clamp(0.0, 1.0) } else { 1.0 };
                }
                Kind::Spatialize => {
                    let radius = f("input_radius", 0.0, inst);
                    let distance = f("input_distance", 0.0, inst);
                    let rear = f("input_rear_stereo_scale", 0.75, inst);
                    let p = input("input_position", [0.0; 6], inst);
                    let mut mono = 0.0;
                    if radius > 0.0 && radius > distance {
                        let half = radius * 0.5;
                        mono = 1.0 - (distance - half).max(0.0) / half;
                    }
                    let dir = Vec3::new(p[0], p[1], p[2]) - env.listener.origin;
                    out[0] = spatialize(env.listener, dir, mono, rear, env.speaker_mode);
                }
                Kind::IterateMerge => {
                    out[0] = input("input", [0.0; 6], inst);
                }
                Kind::Output => output(op, chan, &input, &f, inst),
                Kind::EntryTime => {
                    out[0][0] = chan.entry_elapsed;
                    out[1][0] = chan.sound_elapsed;
                    out[2][0] = chan.stop_elapsed;
                    out[3][0] = chan.duration;
                }
                Kind::StopEntries => {
                    let max = f("input_max_entries", 0.0, inst).max(0.0) as usize;
                    let matches = matching(op, chan, env);
                    out[0][0] = matches.len() as f32;
                    if matches.len() > max {
                        let mut m = matches;
                        // Oldest first, unless told otherwise.
                        m.sort_by(|a, b| a.1.total_cmp(&b.1));
                        if op.flag("stop_oldest", true) {
                            let n = m.len() - max;
                            env.effects.push(Effect::Stop(m[..n].iter().map(|x| x.0).collect()));
                        } else {
                            chan.block_start = true;
                        }
                    }
                }
                Kind::StartEntry => {
                    if f("input_start", 0.0, inst) > 0.0
                        && let Some(name) = op.s("entry_name")
                    {
                        env.effects.push(Effect::Start { entry: name.to_owned(), delay: f("input_delay", 0.0, inst) });
                    }
                }
                Kind::BlockEntries => {
                    if f("input_active", 1.0, inst) > 0.0 {
                        let entry =
                            op.s("match_entry").map(str::to_owned).filter(|e| !e.eq_ignore_ascii_case("none")).or(Some(chan.entry.clone()));
                        env.effects.push(Effect::Block {
                            entry,
                            substring: op.flag("match_substring", false),
                            entity: op.flag("match_entity", false).then_some(chan.entity_index),
                            channel: op.flag("match_channel", false).then_some(chan.slot),
                            until: env.now + f("input_duration", 0.0, inst) as f64,
                        });
                    }
                }
                Kind::MixLayer => {
                    if let (Some(layer), Some(group)) = (op.s("mixlayer"), op.s("mixgroup")) {
                        env.effects.push(Effect::MixLayer {
                            layer: layer.to_owned(),
                            group: group.to_owned(),
                            field: op.s("field").unwrap_or("volume").to_owned(),
                            value: f("input", 1.0, inst),
                        });
                    }
                }
                Kind::ViewInfo => {
                    let l = env.listener;
                    out[0] = [l.origin.x, l.origin.y, l.origin.z, 0.0, 0.0, 0.0];
                    let a = portal2_core::coords::quat_to_angles(Quat::from_rotation_arc(Vec3::X, l.forward.normalize_or(Vec3::X)));
                    out[1] = [a.x, a.y, a.z, 0.0, 0.0, 0.0];
                    out[2][0] = l.velocity.length();
                    out[3] = [l.velocity.x, l.velocity.y, l.velocity.z, 0.0, 0.0, 0.0];
                }
                Kind::Facing => {
                    // 1 when the source faces the listener, 0 facing away.
                    let a = input("input_angles", [0.0; 6], inst);
                    let fwd = portal2_core::coords::angles_forward(Vec3::new(a[0], a[1], a[2]));
                    let to = (env.listener.origin - chan.position.unwrap_or(chan.origin)).normalize_or_zero();
                    out[0][0] = (fwd.dot(to) + 1.0) * 0.5;
                }
                Kind::LogicSwitch => {
                    let s = f("input_switch", 0.0, inst);
                    out[0][0] = if s > 0.0 { f("input2", 0.0, inst) } else { f("input1", 0.0, inst) };
                }
                Kind::GetOpvar => {
                    let v = op.s("opvar").and_then(|n| env.opvars.get(&n.to_ascii_lowercase()));
                    out[0][0] = v.copied().unwrap_or(0.0);
                    out[1][0] = v.is_some() as u8 as f32;
                }
                Kind::SetOpvar => {
                    if let Some(n) = op.s("opvar") {
                        env.effects.push(Effect::Opvar(n.to_ascii_lowercase(), f("input", 0.0, inst)));
                    }
                }
                Kind::SysTime => out[0][0] = env.now as f32,
                Kind::MapName => {
                    out[0][0] = op.s("map_name").is_some_and(|m| m.eq_ignore_ascii_case(env.map)) as u8 as f32;
                }
                Kind::Platform => out[0][0] = op.flag("pc", false) as u8 as f32,
                Kind::Accumulate => out[0][0] = f("input", 0.0, inst),
                Kind::PrintFloat | Kind::Dashboard | Kind::PosVec8 | Kind::Unknown => {}
            }
            if op.kind != Kind::Output {
                inst.outputs[i] = out;
            }
        }
    }
}

fn output(op: &Op, chan: &mut Chan, input: &dyn Fn(&str, V, &Instance) -> V, f: &dyn Fn(&str, f32, &Instance) -> f32, inst: &Instance) {
    let x = f("input_float", 0.0, inst);
    match op.s("output").unwrap_or("").to_ascii_lowercase().as_str() {
        "speakers" => chan.speakers = Some(input("input_speakers", [0.0; 6], inst)),
        "pitch" => chan.out_pitch = Some(x),
        "dsp" => chan.dsp = x,
        "delay" => chan.delay = Some(x),
        "stop_hold" => chan.stop_hold = Some(x > 0.0),
        "mixlayer_trigger" => chan.mixlayer_trigger = x,
        "block_start" => chan.block_start = x > 0.0,
        "position" => {
            let v = input("input_vec3", [0.0; 6], inst);
            chan.position = Some(Vec3::new(v[0], v[1], v[2]));
        }
        _ => {}
    }
}

fn math_float(apply: &str, a: f32, b: f32) -> f32 {
    match apply.to_ascii_lowercase().as_str() {
        "set" => a,
        "add" => a + b,
        "sub" => a - b,
        "mult" => a * b,
        "div" => {
            if b > 0.0 {
                a / b
            } else {
                0.0
            }
        }
        "mod" => {
            if b > 0.0 {
                a % b
            } else {
                0.0
            }
        }
        "max" => {
            if a > b {
                a
            } else {
                b
            }
        }
        "min" => {
            if b > a {
                a
            } else {
                b
            }
        }
        "invert" => 1.0 - a,
        "greater_than" => (a > b) as u8 as f32,
        "less_than" => (b > a) as u8 as f32,
        "greater_than_or_equal" => (a >= b) as u8 as f32,
        "less_than_or_equal" => (b >= a) as u8 as f32,
        "equals" => (a == b) as u8 as f32,
        "not_equal" => (a != b) as u8 as f32,
        "invert_scale" => 1.0 - (1.0 - a) * b,
        _ => 0.0,
    }
}

/// Other playing sounds an entry-matching operator selects: (id, start time).
fn matching(op: &Op, chan: &Chan, env: &Env) -> Vec<(u64, f64)> {
    let entry = op.s("match_entry").filter(|e| !e.eq_ignore_ascii_case("none"));
    let substring = op.flag("match_substring", false);
    let by_entity = op.flag("match_entity", true);
    let by_channel = op.flag("match_channel", false);
    let target = entry.unwrap_or(&chan.entry).to_ascii_lowercase();
    if entry.is_none() && op.s("match_entry").is_some() {
        // "NONE" matches nothing.
        return Vec::new();
    }
    env.others
        .iter()
        .filter(|o| {
            let name = o.entry.to_ascii_lowercase();
            let name_ok = if substring { name.contains(&target) } else { name == target };
            name_ok && (!by_entity || o.entity_index == chan.entity_index) && (!by_channel || o.slot == chan.slot)
        })
        .map(|o| (o.id, o.started))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults(name: &str) -> f32 {
        match name {
            "snd_refdist" => 36.0,
            "snd_foliage_db_loss" => 4.0,
            "snd_gain_max" => 1.0,
            "snd_gain_min" => 0.01,
            _ => 0.0,
        }
    }

    #[test]
    fn falloff_matches_engine_curve() {
        // A 75 dB sound: full (compressed) gain up close, about 0.89 at
        // 202.5 units where the uncompressed gain would be 1/1.08.
        assert!((falloff(10.0, 75.0, &defaults) - 1.0).abs() < 0.01);
        assert!((falloff(202.5, 75.0, &defaults) - 0.8926).abs() < 0.002);
        assert!(falloff(4000.0, 75.0, &defaults) < 0.02);
        // Level 0 plays everywhere.
        assert!(falloff(100_000.0, 0.0, &defaults) > 0.99);
    }

    #[test]
    fn remap_edges_and_defaults() {
        // Equal input range: default_to_max decides the edge itself.
        assert_eq!(remap(0.0, 0.0, 0.0, 0.0, 1.0, true, true), 1.0);
        assert_eq!(remap(0.0, 0.0, 0.0, 0.0, 1.0, true, false), 0.0);
        assert_eq!(remap(0.5, 0.0, 0.0, 0.0, 1.0, true, false), 1.0);
        assert!((remap(0.25, 0.0, 0.5, 0.0, 1.0, true, true) - 0.5).abs() < 1e-6);
        assert_eq!(remap(2.0, 0.0, 0.5, 0.0, 1.0, true, true), 1.0);
        // A reversed output range.
        assert!((remap(0.25, 0.0, 1.0, 1.0, 0.0, true, true) - 0.75).abs() < 1e-6);
    }

    #[test]
    fn math_float_ops() {
        assert_eq!(math_float("div", 1.0, 0.0), 0.0);
        assert_eq!(math_float("invert_scale", 0.25, 0.5), 0.625);
        assert_eq!(math_float("greater_than_or_equal", 2.0, 2.0), 1.0);
        assert_eq!(math_float("not_equal", 2.0, 2.0), 0.0);
    }

    #[test]
    fn stereo_panning() {
        let listener = Listener { origin: Vec3::ZERO, forward: Vec3::X, right: Vec3::NEG_Y, velocity: Vec3::ZERO };
        // Straight ahead: both sides equal.
        let ahead = spatialize(&listener, Vec3::X, 0.0, 0.75, 2);
        assert!((ahead[0] - ahead[1]).abs() < 1e-4);
        // Hard left (+Y in Source): left louder than right.
        let left = spatialize(&listener, Vec3::Y, 0.0, 0.75, 2);
        assert!(left[0] > left[1] + 0.3, "{left:?}");
        // Full mono: 0.9 on both fronts, rears silent.
        let mono = spatialize(&listener, Vec3::Y, 1.0, 0.75, 2);
        assert!((mono[0] - 0.9).abs() < 1e-4 && (mono[1] - 0.9).abs() < 1e-4, "{mono:?}");
    }
}

#[cfg(test)]
mod install_tests {
    use super::*;
    use source_formats::fs::GameFileSystem;
    use source_formats::soundscript::SoundScripts;

    /// Runs an installed entry's update stack once and prints every
    /// operator's first output (skipped without an install).
    #[test]
    fn dialog_stack_outputs() {
        let Some(root) = GameFileSystem::find_install() else { return };
        let Ok(fs) = GameFileSystem::portal2(root) else { return };
        let scripts = SoundScripts::load(&fs).unwrap();
        let lib = Library::load(&fs);
        let mut mix = MixData::load(&fs);
        mix.update_layers(false);
        let entry = scripts.get("announcer.PreHub46").unwrap();
        let stack = lib.compile(StackKind::Update, entry.operator_stacks.as_ref(), Some("update_default")).unwrap();
        let mut inst = stack.instance();
        let mut chan = Chan { entry: entry.name.clone(), volume: 1.0, level: 0.0, pitch: 1.0, ..Default::default() };
        let listener = Listener { origin: Vec3::ZERO, forward: Vec3::X, right: Vec3::NEG_Y, velocity: Vec3::ZERO };
        let cvar = |n: &str| match n {
            "snd_surround_speakers" => -1.0,
            "snd_gain" | "snd_op_test_convar" | "snd_mixer_master_level" | "snd_mixer_master_dsp" | "snd_gain_max" => 1.0,
            "snd_refdist" => 36.0,
            _ => 0.0,
        };
        let opvars = HashMap::new();
        let mut env = Env {
            listener: &listener,
            mix: &mix,
            cvar: &cvar,
            opvars: &opvars,
            others: &[],
            now: 0.0,
            map: "",
            speaker_mode: 2,
            effects: Vec::new(),
        };
        stack.run(&mut inst, &mut chan, &mut env);
        for (op, out) in stack.ops.iter().zip(&inst.outputs) {
            println!("{:<32} {:?} {:?}", op.name, op.kind, out.first().map(|v| v[0]));
        }
        println!("speakers {:?}", chan.speakers);
        for op in stack.ops.iter().filter(|o| o.kind == Kind::Speakers) {
            println!("{} inputs {:?} strings {:?}", op.name, op.inputs, op.strings);
        }
        assert!(chan.speakers.is_some_and(|s| s[0] > 0.5), "{:?}", chan.speakers);
    }
}
