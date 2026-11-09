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
// - With the heat feature (`r_fx_portal_heat`) the view also refracts near
//   the rim at all times, as if seen through a hot, thick lens: pulled in
//   toward the centre close to the edge, rippling with rising heat and split
//   into a faint colour fringe. The middle of the opening stays lined up.
//   The same heat field shimmers over the wall around the portal
//   (`portal_haze.wesl`), so the distortion runs on across the rim.
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

// The heat refraction of the view through, near the rim: how far in from
// the edge it reaches (units, an exponential falloff), how far the view is
// pulled in toward the centre at the edge, how far the rising heat moves it,
// and the colour fringe (the share of the displacement the red and blue
// channels go short of and past the green one).
const HEAT_BAND: f32 = 4.5;
const HEAT_LENS: f32 = 1.3;
const HEAT_RIPPLE: f32 = 0.65;
const HEAT_FRINGE: f32 = 0.3;

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
    // The heat refraction of the view through near the rim (0..1).
    heat: f32,
}

struct PortalFxOut {
    // Light emitted (premultiplied).
    emit: vec3<f32>,
    // How much of what lies behind the quad is hidden.
    alpha: f32,
    // Weight of the view through the portal, and the pixel offset to read
    // it at. With a colour fringe (`view_split` > 0) the red and blue
    // channels are read at their own offsets.
    view: f32,
    view_offset: vec2<f32>,
    view_split: f32,
    view_offset_r: vec2<f32>,
    view_offset_b: vec2<f32>,
}

// Where a point of the quad lies relative to the opening.
struct PortalFxSpace {
    // Smoothstepped open amount, and how far the opening still has to go
    // (drives its distortion).
    s: f32,
    k: f32,
    // Portal space: q -1..1 across the rectangle (down as the texture
    // coordinates), p in units with y up.
    q: vec2<f32>,
    p: vec2<f32>,
    rho: f32,
    // Outward direction around the ellipse (in q, so its angle runs evenly
    // around the rim), the length of rho's gradient in units and the
    // outward normal it points along (units, y up).
    dir: vec2<f32>,
    glen: f32,
    normal: vec2<f32>,
    // The hole's radius (in rho, its edge wobbling while it opens), the
    // signed distance to its edge in units (positive outside the hole) and
    // rho over the radius.
    radius: f32,
    sd: f32,
    rn: f32,
}

