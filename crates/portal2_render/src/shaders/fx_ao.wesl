// Ambient occlusion for the overhaul (see visuals/fx/screen.rs): contact
// darkening where surfaces meet (debris on the floor, props against walls),
// from the main view's depth after the opaque pass. The raw pass samples a
// small normal-oriented hemisphere per pixel; the apply pass blurs it
// within surfaces and multiplies it into the multisampled scene, before
// glass, beams and particles are drawn.

import bevy_render::view::View;

@group(0) @binding(0) var depth_tex: texture_depth_multisampled_2d;
@group(0) @binding(1) var<uniform> view: View;
@group(0) @binding(2) var<uniform> ao: FxAo;
@group(0) @binding(3) var raw_tex: texture_2d<f32>;

struct FxAo {
    // x: radius (units), y: strength, z: fade-out distance, w: samples
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

// Distance along the view direction (positive in front).
fn view_depth(w: vec3<f32>) -> f32 {
    return -(view.view_from_world * vec4<f32>(w, 1.0)).z;
}

// Interleaved gradient noise, fixed per pixel (no temporal filter here).
fn ign(p: vec2<f32>) -> f32 {
    return fract(52.9829189 * fract(dot(p, vec2<f32>(0.06711056, 0.00583715))));
}

@fragment
fn ao_raw(in: FsIn) -> @location(0) vec4<f32> {
    let p = vec2<i32>(in.pos.xy);
    let d = depth_at(p);
    // Sky and anything at the far plane.
    if d <= 0.0 {
        return vec4<f32>(1.0);
    }
    let c = world_at(p);
    // The normal from whichever neighbours lie on the same surface.
    let l = world_at(p - vec2<i32>(1, 0));
    let r = world_at(p + vec2<i32>(1, 0));
    let u = world_at(p - vec2<i32>(0, 1));
    let b = world_at(p + vec2<i32>(0, 1));
    let dx = select(c - l, r - c, distance(r, c) < distance(l, c));
    let dy = select(c - u, b - c, distance(b, c) < distance(u, c));
    var n = normalize(cross(dy, dx));
    let to_eye = view.world_position - c;
    if dot(n, to_eye) < 0.0 {
        n = -n;
    }
    let eye_dist = length(to_eye);
    let fade = 1.0 - smoothstep(ao.params.z * 0.6, ao.params.z, eye_dist);
    if fade <= 0.0 {
        return vec4<f32>(1.0);
    }
    // A basis around the normal, turned per pixel.
    let t0 = normalize(select(cross(n, vec3<f32>(0.0, 0.0, 1.0)), cross(n, vec3<f32>(1.0, 0.0, 0.0)), abs(n.z) > 0.9));
    let b0 = cross(n, t0);
    let angle = ign(in.pos.xy) * 6.2831853;
    let ca = cos(angle);
    let sa = sin(angle);
    let t = t0 * ca + b0 * sa;
    let bt = -t0 * sa + b0 * ca;
    let radius = ao.params.x;
    let count = i32(ao.params.w);
    var occlusion = 0.0;
    for (var i = 0; i < count; i++) {
        let fi = f32(i);
        // Golden-angle spiral over the hemisphere, denser near the centre.
        let k = (fi + 0.5) / f32(count);
        let phi = fi * 2.3999632;
        let st = sqrt(k);
        let dir = t * (cos(phi) * st) + bt * (sin(phi) * st) + n * sqrt(1.0 - k);
        let s = c + dir * (radius * mix(0.15, 1.0, k * k));
        let clip = view.clip_from_world * vec4<f32>(s, 1.0);
        if clip.w <= 0.0 {
            continue;
        }
        let ndc = clip.xy / clip.w;
        let sp = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5) * vec2<f32>(size());
        let sd = depth_at(vec2<i32>(sp));
        if sd <= 0.0 {
            continue;
        }
        let scene = world_from(floor(sp) + 0.5, sd);
        let gap = view_depth(s) - view_depth(scene);
        // Occluded when the scene is in front of the sample, but not when
        // it is far in front (a different object).
        let range = smoothstep(0.0, 1.0, radius / max(abs(gap), 1e-3));
        occlusion += select(0.0, 1.0, gap > 1.0) * range;
    }
    let visible = 1.0 - occlusion / f32(count) * fade;
    return vec4<f32>(visible, 0.0, 0.0, 1.0);
}

// 4x4 blur over the raw result, weighted toward the same surface, then
// multiplied into the scene by the blend state.
@fragment
fn ao_apply(in: FsIn) -> @location(0) vec4<f32> {
    let p = vec2<i32>(in.pos.xy);
    let d = depth_at(p);
    if d <= 0.0 {
        return vec4<f32>(1.0);
    }
    let center = view_depth(world_at(p));
    var sum = 0.0;
    var total = 0.0;
    for (var y = -2; y <= 1; y++) {
        for (var x = -2; x <= 1; x++) {
            let q = clamp(p + vec2<i32>(x, y), vec2<i32>(0), size() - 1);
            let qd = view_depth(world_at(q));
            let w = 1.0 / (1.0 + abs(qd - center) * 0.5);
            sum += textureLoad(raw_tex, q, 0).r * w;
            total += w;
        }
    }
    let visible = sum / max(total, 1e-4);
    let m = mix(1.0, visible, ao.params.y);
    return vec4<f32>(m, m, m, 1.0);
}
