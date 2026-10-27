// Brush surfaces: base texture(s) lit by baked lightmaps, with optional
// tangent-space bump mapping against three directional lightmaps, cube map
// reflections, detail textures and self-illumination.

import bevy_pbr::render::forward_io::VertexOutput;
import bevy_pbr::render::mesh_view_bindings::view;
import bevy_pbr::render::mesh_view_bindings::fog;

struct WorldParams {
    color: vec4<f32>,
    envmap_tint: vec4<f32>,
    selfillum_tint: vec4<f32>,
    // x: detail scale, y: detail blend factor, z: detail blend mode
    detail: vec4<f32>,
    flags: u32,
    alpha_ref: f32,
    lightmap_scale: f32,
    envmap_contrast: f32,
    envmap_saturation: f32,
    envmap_light_scale: f32,
    _pad1: f32,
    _pad2: f32,
}

const FLAG_BASE2: u32 = 1u;
const FLAG_BUMP: u32 = 2u;
const FLAG_SSBUMP: u32 = 4u;
const FLAG_ENVMAP: u32 = 8u;
const FLAG_ENVMAPMASK: u32 = 16u;
const FLAG_BASEALPHA_ENVMAPMASK: u32 = 32u;
const FLAG_NORMALALPHA_ENVMAPMASK: u32 = 64u;
const FLAG_SELFILLUM: u32 = 128u;
const FLAG_ALPHATEST: u32 = 256u;
const FLAG_UNLIT: u32 = 512u;
const FLAG_DETAIL: u32 = 1024u;
const FLAG_LIGHTMAPPED: u32 = 2048u;
const FLAG_TRANSLUCENT: u32 = 4096u;
const FLAG_VERTEX_BLEND: u32 = 8192u;

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> material: WorldParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var base_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var base_samp: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(3) var base2_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(4) var base2_samp: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(5) var bump_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(6) var bump_samp: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(7) var lightmap_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(8) var lightmap_samp: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(9) var envmap_tex: texture_cube<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(10) var envmap_samp: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(11) var envmask_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(12) var envmask_samp: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(13) var detail_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(14) var detail_samp: sampler;

// Tangent-space basis used for directional lightmaps (x: S, y: T, z: normal).
const BUMP_BASIS_0 = vec3<f32>(0.81649658, 0.0, 0.57735027);
const BUMP_BASIS_1 = vec3<f32>(-0.40824829, 0.70710678, 0.57735027);
const BUMP_BASIS_2 = vec3<f32>(-0.40824829, -0.70710678, 0.57735027);

fn has(flag: u32) -> bool {
    return (material.flags & flag) != 0u;
}

fn luminance(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2125, 0.7154, 0.0721));
}

fn apply_detail(albedo: vec3<f32>, detail: vec4<f32>) -> vec3<f32> {
    let mode = u32(material.detail.z);
    let f = material.detail.y;
    switch mode {
        case 1u: { return albedo + detail.rgb * f; }
        case 7u, 8u: { return mix(albedo, albedo * detail.rgb * 2.0, f); }
        default: { return mix(albedo, albedo * detail.rgb * 2.0, f); }
    }
}

