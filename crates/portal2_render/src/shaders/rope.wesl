// Ropes and cables: a strip along the rope's spline, turned to face the
// view in the vertex shader and never thinner than two pixels.

import bevy_pbr::render::mesh_view_bindings::view;
import bevy_pbr::render::mesh_view_bindings::fog;

struct RopeParams {
    // x: minimum width in pixels
    params: vec4<f32>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> rope: RopeParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var base_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var base_samp: sampler;

struct Vertex {
    @location(0) position: vec3<f32>,
    // Direction of the rope at this point.
    @location(1) direction: vec3<f32>,
    // x: side (-1 or 1), y: texture v
    @location(2) uv: vec2<f32>,
    // x: half width
    @location(3) size: vec2<f32>,
    // rgb: light at the rope
    @location(5) color: vec4<f32>,
}

struct RopeVertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec3<f32>,
    @location(2) world_position: vec3<f32>,
}

@vertex
fn vertex(v: Vertex) -> RopeVertexOutput {
    let to_view = normalize(view.world_position - v.position);
    var across = cross(normalize(v.direction), to_view);
    if dot(across, across) < 1e-8 {
        across = cross(to_view, vec3<f32>(0.0, 0.0, 1.0));
    }
    across = normalize(across);
    // World size of one pixel at this depth.
    let centre = view.clip_from_world * vec4<f32>(v.position, 1.0);
    let pixel = 2.0 * max(centre.w, 1e-3) / (view.clip_from_view[1][1] * view.viewport.w);
    let half_width = max(v.size.x, 0.5 * rope.params.x * pixel);
    let p = v.position + across * (half_width * v.uv.x);
    var out: RopeVertexOutput;
    out.clip_position = view.clip_from_world * vec4<f32>(p, 1.0);
    out.uv = vec2<f32>(v.uv.x * 0.5 + 0.5, v.uv.y);
    out.color = v.color.rgb;
    out.world_position = p;
    return out;
}

@fragment
fn fragment(in: RopeVertexOutput) -> @location(0) vec4<f32> {
    // Ropes do not show in projected-texture shadow maps.
    if fog.mode == 2u {
        discard;
    }
    let base = textureSample(base_tex, base_samp, in.uv);
    var color = base.rgb * in.color * view.exposure;
    if fog.mode == 1u {
        let forward = -view.world_from_view[2].xyz;
        let depth = dot(in.world_position - view.world_position, forward);
        let f = saturate(min(fog.directional_light_exponent, (depth - fog.be.x) / max(fog.be.y - fog.be.x, 1.0)));
        color = mix(color, fog.base_color.rgb, f);
    }
    return vec4<f32>(color, 1.0);
}
