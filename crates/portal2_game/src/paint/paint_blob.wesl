// Paint blobs in flight: the paint colour (from the vertices) lit by the
// ambient light at their sprayer, with a rim of reflection from the
// paint's environment map.

import bevy_pbr::render::forward_io::VertexOutput;
import bevy_pbr::render::mesh_view_bindings::view;
import bevy_pbr::render::mesh_view_bindings::fog;

struct BlobParams {
    // +X, -X, +Y, -Y, +Z, -Z
    ambient: array<vec4<f32>, 6>,
    // Per linked portal: centre (w: 1 when linked), forward (w: half
    // width), up (w: half height).
    portals: array<vec4<f32>, 6>,
}

// How far behind a portal's plane a blob still shows (its quad hides the
// sliver), and how deep the space behind its opening reaches.
const CLIP_BEHIND: f32 = 0.5;
const TUNNEL_DEPTH: f32 = 128.0;
const RIM: f32 = 4.0;

// Whether `p` lies in the space behind a linked portal's opening: what is
// there has gone through, and shows coming out of the other portal.
fn in_portal(p: vec3<f32>) -> bool {
    for (var i = 0; i < 2; i++) {
        let c = blob.portals[i * 3];
        if c.w < 0.5 {
            continue;
        }
        let f = blob.portals[i * 3 + 1];
        let u = blob.portals[i * 3 + 2];
        let d = p - c.xyz;
        let x = dot(d, f.xyz);
        if x > -CLIP_BEHIND || x < -TUNNEL_DEPTH {
            continue;
        }
        let y = dot(d, cross(u.xyz, f.xyz));
        let z = dot(d, u.xyz);
        if abs(y) < f.w + RIM && abs(z) < u.w + RIM {
            return true;
        }
    }
    return false;
}

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> blob: BlobParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var env_tex: texture_cube<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var env_samp: sampler;

fn ambient_light(n: vec3<f32>) -> vec3<f32> {
    let sq = n * n;
    let x = select(blob.ambient[1].rgb, blob.ambient[0].rgb, n.x >= 0.0);
    let y = select(blob.ambient[3].rgb, blob.ambient[2].rgb, n.y >= 0.0);
    let z = select(blob.ambient[5].rgb, blob.ambient[4].rgb, n.z >= 0.0);
    return sq.x * x + sq.y * y + sq.z * z;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    // Blobs stay out of projected-texture shadow maps.
    if fog.mode == 2u {
        discard;
    }
    if in_portal(in.world_position.xyz) {
        discard;
    }
    var color = vec3<f32>(1.0);
    @if(VERTEX_COLORS) {
        color = in.color.rgb;
    }
    let n = normalize(in.world_normal);
    let to_eye = normalize(view.world_position - in.world_position.xyz);
    let light = ambient_light(n);
    let ndv = saturate(dot(n, to_eye));
    let env = textureSample(env_tex, env_samp, reflect(-to_eye, n)).rgb;
    let rim = pow(1.0 - ndv, 3.0);
    var c = color * (light * 0.82 + 0.04) + env * (0.15 + rim * 0.6) * (light + 0.25);
    c = c * view.exposure;
    if fog.mode == 1u {
        let forward = -view.world_from_view[2].xyz;
        let depth = dot(in.world_position.xyz - view.world_position, forward);
        let f = saturate(min(fog.directional_light_exponent, (depth - fog.be.x) / max(fog.be.y - fog.be.x, 1.0)));
        c = mix(c, fog.base_color.rgb, f);
    }
    return vec4<f32>(c, 1.0);
}
