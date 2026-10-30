// Studio model surfaces: base texture lit either by baked per-vertex lighting
// (static props) or by an ambient cube plus a few local lights (dynamic
// models), with optional bump, phong specular, cube map and self-illumination.

import bevy_pbr::render::forward_io::VertexOutput;
import bevy_pbr::render::mesh_view_bindings::view;
import bevy_pbr::render::mesh_view_bindings::fog;
import portal2_render::shaders::projected::{is_shadow_view, projected_lighting};

struct ModelLight {
    // xyz: position, w: kind (0 point, 1 spot, 2 directional)
    position: vec4<f32>,
    // rgb: intensity (linear), w: spot cone falloff exponent
    color: vec4<f32>,
    // xyz: direction (spot / directional), w: cos inner cone
    direction: vec4<f32>,
    // x: constant, y: linear, z: quadratic attenuation, w: cos outer cone
    attenuation: vec4<f32>,
}

struct ModelParams {
    color: vec4<f32>,
    envmap_tint: vec4<f32>,
    selfillum_tint: vec4<f32>,
    // x: exponent (0 = exponent texture), y: boost
    phong: vec4<f32>,
    // xyz: Fresnel ranges (facing, middle, edge-on), w: share applied to cube map reflections
    fresnel: vec4<f32>,
    // rgb: specular tint
    phong_tint: vec4<f32>,
    // rgb: 0 = reflection as is, 1 = squared (not phong)
    envmap_contrast: vec4<f32>,
    // rgb: 0 = grey, 1 = as is (not phong)
    envmap_saturation: vec4<f32>,
    ambient: array<vec4<f32>, 6>,
    lights: array<ModelLight, 4>,
    flags: u32,
    alpha_ref: f32,
    num_lights: u32,
    _pad: f32,
}

const FLAG_BUMP: u32 = 1u;
const FLAG_ENVMAP: u32 = 2u;
const FLAG_ENVMAPMASK: u32 = 4u;
const FLAG_BASEALPHA_ENVMAPMASK: u32 = 8u;
const FLAG_NORMALALPHA_ENVMAPMASK: u32 = 16u;
const FLAG_SELFILLUM: u32 = 32u;
const FLAG_ALPHATEST: u32 = 64u;
const FLAG_UNLIT: u32 = 128u;
const FLAG_PHONG: u32 = 256u;
const FLAG_BAKED: u32 = 512u;
const FLAG_TRANSLUCENT: u32 = 1024u;
const FLAG_BASEMAPALPHA_PHONGMASK: u32 = 2048u;
const FLAG_VERTEX_COLOR: u32 = 4096u;
const FLAG_EXPONENT_TEXTURE: u32 = 8192u;
const FLAG_ALBEDO_TINT: u32 = 16384u;
const FLAG_HALF_LAMBERT: u32 = 32768u;
const FLAG_LIGHTWARP: u32 = 65536u;
const FLAG_SELFILLUM_MASK: u32 = 131072u;
const FLAG_BASEMAPLUMINANCE_PHONGMASK: u32 = 262144u;
const FLAG_INVERT_PHONG_ENVMAPMASK: u32 = 524288u;
const FLAG_ADDITIVE: u32 = 1048576u;

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> material: ModelParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var base_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var base_samp: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(3) var bump_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(4) var bump_samp: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(5) var envmap_tex: texture_cube<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(6) var envmap_samp: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(7) var envmask_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(8) var envmask_samp: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(9) var exponent_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(10) var exponent_samp: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(11) var selfillum_mask_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(12) var selfillum_mask_samp: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(13) var lightwarp_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(14) var lightwarp_samp: sampler;

fn has(flag: u32) -> bool {
    return (material.flags & flag) != 0u;
}

fn ambient_cube(n: vec3<f32>) -> vec3<f32> {
    let sq = n * n;
    let neg = vec3<u32>(select(0u, 1u, n.x < 0.0), select(0u, 1u, n.y < 0.0), select(0u, 1u, n.z < 0.0));
    return sq.x * material.ambient[neg.x].rgb
        + sq.y * material.ambient[2u + neg.y].rgb
        + sq.z * material.ambient[4u + neg.z].rgb;
}