fn portal_fx_space(uv: vec2<f32>, open_amount: f32, t: f32) -> PortalFxSpace {
    var o: PortalFxSpace;
    let open = clamp(open_amount, 0.0, 1.0);
    o.s = open * open * (3.0 - 2.0 * open);
    o.k = pow(1.0 - open, 0.6);
    o.q = (uv - vec2<f32>(0.5)) * 2.0;
    o.p = vec2<f32>(o.q.x, -o.q.y) * HALF_SIZE;
    o.rho = max(length(o.q), 1e-4);
    o.dir = o.q / o.rho;
    let grad = vec2<f32>(o.p.x / (HALF_SIZE.x * HALF_SIZE.x), o.p.y / (HALF_SIZE.y * HALF_SIZE.y)) / o.rho;
    o.glen = max(length(grad), 1e-5);
    o.normal = grad / o.glen;
    let wobble = noise_gradient3(vec3<f32>(o.dir * 2.2, t * 7.0 + 3.1));
    o.radius = o.s * o.s * HOLE_SCALE * (1.0 + 0.14 * o.k * wobble);
    o.sd = (o.rho - o.radius) / o.glen;
    o.rn = o.rho / max(o.radius, 1e-4);
    return o;
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

// The heat over and around the rim as a height: fine cells a few units
// across rising fast over broad, slower swells. Laid out like the flames,
// on a circle around the ellipse (`around`, unit length, y up) and by the
// distance from the edge, flowing outward.
fn portal_fx_heat_height(around: vec2<f32>, sd: f32, t: f32) -> f32 {
    let fine = noise_fbm3(vec3<f32>(around * 9.0, sd * 0.24 - t * 2.3), 3);
    let broad = noise_gradient3(vec3<f32>(around * 3.2 + vec2<f32>(7.1, 3.3), sd * 0.075 - t * 0.6));
    return fine + broad * 0.8;
}

// How far the heat bends what is seen through it at a point (units, y up):
// the height's slope, along the rim and outward (a lens-like refraction,
// not a random shove, so features magnify and shrink as the cells pass).
// Mostly within a unit or two.
fn portal_fx_heat(sp: PortalFxSpace, t: f32) -> vec2<f32> {
    // The physical direction around the rim (y up), and the noise's, which
    // turns slowly as the flames do.
    let a = vec2<f32>(sp.dir.x, -sp.dir.y);
    let around = portal_fx_rotate(a, t * 0.07);
    // A step along the rim in units, its angle, and the rim's direction in
    // units at this point (the ellipse's tangent).
    let tangent_raw = vec2<f32>(-a.y * HALF_SIZE.x, a.x * HALF_SIZE.y) * sp.rho;
    let per_radian = max(length(tangent_raw), 1e-3);
    let tangent = tangent_raw / per_radian;
    let du = 0.35;
    let h0 = portal_fx_heat_height(around, sp.sd, t);
    let h_along = portal_fx_heat_height(portal_fx_rotate(around, du / per_radian), sp.sd, t);
    let h_out = portal_fx_heat_height(around, sp.sd + du, t);
    return (tangent * (h_along - h0) + sp.normal * (h_out - h0)) * (3.0 / du);
}

// A displacement of the view through (units, y up) as a pixel offset to
// read the view at, kept inside `limit` of the hole (in rho): the view only
// covers the opening.
fn portal_fx_view_pixels(i: PortalFxIn, offset: vec2<f32>, limit: f32) -> vec2<f32> {
    var duv = vec2<f32>(offset.x, -offset.y) / (2.0 * HALF_SIZE);
    var q2 = (i.uv + duv - vec2<f32>(0.5)) * 2.0;
    let r2 = length(q2);
    if r2 > limit {
        q2 *= limit / r2;
    }
    duv = q2 * 0.5 + vec2<f32>(0.5) - i.uv;
    // Texture coordinates to pixels (the inverse of their derivatives).
    let det = i.uv_dx.x * i.uv_dy.y - i.uv_dy.x * i.uv_dx.y;
    if abs(det) > 1e-12 {
        return vec2<f32>(duv.x * i.uv_dy.y - i.uv_dy.x * duv.y, i.uv_dx.x * duv.y - duv.x * i.uv_dx.y) / det;
    }
    return vec2<f32>(0.0);
}

fn portal_fx(i: PortalFxIn) -> PortalFxOut {
    let t = i.time;
    let open = clamp(i.open, 0.0, 1.0);
    let sp = portal_fx_space(i.uv, i.open, t);
    let s = sp.s;
    let k = sp.k;
    let p = sp.p;
    let dir = sp.dir;
    let normal = sp.normal;
    let radius = sp.radius;
    let sd = sp.sd;
    let rn = sp.rn;

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
    out.view_split = 0.0;
    out.view_offset_r = vec2<f32>(0.0);
    out.view_offset_b = vec2<f32>(0.0);
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
        // The heat's refraction near the rim, at rest too: the view pulled
        // in toward the centre close to the edge (as through the rounded
        // edge of a thick lens), rippling with the rising heat, and a colour
        // fringe where it bends most. Gone a couple of bands in, so the
        // middle of the opening stays lined up with the room around it.
        var split = 0.0;
        if i.heat > 0.0 {
            let band = exp(min(sd, 0.0) / HEAT_BAND) * appear * i.heat;
            if band > 0.01 {
                let lens = -normal * (band * band * HEAT_LENS);
                let ripple = portal_fx_heat(sp, t) * (band * HEAT_RIPPLE * (1.0 + 1.5 * k));
                offset += lens + ripple;
                split = band * HEAT_FRINGE;
            }
        }
        let limit = radius * 0.96;
        out.view_offset = portal_fx_view_pixels(i, offset, limit);
        out.view_offset_r = out.view_offset;
        out.view_offset_b = out.view_offset;
        if split > 0.0 {
            out.view_split = split;
            out.view_offset_r = portal_fx_view_pixels(i, offset * (1.0 - split), limit);
            out.view_offset_b = portal_fx_view_pixels(i, offset * (1.0 + split), limit);
        }
    }
    // Inside, the hole's own light; across the softened edge, the wall's.
    out.emit = mix(outside_emit, emit, cover) * i.brightness;
    out.alpha = mix(smoke, 1.0, cover);
    return out;
}

// The heat haze over the wall around the rim (`portal_haze.wesl`): how far
// to shift what lies behind (pixels), from the same heat field as the
// view's refraction. Zero inside the hole (the opening covers it) and
// toward the quad's edges, so the shifted picture meets the unshifted one
// without a seam.
fn portal_fx_haze(i: PortalFxIn) -> vec2<f32> {
    let t = i.time;
    let sp = portal_fx_space(i.uv, i.open, t);
    if sp.sd < -1.5 {
        return vec2<f32>(0.0);
    }
    let appear = smoothstep(0.0, 0.12, clamp(i.open, 0.0, 1.0)) * sp.s;
    let edge_uv = min(i.uv + i.margins.xw, vec2<f32>(1.0) + i.margins.yz - i.uv);
    let fade = smoothstep(0.0, 7.0 / (2.0 * HALF_SIZE.x), edge_uv.x) * smoothstep(0.0, 7.0 / (2.0 * HALF_SIZE.y), edge_uv.y);
    // Strongest at the edge, a few units out; stronger while opening.
    let amount = exp(-max(sp.sd, 0.0) / 6.0) * fade * appear * (1.0 + 1.5 * sp.k) * i.heat;
    if amount < 0.002 {
        return vec2<f32>(0.0);
    }
    let offset = portal_fx_heat(sp, t) * (amount * 1.4);
    let duv = vec2<f32>(offset.x, -offset.y) / (2.0 * HALF_SIZE);
    let det = i.uv_dx.x * i.uv_dy.y - i.uv_dy.x * i.uv_dx.y;
    if abs(det) > 1e-12 {
        return vec2<f32>(duv.x * i.uv_dy.y - i.uv_dy.x * duv.y, i.uv_dx.x * duv.y - duv.x * i.uv_dx.y) / det;
    }
    return vec2<f32>(0.0);
}
