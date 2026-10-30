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

// Light flags (visuals/fx/lights.rs `light_flags`).
const FXL_DIFFUSE: u32 = 1u;
const FXL_SPECULAR: u32 = 2u;
const FXL_VOLUMETRIC: u32 = 4u;
const FXL_FIXTURE: u32 = 8u;

const FX_PI: f32 = 3.14159265;

// What a surface tells the light loop.
struct FxSurface {
    pos: vec3<f32>,
    // Shading normal (bumped), facing the viewer.
    n: vec3<f32>,
    // Toward the eye.
    v: vec3<f32>,
    // Perceptual roughness and specular reflectance at normal incidence.
    roughness: f32,
    f0: vec3<f32>,
    // Baked light at the surface (lightmap or vertex lighting), which says
    // how much of a fixture's light reaches it.
    baked: vec3<f32>,
    // Whether fixtures (already baked here) add highlights.
    fixtures: bool,
}

struct FxLit {
    // Irradiance on the lightmaps' scale (multiply by albedo).
    diffuse: vec3<f32>,
    // Reflected light, already weighted by the BRDF.
    specular: vec3<f32>,
}

fn fx_luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.299, 0.587, 0.114));
}

// Smooth fade to nothing at the light's range.
fn fx_window(d: f32, range: f32) -> f32 {
    let x = d / max(range, 1.0);
    let w = saturate(1.0 - x * x * x * x);
    return w * w;
}

fn fx_closest_on_segment(a: vec3<f32>, b: vec3<f32>, p: vec3<f32>) -> vec3<f32> {
    let ab = b - a;
    let t = saturate(dot(p - a, ab) / max(dot(ab, ab), 1e-6));
    return a + ab * t;
}

// The point of segment ab closest to the ray from p along r (Karis 2013).
fn fx_segment_to_ray(a: vec3<f32>, b: vec3<f32>, p: vec3<f32>, r: vec3<f32>) -> vec3<f32> {
    let l0 = a - p;
    let ld = b - a;
    let rl = dot(r, ld);
    let t = saturate((dot(r, l0) * rl - dot(l0, ld)) / max(dot(ld, ld) - rl * rl, 1e-4));
    return a + ld * t;
}

// A point in the portal's plane pulled inside its ellipse, or onto its rim
// when `rim` is set.
fn fx_clamp_ellipse(q: vec3<f32>, c: vec3<f32>, right: vec3<f32>, up: vec3<f32>, hw: f32, hh: f32, rim: bool) -> vec3<f32> {
    let d = q - c;
    var e = vec2<f32>(dot(d, right) / hw, dot(d, up) / hh);
    let len = length(e);
    if len > 1.0 || (rim && len > 1e-3) {
        e = e / len;
    }
    return c + right * (e.x * hw) + up * (e.y * hh);
}

fn fx_d_ggx(n_h: f32, a: f32) -> f32 {
    let a2 = a * a;
    let f = n_h * n_h * (a2 - 1.0) + 1.0;
    return a2 / (FX_PI * f * f);
}

fn fx_v_smith(n_v: f32, n_l: f32, a: f32) -> f32 {
    let a2 = a * a;
    let gv = n_l * sqrt(n_v * n_v * (1.0 - a2) + a2);
    let gl = n_v * sqrt(n_l * n_l * (1.0 - a2) + a2);
    return 0.5 / max(gv + gl, 1e-5);
}

fn fx_fresnel(f0: vec3<f32>, v_h: f32) -> vec3<f32> {
    let f = pow(1.0 - v_h, 5.0);
    return f0 + (vec3<f32>(1.0) - f0) * f;
}

