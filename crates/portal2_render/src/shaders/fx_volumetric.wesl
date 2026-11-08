// Light scattered in the air for the overhaul (see visuals/fx/atmosphere.rs):
// shafts from the projected textures (the chambers' sunlight and
// floodlights, through their real openings thanks to their shadow maps),
// beams of the overhaul's volumetric lights and a faint ambient haze that
// keeps large voids from reading as flat black. It runs on the main view
// and on the portals' views, so the air looks the same through a portal. A half-resolution pass
// marches each view ray up to the depth buffer; the composite pass
// upsamples it within surfaces and blends it over the multisampled scene
// as in-scattered light plus transmittance.

import bevy_render::view::View;

// As in projected.wesl.
struct ProjectedLight {
    clip_from_world: mat4x4<f32>,
    position: vec4<f32>,
    color: vec4<f32>,
    atten: vec4<f32>,
    info: vec4<f32>,
}

struct ProjectedLights {
    header: vec4<f32>,
    lights: array<ProjectedLight, 8>,
}

// As in fx.wesl.
struct FxLight {
    position: vec4<f32>,
    color: vec4<f32>,
    axis: vec4<f32>,
    params: vec4<f32>,
    up: vec4<f32>,
}

struct FxGlobals {
    header: vec4<f32>,
    surface: vec4<f32>,
    haze: vec4<f32>,
    haze2: vec4<f32>,
    sun_dir: vec4<f32>,
    sun_color: vec4<f32>,
    reserved0: vec4<f32>,
    reserved1: vec4<f32>,
    shadow_from_world: array<mat4x4<f32>, 4>,
    sun_from_world: mat4x4<f32>,
    shadow_info: vec4<f32>,
    reserved2: array<vec4<f32>, 3>,
    lights: array<FxLight, 64>,
    probes: array<vec4<f32>, 128>,
}

struct FxVolume {
    // x: steps, y: scattering per unit, z: farthest distance, w: ambient haze strength
    params: vec4<f32>,
    // x: extinction per unit, y: anisotropy (g), z: time, w: noise on (0/1)
    medium: vec4<f32>,
}

// The main view is multisampled; the portals' views are not.
@if(MULTISAMPLED) @group(0) @binding(0) var depth_tex: texture_depth_multisampled_2d;
@if(!MULTISAMPLED) @group(0) @binding(0) var depth_tex: texture_depth_2d;
@group(0) @binding(1) var<uniform> view: View;
@group(0) @binding(2) var<uniform> vol: FxVolume;
@group(0) @binding(3) var<storage, read> fx: FxGlobals;
@group(0) @binding(4) var<storage, read> projected: ProjectedLights;
@group(0) @binding(5) var cookie_tex: texture_2d_array<f32>;
@group(0) @binding(6) var cookie_samp: sampler;
@group(0) @binding(7) var pshadow0: texture_2d<f32>;
@group(0) @binding(8) var pshadow1: texture_2d<f32>;
@group(0) @binding(9) var pshadow2: texture_2d<f32>;
@group(0) @binding(10) var pshadow3: texture_2d<f32>;
@group(0) @binding(11) var fshadow0: texture_2d<f32>;
@group(0) @binding(12) var fshadow1: texture_2d<f32>;
@group(0) @binding(13) var fshadow2: texture_2d<f32>;
@group(0) @binding(14) var fshadow3: texture_2d<f32>;
@group(0) @binding(15) var vol_tex: texture_2d<f32>;

const FXL_VOLUMETRIC: u32 = 4u;
const FXL_FIXTURE: u32 = 8u;
const FX_SHADOWS: u32 = 2u;
// The projected textures are the chambers' suns and floodlights, through
// windows and gaps: their shafts carry more than their gentle falloff on
// surfaces suggests.
const PROJECTED_SHARE: f32 = 1.5;
// Air right around the eye scatters less: a light beside the camera (an
// elevator's lamp) must not fill the whole view with glow.
const NEAR_FADE_START: f32 = 16.0;
const NEAR_FADE_END: f32 = 128.0;
// Lights this close to the eye (an elevator's lamp overhead, a lamp the
// player carries) light the view, not the air in it.
const OWN_LIGHT_NEAR: f32 = 64.0;
const OWN_LIGHT_FAR: f32 = 320.0;
// In-scattered light saturates toward this (display-linear luminance), so
// no room fills with smoke however many beams cross it.
const SCATTER_LIMIT: f32 = 0.08;
// Lights the march looks at, at most.
const MAX_VOLUME_LIGHTS: u32 = 8u;

