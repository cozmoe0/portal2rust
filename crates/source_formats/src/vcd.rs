//! Choreographed scenes (`scenes/*.vcd`): actors with channels of timed events
//! (speech, gestures, look-at and move-to targets, flex animation, triggers)
//! as saved by the scene editor. Compiled scenes from `scenes/scenes.image`
//! decode into the same [`Scene`]; see [`crate::scenes_image`].
//!
//! The text is a stream of quoted or bare tokens, `{` / `}` and `//` comments.
//! Keywords are case-insensitive. A few trailing values are optional and only
//! taken from the keyword's own line (`time <start> [end]`, a curve sample's
//! interpolation name), so the lexer remembers line breaks. Tokens this reader
//! does not recognise are kept in `unknown` lists instead of being dropped.

use crate::kv::{parse_f32_lenient, parse_i32_lenient};
use crate::{Error, Result, invalid};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scene {
    pub actors: Vec<Actor>,
    /// Events owned by the scene rather than an actor (sections, stop points).
    pub events: Vec<Event>,
    /// `scene_ramp`: intensity curve over the whole scene.
    pub ramp: Curve,
    pub map_name: Option<String>,
    /// Editor frame rate, clamped to 10..=240 (text only).
    pub fps: Option<u32>,
    /// Editor time snapping (text only).
    pub snap: bool,
    pub ignore_phonemes: bool,
    /// Editor zoom per tool window (text only).
    pub scale_settings: Vec<(String, i32)>,
    /// Unrecognised top-level tokens, as written.
    pub unknown: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Actor {
    pub name: String,
    pub channels: Vec<Channel>,
    pub active: bool,
    /// Model the editor previewed the actor with (text only).
    pub faceposer_model: Option<String>,
    pub unknown: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Channel {
    pub name: String,
    pub events: Vec<Event>,
    pub active: bool,
    pub unknown: Vec<String>,
}

/// Event types, in the order compiled scenes number them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum EventKind {
    #[default]
    Unspecified,
    /// Named point on the timeline.
    Section,
    /// Facial expression: `param` is the expression file, `param2` the name.
    Expression,
    /// Head and eyes follow `param` (offset by `pitch` / `yaw`).
    LookAt,
    /// Move to `param`; `param2` is the movement activity.
    MoveTo,
    /// Play sound `param` with lip sync and captions; `param2` may override the
    /// sound level.
    Speak,
    /// Layered animation sequence `param`, weighted by the event ramp.
    Gesture,
    /// Full animation sequence `param`.
    Sequence,
    /// Turn the body toward `param`.
    Face,
    /// Fire the scene entity's numbered trigger output `param`.
    FireTrigger,
    /// Flex controller curves (`flex_tracks`).
    FlexAnimation,
    /// Play another scene.
    Subscene,
    /// Jump back in time, `loop_count` times.
    Loop,
    Interrupt,
    /// Playback waits here.
    StopPoint,
    PermitResponses,
    Generic,
    Camera,
    Script,
    /// Type this reader does not know: the name as written, or the number from
    /// a compiled scene.
    Other(String),
}

const KIND_NAMES: [&str; 19] = [
    "unspecified",
    "section",
    "expression",
    "lookat",
    "moveto",
    "speak",
    "gesture",
    "sequence",
    "face",
    "firetrigger",
    "flexanimation",
    "subscene",
    "loop",
    "interrupt",
    "stoppoint",
    "permitresponses",
    "generic",
    "camera",
    "script",
];

impl EventKind {
    const KNOWN: [EventKind; 19] = [
        Self::Unspecified,
        Self::Section,
        Self::Expression,
        Self::LookAt,
        Self::MoveTo,
        Self::Speak,
        Self::Gesture,
        Self::Sequence,
        Self::Face,
        Self::FireTrigger,
        Self::FlexAnimation,
        Self::Subscene,
        Self::Loop,
        Self::Interrupt,
        Self::StopPoint,
        Self::PermitResponses,
        Self::Generic,
        Self::Camera,
        Self::Script,
    ];

    pub fn from_index(index: u8) -> Self {
        Self::KNOWN.get(index as usize).cloned().unwrap_or_else(|| Self::Other(index.to_string()))
    }

    pub fn from_name(name: &str) -> Self {
        match KIND_NAMES.iter().position(|n| n.eq_ignore_ascii_case(name)) {
            Some(i) => Self::KNOWN[i].clone(),
            None => Self::Other(name.to_owned()),
        }
    }

    /// Number used by compiled scenes; `None` for [`EventKind::Other`].
    pub fn index(&self) -> Option<u8> {
        Self::KNOWN.iter().position(|k| k == self).map(|i| i as u8)
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Other(name) => name,
            known => KIND_NAMES[known.index().unwrap_or(0) as usize],
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub kind: EventKind,
    pub name: String,
    /// Seconds from the start of the scene.
    pub start: f32,
    /// Seconds from the start of the scene; -1 for events without duration.
    pub end: f32,
    pub param: String,
    pub param2: String,
    pub param3: String,
    /// `event_ramp`: intensity over the event, times relative to `start`.
    pub ramp: Curve,
    pub flags: EventFlags,
    pub distance_to_target: f32,
    /// Look-at offsets in degrees (text only).
    pub pitch: i32,
    pub yaw: i32,
    /// Repeat count of a loop event; -1 when unset.
    pub loop_count: i32,
    /// Length of a gesture's sequence in seconds, when recorded.
    pub sequence_duration: Option<f32>,
    /// `tags`: named points as fractions of the event's duration.
    pub tags: Vec<Tag>,
    /// `flextimingtags`: like `tags`, but lockable in the editor.
    pub timing_tags: Vec<TimingTag>,
    /// `absolutetags playback_time`: gesture tags as fractions of playback.
    pub playback_tags: Vec<Tag>,
    /// `absolutetags shifted_time`: the same tags after time shifting.
    pub shifted_tags: Vec<Tag>,
    /// `relativetag`: the event starts at a tag of another (speak) event.
    pub relative_tag: Option<TagRef>,
    pub flex_tracks: Vec<FlexTrack>,
    pub cc: CloseCaption,
    /// Old `ramp <a> [b] [c]` shorthand (text only).
    pub legacy_ramp: Option<[f32; 3]>,
    /// Unrecognised tokens inside the event block, as written.
    pub unknown: Vec<String>,
}

impl Default for Event {
    fn default() -> Self {
        Self {
            kind: EventKind::Unspecified,
            name: String::new(),
            start: 0.0,
            end: -1.0,
            param: String::new(),
            param2: String::new(),
            param3: String::new(),
            ramp: Curve::default(),
            flags: EventFlags::default(),
            distance_to_target: 0.0,
            pitch: 0,
            yaw: 0,
            loop_count: -1,
            sequence_duration: None,
            tags: Vec::new(),
            timing_tags: Vec::new(),
            playback_tags: Vec::new(),
            shifted_tags: Vec::new(),
            relative_tag: None,
            flex_tracks: Vec::new(),
            cc: CloseCaption::default(),
            legacy_ramp: None,
            unknown: Vec::new(),
        }
    }
}

impl Event {
    pub fn has_duration(&self) -> bool {
        self.end != -1.0
    }

    /// Length in seconds, zero for events without duration.
    pub fn duration(&self) -> f32 {
        if self.has_duration() { (self.end - self.start).max(0.0) } else { 0.0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventFlags {
    pub fixed_length: bool,
    pub resume_condition: bool,
    pub lock_body_facing: bool,
    pub force_short_movement: bool,
    /// Not stored in compiled scenes.
    pub sync_to_following_gesture: bool,
    pub play_over_script: bool,
    pub active: bool,
}

impl Default for EventFlags {
    fn default() -> Self {
        Self {
            fixed_length: false,
            resume_condition: false,
            lock_body_facing: false,
            force_short_movement: false,
            sync_to_following_gesture: false,
            play_over_script: false,
            active: true,
        }
    }
}

/// Caption settings of a speak event.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CloseCaption {
    pub kind: CcType,
    /// Caption token to show instead of the sound's own; empty for the default.
    pub token: String,
    pub using_combined_file: bool,
    pub combined_uses_gender: bool,
    pub no_attenuate: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CcType {
    #[default]
    Master,
    Slave,
    Disabled,
}

impl CcType {
    pub fn from_index(index: u8) -> Self {
        match index {
            1 => Self::Slave,
            2 => Self::Disabled,
            _ => Self::Master,
        }
    }

    /// Unknown names count as `cc_master`, as in the game.
    pub fn from_name(name: &str) -> Self {
        match name.to_ascii_lowercase().as_str() {
            "cc_slave" => Self::Slave,
            "cc_disabled" => Self::Disabled,
            _ => Self::Master,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Master => "cc_master",
            Self::Slave => "cc_slave",
            Self::Disabled => "cc_disabled",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tag {
    pub name: String,
    pub value: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TimingTag {
    pub name: String,
    pub value: f32,
    /// Not stored in compiled scenes.
    pub locked: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagRef {
    /// Name of the tag.
    pub tag: String,
    /// Sound (speak event) that carries it.
    pub wav: String,
}

/// Keyframed 0..1 curve. Sample times are seconds relative to the owner.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Curve {
    pub samples: Vec<Sample>,
    /// Value and interpolation before the first sample (text only).
    pub left_edge: Option<Edge>,
    /// Value and interpolation after the last sample (text only).
    pub right_edge: Option<Edge>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    pub time: f32,
    pub value: f32,
    /// Interpolation into and out of this sample. Compiled event and scene
    /// ramps do not store it (always the default).
    pub curve: CurveType,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Edge {
    pub curve: CurveType,
    pub zero_value: f32,
}

/// Interpolation pair written as `curve_<from>_to_curve_<to>`; compiled
/// scenes pack it as `from << 8 | to`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct CurveType {
    pub from: Interpolator,
    pub to: Interpolator,
}

impl CurveType {
    pub fn from_bits(bits: u16) -> Self {
        Self { from: Interpolator::from_index((bits >> 8) as u8), to: Interpolator::from_index(bits as u8) }
    }

    pub fn bits(self) -> u16 {
        (self.from as u16) << 8 | self.to as u16
    }

    /// Names without the `curve_` prefix fall back to the default pair.
    pub fn parse(name: &str) -> Self {
        let lower = name.to_ascii_lowercase();
        let Some(rest) = lower.strip_prefix("curve_") else { return Self::default() };
        match rest.split_once("_to_curve_") {
            Some((from, to)) => Self { from: Interpolator::from_name(from), to: Interpolator::from_name(to) },
            None => {
                let i = Interpolator::from_name(rest);
                Self { from: i, to: i }
            }
        }
    }

    pub fn name(self) -> String {
        format!("curve_{}_to_curve_{}", self.from.name(), self.to.name())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Interpolator {
    #[default]
    Default,
    CatmullRomNormalizeX,
    EaseIn,
    EaseOut,
    EaseInOut,
    BSpline,
    LinearInterp,
    Kochanek,
    KochanekEarly,
    KochanekLate,
    SimpleCubic,
    CatmullRom,
    CatmullRomNormalize,
    CatmullRomTangent,
    ExponentialDecay,
    Hold,
}

const INTERPOLATORS: [(Interpolator, &str); 16] = [
    (Interpolator::Default, "default"),
    (Interpolator::CatmullRomNormalizeX, "catmullrom_normalize_x"),
    (Interpolator::EaseIn, "easein"),
    (Interpolator::EaseOut, "easeout"),
    (Interpolator::EaseInOut, "easeinout"),
    (Interpolator::BSpline, "bspline"),
    (Interpolator::LinearInterp, "linear_interp"),
    (Interpolator::Kochanek, "kochanek"),
    (Interpolator::KochanekEarly, "kochanek_early"),
    (Interpolator::KochanekLate, "kochanek_late"),
    (Interpolator::SimpleCubic, "simple_cubic"),
    (Interpolator::CatmullRom, "catmullrom"),
    (Interpolator::CatmullRomNormalize, "catmullrom_normalize"),
    (Interpolator::CatmullRomTangent, "catmullrom_tangent"),
    (Interpolator::ExponentialDecay, "exponential_decay"),
    (Interpolator::Hold, "hold"),
];

impl Interpolator {
    /// Out-of-range numbers fall back to the default.
    pub fn from_index(index: u8) -> Self {
        INTERPOLATORS.get(index as usize).map_or(Self::Default, |e| e.0)
    }

    pub fn from_name(name: &str) -> Self {
        INTERPOLATORS.iter().find(|e| e.1.eq_ignore_ascii_case(name)).map_or(Self::Default, |e| e.0)
    }

    pub fn name(self) -> &'static str {
        INTERPOLATORS[self as usize].1
    }
}

/// One flex controller's curve inside a flex animation event.
#[derive(Clone, Debug, PartialEq)]
pub struct FlexTrack {
    pub name: String,
    pub active: bool,
    /// Combo tracks drive a left/right controller pair: `samples` is the
    /// amount and `balance` the left/right mix.
    pub combo: bool,
    pub min: f32,
    pub max: f32,
    /// Sample times are seconds relative to the event start.
    pub samples: Vec<Sample>,
    pub balance: Vec<Sample>,
    /// Text only.
    pub left_edge: Option<Edge>,
    pub right_edge: Option<Edge>,
}

impl Default for FlexTrack {
    fn default() -> Self {
        Self {
            name: String::new(),
            active: true,
            combo: false,
            min: 0.0,
            max: 1.0,
            samples: Vec::new(),
            balance: Vec::new(),
            left_edge: None,
            right_edge: None,
        }
    }
}

impl Scene {
    pub fn parse(text: &str) -> Result<Self> {
        let mut lx = Lexer::new(text);
        let mut scene = Scene::default();
        while let Some(t) = lx.next() {
            match t.text.to_ascii_lowercase().as_str() {
                "event" => scene.events.push(parse_event(&mut lx)?),
                "actor" => scene.actors.push(parse_actor(&mut lx)?),
                "mapname" => scene.map_name = Some(lx.string()?),
                "fps" => scene.fps = Some(lx.int()?.clamp(10, 240) as u32),
                "snap" => scene.snap = lx.need()?.is("on"),
                "ignorephonemes" => scene.ignore_phonemes = lx.need()?.is("on"),
                "scene_ramp" => scene.ramp = parse_curve(&mut lx)?,
                "scalesettings" => scene.scale_settings = parse_scale_settings(&mut lx)?,
                _ => {
                    let raw = lx.unknown(t)?;
                    scene.unknown.push(raw);
                }
            }
        }
        Ok(scene)
    }

    /// Every event: scene-level ones first, then each actor's channels.
    pub fn all_events(&self) -> impl Iterator<Item = &Event> {
        self.events.iter().chain(self.actors.iter().flat_map(|a| a.channels.iter().flat_map(|c| c.events.iter())))
    }

    /// Latest start or end time of any event, in seconds.
    pub fn end_time(&self) -> f32 {
        self.all_events().map(|e| e.start.max(e.end)).fold(0.0, f32::max)
    }

    pub fn actor(&self, name: &str) -> Option<&Actor> {
        self.actors.iter().find(|a| a.name.eq_ignore_ascii_case(name))
    }
}

fn parse_actor(lx: &mut Lexer) -> Result<Actor> {
    let name = lx.string()?;
    lx.open()?;
    let mut actor = Actor { name, channels: Vec::new(), active: true, faceposer_model: None, unknown: Vec::new() };
    loop {
        let t = lx.need()?;
        if t.is_close() {
            return Ok(actor);
        }
        match t.text.to_ascii_lowercase().as_str() {
            "channel" => actor.channels.push(parse_channel(lx)?),
            "faceposermodel" => actor.faceposer_model = Some(lx.string()?),
            "active" => actor.active = lx.int()? != 0,
            _ => {
                let raw = lx.unknown(t)?;
                actor.unknown.push(raw);
            }
        }
    }
}

fn parse_channel(lx: &mut Lexer) -> Result<Channel> {
    let name = lx.string()?;
    lx.open()?;
    let mut channel = Channel { name, events: Vec::new(), active: true, unknown: Vec::new() };
    loop {
        let t = lx.need()?;
        if t.is_close() {
            return Ok(channel);
        }
        match t.text.to_ascii_lowercase().as_str() {
            "event" => channel.events.push(parse_event(lx)?),
            "active" => channel.active = lx.int()? != 0,
            _ => {
                let raw = lx.unknown(t)?;
                channel.unknown.push(raw);
            }
        }
    }
}

fn parse_event(lx: &mut Lexer) -> Result<Event> {
    let kind = EventKind::from_name(lx.need()?.text);
    let name = lx.string()?;
    lx.open()?;
    let mut ev = Event { kind, name, ..Event::default() };
    loop {
        let t = lx.need()?;
        if t.is_close() {
            return Ok(ev);
        }
        match t.text.to_ascii_lowercase().as_str() {
            "time" => {
                ev.start = lx.float()?;
                // The game leaves the end at 1.0 when only the start is given.
                ev.end = lx.same_line_float().unwrap_or(1.0);
            }
            "ramp" => {
                let a = lx.float()?;
                let b = lx.same_line_float().unwrap_or(1.0);
                let c = lx.same_line_float().unwrap_or(1.0);
                ev.legacy_ramp = Some([a, b, c]);
            }
            "param" => ev.param = lx.string()?,
            "param2" => ev.param2 = lx.string()?,
            "param3" => ev.param3 = lx.string()?,
            "pitch" => ev.pitch = lx.int()?,
            "yaw" => ev.yaw = lx.int()?,
            "loopcount" => ev.loop_count = lx.int()?,
            "resumecondition" => ev.flags.resume_condition = true,
            "fixedlength" => ev.flags.fixed_length = true,
            "lockbodyfacing" => ev.flags.lock_body_facing = true,
            "distancetotarget" => ev.distance_to_target = lx.float()?,
            "forceshortmovement" => ev.flags.force_short_movement = true,
            "synctofollowinggesture" => ev.flags.sync_to_following_gesture = true,
            "active" => ev.flags.active = lx.int()? != 0,
            "playoverscript" => ev.flags.play_over_script = true,
            "tags" => ev.tags = parse_tags(lx)?,
            "sequenceduration" => ev.sequence_duration = Some(lx.float()?),
            "absolutetags" => {
                let kind = lx.need()?;
                let tags = parse_tags(lx)?;
                if kind.is("playback_time") {
                    ev.playback_tags = tags;
                } else if kind.is("shifted_time") {
                    ev.shifted_tags = tags;
                } else {
                    ev.unknown.push(format!("absolutetags {}", kind.text));
                }
            }
            "flextimingtags" => ev.timing_tags = parse_timing_tags(lx)?,
            "relativetag" => {
                let tag = lx.string()?;
                let wav = lx.string()?;
                ev.relative_tag = Some(TagRef { tag, wav });
            }
            "flexanimations" => ev.flex_tracks = parse_flex_animations(lx, ev.end - ev.start)?,
            "event_ramp" => ev.ramp = parse_curve(lx)?,
            "cctype" => ev.cc.kind = CcType::from_name(lx.need()?.text),
            "cctoken" => ev.cc.token = lx.string()?,
            "cc_usingcombinedfile" => ev.cc.using_combined_file = true,
            "cc_combinedusesgender" => ev.cc.combined_uses_gender = true,
            "cc_noattenuate" => ev.cc.no_attenuate = true,
            _ => {
                let raw = lx.unknown(t)?;
                ev.unknown.push(raw);
            }
        }
    }
}

/// `{ "name" value ... }`
fn parse_tags(lx: &mut Lexer) -> Result<Vec<Tag>> {
    lx.open()?;
    let mut tags = Vec::new();
    loop {
        let t = lx.need()?;
        if t.is_close() {
            return Ok(tags);
        }
        tags.push(Tag { name: t.text.to_owned(), value: lx.float()? });
    }
}

/// `{ "name" value locked ... }`
fn parse_timing_tags(lx: &mut Lexer) -> Result<Vec<TimingTag>> {
    lx.open()?;
    let mut tags = Vec::new();
    loop {
        let t = lx.need()?;
        if t.is_close() {
            return Ok(tags);
        }
        let value = lx.float()?;
        let locked = lx.int()? != 0;
        tags.push(TimingTag { name: t.text.to_owned(), value, locked });
    }
}

fn parse_scale_settings(lx: &mut Lexer) -> Result<Vec<(String, i32)>> {
    lx.open()?;
    let mut out = Vec::new();
    loop {
        let t = lx.need()?;
        if t.is_close() {
            return Ok(out);
        }
        let scale = lx.int()?;
        out.push((t.text.to_owned(), if scale > 0 { scale } else { 100 }));
    }
}

/// `[leftedge <curve> <value>] [rightedge <curve> <value>] { <time> <value> [curve] ... }`
fn parse_curve(lx: &mut Lexer) -> Result<Curve> {
    let mut curve = Curve::default();
    let mut t = lx.need()?;
    if t.is("leftedge") {
        curve.left_edge = Some(parse_edge(lx)?);
        t = lx.need()?;
    }
    if t.is("rightedge") {
        curve.right_edge = Some(parse_edge(lx)?);
        t = lx.need()?;
    }
    expect_open(t)?;
    curve.samples = parse_samples(lx, 1.0, CurveType::default())?;
    Ok(curve)
}

fn parse_edge(lx: &mut Lexer) -> Result<Edge> {
    let curve = CurveType::parse(lx.need()?.text);
    Ok(Edge { curve, zero_value: lx.float()? })
}

/// Samples up to the closing brace; the opening one is already consumed.
fn parse_samples(lx: &mut Lexer, time_scale: f32, default_curve: CurveType) -> Result<Vec<Sample>> {
    let mut samples = Vec::new();
    loop {
        let t = lx.need()?;
        if t.is_close() {
            return Ok(samples);
        }
        let time = parse_f32_lenient(t.text) * time_scale;
        let value = lx.float()?;
        let curve = lx.same_line().map_or(default_curve, |c| CurveType::parse(c.text));
        samples.push(Sample { time, value, curve });
    }
}

/// `flexanimations [samples_use_time] [defaultcurvetype=<curve>] { <track> ... }`
/// where a track is `"name" [disabled] [combo] [range <min> <max>]
/// [leftedge ..] [rightedge ..] { samples } [{ balance samples }]`. Sample
/// times are fractions of the event's duration unless `samples_use_time`.
fn parse_flex_animations(lx: &mut Lexer, duration: f32) -> Result<Vec<FlexTrack>> {
    let mut time_scale = duration;
    let mut default_curve = CurveType::default();
    let mut t = lx.need()?;
    if t.is("samples_use_time") {
        time_scale = 1.0;
        t = lx.need()?;
    }
    const DEFAULT_CURVE: &str = "defaultcurvetype";
    if let Some(prefix) = t.text.get(..DEFAULT_CURVE.len())
        && prefix.eq_ignore_ascii_case(DEFAULT_CURVE)
    {
        // `defaultcurvetype=name`, with optional spaces around the `=`.
        let mut rest = &t.text[DEFAULT_CURVE.len()..];
        if rest.is_empty() {
            rest = lx.need()?.text;
        }
        rest = rest.strip_prefix('=').unwrap_or(rest);
        if rest.is_empty() {
            rest = lx.need()?.text;
        }
        default_curve = CurveType::parse(rest);
        t = lx.need()?;
    }
    expect_open(t)?;
    let mut tracks = Vec::new();
    loop {
        let t = lx.need()?;
        if t.is_close() {
            return Ok(tracks);
        }
        let mut track = FlexTrack { name: t.text.to_owned(), ..FlexTrack::default() };
        let mut t = lx.need()?;
        if t.is("disabled") {
            track.active = false;
            t = lx.need()?;
        }
        if t.is("combo") {
            track.combo = true;
            t = lx.need()?;
        }
        if t.is("range") {
            track.min = lx.float()?;
            track.max = lx.float()?;
            t = lx.need()?;
        }
        if t.is("leftedge") {
            track.left_edge = Some(parse_edge(lx)?);
            t = lx.need()?;
        }
        if t.is("rightedge") {
            track.right_edge = Some(parse_edge(lx)?);
            t = lx.need()?;
        }
        expect_open(t)?;
        track.samples = parse_samples(lx, time_scale, default_curve)?;
        if track.combo {
            lx.open()?;
            track.balance = parse_samples(lx, time_scale, default_curve)?;
        }
        // The game drops disabled tracks that have no samples.
        if track.active || !track.samples.is_empty() {
            tracks.push(track);
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Tok<'a> {
    text: &'a str,
    quoted: bool,
    /// A line break separates this token from the previous one.
    new_line: bool,
    /// Byte range in the source, including quotes.
    start: usize,
    end: usize,
}

impl Tok<'_> {
    fn is(&self, keyword: &str) -> bool {
        self.text.eq_ignore_ascii_case(keyword)
    }

    fn is_open(&self) -> bool {
        !self.quoted && self.text == "{"
    }

    fn is_close(&self) -> bool {
        !self.quoted && self.text == "}"
    }
}

fn expect_open(t: Tok) -> Result<()> {
    if t.is_open() { Ok(()) } else { invalid(format!("vcd: expected {{, found {:?}", t.text)) }
}

struct Lexer<'a> {
    src: &'a str,
    pos: usize,
    peeked: Option<Tok<'a>>,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str) -> Self {
        Self { src, pos: 0, peeked: None }
    }

    fn scan(&mut self) -> Option<Tok<'a>> {
        let b = self.src.as_bytes();
        let mut new_line = false;
        loop {
            while self.pos < b.len() && b[self.pos].is_ascii_whitespace() {
                new_line |= b[self.pos] == b'\n';
                self.pos += 1;
            }
            if b[self.pos..].starts_with(b"//") {
                while self.pos < b.len() && b[self.pos] != b'\n' {
                    self.pos += 1;
                }
                continue;
            }
            break;
        }
        let start = self.pos;
        let (text, quoted) = match *b.get(start)? {
            b'{' | b'}' => {
                self.pos += 1;
                (&self.src[start..self.pos], false)
            }
            b'"' => {
                let close = b[start + 1..].iter().position(|&c| c == b'"').map_or(b.len(), |p| start + 1 + p);
                self.pos = (close + 1).min(b.len());
                (&self.src[start + 1..close], true)
            }
            _ => {
                while self.pos < b.len()
                    && !b[self.pos].is_ascii_whitespace()
                    && !matches!(b[self.pos], b'{' | b'}' | b'"')
                    && !b[self.pos..].starts_with(b"//")
                {
                    self.pos += 1;
                }
                (&self.src[start..self.pos], false)
            }
        };
        Some(Tok { text, quoted, new_line, start, end: self.pos })
    }

    fn next(&mut self) -> Option<Tok<'a>> {
        self.peeked.take().or_else(|| self.scan())
    }

    fn peek(&mut self) -> Option<Tok<'a>> {
        if self.peeked.is_none() {
            self.peeked = self.scan();
        }
        self.peeked
    }

    fn need(&mut self) -> Result<Tok<'a>> {
        self.next().ok_or(Error::Eof)
    }

    /// The next token if it is on the current line and not a brace.
    fn same_line(&mut self) -> Option<Tok<'a>> {
        match self.peek() {
            Some(t) if !t.new_line && !t.is_open() && !t.is_close() => self.next(),
            _ => None,
        }
    }

    fn same_line_float(&mut self) -> Option<f32> {
        self.same_line().map(|t| parse_f32_lenient(t.text))
    }

    fn string(&mut self) -> Result<String> {
        Ok(self.need()?.text.to_owned())
    }

    fn float(&mut self) -> Result<f32> {
        Ok(parse_f32_lenient(self.need()?.text))
    }

    fn int(&mut self) -> Result<i32> {
        Ok(parse_i32_lenient(self.need()?.text))
    }

    fn open(&mut self) -> Result<()> {
        expect_open(self.need()?)
    }

    /// Source text of an unrecognised token, including a block right after it.
    fn unknown(&mut self, t: Tok<'a>) -> Result<String> {
        if !self.peek().is_some_and(|n| n.is_open()) {
            return Ok(t.text.to_owned());
        }
        let mut depth = 0;
        loop {
            let n = self.need()?;
            if n.is_open() {
                depth += 1;
            } else if n.is_close() {
                depth -= 1;
            }
            if depth == 0 {
                return Ok(self.src[t.start..n.end].to_owned());
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    // Hand-written scene covering every construct the reader knows.
    const SAMPLE: &str = r#"// Choreo version 1
event section "intro"
{
  time 1.5 -1.000000
  param "noaction"
}
actor "Narrator"
{
  faceposermodel "models\person.mdl"
  channel "Audio"
  {
    event speak "line_a"
    {
      time 0.25 2.000000
      param "person.line_a"
      param2 "80dB"
      fixedlength
      cctype "cc_slave"
      cctoken "line_a_alt"
      cc_usingcombinedfile
      cc_noattenuate
      tags
      {
        "beat" 0.500000
      }
    }
    active "1"
  }
  channel "Gestures"
  {
    event gesture "wave"
    {
      time 1 3
      param "g_wave"
      event_ramp leftedge "curve_easein_to_curve_easeout" 0.25
      {
        0.1000 0.5000 "curve_catmullrom_normalize_x_to_curve_hold"
        1.5000 1.0000
      }
      absolutetags playback_time
      {
        "apex" 0.400000
      }
      absolutetags shifted_time
      {
        "apex" 0.450000
      }
      sequenceduration 1.250000
      relativetag "beat" "person.line_a"
      synctofollowinggesture
    }
    event lookat "look"
    {
      time 0 1
      param "!player"
      pitch -10
      yaw 15
      lockbodyfacing
    }
    event loop "again"
    {
      time 4 -1
      param "0.5"
      loopcount "3"
    }
    event flexanimation "face"
    {
      time 2 6
      flexanimations defaultcurvetype=curve_linear_interp_to_curve_linear_interp
      {
        "smile" range 0 2
        {
          0.25 1.0
          0.5 0.5 "curve_hold_to_curve_hold"
        }
        "blink" combo
        {
          0.0 0.0
        }
        {
          0.0 0.5
        }
        "unused" disabled
        {
        }
      }
      flextimingtags
      {
        "t1" 0.2 1
      }
      mystery_flag
      future_block { 1 2 { 3 } }
    }
    active "0"
  }
}
scene_ramp
{
  0.0 1.0
}
mapname "maps\test.bsp"
scalesettings
{
  "CChoreoView" "100"
  "ExpressionTool" "0"
}
fps 500
snap on
ignorePhonemes on
"#;

    #[test]
    fn parses_every_construct() {
        let s = Scene::parse(SAMPLE).unwrap();
        assert!(s.unknown.is_empty(), "{:?}", s.unknown);
        assert_eq!(s.events.len(), 1);
        assert_eq!(s.events[0].kind, EventKind::Section);
        assert_eq!((s.events[0].start, s.events[0].end), (1.5, -1.0));
        assert!(!s.events[0].has_duration());
        assert_eq!(s.map_name.as_deref(), Some("maps\\test.bsp"));
        assert_eq!(s.fps, Some(240));
        assert!(s.snap && s.ignore_phonemes);
        assert_eq!(s.scale_settings, vec![("CChoreoView".to_owned(), 100), ("ExpressionTool".to_owned(), 100)]);
        assert_eq!(s.ramp.samples.len(), 1);

        let actor = s.actor("narrator").unwrap();
        assert_eq!(actor.faceposer_model.as_deref(), Some("models\\person.mdl"));
        let audio = &actor.channels[0];
        assert!(audio.active);
        let speak = &audio.events[0];
        assert_eq!(speak.kind, EventKind::Speak);
        assert_eq!((speak.start, speak.end, speak.duration()), (0.25, 2.0, 1.75));
        assert_eq!((speak.param.as_str(), speak.param2.as_str()), ("person.line_a", "80dB"));
        assert!(speak.flags.fixed_length && speak.flags.active);
        assert_eq!(speak.cc.kind, CcType::Slave);
        assert_eq!(speak.cc.token, "line_a_alt");
        assert!(speak.cc.using_combined_file && speak.cc.no_attenuate && !speak.cc.combined_uses_gender);
        assert_eq!(speak.tags, vec![Tag { name: "beat".into(), value: 0.5 }]);

        let gestures = &actor.channels[1];
        assert!(!gestures.active);
        let wave = &gestures.events[0];
        let edge = wave.ramp.left_edge.unwrap();
        assert_eq!(edge.curve, CurveType { from: Interpolator::EaseIn, to: Interpolator::EaseOut });
        assert_eq!(edge.zero_value, 0.25);
        assert_eq!(wave.ramp.samples[0].curve.bits(), 1 << 8 | 15);
        assert_eq!(wave.ramp.samples[1].curve, CurveType::default());
        assert_eq!(wave.playback_tags[0].value, 0.4);
        assert_eq!(wave.shifted_tags[0].value, 0.45);
        assert_eq!(wave.sequence_duration, Some(1.25));
        assert_eq!(wave.relative_tag, Some(TagRef { tag: "beat".into(), wav: "person.line_a".into() }));
        assert!(wave.flags.sync_to_following_gesture);

        let look = &gestures.events[1];
        assert_eq!((look.pitch, look.yaw), (-10, 15));
        assert!(look.flags.lock_body_facing);
        let lp = &gestures.events[2];
        assert_eq!((lp.kind.clone(), lp.loop_count), (EventKind::Loop, 3));

        let face = &gestures.events[3];
        assert_eq!(face.flex_tracks.len(), 2, "disabled empty track is dropped");
        let smile = &face.flex_tracks[0];
        assert_eq!((smile.min, smile.max), (0.0, 2.0));
        // Fractions of the 4 second event.
        assert_eq!(smile.samples[0].time, 1.0);
        assert_eq!(smile.samples[0].curve.from, Interpolator::LinearInterp);
        assert_eq!(smile.samples[1].curve.from, Interpolator::Hold);
        let blink = &face.flex_tracks[1];
        assert!(blink.combo);
        assert_eq!(blink.balance[0].value, 0.5);
        assert_eq!(face.timing_tags[0].name, "t1");
        assert!(face.timing_tags[0].locked);
        assert_eq!(face.unknown, vec!["mystery_flag".to_owned(), "future_block { 1 2 { 3 } }".to_owned()]);
        assert_eq!(s.all_events().count(), 6);
        assert_eq!(s.end_time(), 6.0);
    }

    #[test]
    fn kinds_and_curves_round_trip() {
        for i in 0..19 {
            let k = EventKind::from_index(i);
            assert_eq!(k.index(), Some(i));
            assert_eq!(EventKind::from_name(k.name()), k);
        }
        assert_eq!(EventKind::from_name("Speak"), EventKind::Speak);
        assert_eq!(EventKind::from_name("dance"), EventKind::Other("dance".into()));
        assert_eq!(EventKind::from_index(40), EventKind::Other("40".into()));
        for bits in [0u16, 0x0102, 0x0f0f] {
            let c = CurveType::from_bits(bits);
            assert_eq!(CurveType::parse(&c.name()), c);
            assert_eq!(c.bits(), bits);
        }
        assert_eq!(CurveType::parse("linear"), CurveType::default());
    }

    #[test]
    fn time_without_end_and_errors() {
        let s = Scene::parse("event stoppoint \"x\" { time 2\n param \"noaction\" }").unwrap();
        assert_eq!((s.events[0].start, s.events[0].end), (2.0, 1.0));
        assert_eq!(s.events[0].param, "noaction");
        assert!(Scene::parse("actor \"a\" { channel \"c\" {").is_err());
        assert!(Scene::parse("event speak \"a\" time").is_err());
    }

    /// Every scene in the installed VPKs parses without unknown tokens.
    #[test]
    fn installed_vcds_parse() {
        let Some(root) = crate::fs::GameFileSystem::find_install() else {
            eprintln!("skipping: Portal 2 install not found");
            return;
        };
        let mut kinds = std::collections::BTreeMap::<String, usize>::new();
        let mut files = 0;
        for (game, vpk) in installed_vpks(&root) {
            for path in vpk.paths().filter(|p| p.starts_with("scenes/") && p.ends_with(".vcd")) {
                let bytes = vpk.read(path).unwrap();
                let text = crate::text::decode_text(&bytes);
                let scene = Scene::parse(&text).unwrap_or_else(|e| panic!("{game}/{path}: {e}"));
                assert!(scene.unknown.is_empty(), "{game}/{path}: {:?}", scene.unknown);
                for a in &scene.actors {
                    assert!(a.unknown.is_empty(), "{game}/{path}: {:?}", a.unknown);
                    for c in &a.channels {
                        assert!(c.unknown.is_empty(), "{game}/{path}: {:?}", c.unknown);
                    }
                }
                for e in scene.all_events() {
                    assert!(e.unknown.is_empty(), "{game}/{path} {}: {:?}", e.name, e.unknown);
                    assert!(!matches!(e.kind, EventKind::Other(_)), "{game}/{path}: {:?}", e.kind);
                    *kinds.entry(e.kind.name().to_owned()).or_default() += 1;
                }
                files += 1;
            }
        }
        eprintln!("{files} scenes, events by type: {kinds:?}");
    }

    pub(crate) fn installed_vpks(root: &std::path::Path) -> Vec<(String, crate::vpk::Vpk)> {
        let mut out = Vec::new();
        let mut dirs: Vec<_> = std::fs::read_dir(root).unwrap().flatten().map(|e| e.path()).collect();
        dirs.sort();
        for dir in dirs {
            let vpk = dir.join("pak01_dir.vpk");
            if vpk.is_file() {
                let game = dir.file_name().unwrap().to_string_lossy().into_owned();
                out.push((game, crate::vpk::Vpk::open(&vpk).unwrap()));
            }
        }
        out
    }
}
