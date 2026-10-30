//! Particle system definitions compiled for simulation. Every entry of a
//! definition's six operator stacks becomes a typed operator holding its
//! parameters: the file's values, or the defaults the game's operator
//! tables give attributes a file leaves out (`docs/re/particles.md`).
//! Operators this runtime does not implement are kept as `Unsupported` so
//! the rest of the stack still runs.

use std::sync::Arc;

use bevy::math::Vec3;
use source_formats::pcf::{Attributes, ParticleOperator, ParticleSystemDefinition};

/// Particle fields, by the index operators name them with.
pub mod field {
    pub const POSITION: usize = 0;
    pub const LIFE: usize = 1;
    pub const PREV_POSITION: usize = 2;
    pub const RADIUS: usize = 3;
    pub const ROLL: usize = 4;
    pub const ROLL_SPEED: usize = 5;
    pub const COLOR: usize = 6;
    pub const ALPHA: usize = 7;
    pub const CREATED: usize = 8;
    pub const SEQUENCE: usize = 9;
    pub const TRAIL_LENGTH: usize = 10;
    pub const ID: usize = 11;
    pub const YAW: usize = 12;
    pub const SEQUENCE1: usize = 13;
    pub const HITBOX_OFFSET: usize = 15;
    pub const ALPHA2: usize = 16;
    pub const SCRATCH_VECTOR: usize = 17;
    pub const NORMAL: usize = 21;
    pub const GLOW_RGB: usize = 22;
    pub const GLOW_ALPHA: usize = 23;
    pub const COUNT: usize = 24;

    pub fn is_vector(f: usize) -> bool {
        matches!(f, POSITION | PREV_POSITION | COLOR | HITBOX_OFFSET | SCRATCH_VECTOR | NORMAL | GLOW_RGB)
    }
}

/// Typed access to an operator's stored attributes with fallbacks.
struct P<'a>(&'a Attributes);

impl P<'_> {
    fn f(&self, name: &str, default: f32) -> f32 {
        self.0.get_f32(name).unwrap_or(default)
    }
    fn i(&self, name: &str, default: i32) -> i32 {
        self.0.get_i32(name).unwrap_or(default)
    }
    fn b(&self, name: &str, default: bool) -> bool {
        self.0.get_bool(name).unwrap_or(default)
    }
    fn v(&self, name: &str, default: Vec3) -> Vec3 {
        self.0.get_vec3(name).unwrap_or(default)
    }
    /// Colour as 0..1 floats (no gamma conversion).
    fn c(&self, name: &str, default: [u8; 4]) -> [f32; 4] {
        self.0.get_color(name).unwrap_or(default).map(|b| b as f32 / 255.0)
    }
    fn field(&self, name: &str, default: usize) -> usize {
        (self.i(name, default as i32).max(0) as usize).min(field::COUNT - 1)
    }
    fn cp(&self, name: &str, default: i32) -> i32 {
        self.i(name, default).clamp(-1, 63)
    }
}

/// Attributes every operator has: when it is active and how strongly.
#[derive(Clone, Debug, Default)]
pub struct Common {
    pub fade_in: (f32, f32),
    pub fade_out: (f32, f32),
    pub fade_oscillate: f32,
    pub time_offset: (i32, f32, f32),
    pub time_scale: (i32, f32, f32),
    pub strength: (i32, f32, f32),
    /// -1 always, 0 only before the end cap, 1 only during it.
    pub end_cap: i32,
    /// No fade, offset, scale or strength attributes: always full strength.
    pub constant: bool,
}

impl Common {
    fn parse(p: &P) -> Self {
        let c = Self {
            fade_in: (p.f("operator start fadein", 0.0), p.f("operator end fadein", 0.0)),
            fade_out: (p.f("operator start fadeout", 0.0), p.f("operator end fadeout", 0.0)),
            fade_oscillate: p.f("operator fade oscillate", 0.0),
            time_offset: (p.i("operator time offset seed", 0), p.f("operator time offset min", 0.0), p.f("operator time offset max", 0.0)),
            time_scale: (p.i("operator time scale seed", 0), p.f("operator time scale min", 1.0), p.f("operator time scale max", 1.0)),
            strength: (
                p.i("operator strength scale seed", 0),
                p.f("operator strength random scale min", 1.0),
                p.f("operator strength random scale max", 1.0),
            ),
            end_cap: p.i("operator end cap state", -1),
            constant: false,
        };
        let constant = c.fade_in == (0.0, 0.0)
            && c.fade_out == (0.0, 0.0)
            && c.fade_oscillate <= 0.0
            && c.time_offset.0 == 0
            && c.time_scale.0 == 0
            && c.strength.0 == 0;
        Self { constant, ..c }
    }
}

#[derive(Clone, Debug)]
pub enum EmitterKind {
    Continuous { start: f32, rate: f32, duration: f32, scale_cp: i32, scale_cp_field: usize, per_used_cp: f32 },
    Instant { start: f32, start_max: f32, count: i32, count_min: i32, per_step: i32, scale_cp: i32, scale_cp_field: usize },
    Noise { start: f32, duration: f32, time_scale: f32, time_offset: f32, absolute: bool, invert: bool, min: f32, max: f32 },
    Maintain { count: i32, start: f32 },
}

#[derive(Clone, Debug)]
pub enum InitKind {
    RandomScalar {
        field: usize,
        min: f32,
        max: f32,
        exponent: f32,
    },
    RandomRotation {
        field: usize,
        initial: f32,
        min: f32,
        max: f32,
        exponent: f32,
        flip: bool,
    },
    RandomColor {
        color1: [f32; 4],
        color2: [f32; 4],
        tint: f32,
        tint_cp: i32,
        clamp_min: [f32; 4],
        clamp_max: [f32; 4],
        blend: i32,
        amplify: f32,
        field: usize,
    },
    RandomSequence {
        field: usize,
        min: i32,
        max: i32,
        shuffle: bool,
        linear: bool,
    },
    YawFlip {
        chance: f32,
    },
    Sphere {
        distance: (f32, f32),
        bias: Vec3,
        bias_abs: Vec3,
        local_bias: bool,
        cp: i32,
        speed: (f32, f32, f32),
        local_speed: (Vec3, Vec3),
        distribute_cps: bool,
        growth_time: f32,
        scale_cp: i32,
    },
    Box {
        min: Vec3,
        max: Vec3,
        cp: i32,
        local: bool,
    },
    Offset {
        min: Vec3,
        max: Vec3,
        cp: i32,
        local: bool,
        proportional: bool,
    },
    Warp {
        min: Vec3,
        max: Vec3,
        cp: i32,
        transition: f32,
        start: f32,
        reverse: bool,
    },
    VelocityNoise {
        cp: i32,
        time_scale: f32,
        spatial_scale: f32,
        time_offset: f32,
        spatial_offset: Vec3,
        absolute: Vec3,
        invert: Vec3,
        min: Vec3,
        max: Vec3,
        local: bool,
    },
    VelocityRandom {
        cp: i32,
        speed: (f32, f32),
        local: (Vec3, Vec3),
    },
    VelocityInherit {
        cp: i32,
        scale: f32,
    },
    Ring {
        cp: i32,
        radius: f32,
        thickness: f32,
        speed: (f32, f32),
        angles: Vec3,
        even: bool,
        even_count: f32,
        xy_velocity: bool,
    },
    AlongPath {
        sequential: bool,
        max_distance: f32,
        bulge: f32,
        start_cp: i32,
        end_cp: i32,
        mid: f32,
        steps: f32,
        looping: bool,
    },
    PlaceOnGround {
        offset: f32,
        max_length: f32,
        kill: bool,
    },
    LifetimeFromSequence {
        fps: f32,
    },
    RemapScalar {
        input: usize,
        in_range: (f32, f32),
        output: usize,
        out_range: (f32, f32),
        scale_initial: bool,
        clamp_to_range: bool,
        emitter_window: (f32, f32),
    },
    RemapNoise {
        field: usize,
        time_scale: f32,
        spatial_scale: f32,
        time_offset: f32,
        spatial_offset: Vec3,
        absolute: bool,
        invert: bool,
        out_range: (f32, f32),
        world_time_scale: f32,
    },
    RemapDistanceToCp {
        cp: i32,
        distance: (f32, f32),
        output: usize,
        out_range: (f32, f32),
        scale_initial: bool,
        within_only: bool,
    },
    RemapCount {
        in_range: (i32, i32),
        output: usize,
        out_range: (f32, f32),
        scale_initial: bool,
        within_only: bool,
    },
    RemapCpToScalar {
        cp: i32,
        component: usize,
        in_range: (f32, f32),
        output: usize,
        out_range: (f32, f32),
        scale_initial: bool,
    },
    RemapCpToVector {
        cp: i32,
        in_range: (Vec3, Vec3),
        output: usize,
        out_range: (Vec3, Vec3),
        scale_initial: bool,
        offset: bool,
    },
    PreAgeNoise {
        time_scale: f32,
        spatial_scale: f32,
        time_offset: f32,
        spatial_offset: Vec3,
        absolute: bool,
        invert: bool,
        age: (f32, f32),
    },
    VelocityFromCp {
        cp: i32,
        scale: f32,
    },
    NormalAlign {
        cp: i32,
    },
    NormalOffset {
        cp: i32,
        min: Vec3,
        max: Vec3,
        local: bool,
        normalize: bool,
    },
    BetweenPoints {
        speed: (f32, f32),
        end_spread: f32,
        start_offset: f32,
        end_offset: f32,
        end_cp: i32,
    },
    FromParent {
        velocity_scale: f32,
        random: bool,
    },
    TimeToImpact {
        max_length: f32,
        offset: f32,
        bias: Vec3,
    },
    RemapScalarToVector {
        input: usize,
        in_range: (f32, f32),
        output: usize,
        out_range: (Vec3, Vec3),
        scale_initial: bool,
        local: bool,
        cp: i32,
        emitter_window: (f32, f32),
    },
    LitColor {
        color1: [f32; 4],
        color2: [f32; 4],
        clamp_min: [f32; 4],
        clamp_max: [f32; 4],
        blend: i32,
        amplify: f32,
    },
    Unsupported(String),
}

