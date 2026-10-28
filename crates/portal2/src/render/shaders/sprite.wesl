// Sprites (`env_sprite`, spotlight halos): a quad around the entity's
// origin, turned in the vertex shader the way the material's
// `$spriteorientation` asks.

import bevy_pbr::render::mesh_view_bindings::view;
import bevy_pbr::render::mesh_view_bindings::fog;
import bevy_pbr::render::mesh_functions::get_world_from_local;

struct SpriteParams {
    // rgb: colour times brightness (linear), a: alpha
    color: vec4<f32>,
    // x, y: half width and height, z: orientation, w: render mode
    shape: vec4<f32>,
    // x: share of the glow that is visible, y: extra brightness (spotlight
    // halos fade with the view angle)
    state: vec4<f32>,
}

// Orientations.
const VP_PARALLEL: f32 = 0.0;
const PARALLEL_UPRIGHT: f32 = 1.0;
const ORIENTED: f32 = 2.0;
// Render modes (Source's numbers).
const MODE_GLOW: f32 = 3.0;
const MODE_TRANS_ADD: f32 = 5.0;
const MODE_WORLD_GLOW: f32 = 9.0;

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> sprite: SpriteParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var base_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var base_samp: sampler;

struct Vertex {
    @builtin(instance_index) instance_index: u32,
    // yz: corner (-1 or 1)
    @location(0) position: vec3<f32>,
    @location(2) uv: vec2<f32>,
}

struct SpriteVertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) world_position: vec3<f32>,
    @location(2) brightness: f32,
}

@vertex
fn vertex(v: Vertex) -> SpriteVertexOutput {
    let world_from_local = get_world_from_local(v.instance_index);
    let centre = (world_from_local * vec4<f32>(0.0, 0.0, 0.0, 1.0)).xyz;
    let dist = max(distance(centre, view.world_position), 1.0);
    var half = sprite.shape.xy;
    var brightness = 1.0;
    let mode = sprite.shape.w;
    if mode == MODE_GLOW || mode == MODE_WORLD_GLOW {
        // Glows fade beyond 1200 units.
        brightness = saturate(1440000.0 / (dist * dist));
        if mode == MODE_GLOW {
            // Plain glows keep their size on screen.
            half = half * (dist / 200.0);
        }
    }
    var right = view.world_from_view[0].xyz;
    var up = view.world_from_view[1].xyz;
    if sprite.shape.z == PARALLEL_UPRIGHT {
        up = vec3<f32>(0.0, 0.0, 1.0);
        let forward = -view.world_from_view[2].xyz;
        right = normalize(cross(forward, up) + vec3<f32>(1e-6, 0.0, 0.0));
    } else if sprite.shape.z == ORIENTED {
        right = -normalize(world_from_local[1].xyz);
        up = normalize(world_from_local[2].xyz);
    }
    let p = centre + right * (v.position.y * half.x) + up * (v.position.z * half.y);
    var out: SpriteVertexOutput;
    out.clip_position = view.clip_from_world * vec4<f32>(p, 1.0);
    out.uv = v.uv;
    out.world_position = p;
    out.brightness = brightness;
    return out;
}

@fragment
fn fragment(in: SpriteVertexOutput) -> @location(0) vec4<f32> {
    // Sprites do not show in projected-texture shadow maps.
    if fog.mode == 2u {
        discard;
    }
    let tex = textureSample(base_tex, base_samp, in.uv);
    let additive = sprite.shape.w == MODE_GLOW || sprite.shape.w == MODE_TRANS_ADD || sprite.shape.w == MODE_WORLD_GLOW;
    var f = 0.0;
    if fog.mode == 1u {
        let forward = -view.world_from_view[2].xyz;
        let depth = dot(in.world_position - view.world_position, forward);
        f = saturate(min(fog.directional_light_exponent, (depth - fog.be.x) / max(fog.be.y - fog.be.x, 1.0)));
    }
    if additive {
        // Additive sprites fog toward black.
        let c = tex.rgb * sprite.color.rgb * (sprite.color.a * in.brightness * sprite.state.x * sprite.state.y) * view.exposure;
        // Premultiplied blending with zero alpha adds the colour.
        return vec4<f32>(c * (1.0 - f), 0.0);
    }
    let c = mix(tex.rgb * sprite.color.rgb * view.exposure, fog.base_color.rgb, f);
    return vec4<f32>(c, tex.a * sprite.color.a * sprite.state.x);
}
