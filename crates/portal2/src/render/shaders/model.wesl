// Studio model surfaces: base texture lit either by baked per-vertex lighting
// (static props) or by an ambient cube plus a few local lights (dynamic
// models), with optional bump, phong specular, cube map and self-illumination.

import bevy_pbr::render::forward_io::VertexOutput;
import bevy_pbr::render::mesh_view_bindings::view;
import bevy_pbr::render::mesh_view_bindings::fog;

struct ModelLight {
    // xyz: position, w: kind (0 point, 1 spot, 2 directional)
    position: vec4<f32>,
    // rgb: intensity (linear), w: unused
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
    // x: exponent, y: boost, z: fresnel min, w: fresnel max
    phong: vec4<f32>,
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

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> material: ModelParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var base_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var base_samp: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(3) var bump_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(4) var bump_samp: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(5) var envmap_tex: texture_cube<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(6) var envmap_samp: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(7) var envmask_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(8) var envmask_samp: sampler;

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
    var atten = 1.0 / max(a.x + a.y * dist + a.z * dist * dist, 1e-4);
    if kind == 1u {
        let cos_angle = dot(-l, light.direction.xyz);
        let inner = light.direction.w;
        let outer = a.w;
        atten = atten * saturate((cos_angle - outer) / max(inner - outer, 1e-4));
    }
    return vec4<f32>(l, atten);
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
    if has(FLAG_ALPHATEST) && alpha < material.alpha_ref {
        discard;
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

    let p = in.world_position.xyz;
    let v = normalize(view.world_position - p);
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
            let ndl = saturate(dot(n, la.xyz));
            diffuse = diffuse + light.color.rgb * ndl * la.w;
            if has(FLAG_PHONG) {
                let s = pow(saturate(dot(r, la.xyz)), max(material.phong.x, 1.0));
                specular = specular + light.color.rgb * s * la.w * ndl;
            }
        }
    }
    if has(FLAG_VERTEX_COLOR) && !has(FLAG_BAKED) {
        @if(VERTEX_COLORS) {
            albedo = albedo * in.color.rgb;
        }
    }

    var color = albedo * diffuse;
    if has(FLAG_PHONG) {
        var mask = bump.a;
        if has(FLAG_BASEMAPALPHA_PHONGMASK) {
            mask = base.a;
        }
        let fresnel = mix(material.phong.z, material.phong.w, pow(1.0 - saturate(dot(n, v)), 5.0));
        color = color + specular * material.phong.y * mask * fresnel;
    }
    if has(FLAG_ENVMAP) {
        let r = reflect(-v, n);
        var spec = textureSample(envmap_tex, envmap_samp, r).rgb;
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
        color = color + spec * material.envmap_tint.rgb * mask;
    }
    if has(FLAG_SELFILLUM) {
        color = mix(color, albedo * material.selfillum_tint.rgb, base.a);
    }
    return vec4<f32>(apply_source_fog(color * view.exposure, in.world_position.xyz), alpha);
}