// Source-style distance fog by view depth: min(max density, (depth - start)
// / (end - start)), clamped. The maximum density rides in the fog uniform's
// `directional_light_exponent` (see render/fog.rs).
fn apply_source_fog(color: vec3<f32>, world_position: vec3<f32>) -> vec3<f32> {
    if fog.mode != 1u {
        return color;
    }
    let forward = -view.world_from_view[2].xyz;
    let depth = dot(world_position - view.world_position, forward);
    let f = saturate(min(fog.directional_light_exponent, (depth - fog.be.x) / max(fog.be.y - fog.be.x, 1.0)));
    return mix(color, fog.base_color.rgb, f);
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> @location(0) vec4<f32> {
    var uv = vec2<f32>(0.0);
    @if(VERTEX_UVS_A) {
        uv = in.uv;
    }
    var base = textureSample(base_tex, base_samp, uv);
    var blend = 0.0;
    @if(VERTEX_COLORS) {
        blend = in.color.a;
    }
    if has(FLAG_VERTEX_BLEND) {
        let base2 = textureSample(base2_tex, base2_samp, uv);
        base = mix(base, base2, blend);
    }
    var albedo = base.rgb * material.color.rgb;
    if has(FLAG_DETAIL) {
        albedo = apply_detail(albedo, textureSample(detail_tex, detail_samp, uv * material.detail.x));
    }
    var alpha = material.color.a;
    if has(FLAG_TRANSLUCENT) || has(FLAG_ALPHATEST) {
        alpha = alpha * base.a;
    }
    if has(FLAG_ALPHATEST) && alpha < material.alpha_ref {
        discard;
    }

    var n = normalize(in.world_normal);
    if !is_front {
        n = -n;
    }
    var bump = vec4<f32>(0.5, 0.5, 1.0, 1.0);
    var tangent_normal = vec3<f32>(0.0, 0.0, 1.0);
    if has(FLAG_BUMP) {
        bump = textureSample(bump_tex, bump_samp, uv);
        tangent_normal = normalize(bump.xyz * 2.0 - 1.0);
    }

    var diffuse = vec3<f32>(1.0);
    if has(FLAG_LIGHTMAPPED) && !has(FLAG_UNLIT) {
        var lm_uv = vec2<f32>(0.0);
        @if(VERTEX_UVS_B) {
            lm_uv = in.uv_b;
        }
        var step = 0.0;
        @if(VERTEX_COLORS) {
            step = in.color.r;
        }
        if has(FLAG_BUMP) && step > 0.0 {
            let l1 = textureSampleLevel(lightmap_tex, lightmap_samp, lm_uv + vec2<f32>(step, 0.0), 0.0).rgb;
            let l2 = textureSampleLevel(lightmap_tex, lightmap_samp, lm_uv + vec2<f32>(step * 2.0, 0.0), 0.0).rgb;
            let l3 = textureSampleLevel(lightmap_tex, lightmap_samp, lm_uv + vec2<f32>(step * 3.0, 0.0), 0.0).rgb;
            if has(FLAG_SSBUMP) {
                diffuse = bump.x * l1 + bump.y * l2 + bump.z * l3;
            } else {
                var dp = vec3<f32>(
                    saturate(dot(tangent_normal, BUMP_BASIS_0)),
                    saturate(dot(tangent_normal, BUMP_BASIS_1)),
                    saturate(dot(tangent_normal, BUMP_BASIS_2)),
                );
                dp = dp * dp;
                let sum = max(dp.x + dp.y + dp.z, 1e-4);
                diffuse = (dp.x * l1 + dp.y * l2 + dp.z * l3) / sum;
            }
        } else {
            diffuse = textureSampleLevel(lightmap_tex, lightmap_samp, lm_uv, 0.0).rgb;
        }
        diffuse = diffuse * material.lightmap_scale;
    }

    var color = albedo * diffuse;
    if has(FLAG_UNLIT) {
        color = albedo;
    }

    if has(FLAG_ENVMAP) {
        var wn = n;
        @if(VERTEX_TANGENTS) {
            if has(FLAG_BUMP) && !has(FLAG_SSBUMP) {
                let t = normalize(in.world_tangent.xyz);
                let b = cross(n, t) * in.world_tangent.w;
                wn = normalize(t * tangent_normal.x + b * tangent_normal.y + n * tangent_normal.z);
            }
        }
        let v = normalize(view.world_position - in.world_position.xyz);
        let r = reflect(-v, wn);
        var spec = textureSample(envmap_tex, envmap_samp, r).rgb;
        let lum = luminance(spec);
        spec = mix(vec3<f32>(lum), spec, material.envmap_saturation);
        spec = mix(spec, spec * spec, material.envmap_contrast);
        var mask = vec3<f32>(1.0);
        if has(FLAG_ENVMAPMASK) {
            mask = textureSample(envmask_tex, envmask_samp, uv).rgb;
        }
        if has(FLAG_BASEALPHA_ENVMAPMASK) {
            mask = vec3<f32>(base.a);
        }
        if has(FLAG_NORMALALPHA_ENVMAPMASK) {
            mask = vec3<f32>(bump.a);
        }
        // Reflections fade with the baked lighting so dark corners stay dark.
        spec = spec * mix(vec3<f32>(1.0), diffuse, saturate(material.envmap_light_scale));
        color = color + spec * material.envmap_tint.rgb * mask;
    }

    if has(FLAG_SELFILLUM) {
        color = mix(color, albedo * material.selfillum_tint.rgb, base.a);
    }

    return vec4<f32>(apply_source_fog(color * view.exposure, in.world_position.xyz), alpha);
}
