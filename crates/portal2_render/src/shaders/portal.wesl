// Portal opening and rim, after the game's PortalRefract stages (see
// docs/re/portals.md): the hole is the ellipse whose radius is the square of
// the smoothstepped open amount; inside it the linked view shows (rendered
// to a texture with the same projection, so it is sampled in screen space).
// Over it lies the rim: two scrolling, self-distorting noise lookups pick
// where the fire burns, and the result looks up the portal's colour
// gradient, times the colour scale. While the portal shows static the fire
// fills the whole hole.

import bevy_pbr::render::forward_io::VertexOutput;
import bevy_pbr::render::mesh_view_bindings::view;

struct PortalParams {
    // rgb: fallback rim colour (linear), a: colour scale
    color: vec4<f32>,
    // x: linked (0/1), y: time, z: open amount (0..1), w: clarity (1 - static)
    state: vec4<f32>,
    // x: rim brightness multiplier (the visual overhaul's HDR glow, else 1)
    glow: vec4<f32>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> portal: PortalParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var view_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var view_samp: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(3) var noise_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(4) var noise_samp: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(5) var gradient_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(6) var gradient_samp: sampler;

fn smooth01(x: f32) -> f32 {
    return x * x * (3.0 - 2.0 * x);
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    var uv = vec2<f32>(0.5);
    @if(VERTEX_UVS_A) {
        uv = in.uv;
    }
    let open = clamp(portal.state.z, 0.0, 1.0);
    let clarity = clamp(portal.state.w, 0.0, 1.0);
    let time = portal.state.y;

    // The rim's coordinates run a little past the quad.
    let t0 = uv * 1.075 - vec2<f32>(0.0375);
    // Two noise coordinate sets scrolling apart, zoomed while opening.
    let zoom = clamp(open + 0.001, 0.0, 1.0);
    let base = ((uv - vec2<f32>(0.5)) / zoom + vec2<f32>(0.5)) * 0.3;
    let a_uv = base + vec2<f32>(time * 0.0275, 0.0);
    let b_uv = base - vec2<f32>(time * 0.0275, 0.0);
    let n0 = textureSample(noise_tex, noise_samp, a_uv).xy;
    let n1 = textureSample(noise_tex, noise_samp, b_uv.yx - n0 * 0.02).xy;
    let n2 = textureSample(noise_tex, noise_samp, a_uv - n1 * 0.02).xy;
    let avg = (n1.y + n2.y) * 0.5;

    let s = smooth01(open);
    let s2 = s * s;
    let d = length(t0 * 2.0 - vec2<f32>(1.0));
    let inside = select(0.0, 1.0, s2 - d >= 0.0);
    // Inside, the rim ramps up over the last 0.3 of the radius; outside it
    // fades within 0.075.
    let edge_in = clamp((d - (s2 - 0.3)) / 0.3, 0.0, 1.0);
    let outside = 1.0 - clamp((d - s2) * 13.333333, 0.0, 1.0);
    let ring = mix(outside, edge_in, inside);
    let strength = ring * max(clamp(s * 2.5, 0.0, 1.0), 1.0 - clarity);
    let burn = 1.0 - smooth01(clamp((avg - (strength - 0.875)) / 1.75, 0.0, 1.0));
    let fill = 1.0 - smooth01(clamp(clarity / max(avg, 0.0001), 0.0, 1.0));
    let alpha = clamp(strength * burn + inside * fill, 0.0, 1.0) * 1.5;
    let look = alpha * sqrt(burn) * (pow(abs(t0.y), 1.5) * 0.8 + 0.2);
    let ramp = textureSampleLevel(gradient_tex, gradient_samp, vec2<f32>(clamp(look, 0.0, 1.0), 0.5), 0.0).rgb;
    // The game saturates its light scale here, so dark scenes do not
    // brighten the fire.
    let fire = ramp * portal.color.a * min(view.exposure, 1.0) * portal.glow.x;
    let cover = min(alpha, 1.0);

    if inside > 0.5 && portal.state.x > 0.5 {
        let screen = in.position.xy / view.viewport.zw;
        let through = textureSampleLevel(view_tex, view_samp, screen, 0.0).rgb;
        return vec4<f32>(mix(through, fire, cover), 1.0);
    }
    return vec4<f32>(fire, cover);
}
