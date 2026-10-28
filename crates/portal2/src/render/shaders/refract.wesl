// Glass with Source's Refract shader on studio models: the scene already
// drawn behind the surface (Bevy's copy of the opaque pass, looked up in
// screen space) shifted by the normal map and tinted, plus a cube map
// reflection. With `$localrefract` the base texture takes the place of the
// scene, looked up with a parallax offset as if it lay a little behind the
// surface (the lab light covers). Neither part is scaled by exposure: the
// copied scene already is, and the game leaves the base texture unscaled.

import bevy_pbr::render::forward_io::VertexOutput;
import bevy_pbr::render::mesh_view_bindings as view_bindings;
import bevy_pbr::render::mesh_view_bindings::view;
import bevy_pbr::render::mesh_view_bindings::fog;

struct RefractParams {
    // rgb: tint of the refracted image (linear)
    refract_tint: vec4<f32>,
    // rgb: cube map tint (linear)
    envmap_tint: vec4<f32>,
    // rgb: 0 = reflection as is, 1 = squared
    envmap_contrast: vec4<f32>,
    // rgb: 0 = grey, 1 = as is
    envmap_saturation: vec4<f32>,
    // x: refract amount, y: local refract depth, z: base texture height / width
    refract: vec4<f32>,
    flags: u32,
    _pad1: f32,
    _pad2: f32,
    _pad3: f32,
}

const FLAG_LOCAL: u32 = 1u;
const FLAG_NORMALMAP: u32 = 2u;
const FLAG_ENVMAP: u32 = 4u;
const FLAG_BLUR: u32 = 8u;
const FLAG_TINT_TEXTURE: u32 = 16u;

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> material: RefractParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var base_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var base_samp: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(3) var normal_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(4) var normal_samp: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(5) var envmap_tex: texture_cube<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(6) var envmap_samp: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(7) var tint_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(8) var tint_samp: sampler;

fn has(flag: u32) -> bool {
    return (material.flags & flag) != 0u;
}

// The opaque scene behind the surface. Views without a transmission copy see
// black there and keep only the reflection.
@if(VIEW_TRANSMISSION_TEXTURE)
fn scene_behind(uv: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(view_bindings::view_transmission_texture, view_bindings::view_transmission_sampler, uv, 0.0).rgb;
}

@if(!VIEW_TRANSMISSION_TEXTURE)
fn scene_behind(uv: vec2<f32>) -> vec3<f32> {
    return vec3<f32>(0.0);
}

@if(VIEW_TRANSMISSION_TEXTURE)
fn scene_size() -> vec2<f32> {
    return vec2<f32>(textureDimensions(view_bindings::view_transmission_texture));
}

@if(!VIEW_TRANSMISSION_TEXTURE)
fn scene_size() -> vec2<f32> {
    return view.viewport.zw;
}

// The blurred variant takes four taps a fixed 1/1024 of the screen apart
// (whatever the resolution), weighted 4:2:2:1 like a small tent filter.
fn refracted_scene(uv: vec2<f32>) -> vec3<f32> {
    if !has(FLAG_BLUR) {
        return scene_behind(uv);
    }
    let o = 1.0 / 1024.0;
    return scene_behind(uv + vec2<f32>(-o, -o)) * (4.0 / 9.0)
        + scene_behind(uv + vec2<f32>(2.0 * o, -o)) * (2.0 / 9.0)
        + scene_behind(uv + vec2<f32>(-o, 2.0 * o)) * (2.0 / 9.0)
        + scene_behind(uv + vec2<f32>(2.0 * o, 2.0 * o)) * (1.0 / 9.0);
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
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    var uv = vec2<f32>(0.0);
    @if(VERTEX_UVS_A) {
        uv = in.uv;
    }
    // The game's tangent frame: binormal = N × T times the tangent sign. Back
    // faces of double-sided glass keep the front normal, as in the game.
    let n_world = normalize(in.world_normal);
    var t_world = vec3<f32>(1.0, 0.0, 0.0);
    var b_world = vec3<f32>(0.0, 1.0, 0.0);
    @if(VERTEX_TANGENTS) {
        t_world = normalize(in.world_tangent.xyz);
        b_world = normalize(cross(n_world, t_world) * in.world_tangent.w);
    }

    var normal = vec4<f32>(0.5, 0.5, 1.0, 1.0);
    if has(FLAG_NORMALMAP) {
        normal = textureSample(normal_tex, normal_samp, uv);
    }
    // Left unnormalised, as the game does.
    let n = normal.xyz * 2.0 - 1.0;
    let nw = n.x * t_world + n.y * b_world + n.z * n_world;

    let p = in.world_position.xyz;
    let e = normalize(view.world_position - p);
    let e_tangent = vec3<f32>(dot(e, t_world), dot(e, b_world), dot(e, n_world));

    var refracted: vec3<f32>;
    if has(FLAG_LOCAL) {
        // Parallax into an interior `depth` texture heights deep; u is scaled
        // by the texture's height / width so both axes move alike.
        let vz = select(min(e_tangent.z, -1e-4), max(e_tangent.z, 1e-4), e_tangent.z >= 0.0);
        let offset = n.xy - e_tangent.xy * (2.0 - n.z) / vz;
        let scale = material.refract.y * vec2<f32>(material.refract.z, 1.0);
        let interior = textureSample(base_tex, base_samp, saturate(uv + offset * scale));
        let haze = textureSample(base_tex, base_samp, saturate(uv + n.xy * 0.1));
        refracted = mix(interior.rgb, vec3<f32>(haze.a), 0.025) * (n.z * n.z * n.z);
    } else {
        // Screen position shifted by the tangent-space normal, scaled by the
        // normal map's alpha.
        let screen = in.position.xy / scene_size();
        refracted = refracted_scene(screen + n.xy * (normal.a * material.refract.x));
    }

    var tint = material.refract_tint.rgb;
    if has(FLAG_TINT_TEXTURE) {
        tint = tint * 2.0 * textureSample(tint_tex, tint_samp, uv).rgb;
    }

    var spec = vec3<f32>(0.0);
    if has(FLAG_ENVMAP) {
        // The game reflects the tangent-space eye vector about the world-space
        // normal; the mixed spaces are kept so reflections land where its do.
        let r = 2.0 * dot(nw, e_tangent) * nw - dot(nw, nw) * e_tangent;
        spec = textureSample(envmap_tex, envmap_samp, r).rgb * normal.a * material.envmap_tint.rgb;
        spec = mix(spec, spec * spec, material.envmap_contrast.rgb);
        let grey = dot(spec, vec3<f32>(0.299, 0.587, 0.114));
        spec = mix(vec3<f32>(grey), spec, material.envmap_saturation.rgb);
        // Stronger toward grazing angles: 0.6 facing, 1.0 edge-on.
        spec = spec * (0.6 + 0.4 * (1.0 - saturate(dot(n, e_tangent))));
    }

    let color = refracted * tint + spec;
    return vec4<f32>(apply_source_fog(color, p), normal.a);
}