impl InitKind {
    /// Modifies values earlier initializers set, so it always runs (the
    /// game's second initializer pass).
    pub fn modifies(&self) -> bool {
        matches!(
            self,
            Self::Offset { .. }
                | Self::Warp { .. }
                | Self::PlaceOnGround { .. }
                | Self::RemapScalar { .. }
                | Self::RemapNoise { .. }
                | Self::RemapDistanceToCp { .. }
                | Self::RemapCount { .. }
                | Self::RemapCpToScalar { .. }
                | Self::RemapCpToVector { .. }
                | Self::NormalOffset { .. }
                | Self::VelocityNoise { .. }
                | Self::VelocityRandom { .. }
                | Self::VelocityInherit { .. }
                | Self::VelocityFromCp { .. }
                | Self::YawFlip { .. }
                | Self::LifetimeFromSequence { .. }
                | Self::PreAgeNoise { .. }
                | Self::TimeToImpact { .. }
                | Self::RemapScalarToVector { .. }
        )
    }

    /// Fields the initializer writes (for skipping initializers whose
    /// fields are all written already).
    pub fn writes(&self) -> u32 {
        use field::*;
        let bit = |f: usize| 1u32 << f;
        match self {
            Self::RandomScalar { field, .. } | Self::RandomRotation { field, .. } => bit(*field),
            Self::RandomColor { field, .. } => bit(*field),
            Self::RandomSequence { field, .. } => bit(*field),
            Self::Sphere { .. } | Self::Box { .. } | Self::Ring { .. } | Self::AlongPath { .. } => bit(POSITION) | bit(PREV_POSITION),
            Self::BetweenPoints { .. } | Self::FromParent { .. } => bit(POSITION) | bit(PREV_POSITION) | bit(LIFE),
            Self::NormalAlign { .. } => bit(NORMAL),
            Self::LitColor { .. } => bit(COLOR),
            _ => 0,
        }
    }
}

#[derive(Clone, Debug)]
pub enum OpKind {
    Movement {
        gravity: Vec3,
        drag: f32,
        passes: i32,
    },
    Decay,
    FadeAndKill {
        alpha: (f32, f32),
        fade_in: (f32, f32),
        fade_out: (f32, f32),
    },
    FadeIn {
        time: (f32, f32),
        exponent: f32,
        proportional: bool,
    },
    FadeOut {
        time: (f32, f32),
        exponent: f32,
        proportional: bool,
        ease: bool,
        bias: f32,
    },
    FadeInSimple {
        time: f32,
    },
    FadeOutSimple {
        time: f32,
    },
    RadiusScale {
        time: (f32, f32),
        scale: (f32, f32),
        ease: bool,
        bias: f32,
    },
    ColorFade {
        color: [f32; 4],
        time: (f32, f32),
        ease: bool,
        field: usize,
    },
    Spin {
        field: usize,
        rate: f32,
        min_rate: f32,
        stop_time: f32,
    },
    SpinUpdate,
    Oscillate {
        field: usize,
        rate: (Vec3, Vec3),
        frequency: (Vec3, Vec3),
        proportional: bool,
        start: (f32, f32),
        end: (f32, f32),
        start_end_proportional: bool,
        multiplier: f32,
        phase: f32,
        vector: bool,
    },
    OscillateSimple {
        field: usize,
        rate: f32,
        frequency: f32,
        multiplier: f32,
        phase: f32,
    },
    Lock {
        cp: i32,
        fade_start: (f32, f32, f32),
        fade_end: (f32, f32, f32),
        distance_fade: f32,
    },
    Ramp {
        field: usize,
        rate: (f32, f32),
        start: (f32, f32),
        end: (f32, f32),
        proportional: bool,
        spline: Option<(f32, bool)>,
    },
    Clamp {
        field: usize,
        min: f32,
        max: f32,
    },
    MinimumDecay {
        field: usize,
        minimum: f32,
    },
    RemapDistanceToCp {
        cp: i32,
        distance: (f32, f32),
        output: usize,
        out_range: (f32, f32),
        scale_initial: bool,
        scale_current: bool,
        within_only: bool,
    },
    RemapScalar {
        input: usize,
        in_range: (f32, f32),
        output: usize,
        out_range: (f32, f32),
    },
    RemapSpeed {
        in_range: (f32, f32),
        output: usize,
        out_range: (f32, f32),
        scale_initial: bool,
        scale_current: bool,
    },
    RemapCpToScalar {
        cp: i32,
        component: usize,
        in_range: (f32, f32),
        output: usize,
        out_range: (f32, f32),
        scale_initial: bool,
        scale_current: bool,
    },
    RemapCpToVector {
        cp: i32,
        in_range: (Vec3, Vec3),
        output: usize,
        out_range: (Vec3, Vec3),
        scale_initial: bool,
        /// The output range is turned into this control point's space.
        local_cp: i32,
    },
    RemapDirectionToCp {
        cp: i32,
        output: usize,
        scale: f32,
        normalize: bool,
    },
    RemapVelocityToVector {
        output: usize,
        scale: f32,
        normalize: bool,
    },
    PercentageBetweenCps {
        range: (f32, f32),
        output: usize,
        out_range: (f32, f32),
        start_cp: i32,
        end_cp: i32,
        scale_initial: bool,
        scale_current: bool,
        within_only: bool,
        radius: bool,
    },
    Dampen {
        cp: i32,
        range: f32,
        scale: f32,
    },
    MaxVelocity {
        max: f32,
    },
    SetCpPositions {
        world: bool,
        offset_cp: i32,
        points: [(i32, i32, Vec3); 4],
    },
    SetCpRotation {
        axis: Vec3,
        rate: f32,
        cp: i32,
        local_cp: i32,
    },
    MaintainPath {
        max_distance: f32,
        bulge: f32,
        start_cp: i32,
        end_cp: i32,
        mid: f32,
        steps: f32,
        looping: bool,
        cohesion: f32,
        use_count: bool,
    },
    CullPlane {
        cp: i32,
        offset: f32,
        normal: Vec3,
    },
    CullSphere {
        cp: i32,
        distance: f32,
        offset: Vec3,
        inside: bool,
    },
    CullRandom {
        time: (f32, f32),
        exponent: f32,
        percentage: f32,
    },
    LerpInitial {
        time: (f32, f32),
        field: usize,
        value: f32,
    },
    SetChildCps {
        group: i32,
        first_cp: i32,
        count: i32,
        first_particle: i32,
        orient: bool,
    },
    SetCpsFromParticles {
        first_cp: i32,
        count: i32,
        first_particle: i32,
    },
    ImpactPoint {
        set_cp: i32,
        from_cp: i32,
        direction: Vec3,
        rate: f32,
        length: f32,
        offset: f32,
    },
    RemapCpSpeedToCp {
        input: i32,
        in_range: (f32, f32),
        output: i32,
        component: usize,
        out_range: (f32, f32),
    },
    RestartAfter {
        time: (f32, f32),
        children_only: bool,
    },
    NormalLock {
        cp: i32,
    },
    Unsupported(String),
}

