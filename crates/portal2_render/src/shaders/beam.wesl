// Spotlight beams: the cone texture on a quad hanging from the light along
// its forward axis, turned about that axis to face the view, added to the
// scene and faded as the view looks along the beam (the `LampBeam` proxy).

import bevy_pbr::render::mesh_view_bindings::view;
import bevy_pbr::render::mesh_view_bindings::fog;
import bevy_pbr::render::mesh_functions::get_world_from_local;

struct BeamParams {
    // rgb: colour (linear, brightness included)
    color: vec4<f32>,
    // x: length, y: half width at the far end
    shape: vec4<f32>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> beam: BeamParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var base_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var base_samp: sampler;

struct Vertex {
    @builtin(instance_index) instance_index: u32,
    // x: 0 at the start, 1 at the end; y: side (-1 or 1)
    @location(0) position: vec3<f32>,
}

struct BeamVertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) world_position: vec3<f32>,
    @location(2) fade: f32,
}

@vertex
fn vertex(v: Vertex) -> BeamVertexOutput {
    let world_from_local = get_world_from_local(v.instance_index);
    let start = (world_from_local * vec4<f32>(0.0, 0.0, 0.0, 1.0)).xyz;
    let forward = normalize(world_from_local[0].xyz);
    let along = start + forward * (beam.shape.x * v.position.x);
    let to_view = normalize(view.world_position - along);
    var across = cross(forward, to_view);
    if dot(across, across) < 1e-8 {
        across = cross(forward, vec3<f32>(0.0, 0.0, 1.0));
    }
    across = normalize(across);
    let p = along + across * (beam.shape.y * v.position.y);
    // Seen along its axis the beam fades away.
    let middle = start + forward * (beam.shape.x * 0.5);
    let side_on = 1.0 - abs(dot(normalize(view.world_position - middle), forward));
    var out: BeamVertexOutput;
    out.clip_position = view.clip_from_world * vec4<f32>(p, 1.0);
    out.uv = vec2<f32>(v.position.y * 0.5 + 0.5, v.position.x);
    out.world_position = p;
    out.fade = smoothstep(0.0, 0.35, side_on);
    return out;
}

@fragment
fn fragment(in: BeamVertexOutput) -> @location(0) vec4<f32> {
    if fog.mode == 2u {
        discard;
    }
    let tex = textureSample(base_tex, base_samp, in.uv);
    var c = tex.rgb * beam.color.rgb * in.fade * view.exposure;
    if fog.mode == 1u {
        let forward = -view.world_from_view[2].xyz;
        let depth = dot(in.world_position - view.world_position, forward);
        let f = saturate(min(fog.directional_light_exponent, (depth - fog.be.x) / max(fog.be.y - fog.be.x, 1.0)));
        c = c * (1.0 - f);
    }
    // Premultiplied blending with zero alpha adds the colour.
    return vec4<f32>(c, 0.0);
}
