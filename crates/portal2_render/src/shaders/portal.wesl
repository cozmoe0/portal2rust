// Portal opening and rim, after the game's PortalRefract stages (see
// docs/re/portals.md): the hole is the ellipse whose radius is the square of
// the smoothstepped open amount; inside it the linked view shows (rendered
// to a texture with the same projection, so it is sampled in screen space).
// Over it lies the rim: two scrolling, self-distorting noise lookups pick
// where the fire burns, and the result looks up the portal's colour
// gradient, times the colour scale. While the portal shows static the fire
// fills the whole hole.
//
// With the visual overhaul's `r_fx_portals` (the `FX` shader def) the
// opening is procedural fire and smoke instead (`portal_fx.wesl`), drawn
// with premultiplied blending on a quad reaching past the rectangle.

import bevy_pbr::render::forward_io::VertexOutput;
import bevy_pbr::render::mesh_view_bindings::view;
import bevy_pbr::render::mesh_view_bindings::globals;
import portal2_render::shaders::portal_fx::{PortalFxIn, portal_fx};

struct PortalParams {
    // rgb: fallback rim colour (linear), a: colour scale
    color: vec4<f32>,
    // x: linked (0/1), y: unused, z: open amount (0..1), w: clarity (1 - static)
    state: vec4<f32>,
    // x: rim brightness multiplier (the visual overhaul's HDR glow, else 1),
    // yz: the target's pixel size, w: the overhaul's heat refraction (0..1)
    glow: vec4<f32>,
    // The overhaul's fire: the quad's reach past the rectangle (left,
    // right, bottom, top) in texture coordinates
    shape: vec4<f32>,
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
    @if(FX) {
        var fx_in: PortalFxIn;
        fx_in.uv = uv;
        fx_in.uv_dx = dpdx(uv);
        fx_in.uv_dy = dpdy(uv);
        fx_in.color = portal.color.rgb;
        fx_in.linked = portal.state.x;
        fx_in.open = portal.state.z;
        fx_in.clarity = portal.state.w;
        fx_in.margins = portal.shape;
        fx_in.time = globals.time;
        fx_in.brightness = min(view.exposure, 1.0) * portal.glow.x;
        fx_in.heat = portal.glow.w;
        // (WESL does not scope the names inside `@if`.)
        let fx_out = portal_fx(fx_in);
        var fx_rgb = fx_out.emit;
        if fx_out.view > 0.0 {
            let fx_size = select(vec2<f32>(textureDimensions(view_tex)), portal.glow.yz, portal.glow.y > 1.5);
            var fx_through = textureSampleLevel(view_tex, view_samp, (in.position.xy + fx_out.view_offset) / fx_size, 0.0).rgb;
            // The heat's colour fringe near the rim.
            if fx_out.view_split > 0.0 {
                fx_through.r = textureSampleLevel(view_tex, view_samp, (in.position.xy + fx_out.view_offset_r) / fx_size, 0.0).r;
                fx_through.b = textureSampleLevel(view_tex, view_samp, (in.position.xy + fx_out.view_offset_b) / fx_size, 0.0).b;
            }
            fx_rgb += fx_through * fx_out.view;
        }
        return vec4<f32>(fx_rgb, fx_out.alpha);
    }
    let open = clamp(portal.state.z, 0.0, 1.0);
    let clarity = clamp(portal.state.w, 0.0, 1.0);
    // The frame's time from the view (the same game clock): the material
    // itself only changes while a portal opens or its static fades.
    let time = globals.time;

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
        // The view texture covers the target this surface is drawn into
        // (glow.yz, its pixel size), maybe at a lower resolution; a view
        // drawing only part of it (a cropped portal view showing an inner
        // portal) has a smaller viewport at the same place.
        let target_size = select(vec2<f32>(textureDimensions(view_tex)), portal.glow.yz, portal.glow.y > 1.5);
        let screen = in.position.xy / target_size;
        let through = textureSampleLevel(view_tex, view_samp, screen, 0.0).rgb;
        return vec4<f32>(mix(through, fire, cover), 1.0);
    }
    return vec4<f32>(fire, cover);
}