impl OpKind {
    /// Operators the game runs before emission (control point setters).
    pub fn before_emission(&self) -> bool {
        matches!(
            self,
            Self::SetCpPositions { .. }
                | Self::SetCpRotation { .. }
                | Self::ImpactPoint { .. }
                | Self::RemapCpSpeedToCp { .. }
                | Self::RestartAfter { .. }
        )
    }
}

#[derive(Clone, Debug)]
pub enum ForceKind {
    Random { min: Vec3, max: Vec3 },
    Attract { cp: i32, amount: f32, falloff: f32 },
    Twist { amount: f32, axis: Vec3, local: bool },
    PlaneDistance { cp: i32, min: f32, max: f32, force_min: Vec3, force_max: Vec3, normal: Vec3, exponent: f32 },
    Turbulence { scales: [f32; 4], amounts: [Vec3; 4] },
    Unsupported(String),
}

#[derive(Clone, Debug)]
pub enum ConstraintKind {
    Collide { bounce: f32, slide: f32, radius_scale: f32, kill: bool, min_kill_speed: f32 },
    Distance { cp: i32, min: f32, max: f32, offset: Vec3, global: bool },
    Path { min: f32, max: f32, max_mid: f32, max_end: f32, travel_time: f32, start_cp: i32, end_cp: i32, mid: f32 },
    Plane { cp: i32, point: Vec3, normal: Vec3, global_origin: bool, global_normal: bool },
    Unsupported(String),
}

#[derive(Clone, Debug)]
pub struct Visibility {
    pub cp: i32,
    pub radius: f32,
    pub input: (f32, f32),
    pub alpha: (f32, f32),
    pub radius_scale: (f32, f32),
}

#[derive(Clone, Debug)]
pub enum RendererKind {
    Sprites {
        rate: f32,
        fit_lifetime: bool,
        orientation: i32,
        orientation_cp: i32,
        second_rate: f32,
        fps: bool,
    },
    Trail {
        rate: f32,
        length_fade_in: f32,
        max_length: f32,
        min_length: f32,
        constrain_radius: bool,
        ignore_dt: bool,
        tail_scale: [f32; 4],
    },
    Rope {
        subdivisions: i32,
        texel_size: f32,
        scroll_rate: f32,
        offset: f32,
    },
    Blobs,
    Unsupported(String),
}

#[derive(Clone, Debug)]
pub struct Op<K> {
    pub common: Common,
    pub kind: K,
}

#[derive(Clone, Debug)]
pub struct Renderer {
    pub common: Common,
    pub kind: RendererKind,
    pub visibility: Option<Visibility>,
}

#[derive(Clone, Debug)]
pub struct Child {
    pub def: usize,
    pub delay: f32,
    pub end_cap: bool,
}

/// One compiled particle system.
#[derive(Clone, Debug)]
pub struct Def {
    pub name: String,
    pub max_particles: usize,
    pub initial_particles: usize,
    /// Material path under `materials/`, normalised (`particle/smoke1/smoke1`).
    pub material: String,
    pub radius: f32,
    pub color: [f32; 4],
    pub rotation: f32,
    pub rotation_speed: f32,
    pub normal: Vec3,
    pub sequence: i32,
    pub sequence1: i32,
    pub group: i32,
    pub max_time_step: f32,
    pub min_time_step: f32,
    pub max_draw_distance: f32,
    pub sort: bool,
    pub view_model: bool,
    pub freeze_after: f32,
    pub emitters: Vec<Op<EmitterKind>>,
    pub initializers: Vec<Op<InitKind>>,
    pub operators: Vec<Op<OpKind>>,
    pub forces: Vec<Op<ForceKind>>,
    pub constraints: Vec<Op<ConstraintKind>>,
    pub renderers: Vec<Renderer>,
    pub children: Vec<Child>,
}

pub type DefRef = Arc<Def>;

fn deg(v: f32) -> f32 {
    v.to_radians()
}

fn emitter(op: &ParticleOperator) -> EmitterKind {
    let p = P(&op.attributes);
    match op.function_name.to_ascii_lowercase().as_str() {
        "emit_continuously" => EmitterKind::Continuous {
            start: p.f("emission_start_time", 0.0),
            rate: p.f("emission_rate", 100.0),
            duration: p.f("emission_duration", 0.0),
            scale_cp: p.cp("emission count scale control point", -1),
            scale_cp_field: p.i("emission count scale control point field", 0).clamp(0, 2) as usize,
            per_used_cp: p.f("scale emission to used control points", 0.0),
        },
        "emit_instantaneously" => {
            let count = p.i("num_to_emit", 100);
            let mut count_min = p.i("num_to_emit_minimum", -1);
            let mut count = count;
            if count_min >= 0 && count_min > count {
                std::mem::swap(&mut count, &mut count_min);
            }
            EmitterKind::Instant {
                start: p.f("emission_start_time", 0.0),
                start_max: p.f("emission_start_time max", -1.0),
                count,
                count_min,
                per_step: p.i("maximum emission per frame", -1),
                scale_cp: p.cp("emission count scale control point", -1),
                scale_cp_field: p.i("emission count scale control point field", 0).clamp(0, 2) as usize,
            }
        }
        "emit noise" => EmitterKind::Noise {
            start: p.f("emission_start_time", 0.0),
            duration: p.f("emission_duration", 0.0),
            time_scale: p.f("time noise coordinate scale", 0.1),
            time_offset: p.f("time coordinate offset", 0.0),
            absolute: p.b("absolute value", false),
            invert: p.b("invert absolute value", false),
            min: p.f("emission minimum", 0.0),
            max: p.f("emission maximum", 100.0),
        },
        "emit to maintain count" => EmitterKind::Maintain { count: p.i("count to maintain", 100), start: p.f("emission start time", 0.0) },
        // Unknown emitters emit nothing.
        _ => EmitterKind::Instant { start: 0.0, start_max: -1.0, count: 0, count_min: -1, per_step: -1, scale_cp: -1, scale_cp_field: 0 },
    }
}

