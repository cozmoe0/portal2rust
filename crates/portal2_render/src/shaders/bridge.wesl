// Hard light bridges (`prop_wall_projector`), as `C_ProjectedWallEntity`
// draws them (client.dll `sub_103AADB0`): one quad on the bridge's top face
// with `effects/projected_wall`, U across the width (0 at the right edge),
// V running 1 per 128 units from 0 at the far end back to the projector;
// and two rails, quads 12 units tall standing on the side edges with
// `effects/projected_wall_rail`, U down their height.
//
// The surface is Portal 2's `SolidEnergy` shader, static combo 15 of
// `solidenergy_ps30` (additive, two details, detail 1 blend mode 1):
//     base * d1 + base.a * base * (1 - d1) + d2 * d1
// faded in over the first 40 units in front of the camera, with the
// material's proxies driving the three texture transforms. The rails are
// UnlitGeneric: the texture added at the flickering `$alpha`, unfogged.
//
// With the visual overhaul on, the same surface gains HDR edges for the
// bloom, a grazing-angle sheen, pulses of light running out from the
// projector along the strands, glows where the bridge leaves its emitter
// and meets the wall, a bright front while it projects, and ripples around
// whatever stands on it; fine strands of light weave through it, the fill
// between the bright parts stays a deep saturated blue whatever the
// exposure, and the rails get white-hot cores.

import bevy_pbr::render::forward_io::VertexOutput;
import bevy_pbr::render::mesh_view_bindings::view;
import bevy_pbr::render::mesh_view_bindings::fog;
import bevy_pbr::render::mesh_view_bindings::globals;

