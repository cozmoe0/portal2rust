// Portal opening: an oval that shows the linked portal's view (rendered to a
// texture with the same projection, so it is sampled in screen space) inside
// a thin coloured ring. An unlinked portal shows a flat tint instead.

import bevy_pbr::render::forward_io::VertexOutput;
import bevy_pbr::render::mesh_view_bindings::view;

struct PortalParams {
    // rgb: ring colour (linear), a: ring brightness
    color: vec4<f32>,
    // x: linked (0/1), y: time, z: opening (0..1), w: unused
    state: vec4<f32>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> portal: PortalParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var view_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var view_samp: sampler;

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    var uv = vec2<f32>(0.5);
    @if(VERTEX_UVS_A) {
        uv = in.uv;
    }
    let p = (uv - vec2<f32>(0.5)) * 2.0;
    let open = clamp(portal.state.z, 0.0, 1.0);
    let r = length(p) / max(open, 0.001);
    if r > 1.0 {
        discard;
    }
    let ring = smoothstep(0.86, 1.0, r);
    var color: vec3<f32>;
    if portal.state.x > 0.5 {
        let screen = in.position.xy / view.viewport.zw;
        color = textureSampleLevel(view_tex, view_samp, screen, 0.0).rgb;
    } else {
        let pulse = 0.5 + 0.5 * sin(portal.state.y * 2.0 + r * 6.0);
        color = portal.color.rgb * (0.15 + 0.1 * pulse) * view.exposure;
    }
    let rim = portal.color.rgb * portal.color.a * view.exposure;
    return vec4<f32>(mix(color, rim, ring), 1.0);
}