fn initializer(op: &ParticleOperator) -> InitKind {
    use field::*;
    let p = P(&op.attributes);
    let name = op.function_name.to_ascii_lowercase();
    match name.as_str() {
        "lifetime random" => InitKind::RandomScalar {
            field: LIFE,
            min: p.f("lifetime_min", 0.0),
            max: p.f("lifetime_max", 0.0),
            exponent: p.f("lifetime_random_exponent", 1.0),
        },
        "radius random" => InitKind::RandomScalar {
            field: RADIUS,
            min: p.f("radius_min", 1.0),
            max: p.f("radius_max", 1.0),
            exponent: p.f("radius_random_exponent", 1.0),
        },
        "alpha random" => InitKind::RandomScalar {
            field: ALPHA,
            min: p.f("alpha_min", 255.0) / 255.0,
            max: p.f("alpha_max", 255.0) / 255.0,
            exponent: p.f("alpha_random_exponent", 1.0),
        },
        "trail length random" => InitKind::RandomScalar {
            field: TRAIL_LENGTH,
            min: p.f("length_min", 0.1),
            max: p.f("length_max", 0.1),
            exponent: p.f("length_random_exponent", 1.0),
        },
        "rotation random" => InitKind::RandomRotation {
            field: ROLL,
            initial: deg(p.f("rotation_initial", 0.0)),
            min: deg(p.f("rotation_offset_min", 0.0)),
            max: deg(p.f("rotation_offset_max", 360.0)),
            exponent: p.f("rotation_random_exponent", 1.0),
            flip: p.b("randomly_flip_direction", true),
        },
        "rotation yaw random" => InitKind::RandomRotation {
            field: YAW,
            initial: deg(p.f("yaw_initial", 0.0)),
            min: deg(p.f("yaw_offset_min", 0.0)),
            max: deg(p.f("yaw_offset_max", 360.0)),
            exponent: p.f("yaw_random_exponent", 1.0),
            flip: false,
        },
        "rotation speed random" => InitKind::RandomRotation {
            field: ROLL_SPEED,
            initial: deg(p.f("rotation_speed_constant", 0.0)),
            min: deg(p.f("rotation_speed_random_min", 0.0)),
            max: deg(p.f("rotation_speed_random_max", 360.0)),
            exponent: p.f("rotation_speed_random_exponent", 1.0),
            flip: p.b("randomly_flip_direction", true),
        },
        "rotation yaw flip random" => InitKind::YawFlip { chance: p.f("Flip Percentage", 0.5) },
        "color random" => InitKind::RandomColor {
            color1: p.c("color1", [255; 4]),
            color2: p.c("color2", [255; 4]),
            tint: p.f("tint_perc", 0.0),
            tint_cp: p.cp("tint control point", 0),
            clamp_min: p.c("tint clamp min", [0; 4]),
            clamp_max: p.c("tint clamp max", [255; 4]),
            blend: p.i("tint blend mode", 0),
            amplify: p.f("light amplification amount", 1.0),
            field: p.field("output field", COLOR),
        },
        "sequence random" => InitKind::RandomSequence {
            field: SEQUENCE,
            min: p.i("sequence_min", 0),
            max: p.i("sequence_max", 0),
            shuffle: p.b("shuffle", false),
            linear: p.b("linear", false),
        },
        "sequence two random" => InitKind::RandomSequence {
            field: SEQUENCE1,
            min: p.i("sequence_min", 0),
            max: p.i("sequence_max", 0),
            shuffle: false,
            linear: false,
        },
        "position within sphere random" => InitKind::Sphere {
            distance: (p.f("distance_min", 0.0), p.f("distance_max", 0.0)),
            bias: p.v("distance_bias", Vec3::ONE),
            bias_abs: p.v("distance_bias_absolute_value", Vec3::ZERO),
            local_bias: p.b("bias in local system", false),
            cp: p.cp("control_point_number", 0),
            speed: (p.f("speed_min", 0.0), p.f("speed_max", 0.0), p.f("speed_random_exponent", 1.0)),
            local_speed: (p.v("speed_in_local_coordinate_system_min", Vec3::ZERO), p.v("speed_in_local_coordinate_system_max", Vec3::ZERO)),
            distribute_cps: p.b("randomly distribute to highest supplied Control Point", false),
            growth_time: p.f("randomly distribution growth time", 0.0),
            scale_cp: p.cp("scale cp (distance/speed/local speed)", -1),
        },
        "position within box random" => InitKind::Box {
            min: p.v("min", Vec3::ZERO),
            max: p.v("max", Vec3::ZERO),
            cp: p.cp("control point number", 0),
            local: p.b("use local space", false),
        },
        "position modify offset random" => InitKind::Offset {
            min: p.v("offset min", Vec3::ZERO),
            max: p.v("offset max", Vec3::ZERO),
            cp: p.cp("control_point_number", 0),
            local: p.b("offset in local space 0/1", false),
            proportional: p.b("offset proportional to radius 0/1", false),
        },
        "position modify warp random" => InitKind::Warp {
            min: p.v("warp min", Vec3::ONE),
            max: p.v("warp max", Vec3::ONE),
            cp: p.cp("control point number", 0),
            transition: p.f("warp transition time (treats min/max as start/end sizes)", 0.0),
            start: p.f("warp transition start time", 0.0),
            reverse: p.b("reverse warp (0/1)", false),
        },
        "velocity noise" => InitKind::VelocityNoise {
            cp: p.cp("Control Point Number", 0),
            time_scale: p.f("Time Noise Coordinate Scale", 1.0),
            spatial_scale: p.f("Spatial Noise Coordinate Scale", 0.01),
            time_offset: p.f("Time Coordinate Offset", 0.0),
            spatial_offset: p.v("Spatial Coordinate Offset", Vec3::ZERO),
            absolute: p.v("Absolute Value", Vec3::ZERO),
            invert: p.v("Invert Abs Value", Vec3::ZERO),
            min: p.v("output minimum", Vec3::ZERO),
            max: p.v("output maximum", Vec3::ONE),
            local: p.b("Apply Velocity in Local Space (0/1)", false),
        },
        "velocity random" => InitKind::VelocityRandom {
            cp: p.cp("control_point_number", 0),
            speed: (p.f("random_speed_min", 0.0), p.f("random_speed_max", 0.0)),
            local: (p.v("speed_in_local_coordinate_system_min", Vec3::ZERO), p.v("speed_in_local_coordinate_system_max", Vec3::ZERO)),
        },
        "velocity inherit from control point" => {
            InitKind::VelocityInherit { cp: p.cp("control point number", 0), scale: p.f("velocity scale", 1.0) }
        }
        "velocity set from control point" => {
            InitKind::VelocityFromCp { cp: p.cp("control point number", 0), scale: p.f("velocity scale", 1.0) }
        }
        "position along ring" => InitKind::Ring {
            cp: p.cp("control point number", 0),
            radius: p.f("initial radius", 0.0),
            thickness: p.f("thickness", 0.0),
            speed: (p.f("min initial speed", 0.0), p.f("max initial speed", 0.0)),
            angles: Vec3::new(p.f("pitch", 0.0), p.f("yaw", 0.0), p.f("roll", 0.0)),
            even: p.b("even distribution", false),
            even_count: p.f("even distribution count", -1.0),
            xy_velocity: p.b("XY velocity only", true),
        },
        "position along path sequential" | "position along path random" => InitKind::AlongPath {
            sequential: name.ends_with("sequential"),
            max_distance: p.f("maximum distance", 0.0),
            bulge: p.f("bulge", 0.0),
            start_cp: p.cp("start control point number", 0),
            end_cp: p.cp("end control point number", 0),
            mid: p.f("mid point position", 0.5),
            steps: p.f("particles to map from start to end", 100.0),
            looping: p.b("restart behavior (0 = bounce, 1 = loop )", true),
        },
        "position modify place on ground" => InitKind::PlaceOnGround {
            offset: p.f("offset", 0.0),
            max_length: p.f("max trace length", 128.0),
            kill: p.b("kill on no collision", false),
        },
        "lifetime from sequence" => InitKind::LifetimeFromSequence { fps: p.f("Frames Per Second", 30.0) },
        "remap initial scalar" => InitKind::RemapScalar {
            input: p.field("input field", CREATED),
            in_range: (p.f("input minimum", 0.0), p.f("input maximum", 1.0)),
            output: p.field("output field", RADIUS),
            out_range: (p.f("output minimum", 0.0), p.f("output maximum", 1.0)),
            scale_initial: p.b("output is scalar of initial random range", false),
            clamp_to_range: p.b("only active within specified input range", false),
            emitter_window: (p.f("emitter lifetime start time (seconds)", -1.0), p.f("emitter lifetime end time (seconds)", -1.0)),
        },
        "remap noise to scalar" => InitKind::RemapNoise {
            field: p.field("output field", RADIUS),
            time_scale: p.f("time noise coordinate scale", 0.1),
            spatial_scale: p.f("spatial noise coordinate scale", 0.001),
            time_offset: p.f("time coordinate offset", 0.0),
            spatial_offset: p.v("spatial coordinate offset", Vec3::ZERO),
            absolute: p.b("absolute value", false),
            invert: p.b("invert absolute value", false),
            out_range: (p.f("output minimum", 0.0), p.f("output maximum", 1.0)),
            world_time_scale: p.f("world time noise coordinate scale", 0.0),
        },
        "remap initial distance to control point to scalar" => InitKind::RemapDistanceToCp {
            cp: p.cp("control point", 0),
            distance: (p.f("distance minimum", 0.0), p.f("distance maximum", 128.0)),
            output: p.field("output field", RADIUS),
            out_range: (p.f("output minimum", 0.0), p.f("output maximum", 1.0)),
            scale_initial: p.b("output is scalar of initial random range", false),
            within_only: p.b("only active within specified distance", false),
        },
        "remap particle count to scalar" => InitKind::RemapCount {
            in_range: (p.i("input minimum", 0), p.i("input maximum", 10)),
            output: p.field("output field", RADIUS),
            out_range: (p.f("output minimum", 0.0), p.f("output maximum", 1.0)),
            scale_initial: p.b("output is scalar of initial random range", false),
            within_only: p.b("only active within specified input range", false),
        },
        "remap control point to scalar" => InitKind::RemapCpToScalar {
            cp: p.cp("input control point number", 0),
            component: p.i("input field 0-2 X/Y/Z", 0).clamp(0, 2) as usize,
            in_range: (p.f("input minimum", 0.0), p.f("input maximum", 1.0)),
            output: p.field("output field", RADIUS),
            out_range: (p.f("output minimum", 0.0), p.f("output maximum", 1.0)),
            scale_initial: p.b("output is scalar of initial random range", false),
        },
        "remap control point to vector" => InitKind::RemapCpToVector {
            cp: p.cp("input control point number", 0),
            in_range: (p.v("input minimum", Vec3::ZERO), p.v("input maximum", Vec3::ZERO)),
            output: p.field("output field", POSITION),
            out_range: (p.v("output minimum", Vec3::ZERO), p.v("output maximum", Vec3::ZERO)),
            scale_initial: p.b("output is scalar of initial random range", false),
            offset: p.b("offset position", false),
        },
        "lifetime pre-age noise" => InitKind::PreAgeNoise {
            time_scale: p.f("time noise coordinate scale", 1.0),
            spatial_scale: p.f("spatial noise coordinate scale", 1.0),
            time_offset: p.f("time coordinate offset", 0.0),
            spatial_offset: p.v("spatial coordinate offset", Vec3::ZERO),
            absolute: p.b("absolute value", false),
            invert: p.b("invert absolute value", false),
            age: (p.f("start age minimum", 0.0), p.f("start age maximum", 1.0)),
        },
        "normal align to cp" => InitKind::NormalAlign { cp: p.cp("control_point_number", 0) },
        "normal modify offset random" => InitKind::NormalOffset {
            cp: p.cp("control_point_number", 0),
            min: p.v("offset min", Vec3::ZERO),
            max: p.v("offset max", Vec3::ZERO),
            local: p.b("offset in local space 0/1", false),
            normalize: p.b("normalize output 0/1", false),
        },
        "move particles between 2 control points" => InitKind::BetweenPoints {
            speed: (p.f("minimum speed", 1.0), p.f("maximum speed", 1.0)),
            end_spread: p.f("end spread", 0.0),
            start_offset: p.f("start offset", 0.0),
            end_offset: p.f("end offset", 0.0),
            end_cp: p.cp("end control point", 1),
        },
        "lifetime from time to impact" => InitKind::TimeToImpact {
            max_length: p.f("maximum trace length", 1024.0),
            offset: p.f("trace offset", 0.0),
            bias: p.v("bias distance", Vec3::ONE),
        },
        "remap scalar to vector" => InitKind::RemapScalarToVector {
            input: p.field("input field", CREATED),
            in_range: (p.f("input minimum", 0.0), p.f("input maximum", 1.0)),
            output: p.field("output field", POSITION),
            out_range: (p.v("output minimum", Vec3::ZERO), p.v("output maximum", Vec3::ONE)),
            scale_initial: p.b("output is scalar of initial random range", false),
            local: p.b("use local system", true),
            cp: p.cp("control_point_number", 0),
            emitter_window: (p.f("emitter lifetime start time (seconds)", -1.0), p.f("emitter lifetime end time (seconds)", -1.0)),
        },
        "color lit per particle" => InitKind::LitColor {
            color1: p.c("color1", [255; 4]),
            color2: p.c("color2", [255; 4]),
            clamp_min: p.c("tint clamp min", [0; 4]),
            clamp_max: p.c("tint clamp max", [255; 4]),
            blend: p.i("tint blend mode", 0),
            amplify: p.f("light amplification amount", 1.0),
        },
        "position from parent particles" => InitKind::FromParent {
            velocity_scale: p.f("Inherited Velocity Scale", 0.0),
            random: p.b("Random Parent Particle Distribution", false),
        },
        _ => InitKind::Unsupported(op.function_name.clone()),
    }
}