struct BridgeParams {
    // x: shown length of this piece, y: width, z: 0 surface / 1 rail,
    // w: 1 with the visual overhaul
    shape: vec4<f32>,
    // x: distance along the bridge from the projector to this piece's start,
    // y: brightness of the projection front at the far end (0..1),
    // z: 1 when the piece starts at a projector or portal (an emitter glow),
    // w: 1 when the far end meets a surface rather than a portal
    path: vec4<f32>,
    // Things touching the surface: x units along from the piece's start,
    // y units left of the centre line, z strength (0 for none), w unused.
    contacts: array<vec4<f32>, 4>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> bridge: BridgeParams;
// All four textures repeat and filter alike, so they share one sampler (the
// browser's WebGPU allows few samplers per shader).
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var base_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var samp: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(3) var detail1_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(4) var detail2_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(5) var rail_tex: texture_2d<f32>;

const TAU: f32 = 6.2831853;

// The `Sine` material proxy.
fn sine_proxy(t: f32, lo: f32, hi: f32, period: f32, offset: f32) -> f32 {
    return lo + (hi - lo) * (sin(TAU * (t - offset) / period) * 0.5 + 0.5);
}

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// The surface's brightness for the tone map scale. SolidEnergy multiplies
// its colour by the saturated light scale and also outputs that scale as
// alpha for SRC_ALPHA, ONE blending, so the scale counts twice; matched
// against the game's captures at forced scales from 0.25 to 2, its colour
// comes out 2.25 times what the textures give here.
fn surface_scale() -> f32 {
    let s = saturate(view.exposure);
    return 2.25 * s * s;
}

fn view_depth(p: vec3<f32>) -> f32 {
    return dot(p - view.world_position, -view.world_from_view[2].xyz);
}

// Additive surfaces fade to black in the fog.
fn fade_with_fog(color: vec3<f32>, depth: f32) -> vec3<f32> {
    if fog.mode != 1u {
        return color;
    }
    let f = saturate(min(fog.directional_light_exponent, (depth - fog.be.x) / max(fog.be.y - fog.be.x, 1.0)));
    return color * (1.0 - f);
}

fn rail(uv: vec2<f32>, t: f32) -> vec3<f32> {
    let alpha = sine_proxy(t, 0.8, 1.0, 0.1, 0.0);
    var c = textureSample(rail_tex, samp, vec2<f32>(uv.x, uv.y * bridge.shape.x / 128.0)).rgb * alpha;
    if bridge.shape.w > 0.5 {
        // Brighter than the display's white, so the rails bloom, with a
        // white-hot core inside the cyan.
        let l = luma(c);
        c = c * 1.8 + vec3<f32>(l * l * 1.1);
    }
    return c;
}

// A thin strand of light weaving across the bridge: `x` units along the
// bridge, `y` units from its centre line; the strand swings `amp` units
// each side with `freq` radians a unit, running out at `speed` units a
// second. A sharp core inside a soft halo, kept a pixel wide at a distance.
fn strand_glow(x: f32, y: f32, amp: f32, freq: f32, phase: f32, speed: f32, t: f32) -> f32 {
    let a = (x - t * speed) * freq + phase;
    let curve = amp * sin(a);
    let slope = amp * freq * cos(a);
    let d = abs(y - curve) / sqrt(1.0 + slope * slope);
    let core_w = max(0.45, fwidth(d) * 0.8);
    return exp(-d * d / (core_w * core_w)) * 0.8 + exp(-d / 3.5) * 0.1;
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> @location(0) vec4<f32> {
    // Effects stay out of projected-texture shadow maps.
    if fog.mode == 2u {
        discard;
    }
    var uv = vec2<f32>(0.5);
    @if(VERTEX_UVS_A) {
        uv = in.uv;
    }
    let t = globals.time;
    let len = max(bridge.shape.x, 0.01);
    let width = bridge.shape.y;
    let fx = bridge.shape.w > 0.5;
    let depth = view_depth(in.world_position.xyz);
    if bridge.shape.z > 0.5 {
        return vec4<f32>(rail(uv, t) * view.exposure, 0.0);
    }

    // Units from this piece's start, and the material's V (0 at the far end).
    let along = uv.y * len;
    let v = (len - along) / 128.0;
    let u = uv.x;

    // $basetexturetransform: scaled about the centre by a fast Sine, scrolled
    // by a LinearRamp. $detail1texturetransform: a slow Sine sweeps the bright
    // band of the 256x1 `projected_wall_draw` across the width.
    // $detail2texturetransform: the noise, scaled by a fast Sine and scrolled.
    let base_uv = vec2<f32>((u - 0.5) * sine_proxy(t, 0.995, 1.005, 0.1, 0.0) + 0.5, v + fract(t * 0.15));
    let d1_uv = vec2<f32>(u + sine_proxy(t, 0.0, 0.94, 5.0, 3.0), v - fract(t * 3.0));
    let d2_uv = vec2<f32>((u - 0.5) * sine_proxy(t, 0.99, 1.01, 0.125, 0.0) + 0.5, v + fract(t * 0.18));
    let base = textureSample(base_tex, samp, base_uv);
    let d1 = textureSample(detail1_tex, samp, d1_uv).rgb;
    let d2 = textureSample(detail2_tex, samp, d2_uv).rgb;
    var c = base.rgb * d1 + base.a * base.rgb * (1.0 - d1) + d2 * d1;

    if fx {
        let edge = base.a;
        // The helix strands: the brighter parts of the base away from the edges.
        let strand = smoothstep(0.18, 0.5, luma(base.rgb)) * (1.0 - edge);
        let tint = vec3<f32>(0.32, 0.78, 1.0);

        // A soft sheen at grazing angles, and edges bright enough to bloom.
        let n = normalize(in.world_normal);
        let to_eye = normalize(view.world_position - in.world_position.xyz);
        let grazing = pow(1.0 - abs(dot(n, to_eye)), 4.0);
        c = c * (1.0 + 0.25 * grazing) + base.rgb * edge * 1.4;

        // A second, finer layer of the noise drifting the other way, so the
        // light inside the bridge seems to flow.
        let drift = textureSample(detail2_tex, samp, vec2<f32>(u * 1.7 + 0.31, v * 2.3 - fract(t * 0.07))).rgb;
        c = c + drift * d1 * 0.35;

        // Depth: the cloudy fill between the strands sinks toward a deep,
        // saturated blue, so the bright parts stand out of it.
        let fill = 1.0 - saturate(edge + strand * 1.5);
        c = c * mix(vec3<f32>(1.0), vec3<f32>(0.4, 0.64, 1.08), fill);

        // Fine strands of light weaving through it between the big helix,
        // each with its own swing and pace, running out from the projector
        // (continuous across portals) and waxing and waning along their
        // length so they read as threads rather than stripes. Positions in
        // units, so they never stretch.
        let x = bridge.path.x + along;
        let y = (u - 0.5) * width;
        let half = width * 0.5 - 3.0;
        var fine = 0.0;
        fine += strand_glow(x, y, half * 0.82, 0.021, 0.0, 34.0, t) * (0.55 + 0.45 * sin(x * 0.006 - t * 0.7));
        fine += strand_glow(x, y, half * 0.55, 0.033, 2.1, 52.0, t) * (0.5 + 0.5 * sin(x * 0.009 + 1.3 - t * 0.9));
        fine += strand_glow(x, y, half * 0.95, 0.014, 4.2, 22.0, t) * (0.5 + 0.5 * sin(x * 0.004 + 2.6 - t * 0.5));
        fine += strand_glow(x, y, half * 0.35, 0.047, 1.0, 70.0, t) * 0.35 * (0.5 + 0.5 * sin(x * 0.011 + 0.4 - t * 1.1));
        c = c + vec3<f32>(0.42, 0.84, 1.0) * fine * 0.55 * (1.0 - edge);

        // The light gathers inward from the rails.
        let to_edge = min(u, 1.0 - u) * width;
        c = c + tint * (exp(-to_edge / 3.5) * 0.4 + exp(-to_edge / 12.0) * 0.05);

        // Pulses running out from the projector along the strands and edges,
        // continuous across portals.
        let path = bridge.path.x + along;
        let spacing = 640.0;
        let d = (fract((t * 420.0 - path) / spacing + 0.5) - 0.5) * spacing;
        // A soft band with a brighter leading edge.
        let pulse = exp(-d * d / (2.0 * 40.0 * 40.0)) + 0.6 * exp(-(d + 18.0) * (d + 18.0) / (2.0 * 8.0 * 8.0));
        c = c + pulse * (strand * tint * 2.2 + edge * base.rgb * 1.2 + tint * 0.12);

        // Glow where the bridge leaves its emitter and where it meets a wall.
        let to_end = len - along;
        let end_glow = exp(-to_end / 9.0) * bridge.path.w + exp(-along / 14.0) * bridge.path.z * 0.6;
        c = c + tint * end_glow * (0.6 + edge);

        // The projection front, bright while the bridge reaches out.
        c = c + tint * exp(-to_end / 30.0) * bridge.path.y * (1.5 + 2.0 * edge);

        // Keep the deep blue deep: between the bright strands and edges the
        // fill is saturated and pulled down, so the exposure of a dark room
        // does not wash it toward white.
        let calm = 1.0 - saturate(edge + strand + fine * 0.8 + pulse * 0.5);
        let level = luma(c);
        c = max(mix(vec3<f32>(level), c, 1.0 + 0.5 * calm), vec3<f32>(0.0)) * (1.0 - 0.32 * calm);

        // Ripples around the player's feet and props on the surface.
        let here = vec2<f32>(along, (u - 0.5) * width);
        for (var i = 0u; i < 4u; i = i + 1u) {
            let k = bridge.contacts[i];
            if k.z <= 0.0 {
                continue;
            }
            let r = distance(here, k.xy);
            let spot = exp(-r * r / (2.0 * 16.0 * 16.0));
            let ring = pow(max(sin(r * 0.16 - t * 7.0), 0.0), 8.0) * exp(-r / 56.0);
            c = c + tint * k.z * (spot * 0.45 + ring * (0.35 + strand));
        }

        // From below the surface reads a little dimmer.
        if !is_front {
            c = c * 0.8;
        }
    }

    // SolidEnergy fades the surface in over the first 40 units past the near
    // plane.
    c = c * smoothstep(0.0, 1.0, saturate((depth - 7.0) / 40.0));
    return vec4<f32>(fade_with_fog(c * surface_scale(), depth), 0.0);
}