fn light_falloff(light: ModelLight, p: vec3<f32>) -> vec4<f32> {
    // Returns (direction to light, attenuation).
    let kind = u32(light.position.w);
    if kind == 2u {
        return vec4<f32>(-light.direction.xyz, 1.0);
    }
    let d = light.position.xyz - p;
    let dist = max(length(d), 0.001);
    let l = d / dist;
    let a = light.attenuation;
    var atten = 1.0 / max(a.x + a.y * dist + a.z * dist * dist, 1e-6);
    if kind == 1u {
        // Linear across the cone edge, shaped by the falloff exponent.
        let cos_angle = dot(-l, light.direction.xyz);
        let inner = light.direction.w;
        let outer = a.w;
        let edge = max((cos_angle - outer) / max(inner - outer, 1e-4), 1e-4);
        atten = atten * min(pow(edge, light.color.w), 1.0);
    }
    return vec4<f32>(l, atten);
}

// Phong Fresnel term: (1 - N·V)² picks a point on two lines, from the facing
// value to the middle one and from the middle one to the edge-on value.
fn fresnel_ranges(n_dot_v: f32) -> f32 {
    let f = (1.0 - n_dot_v) * (1.0 - n_dot_v);
    let r = material.fresnel.xyz;
    if f <= 0.5 {
        return mix(r.x, r.y, 2.0 * f);
    }
    return mix(r.y, r.z, 2.0 * f - 1.0);
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
    let base = textureSample(base_tex, base_samp, uv);
    var albedo = base.rgb * material.color.rgb;
    var alpha = material.color.a;
    if has(FLAG_TRANSLUCENT) || has(FLAG_ALPHATEST) {
        alpha = alpha * base.a;
    }
    let tested_alpha = alpha;
    // `$allowalphatocoverage` materials drawn with MSAA turn alpha into
    // sample coverage instead of testing it (the game's alpha to coverage).
    // Texture alpha fades over several texels (more in the smaller mips);
    // used as coverage as it is, that fringe would dither the background
    // through the outline of every leaf. Coverage instead ramps over about
    // a pixel around the test reference, so edges are smoothed and the rest
    // is solid as with the test.
    @if(ALPHA_TO_COVERAGE) {
        let ramp = max(fwidth(alpha), 1e-4);
        if has(FLAG_ALPHATEST) {
            alpha = saturate((alpha - material.alpha_ref) / ramp + 0.5);
        }
    }
    @if(!ALPHA_TO_COVERAGE) {
        if has(FLAG_ALPHATEST) && alpha < material.alpha_ref {
            discard;
        }
    }
    if is_shadow_view() {
        // Shadow maps hold the distance to the nearest opaque surface.
        if has(FLAG_TRANSLUCENT) || (has(FLAG_ALPHATEST) && tested_alpha < material.alpha_ref) {
            discard;
        }
        return vec4<f32>(distance(view.world_position, in.world_position.xyz), 0.0, 0.0, 1.0);
    }

    var n = normalize(in.world_normal);
    if !is_front {
        n = -n;
    }
    var bump = vec4<f32>(0.5, 0.5, 1.0, 1.0);
    @if(VERTEX_TANGENTS) {
        if has(FLAG_BUMP) {
            bump = textureSample(bump_tex, bump_samp, uv);
            let tn = normalize(bump.xyz * 2.0 - 1.0);
            let t = normalize(in.world_tangent.xyz);
            let b = cross(n, t) * in.world_tangent.w;
            n = normalize(t * tn.x + b * tn.y + n * tn.z);
        }
    }
    var exponent_map = vec4<f32>(1.0);
    if has(FLAG_EXPONENT_TEXTURE) {
        exponent_map = textureSample(exponent_tex, exponent_samp, uv);
    }
    // A constant exponent wins over the map, whose red spans 1..150.
    var exponent = material.phong.x;
    if exponent <= 0.0 {
        exponent = 1.0 + 149.0 * exponent_map.r;
    }

    let p = in.world_position.xyz;
    let v = normalize(view.world_position - p);
    let n_dot_v = saturate(dot(n, v));
    var diffuse = vec3<f32>(0.0);
    var specular = vec3<f32>(0.0);
    if has(FLAG_UNLIT) {
        diffuse = vec3<f32>(1.0);
    } else if has(FLAG_BAKED) {
        @if(VERTEX_COLORS) {
            diffuse = in.color.rgb;
        }
    } else {
        diffuse = ambient_cube(n);
        let r = reflect(-v, n);
        for (var i = 0u; i < material.num_lights; i = i + 1u) {
            let light = material.lights[i];
            let la = light_falloff(light, p);
            let lit = light.color.rgb * la.w;
            let n_dot_l = dot(n, la.xyz);
            var shade = saturate(n_dot_l);
            if has(FLAG_HALF_LAMBERT) {
                shade = saturate(n_dot_l * 0.5 + 0.5);
            }
            if has(FLAG_LIGHTWARP) {
                diffuse = diffuse + lit * 2.0 * textureSample(lightwarp_tex, lightwarp_samp, vec2<f32>(shade, shade)).rgb;
            } else if has(FLAG_HALF_LAMBERT) {
                diffuse = diffuse + lit * shade * shade;
            } else {
                diffuse = diffuse + lit * shade;
            }
            if has(FLAG_PHONG) {
                let s = pow(saturate(dot(r, la.xyz)), exponent);
                specular = specular + lit * s * saturate(n_dot_l);
            }
        }
    }
    if has(FLAG_VERTEX_COLOR) && !has(FLAG_BAKED) {
        @if(VERTEX_COLORS) {
            albedo = albedo * in.color.rgb;
        }
    }

    var color = albedo * diffuse;
    if !has(FLAG_UNLIT) {
        color = color + albedo * projected_lighting(p, n);
    }
    if has(FLAG_SELFILLUM) {
        var mask = vec3<f32>(base.a);
        if has(FLAG_SELFILLUM_MASK) {
            mask = textureSample(selfillum_mask_tex, selfillum_mask_samp, uv).rgb;
        }
        color = mix(color, base.rgb * material.selfillum_tint.rgb, mask);
    }
    var fresnel = 1.0;
    var phong_mask = bump.a;
    if has(FLAG_PHONG) {
        if has(FLAG_BASEMAPALPHA_PHONGMASK) {
            phong_mask = base.a;
        }
        if has(FLAG_BASEMAPLUMINANCE_PHONGMASK) {
            phong_mask = dot(base.rgb, vec3<f32>(0.3, 0.59, 0.11));
        }
        var tint = material.phong_tint.rgb;
        if has(FLAG_ALBEDO_TINT) {
            tint = mix(vec3<f32>(1.0), base.rgb, exponent_map.g);
        }
        fresnel = fresnel_ranges(n_dot_v);
        color = color + specular * material.phong.y * phong_mask * tint * fresnel;
    }
    if has(FLAG_ENVMAP) {
        let r = reflect(-v, n);
        var spec = textureSample(envmap_tex, envmap_samp, r).rgb * material.envmap_tint.rgb;
        var mask = vec3<f32>(1.0);
        if has(FLAG_PHONG) {
            // Phong materials mask reflections by base alpha, or with
            // `$normalmapalphaenvmapmask` like their highlights, optionally
            // inverted; they have no mask texture. They can share the Fresnel
            // ranges.
            var m = base.a;
            if has(FLAG_NORMALALPHA_ENVMAPMASK) {
                m = phong_mask;
            }
            if has(FLAG_INVERT_PHONG_ENVMAPMASK) {
                m = 1.0 - m;
            }
            mask = vec3<f32>(m);
            spec = spec * mix(1.0, fresnel, material.fresnel.w);
        } else {
            // The rest mask first (the mask texture's red channel), then
            // apply contrast and saturation to the tinted reflection.
            var m = 1.0;
            if has(FLAG_ENVMAPMASK) {
                m = textureSample(envmask_tex, envmask_samp, uv).r;
            }
            if has(FLAG_BASEALPHA_ENVMAPMASK) {
                m = base.a;
            }
            if has(FLAG_NORMALALPHA_ENVMAPMASK) {
                m = bump.a;
            }
            spec = spec * m;
            spec = mix(spec, spec * spec, material.envmap_contrast.rgb);
            let grey = dot(spec, vec3<f32>(0.299, 0.587, 0.114));
            spec = mix(vec3<f32>(grey), spec, material.envmap_saturation.rgb);
        }
        color = color + spec * mask;
    }
    if has(FLAG_ADDITIVE) {
        // Premultiplied blending: added in full, fogging toward black.
        return vec4<f32>(fade_with_fog(color * view.exposure * alpha, in.world_position.xyz), 0.0);
    }
    return vec4<f32>(apply_source_fog(color * view.exposure, in.world_position.xyz), alpha);
}

// Additive surfaces fade out in fog instead of taking on its colour.
fn fade_with_fog(color: vec3<f32>, world_position: vec3<f32>) -> vec3<f32> {
    if fog.mode != 1u {
        return color;
    }
    let forward = -view.world_from_view[2].xyz;
    let depth = dot(world_position - view.world_position, forward);
    let f = saturate(min(fog.directional_light_exponent, (depth - fog.be.x) / max(fog.be.y - fog.be.x, 1.0)));
    return color * (1.0 - f);
}