fn operator(op: &ParticleOperator) -> OpKind {
    use field::*;
    let p = P(&op.attributes);
    let name = op.function_name.to_ascii_lowercase();
    match name.as_str() {
        "movement basic" => {
            OpKind::Movement { gravity: p.v("gravity", Vec3::ZERO), drag: p.f("drag", 0.0), passes: p.i("max constraint passes", 3) }
        }
        "lifespan decay" => OpKind::Decay,
        "alpha fade and decay" => OpKind::FadeAndKill {
            alpha: (p.f("start_alpha", 1.0), p.f("end_alpha", 0.0)),
            fade_in: (p.f("start_fade_in_time", 0.0), p.f("end_fade_in_time", 0.5)),
            fade_out: (p.f("start_fade_out_time", 0.5), p.f("end_fade_out_time", 1.0)),
        },
        "alpha fade in random" => OpKind::FadeIn {
            time: (p.f("fade in time min", 0.25), p.f("fade in time max", 0.25)),
            exponent: p.f("fade in time exponent", 1.0),
            proportional: p.b("proportional 0/1", true),
        },
        "alpha fade out random" => OpKind::FadeOut {
            time: (p.f("fade out time min", 0.25), p.f("fade out time max", 0.25)),
            exponent: p.f("fade out time exponent", 1.0),
            proportional: p.b("proportional 0/1", true),
            ease: p.b("ease in and out", true),
            bias: p.f("fade bias", 0.5),
        },
        "alpha fade in simple" => OpKind::FadeInSimple { time: p.f("proportional fade in time", 0.25) },
        "alpha fade out simple" => OpKind::FadeOutSimple { time: p.f("proportional fade out time", 0.25) },
        "radius scale" => OpKind::RadiusScale {
            time: (p.f("start_time", 0.0), p.f("end_time", 1.0)),
            scale: (p.f("radius_start_scale", 1.0), p.f("radius_end_scale", 1.0)),
            ease: p.b("ease_in_and_out", false),
            bias: p.f("scale_bias", 0.5),
        },
        "color fade" => OpKind::ColorFade {
            color: p.c("color_fade", [255; 4]),
            time: (p.f("fade_start_time", 0.0), p.f("fade_end_time", 1.0)),
            ease: p.b("ease_in_and_out", true),
            field: p.field("output field", COLOR),
        },
        "rotation spin roll" => OpKind::Spin {
            field: ROLL,
            rate: deg(p.f("spin_rate_degrees", 0.0)),
            min_rate: deg(p.f("spin_rate_min", 0.0)),
            stop_time: p.f("spin_stop_time", 0.0),
        },
        "rotation spin yaw" => OpKind::Spin {
            field: YAW,
            rate: deg(p.f("yaw_rate_degrees", 0.0)),
            min_rate: deg(p.f("yaw_rate_min", 0.0)),
            stop_time: p.f("yaw_stop_time", 0.0),
        },
        "rotation basic" => OpKind::SpinUpdate,
        "oscillate scalar" => OpKind::Oscillate {
            field: p.field("oscillation field", ALPHA),
            rate: (Vec3::splat(p.f("oscillation rate min", 0.0)), Vec3::splat(p.f("oscillation rate max", 0.0))),
            frequency: (Vec3::splat(p.f("oscillation frequency min", 1.0)), Vec3::splat(p.f("oscillation frequency max", 1.0))),
            proportional: p.b("proportional 0/1", true),
            start: (p.f("start time min", 0.0), p.f("start time max", 0.0)),
            end: (p.f("end time min", 1.0), p.f("end time max", 1.0)),
            start_end_proportional: p.b("start/end proportional", true),
            multiplier: p.f("oscillation multiplier", 2.0),
            phase: p.f("oscillation start phase", 0.5),
            vector: false,
        },
        "oscillate vector" => OpKind::Oscillate {
            field: p.field("oscillation field", POSITION),
            rate: (p.v("oscillation rate min", Vec3::ZERO), p.v("oscillation rate max", Vec3::ZERO)),
            frequency: (p.v("oscillation frequency min", Vec3::ONE), p.v("oscillation frequency max", Vec3::ONE)),
            proportional: p.b("proportional 0/1", true),
            start: (p.f("start time min", 0.0), p.f("start time max", 0.0)),
            end: (p.f("end time min", 1.0), p.f("end time max", 1.0)),
            start_end_proportional: p.b("start/end proportional", true),
            multiplier: p.f("oscillation multiplier", 2.0),
            phase: p.f("oscillation start phase", 0.5),
            vector: true,
        },
        "oscillate scalar simple" => OpKind::OscillateSimple {
            field: p.field("oscillation field", ALPHA),
            rate: p.f("oscillation rate", 0.0),
            frequency: p.f("oscillation frequency", 1.0),
            multiplier: p.f("oscillation multiplier", 2.0),
            phase: p.f("oscillation start phase", 0.5),
        },
        "movement lock to control point" => OpKind::Lock {
            cp: p.cp("control_point_number", 0),
            fade_start: (p.f("start_fadeout_min", 1.0), p.f("start_fadeout_max", 1.0), p.f("start_fadeout_exponent", 1.0)),
            fade_end: (p.f("end_fadeout_min", 1.0), p.f("end_fadeout_max", 1.0), p.f("end_fadeout_exponent", 1.0)),
            distance_fade: p.f("distance fade range", 0.0),
        },
        "ramp scalar linear simple" => {
            let rate = p.f("ramp rate", 0.0);
            let (start, end) = (p.f("start time", 0.0), p.f("end time", 1.0));
            OpKind::Ramp {
                field: p.field("ramp field", RADIUS),
                rate: (rate, rate),
                start: (start, start),
                end: (end, end),
                proportional: true,
                spline: None,
            }
        }
        "ramp scalar spline simple" => {
            let rate = p.f("ramp rate", 0.0);
            let (start, end) = (p.f("start time", 0.0), p.f("end time", 1.0));
            OpKind::Ramp {
                field: p.field("ramp field", RADIUS),
                rate: (rate, rate),
                start: (start, start),
                end: (end, end),
                proportional: true,
                spline: Some((0.5, p.b("ease out", false))),
            }
        }
        "ramp scalar linear random" | "ramp scalar spline random" => OpKind::Ramp {
            field: p.field("ramp field", RADIUS),
            rate: (p.f("ramp rate min", 0.0), p.f("ramp rate max", 0.0)),
            start: (p.f("start time min", 0.0), p.f("start time max", 0.0)),
            end: (p.f("end time min", 1.0), p.f("end time max", 1.0)),
            proportional: p.b("start/end proportional", true),
            spline: name.contains("spline").then(|| (p.f("bias", 0.5), p.b("ease out", false))),
        },
        "clamp scalar" => {
            OpKind::Clamp { field: p.field("output field", RADIUS), min: p.f("output minimum", 0.0), max: p.f("output maximum", 1.0) }
        }
        "lifespan minimum radius decay" => OpKind::MinimumDecay { field: RADIUS, minimum: p.f("minimum radius", 1.0) },
        "lifespan minimum alpha decay" => OpKind::MinimumDecay { field: ALPHA, minimum: p.f("minimum alpha", 0.0) },
        "lifespan minimum velocity decay" => OpKind::MinimumDecay { field: PREV_POSITION, minimum: p.f("minimum velocity", 1.0) },
        "remap distance to control point to scalar" => OpKind::RemapDistanceToCp {
            cp: p.cp("control point", 0),
            distance: (p.f("distance minimum", 0.0), p.f("distance maximum", 128.0)),
            output: p.field("output field", RADIUS),
            out_range: (p.f("output minimum", 0.0), p.f("output maximum", 1.0)),
            scale_initial: p.b("output is scalar of initial random range", false),
            scale_current: p.b("output is scalar of current value", false),
            within_only: p.b("only active within specified distance", false),
        },
        "remap scalar" => OpKind::RemapScalar {
            input: p.field("input field", ALPHA),
            in_range: (p.f("input minimum", 0.0), p.f("input maximum", 1.0)),
            output: p.field("output field", RADIUS),
            out_range: (p.f("output minimum", 0.0), p.f("output maximum", 1.0)),
        },
        "remap speed to scalar" => OpKind::RemapSpeed {
            in_range: (p.f("input minimum", 0.0), p.f("input maximum", 1.0)),
            output: p.field("output field", RADIUS),
            out_range: (p.f("output minimum", 0.0), p.f("output maximum", 1.0)),
            scale_initial: p.b("output is scalar of initial random range", false),
            scale_current: p.b("output is scalar of current value", false),
        },
        "remap control point to scalar" => OpKind::RemapCpToScalar {
            cp: p.cp("input control point number", 0),
            component: p.i("input field 0-2 X/Y/Z", 0).clamp(0, 2) as usize,
            in_range: (p.f("input minimum", 0.0), p.f("input maximum", 1.0)),
            output: p.field("output field", RADIUS),
            out_range: (p.f("output minimum", 0.0), p.f("output maximum", 1.0)),
            scale_initial: p.b("output is scalar of initial random range", false),
            scale_current: p.b("output is scalar of current value", false),
        },
        // Portal shots and openings take the portal colour from control
        // point 2 this way.
        "remap control point to vector" => OpKind::RemapCpToVector {
            cp: p.cp("input control point number", 0),
            in_range: (p.v("input minimum", Vec3::ZERO), p.v("input maximum", Vec3::ZERO)),
            output: p.field("output field", POSITION),
            out_range: (p.v("output minimum", Vec3::ZERO), p.v("output maximum", Vec3::ZERO)),
            scale_initial: p.b("output is scalar of initial random range", false),
            local_cp: p.i("local space CP", -1),
        },
        "remap direction to cp to vector" => OpKind::RemapDirectionToCp {
            cp: p.cp("control point", 0),
            output: p.field("output field", POSITION),
            scale: p.f("scale factor", 1.0),
            normalize: p.b("normalize", false),
        },
        "remap velocity to vector" => OpKind::RemapVelocityToVector {
            output: p.field("output field", POSITION),
            scale: p.f("scale factor", 1.0),
            normalize: p.b("normalize", false),
        },
        "remap percentage between two control points to scalar" => OpKind::PercentageBetweenCps {
            range: (p.f("percentage minimum", 0.0), p.f("percentage maximum", 1.0)),
            output: p.field("output field", RADIUS),
            out_range: (p.f("output minimum", 0.0), p.f("output maximum", 1.0)),
            start_cp: p.cp("starting control point", 0),
            end_cp: p.cp("ending control point", 1),
            scale_initial: p.b("output is scalar of initial random range", false),
            scale_current: p.b("output is scalar of current value", false),
            within_only: p.b("only active within input range", false),
            radius: p.b("treat distance between points as radius", true),
        },
        "movement dampen relative to control point" => {
            OpKind::Dampen { cp: p.cp("control_point_number", 0), range: p.f("falloff range", 100.0), scale: p.f("dampen scale", 1.0) }
        }
        "movement max velocity" => OpKind::MaxVelocity { max: p.f("Maximum Velocity", 0.0) },
        "set control point positions" => OpKind::SetCpPositions {
            world: p.b("Set positions in world space", false),
            offset_cp: p.cp("Control Point to offset positions from", 0),
            points: [
                (
                    p.cp("First Control Point Number", 1),
                    p.cp("First Control Point Parent", 0),
                    p.v("First Control Point Location", Vec3::new(128.0, 0.0, 0.0)),
                ),
                (
                    p.cp("Second Control Point Number", 2),
                    p.cp("Second Control Point Parent", 0),
                    p.v("Second Control Point Location", Vec3::new(0.0, 128.0, 0.0)),
                ),
                (
                    p.cp("Third Control Point Number", 3),
                    p.cp("Third Control Point Parent", 0),
                    p.v("Third Control Point Location", Vec3::new(-128.0, 0.0, 0.0)),
                ),
                (
                    p.cp("Fourth Control Point Number", 4),
                    p.cp("Fourth Control Point Parent", 0),
                    p.v("Fourth Control Point Location", Vec3::new(0.0, -128.0, 0.0)),
                ),
            ],
        },
        "set control point rotation" => OpKind::SetCpRotation {
            axis: p.v("Rotation Axis", Vec3::Z),
            rate: deg(p.f("Rotation Rate", 180.0)),
            cp: p.cp("Control Point", 0),
            local_cp: p.cp("Local Space Control Point", -1),
        },
        "movement maintain position along path" => OpKind::MaintainPath {
            max_distance: p.f("maximum distance", 0.0),
            bulge: p.f("bulge", 0.0),
            start_cp: p.cp("start control point number", 0),
            end_cp: p.cp("end control point number", 0),
            mid: p.f("mid point position", 0.5),
            steps: p.f("particles to map from start to end", 100.0),
            looping: p.b("restart behavior (0 = bounce, 1 = loop )", true),
            cohesion: p.f("cohesion strength", 1.0),
            use_count: p.b("use existing particle count", false),
        },
        "cull when crossing plane" => OpKind::CullPlane {
            cp: p.cp("Control Point for point on plane", 0),
            offset: p.f("Cull plane offset", 0.0),
            normal: p.v("Plane Normal", Vec3::Z),
        },
        "cull when crossing sphere" => OpKind::CullSphere {
            cp: p.cp("Control Point", 0),
            distance: p.f("Cull Distance", 0.0),
            offset: p.v("Control Point offset", Vec3::ZERO),
            inside: p.b("Cull inside instead of outside", false),
        },
        "cull random" => OpKind::CullRandom {
            time: (p.f("Cull Start Time", 0.0), p.f("Cull End Time", 1.0)),
            exponent: p.f("Cull Time Exponent", 1.0),
            percentage: p.f("Cull Percentage", 0.5),
        },
        "lerp initial scalar" => OpKind::LerpInitial {
            time: (p.f("start time", 0.0), p.f("end time", 1.0)),
            field: p.field("output field", RADIUS),
            value: p.f("value to lerp to", 1.0),
        },
        "set child control points from particle positions" => OpKind::SetChildCps {
            group: p.i("Group ID to affect", 0),
            first_cp: p.cp("First control point to set", 0),
            count: p.i("# of control points to set", 1),
            first_particle: p.i("first particle to copy", 0),
            orient: p.b("set orientation", false),
        },
        "set control points from particle positions" => OpKind::SetCpsFromParticles {
            first_cp: p.cp("First control point to set", 0),
            count: p.i("# of control points to set", 1),
            first_particle: p.i("first particle to copy", 0),
        },
        "set control point to impact point" => OpKind::ImpactPoint {
            set_cp: p.cp("Control Point to Set", 1),
            from_cp: p.cp("Control Point to Trace From", 1),
            direction: p.v("Trace Direction Override", Vec3::ZERO),
            rate: p.f("Trace Update Rate", 0.5),
            length: p.f("Max Trace Length", 1024.0),
            offset: p.f("Offset End Point Amount", 0.0),
        },
        "remap cp speed to cp" => OpKind::RemapCpSpeedToCp {
            input: p.cp("input control point", 0),
            in_range: (p.f("input minimum", 0.0), p.f("input maximum", 1.0)),
            output: p.cp("output control point", -1),
            component: p.i("Output field 0-2 X/Y/Z", 0).clamp(0, 2) as usize,
            out_range: (p.f("output minimum", 0.0), p.f("output maximum", 1.0)),
        },
        "restart effect after duration" => OpKind::RestartAfter {
            time: (p.f("Minimum Restart Time", 0.0), p.f("Maximum Restart Time", 1.0)),
            children_only: p.b("Only Restart Children", false),
        },
        "normal lock to control point" => OpKind::NormalLock { cp: p.cp("control_point_number", 0) },
        _ => OpKind::Unsupported(op.function_name.clone()),
    }
}

