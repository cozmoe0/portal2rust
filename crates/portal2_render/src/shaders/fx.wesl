// The visual overhaul's state shared by the world and model shaders (see
// portal2_render/src/fx.rs and the game's visuals/fx): which features are
// on, the overhaul's lights, and the parameters the materials read. Only
// the `FX` shader variants use any of it.

struct FxLight {
    // xyz: position (line lights: start), w: kind (0 point, 1 spot, 2 line, 3 ellipse)
    position: vec4<f32>,
    // rgb: colour times intensity (linear, lightmap scale), w: range
    color: vec4<f32>,
    // xyz: spot axis, ellipse normal or line end; w: spot cos outer / ellipse half width
    axis: vec4<f32>,
    // x: spot cos inner / ellipse half height, y: flags, z: shadow slot (-1 none), w: source radius
    params: vec4<f32>,
    // xyz: ellipse up, w: unused
    up: vec4<f32>,
}

struct FxGlobals {
    // x: feature bits, y: time, z: light count, w: emissive boost
    header: vec4<f32>,
    // x: wetness amount, y: specular scale, z: fixture specular scale, w: unused
    surface: vec4<f32>,
    // rgb: haze colour (linear), w: haze density per unit
    haze: vec4<f32>,
    // x: haze start distance, y: maximum opacity, z: height falloff, w: reference height
    haze2: vec4<f32>,
    // xyz: direction toward the sun, w: 1 when there is one
    sun_dir: vec4<f32>,
    // rgb: sun colour
    sun_color: vec4<f32>,
    reserved0: vec4<f32>,
    reserved1: vec4<f32>,
    shadow_from_world: array<mat4x4<f32>, 4>,
    sun_from_world: mat4x4<f32>,
    // x: shadow map size, y: sun shadow map size
    shadow_info: vec4<f32>,
    reserved2: array<vec4<f32>, 3>,
    lights: array<FxLight, 64>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(37) var<storage, read> fx: FxGlobals;

// Feature bits (visuals/fx/mod.rs `features`).
const FX_LIGHTS: u32 = 1u;
const FX_SHADOWS: u32 = 2u;
const FX_AO: u32 = 4u;
const FX_SSR: u32 = 8u;
const FX_WETNESS: u32 = 16u;
const FX_FOG: u32 = 32u;
const FX_VOLUMETRICS: u32 = 64u;
const FX_BLOOM: u32 = 128u;
const FX_GRADE: u32 = 256u;
const FX_EMISSIVE: u32 = 512u;
const FX_FOLIAGE: u32 = 1024u;
const FX_REFLECTIONS: u32 = 2048u;
const FX_PROCEDURAL: u32 = 4096u;

fn fx_on(feature: u32) -> bool {
    return (u32(fx.header.x) & feature) != 0u;
}

fn fx_time() -> f32 {
    return fx.header.y;
}

// Self-illumination multiplier: HDR signs and indicators for the bloom.
fn fx_emissive_boost() -> f32 {
    if fx_on(FX_EMISSIVE) {
        return fx.header.w;
    }
    return 1.0;
}
