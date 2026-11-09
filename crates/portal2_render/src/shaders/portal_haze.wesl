// The visual overhaul's heat haze around a portal (`r_fx_portal_heat`): the
// wall around the rim, seen through the air the fire heats, shimmers. Drawn
// on the fire's quad (which reaches past the portal's rectangle only where
// the surface goes on) in the transmissive pass, so it reads Bevy's copy of
// the opaque scene and draws it back shifted by the heat field shared with
// the view's refraction near the rim (`portal_fx.wesl`). The fire and the
// rest of the transparent scene draw over it afterwards, in their order.
//
// Where the shift is about a pixel or more the picture also softens a
// little, as heat haze does; where it fades out, toward the quad's edges,
// the copy matches what is already there, so there is no seam.

import bevy_pbr::render::forward_io::VertexOutput;
import bevy_pbr::render::mesh_view_bindings as view_bindings;
import bevy_pbr::render::mesh_view_bindings::globals;
import portal2_render::shaders::portal_fx::{PortalFxIn, portal_fx_haze};

struct PortalHazeParams {
    // x: open amount (0..1), y: strength
    state: vec4<f32>,
    // The quad's reach past the rectangle (left, right, bottom, top) in
    // texture coordinates
    shape: vec4<f32>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> haze: PortalHazeParams;

@if(VIEW_TRANSMISSION_TEXTURE)
fn haze_scene(px: vec2<f32>) -> vec3<f32> {
    let size = vec2<f32>(textureDimensions(view_bindings::view_transmission_texture));
    return textureSampleLevel(view_bindings::view_transmission_texture, view_bindings::view_transmission_sampler, px / size, 0.0).rgb;
}

@if(!VIEW_TRANSMISSION_TEXTURE)
fn haze_scene(px: vec2<f32>) -> vec3<f32> {
    return vec3<f32>(0.0);
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    var uv = vec2<f32>(0.5);
    @if(VERTEX_UVS_A) {
        uv = in.uv;
    }
    // Without a copy of the scene there is nothing to shift.
    var has_scene = false;
    @if(VIEW_TRANSMISSION_TEXTURE) {
        has_scene = true;
    }
    var fx_in: PortalFxIn;
    fx_in.uv = uv;
    fx_in.uv_dx = dpdx(uv);
    fx_in.uv_dy = dpdy(uv);
    fx_in.open = haze.state.x;
    fx_in.heat = haze.state.y;
    fx_in.margins = haze.shape;
    fx_in.time = globals.time;
    let shift = portal_fx_haze(fx_in);
    let reach = length(shift);
    if !has_scene || reach < 0.01 {
        discard;
    }
    let at = in.position.xy + shift;
    // A small rotated-grid blur, growing with the shift.
    let r = min(reach * 0.22, 1.25);
    if r < 0.2 {
        return vec4<f32>(haze_scene(at), 1.0);
    }
    let a = vec2<f32>(0.4, 0.92) * r;
    let b = vec2<f32>(-0.92, 0.4) * r;
    let c = (haze_scene(at + a) + haze_scene(at - a) + haze_scene(at + b) + haze_scene(at - b)) * 0.25;
    return vec4<f32>(c, 1.0);
}