fn force(op: &ParticleOperator) -> ForceKind {
    let p = P(&op.attributes);
    match op.function_name.to_ascii_lowercase().as_str() {
        "random force" => ForceKind::Random { min: p.v("min force", Vec3::ZERO), max: p.v("max force", Vec3::ZERO) },
        "pull towards control point" => ForceKind::Attract {
            cp: p.cp("control point number", 0),
            amount: p.f("amount of force", 0.0),
            falloff: p.f("falloff power", 2.0),
        },
        "twist around axis" => ForceKind::Twist {
            amount: p.f("amount of force", 0.0),
            axis: p.v("twist axis", Vec3::Z),
            local: p.b("object local space axis 0/1", false),
        },
        "force based on distance from plane" => ForceKind::PlaneDistance {
            cp: p.cp("Control point number", 0),
            min: p.f("Min distance from plane", 0.0),
            max: p.f("Max Distance from plane", 1.0),
            force_min: p.v("Force at Min distance", Vec3::ZERO),
            force_max: p.v("Force at Max distance", Vec3::ZERO),
            normal: p.v("Plane Normal", Vec3::Z),
            exponent: p.f("Exponent", 1.0),
        },
        "turbulent force" => ForceKind::Turbulence {
            scales: [p.f("Noise scale 0", 1.0), p.f("Noise scale 1", 0.0), p.f("Noise scale 2", 0.0), p.f("Noise scale 3", 0.0)],
            amounts: [
                p.v("Noise amount 0", Vec3::ONE),
                p.v("Noise amount 1", Vec3::splat(0.5)),
                p.v("Noise amount 2", Vec3::splat(0.25)),
                p.v("Noise amount 3", Vec3::splat(0.125)),
            ],
        },
        _ => ForceKind::Unsupported(op.function_name.clone()),
    }
}

