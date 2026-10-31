// Screen-space reflections for the overhaul (see visuals/fx/ssr.rs): on
// pixels the FX world shader marked as mirror-like (puddles, very glossy
// surfaces) in the scene's alpha, a reflected ray is marched against the
// main view's depth and the scene seen there (portals, lasers and lights
// included, since this runs after the transparent pass) is added with
// Fresnel. Rays that leave the screen or find nothing fade out; the probe
// reflection the material already added stays as the fallback. The pass
// also writes alpha 1 back over the marks.

import bevy_render::view::View;

@group(0) @binding(0) var depth_tex: texture_depth_multisampled_2d;
@group(0) @binding(1) var<uniform> view: View;
@group(0) @binding(2) var color_tex: texture_2d<f32>;
@group(0) @binding(3) var color_samp: sampler;
@group(0) @binding(4) var<uniform> ssr: FxSsr;

struct FxSsr {
    // x: farthest reach, y: steps, z: strength, w: unused
    params: vec4<f32>,
}

struct FsIn {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

fn size() -> vec2<i32> {
    return vec2<i32>(textureDimensions(depth_tex));
}

fn depth_at(p: vec2<i32>) -> f32 {
    return textureLoad(depth_tex, clamp(p, vec2<i32>(0), size() - 1), 0);
}

fn world_from(p: vec2<f32>, depth: f32) -> vec3<f32> {
    let uv = p / vec2<f32>(size());
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    let w = view.world_from_clip * vec4<f32>(ndc, depth, 1.0);
    return w.xyz / w.w;
}

fn world_at(p: vec2<i32>) -> vec3<f32> {
    return world_from(vec2<f32>(p) + 0.5, depth_at(p));
}

fn view_depth(w: vec3<f32>) -> f32 {
    return -(view.view_from_world * vec4<f32>(w, 1.0)).z;
}

fn ign(p: vec2<f32>) -> f32 {
    return fract(52.9829189 * fract(dot(p, vec2<f32>(0.06711056, 0.00583715))));
}

// The copy of the scene the reflection pass reads.
@fragment
fn copy(in: FsIn) -> @location(0) vec4<f32> {
    return textureLoad(color_tex, vec2<i32>(in.pos.xy), 0);
}

@fragment
fn reflection(in: FsIn) -> @location(0) vec4<f32> {
    let p = vec2<i32>(in.pos.xy);
    let mirror = saturate(1.0 - textureLoad(color_tex, p, 0).a);
    if mirror < 0.02 {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    let d = depth_at(p);
    if d <= 0.0 {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    let c = world_at(p);
    let l = world_at(p - vec2<i32>(1, 0));
    let r = world_at(p + vec2<i32>(1, 0));
    let u = world_at(p - vec2<i32>(0, 1));
    let b = world_at(p + vec2<i32>(0, 1));
    let dx = select(c - l, r - c, distance(r, c) < distance(l, c));
    let dy = select(c - u, b - c, distance(b, c) < distance(u, c));
    var n = normalize(cross(dy, dx));
    let v = normalize(c - view.world_position);
    if dot(n, v) > 0.0 {
        n = -n;
    }
    let dir = reflect(v, n);
    let reach = ssr.params.x;
    let steps = max(i32(ssr.params.y), 1);
    let jitter = ign(in.pos.xy);
    var prev_t = 0.0;
    var hit = false;
    var hit_uv = vec2<f32>(0.0);
    var hit_t = 0.0;
    for (var i = 0; i < steps; i++) {
        // Denser steps near the surface.
        let k = (f32(i) + jitter) / f32(steps);
        let t = reach * k * k + 2.0;
        let s = c + dir * t;
        let clip = view.clip_from_world * vec4<f32>(s, 1.0);
        if clip.w <= 0.0 {
            break;
        }
        let ndc = clip.xy / clip.w;
        if abs(ndc.x) >= 1.0 || abs(ndc.y) >= 1.0 {
            break;
        }
        let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
        let sp = vec2<i32>(uv * vec2<f32>(size()));
        let sd = depth_at(sp);
        let behind = view_depth(s) - select(1e9, view_depth(world_from(vec2<f32>(sp) + 0.5, sd)), sd > 0.0);
        let thickness = max(6.0, (t - prev_t) * 1.5);
        if behind > 0.0 && behind < thickness {
            // Refine between the last two steps.
            var lo = prev_t;
            var hi = t;
            for (var j = 0; j < 5; j++) {
                let mid = (lo + hi) * 0.5;
                let ms = c + dir * mid;
                let mc = view.clip_from_world * vec4<f32>(ms, 1.0);
                let muv = vec2<f32>(mc.x / mc.w * 0.5 + 0.5, 0.5 - mc.y / mc.w * 0.5);
                let mp = vec2<i32>(muv * vec2<f32>(size()));
                let md = depth_at(mp);
                if md > 0.0 && view_depth(ms) > view_depth(world_from(vec2<f32>(mp) + 0.5, md)) {
                    hi = mid;
                } else {
                    lo = mid;
                }
            }
            let fs = c + dir * hi;
            let fc = view.clip_from_world * vec4<f32>(fs, 1.0);
            hit_uv = vec2<f32>(fc.x / fc.w * 0.5 + 0.5, 0.5 - fc.y / fc.w * 0.5);
            hit_t = hi;
            hit = true;
            break;
        }
        prev_t = t;
    }
    if !hit {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    let edge = min(min(hit_uv.x, 1.0 - hit_uv.x), min(hit_uv.y, 1.0 - hit_uv.y));
    let fade = smoothstep(0.0, 0.08, edge) * (1.0 - smoothstep(0.6, 1.0, hit_t / reach));
    let n_v = saturate(dot(n, -v));
    let fresnel = 0.02 + 0.98 * pow(1.0 - n_v, 5.0);
    let seen = textureSampleLevel(color_tex, color_samp, hit_uv, 0.0).rgb;
    return vec4<f32>(seen * (fresnel * mirror * fade * ssr.params.z), 1.0);
}
