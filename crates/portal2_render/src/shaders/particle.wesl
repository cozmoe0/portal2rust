// Particles drawn with SpriteCard-style materials: camera-facing or
// world-oriented sprites, trails and ropes. The CPU picks each particle's
// sheet frames; the vertex shader turns the quads to face the view, so
// every view (portals included) sees them face on.

import bevy_pbr::render::mesh_view_bindings::view;
import bevy_pbr::render::mesh_view_bindings::fog;
import bevy_pbr::render::mesh_functions::get_world_from_local;

struct ParticleParams {
    // x: overbright factor, y: flags, z: dual sequence blend mode
    params: vec4<f32>,
}

const FLAG_ADDITIVE: u32 = 1u;
const FLAG_BLEND_FRAMES: u32 = 2u;
const FLAG_DUAL_SEQUENCE: u32 = 4u;
const FLAG_NO_FOG: u32 = 8u;
const FLAG_MOD2X: u32 = 16u;

// Vertex modes.
const MODE_SCREEN: f32 = 0.0;
const MODE_SCREEN_Z: f32 = 1.0;
const MODE_WORLD: f32 = 2.0;
const MODE_TRAIL: f32 = 3.0;
const MODE_ROPE: f32 = 4.0;

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> material: ParticleParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var base_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var base_samp: sampler;

struct Vertex {
    @builtin(instance_index) instance_index: u32,
    // Sprite centre, trail head or rope point (relative to the entity).
    @location(0) position: vec3<f32>,
    // xy: corner (0..1), z: mode, w: frame blend
    @location(1) corner: vec4<f32>,
    // x: radius (half size or half width), y: roll, z: tail alpha scale / rope v
    @location(2) shape: vec4<f32>,
    @location(3) rect0: vec4<f32>,
    @location(4) rect1: vec4<f32>,
    @location(5) rect2: vec4<f32>,
    // World sprites: x axis. Trails: tail point. Ropes: direction.
    @location(6) axis0: vec3<f32>,
    // World sprites: y axis. Trails: tail colour scale.
    @location(7) axis1: vec3<f32>,
    @location(8) color: vec4<f32>,
}

struct ParticleVertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv0: vec2<f32>,
    @location(1) uv1: vec2<f32>,
    @location(2) uv2: vec2<f32>,
    @location(3) color: vec4<f32>,
    @location(4) world_position: vec3<f32>,
    @location(5) blend: f32,
}

fn rect_uv(r: vec4<f32>, c: vec2<f32>) -> vec2<f32> {
    // Texture v runs down from the top edge.
    return vec2<f32>(mix(r.x, r.z, c.x), mix(r.w, r.y, c.y));
}