fn constraint(op: &ParticleOperator) -> ConstraintKind {
    let p = P(&op.attributes);
    match op.function_name.to_ascii_lowercase().as_str() {
        "collision via traces" => ConstraintKind::Collide {
            bounce: p.f("amount of bounce", 0.0),
            slide: p.f("amount of slide", 0.0),
            radius_scale: p.f("radius scale", 1.0),
            kill: p.b("kill particle on collision", false),
            min_kill_speed: p.f("minimum speed to kill on collision", -1.0),
        },
        "constrain distance to control point" => ConstraintKind::Distance {
            cp: p.cp("control point number", 0),
            min: p.f("minimum distance", 0.0),
            max: p.f("maximum distance", 100.0),
            offset: p.v("offset of center", Vec3::ZERO),
            global: p.b("global center point", false),
        },
        "constrain distance to path between two control points" => ConstraintKind::Path {
            min: p.f("minimum distance", 0.0),
            max: p.f("maximum distance", 100.0),
            max_mid: p.f("maximum distance middle", -1.0),
            max_end: p.f("maximum distance end", -1.0),
            travel_time: p.f("travel time", 10.0),
            start_cp: p.cp("start control point number", 0),
            end_cp: p.cp("end control point number", 0),
            mid: p.f("mid point position", 0.5),
        },
        "prevent passing through a plane" => ConstraintKind::Plane {
            cp: p.cp("control point number", 0),
            point: p.v("plane point", Vec3::ZERO),
            normal: p.v("plane normal", Vec3::Z),
            global_origin: p.b("global origin", false),
            global_normal: p.b("global normal", false),
        },
        _ => ConstraintKind::Unsupported(op.function_name.clone()),
    }
}

