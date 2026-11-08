// The visual overhaul's portals (`r_fx_portals`): the opening drawn as
// procedural fire and smoke instead of the game's noise texture and colour
// gradient (`portal.wesl` keeps that look for the overhaul's off preset).
//
// - The rim: a thin white-hot line on the edge of the hole, with tongues of
//   flame streaming outward from it and softer licks reaching in. They come
//   from fractal noise laid around the ellipse (seamless, no polar cut) and
//   scrolled outward, slowly turning, warped by a second noise.
// - A halo of the portal's colour on the wall around it (the bloom spreads
//   it further), faded out before the quad's edge.
// - Unlinked (or full of static), the hole churns with smoke-fire: a
//   twice domain-warped noise, slowly turning, dark smoke between bright
//   fire, brighter toward the rim.
// - Linked, the view through shows inside the hole, a little darker near
//   the rim and tinted by the portal's light, so it sits inside the fire.
//   It shimmers in the heat only while the portal opens: at rest the view
//   lines up exactly with the room around the portal (a light bridge or a
//   floor running through reads as one). When the partner (re)opens, the static
//   burns away from the centre outward behind a bright front.
// - While it opens (half a second), the fire around the rim smears along
//   it, the edge wobbles and the flames run taller and faster; all of it
//   settles as the portal finishes opening.
//
// Colours come from the portal's colour: a deep tint for the cool smoke,
// the colour itself, and a near-white with its hue for the hottest parts.

import portal2_render::shaders::noise::{noise_fbm3, noise_gradient3};

// The portal's half width and height (units).
const HALF_SIZE: vec2<f32> = vec2<f32>(32.0, 56.0);
// The hole at full size is the game's: 1 / 1.075 of the portal's rectangle.
const HOLE_SCALE: f32 = 0.930233;

struct PortalFxIn {
    // The quad's texture coordinates: 0..1 across the portal's rectangle,
    // past it into the margins. And their screen derivatives.
    uv: vec2<f32>,
    uv_dx: vec2<f32>,
    uv_dy: vec2<f32>,
    // The portal's colour (linear).
    color: vec3<f32>,
    // 1 when the view through shows (linked), else 0.
    linked: f32,
    // Open amount (0..1, half a second) and clarity (1 - static).
    open: f32,
    clarity: f32,
    // The quad's extent past the rectangle (left, right, bottom, top) in
    // texture coordinates.
    margins: vec4<f32>,
    time: f32,
    // Multiplier on everything emitted (exposure and the overhaul's glow).
    brightness: f32,
}

struct PortalFxOut {
    // Light emitted (premultiplied).
    emit: vec3<f32>,
    // How much of what lies behind the quad is hidden.
    alpha: f32,
    // Weight of the view through the portal, and the pixel offset to read
    // it at.
    view: f32,
    view_offset: vec2<f32>,
}

// The fire's colour at temperature `t` (0..1): smoke, a deep saturated
// shade of the portal's colour, the colour itself, and a near-white with
// its hue. The middle stops stay saturated so bright fire keeps its colour
// through the tone curve's shoulder.
fn portal_fx_ramp(color: vec3<f32>, t: f32) -> vec3<f32> {
    let c = color / max(max(color.r, color.g), max(color.b, 1e-4));
    let smoke = c * c * c * 0.05;
    let deep = c * c * 0.6;
    var hot = mix(c, vec3<f32>(1.0), 0.45);
    hot.g = max(hot.g, 0.8);
    let x = clamp(t, 0.0, 1.0);
    if x < 0.35 {
        return mix(smoke, deep, x / 0.35);
    }
    if x < 0.7 {
        return mix(deep, c, (x - 0.35) / 0.35);
    }
    return mix(c, hot, (x - 0.7) / 0.3);
}

fn portal_fx_rotate(v: vec2<f32>, a: f32) -> vec2<f32> {
    let s = sin(a);
    let c = cos(a);
    return vec2<f32>(c * v.x - s * v.y, s * v.x + c * v.y);
}