struct FsIn {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

fn full_size() -> vec2<i32> {
    return vec2<i32>(textureDimensions(depth_tex));
}

fn depth_at(p: vec2<i32>) -> f32 {
    return textureLoad(depth_tex, clamp(p, vec2<i32>(0), full_size() - 1), 0);
}

// A pixel's point at a depth, through the view's viewport (part of the
// texture for a portal view cropped to its portal).
fn world_from(p: vec2<f32>, depth: f32) -> vec3<f32> {
    let uv = (p - view.viewport.xy) / view.viewport.zw;
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    let w = view.world_from_clip * vec4<f32>(ndc, depth, 1.0);
    return w.xyz / w.w;
}

fn ign(p: vec2<f32>) -> f32 {
    return fract(52.9829189 * fract(dot(p, vec2<f32>(0.06711056, 0.00583715))));
}

fn hash3(p: vec3<f32>) -> f32 {
    var q = fract(p * 0.1031);
    q += dot(q, q.zyx + 31.32);
    return fract((q.x + q.y) * q.z);
}

fn noise3(p: vec3<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = mix(mix(hash3(i), hash3(i + vec3<f32>(1.0, 0.0, 0.0)), u.x), mix(hash3(i + vec3<f32>(0.0, 1.0, 0.0)), hash3(i + vec3<f32>(1.0, 1.0, 0.0)), u.x), u.y);
    let b = mix(
        mix(hash3(i + vec3<f32>(0.0, 0.0, 1.0)), hash3(i + vec3<f32>(1.0, 0.0, 1.0)), u.x),
        mix(hash3(i + vec3<f32>(0.0, 1.0, 1.0)), hash3(i + vec3<f32>(1.0, 1.0, 1.0)), u.x),
        u.y,
    );
    return mix(a, b, u.z);
}

// Henyey-Greenstein phase.
fn phase(cos_theta: f32) -> f32 {
    let g = vol.medium.y;
    let g2 = g * g;
    return (1.0 - g2) / (12.566371 * pow(max(1.0 + g2 - 2.0 * g * cos_theta, 1e-4), 1.5));
}

fn pshadow(slot: i32, texel: vec2<i32>) -> f32 {
    switch slot {
        case 0: { return textureLoad(pshadow0, texel, 0).r; }
        case 1: { return textureLoad(pshadow1, texel, 0).r; }
        case 2: { return textureLoad(pshadow2, texel, 0).r; }
        default: { return textureLoad(pshadow3, texel, 0).r; }
    }
}

fn fshadow(slot: u32, texel: vec2<i32>) -> f32 {
    switch slot {
        case 0u: { return textureLoad(fshadow0, texel, 0).r; }
        case 1u: { return textureLoad(fshadow1, texel, 0).r; }
        case 2u: { return textureLoad(fshadow2, texel, 0).r; }
        default: { return textureLoad(fshadow3, texel, 0).r; }
    }
}

// Light from the projected textures arriving at `p` toward the eye.
fn projected_in(p: vec3<f32>, dir: vec3<f32>) -> vec3<f32> {
    var total = vec3<f32>(0.0);
    let count = min(u32(projected.header.x), 8u);
    for (var i = 0u; i < count; i++) {
        let l = projected.lights[i];
        let clip = l.clip_from_world * vec4<f32>(p, 1.0);
        if clip.w <= 0.0 {
            continue;
        }
        let ndc = clip.xy / clip.w;
        if abs(ndc.x) >= 1.0 || abs(ndc.y) >= 1.0 {
            continue;
        }
        let delta = p - l.position.xyz;
        let dist = length(delta);
        let far = l.position.w;
        if dist >= far || dist <= 0.0 {
            continue;
        }
        let fade = saturate((far - dist) / (0.4 * far));
        let atten = saturate(fade * dot(l.atten.xyz, vec3<f32>(1.0, 1.0 / dist, 1.0 / (dist * dist))));
        let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
        var light = textureSampleLevel(cookie_tex, cookie_samp, uv, i32(l.info.x), 0.0).rgb * atten;
        let slot = i32(l.info.y);
        if slot >= 0 {
            let size = vec2<f32>(textureDimensions(pshadow0));
            let texel = clamp(vec2<i32>(uv * size), vec2<i32>(0), vec2<i32>(size) - 1);
            if dist - 2.0 > pshadow(slot, texel) {
                light = vec3<f32>(0.0);
            }
        }
        // reserved0.y undoes temporary flashlight boosts (the first map's
        // container ride raises r_flashlightbrightness twelvefold).
        let own = smoothstep(OWN_LIGHT_NEAR, OWN_LIGHT_FAR, distance(l.position.xyz, view.world_position));
        total += light * l.color.rgb * (PROJECTED_SHARE * fx.reserved0.y * own * phase(dot(delta / dist, -dir)));
    }
    return total;
}

fn on_segment(a: vec3<f32>, b: vec3<f32>, p: vec3<f32>) -> vec3<f32> {
    let ab = b - a;
    return a + ab * saturate(dot(p - a, ab) / max(dot(ab, ab), 1e-4));
}

// Light from the overhaul's volumetric lights arriving at `p`.
fn fx_in(p: vec3<f32>, dir: vec3<f32>) -> vec3<f32> {
    var total = vec3<f32>(0.0);
    let count = min(u32(fx.header.z), 64u);
    var used = 0u;
    for (var i = 0u; i < count; i++) {
        if used >= MAX_VOLUME_LIGHTS {
            break;
        }
        let l = fx.lights[i];
        if (u32(l.params.y) & FXL_VOLUMETRIC) == 0u {
            continue;
        }
        used += 1u;
        let kind = u32(l.position.w);
        // A tube's light comes from its nearest point.
        var delta = p - l.position.xyz;
        if kind == 2u {
            delta = p - on_segment(l.position.xyz, l.axis.xyz, p);
        }
        let d = max(length(delta), 1.0);
        var atten = 0.0;
        if kind <= 1u {
            atten = 1.0 / max(l.up.x + l.up.y * d + l.up.z * d * d, 1e-6);
            if kind == 1u {
                let cone = saturate((dot(delta / d, l.axis.xyz) - l.axis.w) / max(l.params.x - l.axis.w, 1e-4));
                atten *= pow(cone, max(l.params.w, 1e-3));
            }
        } else if kind == 2u {
            // A tube (a light bridge's glow), softened over its radius.
            atten = 1.0 / (d * d + l.up.w * l.up.w);
        } else if kind == 3u {
            // A portal glows into the air in front of it.
            let side = dot(delta, l.axis.xyz);
            if side <= 0.0 {
                continue;
            }
            atten = (side / d) / (d * d + 256.0);
        } else {
            continue;
        }
        let x = d / max(l.color.w, 1.0);
        let w = saturate(1.0 - x * x * x * x);
        atten *= w * w;
        let slot = i32(l.params.z);
        if atten > 0.0 && slot >= 0 && (u32(fx.header.x) & FX_SHADOWS) != 0u {
            let clip = fx.shadow_from_world[slot] * vec4<f32>(p, 1.0);
            if clip.w > 0.0 {
                let ndc = clip.xy / clip.w;
                if abs(ndc.x) < 1.0 && abs(ndc.y) < 1.0 {
                    let size = vec2<f32>(textureDimensions(fshadow0));
                    let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
                    let texel = clamp(vec2<i32>(uv * size), vec2<i32>(0), vec2<i32>(size) - 1);
                    if d - 2.0 > fshadow(u32(slot), texel) {
                        atten = 0.0;
                    }
                }
            }
        }
        // Fixtures are bright point sources; their beams stay a hint. A tube
        // (a bridge's glow) runs past the viewer and is never their own.
        let own = select(smoothstep(OWN_LIGHT_NEAR, OWN_LIGHT_FAR, distance(l.position.xyz, view.world_position)), 1.0, kind == 2u);
        let share = select(1.0, 0.09, (u32(l.params.y) & FXL_FIXTURE) != 0u) * own;
        total += l.color.rgb * (atten * share) * phase(dot(delta / d, -dir));
    }
    return total;
}

@fragment
fn march(in: FsIn) -> @location(0) vec4<f32> {
    // The full-resolution pixel this half-resolution one stands for.
    let fp = vec2<i32>(in.pos.xy) * 2;
    let d = depth_at(fp);
    let eye = view.world_position;
    var end = vol.params.z;
    var far_point = world_from(vec2<f32>(fp) + 0.5, max(d, 1e-7));
    if d > 0.0 {
        end = min(distance(far_point, eye), end);
    }
    let dir = normalize(far_point - eye);
    let steps = max(i32(vol.params.x), 1);
    let ds = end / f32(steps);
    // A fixed per-pixel offset (no temporal filter to hide moving noise).
    let jitter = ign(in.pos.xy);
    let sigma_s = vol.params.y;
    let sigma_t = vol.medium.x;
    let ambient = fx.haze.rgb * vol.params.w;
    var scatter = vec3<f32>(0.0);
    var transmittance = 1.0;
    for (var i = 0; i < steps; i++) {
        let t = (f32(i) + jitter) * ds;
        let p = eye + dir * t;
        var density = smoothstep(NEAR_FADE_START, NEAR_FADE_END, t);
        if vol.medium.w > 0.5 {
            density *= 0.55 + 0.9 * noise3(p / 260.0 + vec3<f32>(0.0, 0.0, vol.medium.z * 0.015));
        }
        let lin = projected_in(p, dir) + fx_in(p, dir) + ambient;
        let step_t = exp(-sigma_t * density * ds);
        scatter += transmittance * lin * (sigma_s * density * ds);
        transmittance *= step_t;
    }
    // Linear while faint, rolling off toward the limit when strong.
    let exposed = scatter * view.exposure;
    let luma = dot(exposed, vec3<f32>(0.2126, 0.7152, 0.0722));
    var limited = exposed;
    if luma > 1e-5 {
        limited = exposed * (SCATTER_LIMIT * (1.0 - exp(-luma / SCATTER_LIMIT)) / luma);
    }
    return vec4<f32>(limited, max(transmittance, 0.75));
}

// Upsamples the march within surfaces (the half-resolution texels whose
// depth matches this pixel's best) and blends it over the scene.
@fragment
fn composite(in: FsIn) -> @location(0) vec4<f32> {
    let p = vec2<i32>(in.pos.xy);
    let d = depth_at(p);
    let center = select(1e9, distance(world_from(vec2<f32>(p) + 0.5, d), view.world_position), d > 0.0);
    let half_size = vec2<i32>(textureDimensions(vol_tex));
    let base = p / 2;
    var sum = vec4<f32>(0.0);
    var total = 0.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let q = clamp(base + vec2<i32>(x, y), vec2<i32>(0), half_size - 1);
            let qd = depth_at(q * 2);
            let qdist = select(1e9, distance(world_from(vec2<f32>(q * 2) + 0.5, qd), view.world_position), qd > 0.0);
            let w = 1.0 / (1.0 + abs(qdist - center) / max(center * 0.02, 1.0)) * select(1.0, 2.0, x == 0 && y == 0);
            sum += textureLoad(vol_tex, q, 0) * w;
            total += w;
        }
    }
    let v = sum / max(total, 1e-4);
    if any(v != v) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    return vec4<f32>(max(v.rgb, vec3<f32>(0.0)), saturate(v.a));
}