fn renderer(op: &ParticleOperator) -> Renderer {
    let p = P(&op.attributes);
    let visibility = {
        let cp = p.cp("Visibility Proxy Input Control Point Number", -1);
        (cp >= 0).then(|| Visibility {
            cp,
            radius: p.f("Visibility Proxy Radius", 1.0),
            input: (p.f("Visibility input minimum", 0.0), p.f("Visibility input maximum", 1.0)),
            alpha: (p.f("Visibility Alpha Scale minimum", 0.0), p.f("Visibility Alpha Scale maximum", 1.0)),
            radius_scale: (p.f("Visibility Radius Scale minimum", 1.0), p.f("Visibility Radius Scale maximum", 1.0)),
        })
    };
    let kind = match op.function_name.to_ascii_lowercase().as_str() {
        "render_animated_sprites" => RendererKind::Sprites {
            rate: p.f("animation rate", 0.1),
            fit_lifetime: p.b("animation_fit_lifetime", false),
            orientation: p.i("orientation_type", 0).clamp(0, 3),
            orientation_cp: p.cp("orientation control point", -1),
            second_rate: p.f("second sequence animation rate", 0.0),
            fps: p.b("use animation rate as FPS", false),
        },
        "render_sprite_trail" => RendererKind::Trail {
            rate: p.f("animation rate", 0.1),
            length_fade_in: p.f("length fade in time", 0.0),
            max_length: p.f("max length", 2000.0),
            min_length: p.f("min length", 0.0),
            constrain_radius: p.b("constrain radius to length", true),
            ignore_dt: p.b("ignore delta time", false),
            tail_scale: match op.attributes.get("tail color and alpha scale factor") {
                Some(source_formats::dmx::Value::Vector4(v)) => v.to_array(),
                _ => [1.0; 4],
            },
        },
        "render_rope" => RendererKind::Rope {
            subdivisions: p.i("subdivision_count", 3).clamp(1, 16),
            texel_size: p.f("texel_size", 4.0).max(0.01),
            scroll_rate: p.f("texture_scroll_rate", 0.0),
            offset: p.f("texture_offset", 0.0),
        },
        "render_blobs" => RendererKind::Blobs,
        _ => RendererKind::Unsupported(op.function_name.clone()),
    };
    Renderer { common: Common::parse(&p), kind, visibility }
}

fn ops<K>(stack: &[ParticleOperator], f: impl Fn(&ParticleOperator) -> K) -> Vec<Op<K>> {
    stack.iter().map(|op| Op { common: Common::parse(&P(&op.attributes)), kind: f(op) }).collect()
}

/// `particle\smoke1\smoke1.vmt` → `particle/smoke1/smoke1`.
pub fn material_name(path: &str) -> String {
    let p = path.trim().replace('\\', "/").to_ascii_lowercase();
    let p = p.trim_start_matches('/').trim_start_matches("materials/");
    p.strip_suffix(".vmt").unwrap_or(p).to_owned()
}

impl Def {
    /// Compiles a definition; `child_index` maps the definition's child
    /// links (indices into its file) to library indices.
    pub fn compile(d: &ParticleSystemDefinition, child_index: impl Fn(usize) -> Option<usize>) -> Self {
        let c = d.color.map(|b| b as f32 / 255.0);
        Self {
            name: d.name.clone(),
            max_particles: d.max_particles.clamp(0, 5000) as usize,
            initial_particles: d.initial_particles.clamp(0, 5000) as usize,
            material: material_name(&d.material),
            radius: d.radius,
            color: c,
            rotation: d.rotation.to_radians(),
            rotation_speed: d.rotation_speed.to_radians(),
            normal: d.normal,
            sequence: d.sequence_number,
            sequence1: d.sequence_number_1,
            group: d.group_id,
            max_time_step: if d.max_time_step > 0.0 { d.max_time_step } else { 0.1 },
            min_time_step: d.min_simulation_time_step.max(0.0),
            max_draw_distance: d.max_draw_distance,
            sort: d.sort_particles,
            view_model: d.view_model_effect,
            freeze_after: d.freeze_simulation_after_time,
            emitters: ops(&d.emitters, emitter),
            initializers: ops(&d.initializers, initializer),
            operators: ops(&d.operators, operator),
            forces: ops(&d.forces, force),
            constraints: ops(&d.constraints, constraint),
            renderers: d.renderers.iter().map(renderer).collect(),
            children: d
                .children
                .iter()
                .filter_map(|c| Some(Child { def: child_index(c.index)?, delay: c.delay, end_cap: c.end_cap_effect }))
                .collect(),
        }
    }

    /// Names of operators this runtime skips, for diagnostics.
    pub fn unsupported(&self) -> Vec<String> {
        let mut out = Vec::new();
        out.extend(self.initializers.iter().filter_map(|o| match &o.kind {
            InitKind::Unsupported(n) => Some(n.clone()),
            _ => None,
        }));
        out.extend(self.operators.iter().filter_map(|o| match &o.kind {
            OpKind::Unsupported(n) => Some(n.clone()),
            _ => None,
        }));
        out.extend(self.forces.iter().filter_map(|o| match &o.kind {
            ForceKind::Unsupported(n) => Some(n.clone()),
            _ => None,
        }));
        out.extend(self.constraints.iter().filter_map(|o| match &o.kind {
            ConstraintKind::Unsupported(n) => Some(n.clone()),
            _ => None,
        }));
        out.extend(self.renderers.iter().filter_map(|o| match &o.kind {
            RendererKind::Unsupported(n) => Some(n.clone()),
            _ => None,
        }));
        out
    }
}
