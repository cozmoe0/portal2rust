// Light from `env_projectedtexture`, shared by the world and model shaders
// (see render/projected.rs for the model it follows).

import bevy_pbr::render::mesh_view_bindings::fog;

struct ProjectedLight {
    clip_from_world: mat4x4<f32>,
    // xyz: position, w: far distance
    position: vec4<f32>,
    // rgb: colour (linear, scaled), w: share of the light kept in shadow
    color: vec4<f32>,
    // xyz: constant, linear and quadratic attenuation, w: tan(fov / 2)
    atten: vec4<f32>,
    // x: cookie layer, y: shadow map (-1 for none)
    info: vec4<f32>,
}

struct ProjectedLights {
    // x: number of lights
    header: vec4<f32>,
    lights: array<ProjectedLight, 8>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(30) var<storage, read> projected: ProjectedLights;
@group(constants::MATERIAL_BIND_GROUP) @binding(31) var cookie_tex: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(32) var cookie_samp: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(33) var shadow_tex0: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(34) var shadow_tex1: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(35) var shadow_tex2: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(36) var shadow_tex3: texture_2d<f32>;

// Views drawing a shadow map carry exponential fog; they want the
// distance to the light instead of a colour.
fn is_shadow_view() -> bool {
    return fog.mode == 2u;
}

fn shadow_distance(slot: i32, texel: vec2<i32>) -> f32 {
    switch slot {
        case 0: { return textureLoad(shadow_tex0, texel, 0).r; }
        case 1: { return textureLoad(shadow_tex1, texel, 0).r; }
        case 2: { return textureLoad(shadow_tex2, texel, 0).r; }
        default: { return textureLoad(shadow_tex3, texel, 0).r; }
    }
}

// Share of the light reaching `dist` from the light at `uv` in its shadow
// map: 3x3 comparisons, bilinearly weighted.
fn shadow_lit(slot: i32, uv: vec2<f32>, dist: f32, bias: f32) -> f32 {
    let size = vec2<f32>(textureDimensions(shadow_tex0));
    let p = uv * size - 0.5;
    let base = vec2<i32>(floor(p));
    let f = fract(p);
    let max_texel = vec2<i32>(size) - 1;
    var lit = 0.0;
    for (var y = -1; y <= 2; y++) {
        for (var x = -1; x <= 2; x++) {
            let texel = clamp(base + vec2<i32>(x, y), vec2<i32>(0), max_texel);
            let wx = select(select(1.0, f.x, x == 2), 1.0 - f.x, x == -1);
            let wy = select(select(1.0, f.y, y == 2), 1.0 - f.y, y == -1);
            let unshadowed = select(0.0, 1.0, dist - bias <= shadow_distance(slot, texel));
            lit = lit + unshadowed * wx * wy;
        }
    }
    return lit / 9.0;
}

// Projected light arriving at `pos` on a surface facing `n` (without the
// surface's albedo).
fn projected_lighting(pos: vec3<f32>, n: vec3<f32>) -> vec3<f32> {
    var total = vec3<f32>(0.0);
    let count = min(u32(projected.header.x), 8u);
    for (var i = 0u; i < count; i++) {
        let l = projected.lights[i];
        let clip = l.clip_from_world * vec4<f32>(pos, 1.0);
        if clip.w <= 0.0 {
            continue;
        }
        let ndc = clip.xy / clip.w;
        if abs(ndc.x) >= 1.0 || abs(ndc.y) >= 1.0 {
            continue;
        }
        let delta = l.position.xyz - pos;
        let dist = length(delta);
        let far = l.position.w;
        if dist >= far || dist <= 0.0 {
            continue;
        }
        let to_light = delta / dist;
        // Fades out from 60% of the far distance to the far distance.
        let fade = saturate((far - dist) / (0.4 * far));
        let atten = saturate(fade * dot(l.atten.xyz, vec3<f32>(1.0, 1.0 / dist, 1.0 / (dist * dist))));
        let n_dot_l = saturate(dot(to_light, n));
        if atten * n_dot_l <= 0.0 {
            continue;
        }
        let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
        var light = textureSampleLevel(cookie_tex, cookie_samp, uv, i32(l.info.x), 0.0).rgb * (atten * n_dot_l);
        let slot = i32(l.info.y);
        if slot >= 0 {
            // One and a half shadow texels at this distance, more on slopes.
            let texel = 2.0 * dist * l.atten.w / f32(textureDimensions(shadow_tex0).x);
            let slope = sqrt(max(1.0 - n_dot_l * n_dot_l, 0.0)) / max(n_dot_l, 0.2);
            let bias = 1.0 + texel * (1.5 + 2.0 * slope);
            let lit = shadow_lit(slot, uv, dist, bias);
            let attenuated = mix(lit, 1.0, l.color.w);
            light = light * saturate(mix(attenuated, lit, atten));
        }
        total = total + light * l.color.rgb;
    }
    return total;
}
