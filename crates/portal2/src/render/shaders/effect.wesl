// Simple glowing effects for beams, bridges and funnels: a flat colour with
// soft edges across the mesh's U axis and a texture-free scrolling ripple
// along V. Rendered unlit, added on top of the scene or alpha blended.

import bevy_pbr::render::forward_io::VertexOutput;
import bevy_pbr::render::mesh_view_bindings::view;

struct EffectParams {
    // rgb: colour (linear), a: opacity
    color: vec4<f32>,
    // x: time, y: ripple scroll speed, z: ripple density, w: edge softness
    params: vec4<f32>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> effect: EffectParams;

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    var uv = vec2<f32>(0.5);
    @if(VERTEX_UVS_A) {
        uv = in.uv;
    }
    let edge = 1.0 - abs(uv.x - 0.5) * 2.0;
    let soft = smoothstep(0.0, max(effect.params.w, 1e-3), edge);
    let ripple = 0.75 + 0.25 * sin((uv.y * effect.params.z - effect.params.x * effect.params.y) * 6.2831853);
    let intensity = soft * ripple;
    return vec4<f32>(effect.color.rgb * intensity * view.exposure, effect.color.a * intensity);
}
