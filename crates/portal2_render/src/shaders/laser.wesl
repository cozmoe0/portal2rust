// Laser beams as the client draws `env_portal_laser`: a strip from the
// segment's start along its local X (scaled to the segment's length),
// turned about that axis to face the view, the full width across U and the
// texture repeating every 100 units along V (at least half a repeat), as
// the engine's beam segments are. `sprites/purplelaser1` is UnlitTwoTexture:
// the base times the second texture (with its scrolling texture transform)
// times the flickering `$color`, scaled by the exposure and added to the
// scene unfogged.

import bevy_pbr::render::mesh_view_bindings::view;
import bevy_pbr::render::mesh_view_bindings::fog;
import bevy_pbr::render::mesh_view_bindings::globals;
import bevy_pbr::render::mesh_functions::get_world_from_local;

struct LaserParams {
    // rgb: colour (linear), a: 1 to flicker by `$color`'s UniformNoise
    color: vec4<f32>,
    // x: half width, y: noise minimum, z: noise maximum, w: 1 with a second texture
    shape: vec4<f32>,
    // Second texture transform: x scale, y rotation (radians), z offset per
    // second; w: 1 for the visual overhaul's soft halo instead (no texture)
    transform: vec4<f32>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> laser: LaserParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var base_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var base_samp: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(3) var base2_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(4) var base2_samp: sampler;

struct Vertex {
    @builtin(instance_index) instance_index: u32,
    // x: 0 at the start, 1 at the end; y: side (-1 or 1)
    @location(0) position: vec3<f32>,
}

struct LaserVertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vertex(v: Vertex) -> LaserVertexOutput {
    let world_from_local = get_world_from_local(v.instance_index);
    let start = (world_from_local * vec4<f32>(0.0, 0.0, 0.0, 1.0)).xyz;
    let axis = world_from_local[0].xyz;
    let len = max(length(axis), 0.001);
    let forward = axis / len;
    let along = start + axis * v.position.x;
    var across = cross(forward, normalize(view.world_position - along));
    if dot(across, across) < 1e-8 {
        across = cross(forward, vec3<f32>(0.0, 0.0, 1.0));
    }
    across = normalize(across);
    let p = along + across * (laser.shape.x * v.position.y);
    var out: LaserVertexOutput;
    out.clip_position = view.clip_from_world * vec4<f32>(p, 1.0);
    out.uv = vec2<f32>(v.position.y * 0.5 + 0.5, v.position.x * max(len * 0.01, 0.5));
    return out;
}

fn hash(n: u32) -> f32 {
    var x = n * 747796405u + 2891336453u;
    x = ((x >> ((x >> 28u) + 4u)) ^ x) * 277803737u;
    x = (x >> 22u) ^ x;
    return f32(x) / 4294967295.0;
}

@fragment
fn fragment(in: LaserVertexOutput) -> @location(0) vec4<f32> {
    // Effects stay out of projected-texture shadow maps.
    if fog.mode == 2u {
        discard;
    }
    if laser.transform.w > 0.5 {
        // A hot thin core inside a soft halo across the beam, faintly
        // shimmering along it as if through dust.
        let x = (in.uv.x - 0.5) * 2.0;
        let shimmer = 0.85 + 0.15 * sin(in.uv.y * 23.0 - globals.time * 3.0) * sin(in.uv.y * 7.0 + globals.time * 1.3);
        let halo = (exp(-x * x * 5.0) * 0.45 + exp(-x * x * 60.0) * 0.9) * shimmer;
        return vec4<f32>(laser.color.rgb * halo * view.exposure, 0.0);
    }
    var c = textureSample(base_tex, base_samp, in.uv).rgb * laser.color.rgb;
    if laser.shape.w > 0.5 {
        // TextureTransform: scaled and turned about the texture's centre,
        // then offset by the LinearRamp.
        let s = laser.transform.x;
        let r = laser.transform.y;
        let centered = (in.uv - vec2<f32>(0.5)) * s;
        let turned = vec2<f32>(centered.x * cos(r) - centered.y * sin(r), centered.x * sin(r) + centered.y * cos(r));
        let uv2 = turned + vec2<f32>(0.5) + vec2<f32>(globals.time * laser.transform.z);
        c = c * textureSample(base2_tex, base2_samp, uv2).rgb;
    }
    if laser.color.a > 0.5 {
        // UniformNoise picks a new `$color` each frame.
        c = c * mix(laser.shape.y, laser.shape.z, hash(globals.frame_count));
    }
    // Premultiplied blending with zero alpha adds the colour.
    return vec4<f32>(c * view.exposure, 0.0);
}