fn portal_fx(i: PortalFxIn) -> PortalFxOut {
    let t = i.time;
    let open = clamp(i.open, 0.0, 1.0);
    let s = open * open * (3.0 - 2.0 * open);
    // How far the opening still has to go: drives its distortion.
    let k = pow(1.0 - open, 0.6);

    // Portal space: q -1..1 across the rectangle, p in units with y up.
    let q = (i.uv - vec2<f32>(0.5)) * 2.0;
    let p = vec2<f32>(q.x, -q.y) * HALF_SIZE;
    let rho = max(length(q), 1e-4);
    // Outward direction around the ellipse (in q, so its angle runs evenly
    // around the rim) and the gradient of rho in units.
    let dir = q / rho;
    let grad = vec2<f32>(p.x / (HALF_SIZE.x * HALF_SIZE.x), p.y / (HALF_SIZE.y * HALF_SIZE.y)) / rho;
    let glen = max(length(grad), 1e-5);
    let normal = grad / glen;

    // The hole, its edge wobbling while it opens.
    let wobble = noise_gradient3(vec3<f32>(dir * 2.2, t * 7.0 + 3.1));
    let radius = s * s * HOLE_SCALE * (1.0 + 0.14 * k * wobble);
    // Signed distance to the edge in units (positive outside the hole).
    let sd = (rho - radius) / glen;
    let rn = rho / max(radius, 1e-4);

    // Fire around the rim: noise on a circle (the direction) times the
    // distance from the edge flowing outward. While opening it smears along
    // the rim and runs faster.
    // Only worked out where flames can reach (most of a linked hole has
    // none).
    let tall = 10.0 * (1.0 + 0.8 * k);
    var nf = 0.0;
    if sd > -7.0 && sd < tall * 1.3 {
        let smear = k * (0.5 + 0.45 * noise_gradient3(vec3<f32>(dir * 2.0, t * 6.0)));
        let around = portal_fx_rotate(vec2<f32>(dir.x, -dir.y), t * 0.07 + smear) * 6.5;
        let flow = sd * 0.16 - t * 1.6 + k * 2.0;
        let warp = vec2<f32>(
            noise_gradient3(vec3<f32>(around * 0.45, flow * 0.5 + t * 0.35)),
            noise_gradient3(vec3<f32>(around * 0.45 + vec2<f32>(19.7, 7.3), flow * 0.5 - t * 0.3)),
        );
        let f = noise_fbm3(vec3<f32>(around + warp * 1.1, flow), 4);
        nf = clamp(f * 1.4 + 0.5, 0.0, 1.0);
    }
    let dens_out = clamp((nf * 1.25 - max(sd, 0.0) / tall) * 2.2, 0.0, 1.0);
    let dens_in = clamp((nf * 0.9 + min(sd, 0.0) / 6.0) * 2.2, 0.0, 1.0);
    let dens = select(dens_in, dens_out, sd > 0.0);
    // The hot line along the edge, a couple of units wide (a thinner one
    // shows as a hard ring in mirror-like floors).
    let core = exp(-abs(sd + 0.8) / 1.8);
    let temp = clamp(dens * (0.3 + 0.5 * nf) + core * 0.55, 0.0, 1.0);
    let appear = smoothstep(0.0, 0.12, open);
    let rim = portal_fx_ramp(i.color, temp) * (dens * (0.4 + 1.8 * temp * temp) + core * 3.2) * appear;

    // Halo on the wall, gone before the quad's edges.
    let edge_uv = min(i.uv + i.margins.xw, vec2<f32>(1.0) + i.margins.yz - i.uv);
    let fade = smoothstep(0.0, 6.0 / (2.0 * HALF_SIZE.x), edge_uv.x) * smoothstep(0.0, 6.0 / (2.0 * HALF_SIZE.y), edge_uv.y);
    let halo = portal_fx_ramp(i.color, 0.6) * exp(-max(sd, 0.0) / 10.0) * 0.6 * s;

    var out: PortalFxOut;
    out.view = 0.0;
    out.view_offset = vec2<f32>(0.0);
    // What lies outside the hole: flames over the wall, a little smoky.
    let outside_emit = (rim + halo) * fade;
    let smoke = dens_out * 0.25 * fade * appear;
    // The hole's edge, softened over a unit.
    let cover = smoothstep(0.6, -0.6, sd) * appear;
    if cover <= 0.0 {
        out.emit = outside_emit * i.brightness;
        out.alpha = smoke;
        return out;
    }

    // Inside the hole.
    let clarity = select(0.0, clamp(i.clarity, 0.0, 1.0), i.linked > 0.5);
    var fill_amount = 0.0;
    var fill = vec3<f32>(0.0);
    var front = 0.0;
    if clarity < 1.0 {
        // Smoke-fire, twice domain-warped, slowly turning.
        let rp = portal_fx_rotate(p, t * 0.1) * 0.07;
        let wq = vec2<f32>(
            noise_fbm3(vec3<f32>(rp * 0.5, t * 0.15), 3),
            noise_fbm3(vec3<f32>(rp * 0.5 + vec2<f32>(5.2, 1.3), t * 0.15), 3),
        );
        let wr = vec2<f32>(
            noise_fbm3(vec3<f32>(rp + wq * 1.2 + vec2<f32>(1.7, 9.2), t * 0.22), 3),
            noise_fbm3(vec3<f32>(rp + wq * 1.2 + vec2<f32>(8.3, 2.8), t * 0.22), 3),
        );
        let v = clamp(noise_fbm3(vec3<f32>(rp + wr * 0.9, t * 0.3), 5) * 1.5 + 0.5, 0.0, 1.0);
        // Fire where the noise is high, smoke where it is low; hotter
        // toward the rim.
        let burning = smoothstep(0.22, 0.62, v);
        let rim_heat = rn * rn * rn * rn;
        // Kept below the tone curve's knee where it can, so it stays the
        // portal's colour rather than washing out to cream.
        let fill_temp = clamp(0.3 + 0.35 * burning + 0.3 * rim_heat, 0.0, 1.0);
        fill = portal_fx_ramp(i.color, fill_temp) * (0.5 + 1.2 * fill_temp * fill_temp) * mix(0.15, 1.0, burning);
        // The static burns away from the centre outward.
        let m = rn * 0.75 + v * 0.3;
        let threshold = clarity * 1.2 - 0.1;
        fill_amount = smoothstep(threshold - 0.04, threshold + 0.06, m);
        front = exp(-abs(m - threshold) / 0.035) * clarity * (1.0 - clarity) * 4.0;
    }
    let mid = portal_fx_ramp(i.color, 0.5);
    let near_rim = exp(sd / 6.0);
    var emit = rim + fill * fill_amount + portal_fx_ramp(i.color, 0.85) * front * 3.0;
    if i.linked > 0.5 {
        let see = (1.0 - fill_amount) * (1.0 - dens_in * 0.75);
        out.view = see * (1.0 - 0.1 * near_rim) * cover;
        // The portal's light scattered over the view near the rim.
        emit += mid * 0.3 * exp(sd / 8.0) * see;
        // Heat shimmer near the rim while opening, pulled slightly inward;
        // the point read stays inside the hole (the view only covers the
        // opening).
        let shimmer = exp(sd / 6.0) * 3.0 * k;
        var offset = vec2<f32>(0.0);
        if shimmer > 0.03 {
            let d = vec2<f32>(
                noise_gradient3(vec3<f32>(p * 0.11, t * 1.7)),
                noise_gradient3(vec3<f32>(p * 0.11 + vec2<f32>(41.0, 13.0), t * 1.7)),
            ) * 1.4 - normal * 0.5;
            offset = d * shimmer;
        }
        var duv = vec2<f32>(offset.x, -offset.y) / (2.0 * HALF_SIZE);
        var q2 = (i.uv + duv - vec2<f32>(0.5)) * 2.0;
        let limit = radius * 0.96;
        let r2 = length(q2);
        if r2 > limit {
            q2 *= limit / r2;
        }
        duv = q2 * 0.5 + vec2<f32>(0.5) - i.uv;
        // Texture coordinates to pixels (the inverse of their derivatives).
        let det = i.uv_dx.x * i.uv_dy.y - i.uv_dy.x * i.uv_dx.y;
        if abs(det) > 1e-12 {
            out.view_offset = vec2<f32>(duv.x * i.uv_dy.y - i.uv_dy.x * duv.y, i.uv_dx.x * duv.y - duv.x * i.uv_dx.y) / det;
        }
    }
    // Inside, the hole's own light; across the softened edge, the wall's.
    out.emit = mix(outside_emit, emit, cover) * i.brightness;
    out.alpha = mix(smoke, 1.0, cover);
    return out;
}
