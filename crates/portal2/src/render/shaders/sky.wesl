// The 2D sky box: one textured quad per side, centred on whichever view
// draws it and pushed to infinite depth so every surface lands in front.

import bevy_pbr::render::mesh_view_bindings::view;
import bevy_pbr::render::mesh_view_bindings::fog;

struct SkyParams {
    // rgb: `$color` (linear), a: unused
    color: vec4<f32>,
    flags: u32,
}

const FLAG_TEXTURE: u32 = 1u;
const FLAG_NOFOG: u32 = 2u;

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> sky: SkyParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var base_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var base_samp: sampler;

struct Vertex {
    @location(0) position: vec3<f32>,
    @location(2) uv: vec2<f32>,
}

struct SkyVertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

// Far beyond any map, still in front of the infinite far plane.
const SKY_DISTANCE: f32 = 1.0e6;

@vertex
fn vertex(v: Vertex) -> SkyVertexOutput {
    // The mesh holds directions, taken relative to the view so the sky is
    // centred on whichever camera draws it.
    let view_position = (view.view_from_world * vec4<f32>(v.position * SKY_DISTANCE, 0.0)).xyz;
    var out: SkyVertexOutput;
    out.clip_position = view.clip_from_view * vec4<f32>(view_position, 1.0);
    out.uv = v.uv;
    return out;
}

@fragment
fn fragment(in: SkyVertexOutput) -> @location(0) vec4<f32> {
    var color = sky.color.rgb;
    if (sky.flags & FLAG_TEXTURE) != 0u {
        color = color * textureSample(base_tex, base_samp, in.uv).rgb;
    }
    color = color * view.exposure;
    // The sky sits beyond any fog end, so it takes the fog's full density.
    if (sky.flags & FLAG_NOFOG) == 0u && fog.mode == 1u {
        color = mix(color, fog.base_color.rgb, saturate(fog.directional_light_exponent));
    }
    return vec4<f32>(color, 1.0);
}