// Light from the overhaul's lights at a surface.
fn fx_lighting(s: FxSurface) -> FxLit {
    var out: FxLit;
    out.diffuse = vec3<f32>(0.0);
    out.specular = vec3<f32>(0.0);
    if !fx_on(FX_LIGHTS) {
        return out;
    }
    // Rougher where the normal varies across the pixel, so highlights do
    // not sparkle (Tokuyoshi and Kaplanyan's specular antialiasing).
    let dn = fwidth(s.n);
    let variance = min(dot(dn, dn) * 0.5, 0.18);
    let a0 = max(s.roughness * s.roughness, 0.002);
    let a = sqrt(a0 * a0 + variance);
    let r = reflect(-s.v, s.n);
    let n_v = max(dot(s.n, s.v), 1e-4);
    let baked_luma = fx_luma(s.baked);
    // Fixtures already shine in the cube map reflections; their sharper
    // highlights only show on glossy (wet, polished) surfaces.
    let gloss = saturate((0.8 - s.roughness) / 0.5);
    let fixture_gloss = gloss * gloss * fx.surface.z;
    let count = min(u32(fx.header.z), 64u);
    for (var i = 0u; i < count; i++) {
        let l = fx.lights[i];
        let flags = u32(l.params.y);
        let fixture = (flags & FXL_FIXTURE) != 0u;
        if fixture && !s.fixtures {
            continue;
        }
        let kind = u32(l.position.w);
        let range = l.color.w;
        var lp = l.position.xyz;
        var sp = l.position.xyz;
        var atten = 1.0;
        var size = l.up.w;
        switch kind {
            // Point and spot: the map's own attenuation terms.
            case 0u, 1u: {
                let d = max(distance(lp, s.pos), 1.0);
                atten = 1.0 / max(l.up.x + l.up.y * d + l.up.z * d * d, 1e-6);
                if kind == 1u {
                    let cos_angle = dot(normalize(s.pos - lp), l.axis.xyz);
                    let cone = saturate((cos_angle - l.axis.w) / max(l.params.x - l.axis.w, 1e-4));
                    atten *= pow(cone, max(l.params.w, 1e-3));
                }
                // The nearest point of a small sphere to the reflection.
                let to_l = lp - s.pos;
                let along = dot(to_l, r) * r - to_l;
                sp = lp + along * saturate(size / max(length(along), 1e-4));
            }
            // A tube (laser beam): nearest points for diffuse and specular.
            case 2u: {
                lp = fx_closest_on_segment(l.position.xyz, l.axis.xyz, s.pos);
                sp = fx_segment_to_ray(l.position.xyz, l.axis.xyz, s.pos, r);
                let d = distance(lp, s.pos);
                atten = 1.0 / (d * d + size * size);
            }
            // A one-sided ellipse (portal), with a little wrap over its edge
            // so the frame around the opening catches the glow.
            default: {
                let c = l.position.xyz;
                let nrm = l.axis.xyz;
                let up = l.up.xyz;
                let right = cross(up, nrm);
                let side = dot(s.pos - c, nrm);
                if side < -8.0 {
                    continue;
                }
                // Surfaces level with the opening (its frame and wall) see
                // the burning rim, not the disc in front of them.
                let level = side < 4.0;
                lp = fx_clamp_ellipse(s.pos - nrm * side, c, right, up, l.axis.w, l.params.x, level);
                let denom = dot(r, nrm);
                var hit = s.pos - nrm * side;
                if abs(denom) > 1e-3 {
                    let t = -side / denom;
                    if t > 0.0 {
                        hit = s.pos + r * t;
                    }
                }
                sp = fx_clamp_ellipse(hit, c, right, up, l.axis.w, l.params.x, false);
                let d = distance(lp, s.pos);
                let cos_e = dot(nrm, (s.pos - lp) / max(d, 1e-3));
                var emit = saturate((cos_e + 0.6) / 1.6);
                // The rim's fire burns out of the wall's plane too.
                if level {
                    emit = max(emit, 0.45);
                }
                // Softened close to the rim, whose fire is a hand wide.
                atten = emit / (d * d + 256.0);
                size = sqrt(l.axis.w * l.params.x);
            }
        }
        let dl = distance(lp, s.pos);
        atten *= fx_window(dl, range);
        if atten <= 0.0 {
            continue;
        }
        let ld = (lp - s.pos) / max(dl, 1e-4);
        let n_l = dot(s.n, ld);
        let irradiance = l.color.rgb * atten;
        var visible = 1.0;
        if fixture {
            // The share of this fixture's light the baked lighting shows.
            let expected = fx_luma(irradiance) * max(n_l, 0.25);
            visible = saturate(baked_luma / max(expected, 1e-4)) * fixture_gloss;
            if visible <= 0.0 {
                continue;
            }
        }
        if (flags & FXL_DIFFUSE) != 0u {
            out.diffuse += irradiance * saturate(n_l);
        }
        if (flags & FXL_SPECULAR) != 0u {
            let ds = max(distance(sp, s.pos), 1e-3);
            let ls = (sp - s.pos) / ds;
            let n_ls = saturate(dot(s.n, ls));
            if n_ls > 0.0 {
                let h = normalize(s.v + ls);
                // Wider lobes for bigger, nearer emitters, energy kept.
                let a_wide = saturate(a + size / (2.0 * ds));
                let norm = (a / a_wide) * (a / a_wide);
                let brdf = fx_d_ggx(saturate(dot(s.n, h)), a_wide) * fx_v_smith(n_v, n_ls, a_wide) * norm;
                let f = fx_fresnel(s.f0, saturate(dot(s.v, h)));
                // The lightmaps leave out 1/pi, so the specular gains it.
                out.specular += irradiance * n_ls * brdf * f * FX_PI * visible * fx.surface.y;
            }
        }
    }
    return out;
}