@vertex
fn vertex(v: Vertex) -> ParticleVertexOutput {
    let world_from_local = get_world_from_local(v.instance_index);
    let centre = (world_from_local * vec4<f32>(v.position, 1.0)).xyz;
    let c = v.corner.xy;
    let s = c * 2.0 - vec2<f32>(1.0);
    let mode = v.corner.z;
    let radius = v.shape.x;
    var color = v.color;
    var p = centre;
    var uv_corner = c;
    if mode == MODE_TRAIL || mode == MODE_ROPE {
        var along: vec3<f32>;
        var mid: vec3<f32>;
        if mode == MODE_TRAIL {
            let tail = (world_from_local * vec4<f32>(v.axis0, 1.0)).xyz;
            along = tail - centre;
            mid = mix(centre, tail, c.y);
            color = vec4<f32>(color.rgb * mix(vec3<f32>(1.0), v.axis1, c.y), color.a * mix(1.0, v.shape.z, c.y));
        } else {
            along = v.axis0;
            mid = centre;
            uv_corner = vec2<f32>(c.x, v.shape.z);
        }
        let to_eye = view.world_position - mid;
        var side = cross(along, to_eye);
        let len = length(side);
        side = select(vec3<f32>(0.0, 0.0, 1.0), side / max(len, 1e-6), len > 1e-6);
        p = mid + side * (s.x * radius);
    } else {
        var right: vec3<f32>;
        var up: vec3<f32>;
        if mode == MODE_WORLD {
            right = v.axis0;
            up = v.axis1;
        } else if mode == MODE_SCREEN_Z {
            up = vec3<f32>(0.0, 0.0, 1.0);
            let forward = -view.world_from_view[2].xyz;
            let r = cross(forward, up);
            right = select(view.world_from_view[0].xyz, normalize(r), length(r) > 1e-4);
        } else {
            let cr = cos(v.shape.y);
            let sr = sin(v.shape.y);
            let vr = view.world_from_view[0].xyz;
            let vu = view.world_from_view[1].xyz;
            right = vr * cr + vu * sr;
            up = vu * cr - vr * sr;
        }
        p = centre + (right * s.x + up * s.y) * radius;
    }
    var out: ParticleVertexOutput;
    out.clip_position = view.clip_from_world * vec4<f32>(p, 1.0);
    if mode == MODE_ROPE {
        out.uv0 = uv_corner;
        out.uv1 = uv_corner;
        out.uv2 = uv_corner;
    } else {
        out.uv0 = rect_uv(v.rect0, uv_corner);
        out.uv1 = rect_uv(v.rect1, uv_corner);
        out.uv2 = rect_uv(v.rect2, uv_corner);
    }
    out.color = color;
    out.world_position = p;
    out.blend = v.corner.w;
    return out;
}

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    return select(pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4)), c / 12.92, c <= vec3<f32>(0.04045));
}

@fragment
fn fragment(in: ParticleVertexOutput) -> @location(0) vec4<f32> {
    // Particles do not show in projected-texture shadow maps.
    if fog.mode == 2u {
        discard;
    }
    let flags = u32(material.params.y);
    let t0 = textureSample(base_tex, base_samp, in.uv0);
    let t1 = textureSample(base_tex, base_samp, in.uv1);
    let t2 = textureSample(base_tex, base_samp, in.uv2);
    var tex = t0;
    if (flags & FLAG_BLEND_FRAMES) != 0u {
        tex = mix(t0, t1, in.blend);
    }
    if (flags & FLAG_DUAL_SEQUENCE) != 0u {
        if material.params.z >= 1.0 {
            // Colour from the second sequence, alpha from the first.
            tex = vec4<f32>(t2.rgb, tex.a * t2.a);
        } else {
            tex = (tex + t2) * 0.5;
        }
    }
    var rgb = tex.rgb * to_linear(clamp(in.color.rgb, vec3<f32>(0.0), vec3<f32>(1.0))) * material.params.x;
    let alpha = clamp(tex.a * in.color.a, 0.0, 1.0);
    var f = 0.0;
    if fog.mode == 1u && (flags & FLAG_NO_FOG) == 0u {
        let forward = -view.world_from_view[2].xyz;
        let depth = dot(in.world_position - view.world_position, forward);
        f = saturate(min(fog.directional_light_exponent, (depth - fog.be.x) / max(fog.be.y - fog.be.x, 1.0)));
    }
    if (flags & FLAG_ADDITIVE) != 0u {
        // Additive particles fog toward black; premultiplied blending with
        // zero alpha adds the colour.
        return vec4<f32>(rgb * alpha * (1.0 - f) * view.exposure, 0.0);
    }
    if (flags & FLAG_MOD2X) != 0u {
        // Multiplied in: the blend gives what is behind × (1 + output) with
        // zero alpha, so twice the colour eased to grey by the alpha (and by
        // the fog) goes out less one.
        let m = mix(mix(vec3<f32>(0.5), tex.rgb * to_linear(clamp(in.color.rgb, vec3<f32>(0.0), vec3<f32>(1.0))), alpha), vec3<f32>(0.5), f);
        return vec4<f32>(m * 2.0 - 1.0, 0.0);
    }
    let c = mix(rgb * view.exposure, fog.base_color.rgb, f);
    return vec4<f32>(c, alpha);
}
