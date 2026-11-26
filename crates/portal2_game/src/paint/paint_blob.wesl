// Paint blobs in flight, after the game's `paintblob` shader (its
// `paintblobs/blob_surface_<paint>` materials, see docs/re/paint.md). The
// surface takes a triplanar normal map (`$uvscale` 0.006) and is lit as a
// model where its sprayer's light position is: the ambient cube plus a few
// world lights, with a half-Lambert diffuse, two Phong lobes, a Fresnel rim
// and an environment reflection. (No blob material sets a spec mask; the
// game binds white there.)
//
// Opaque paint (`$interior 0`) is that lit colour. Erase paint (`$interior
// 1`, `$opacitytexture`) is water: the scene behind it, bent by the normal
// map and tinted by the base colour, with the reflection and the soft Phong
// lobe on top and only `ERASE_OPACITY` of the lit colour over it.
//
// Vertex colours carry each paint's base colour (rgb) and how opaque the
// paint is (a: 1 opaque, 0 erase); where blobs of both merge the two
// results blend.

import bevy_pbr::render::forward_io::VertexOutput;
import bevy_pbr::render::mesh_view_bindings as view_bindings;
import bevy_pbr::render::mesh_view_bindings::view;
import bevy_pbr::render::mesh_view_bindings::fog;

struct BlobLight {
    // xyz: position, w: kind (0 point, 1 spot, 2 directional)
    position: vec4<f32>,
    // rgb: intensity (linear), w: spot cone falloff exponent
    color: vec4<f32>,
    // xyz: direction (spot / directional), w: cos inner cone
    direction: vec4<f32>,
    // x: constant, y: linear, z: quadratic attenuation, w: cos outer cone
    attenuation: vec4<f32>,
}

struct BlobParams {
    // +X, -X, +Y, -Y, +Z, -Z
    ambient: array<vec4<f32>, 6>,
    // Per linked portal: centre (w: 1 when linked), forward (w: half
    // width), up (w: half height).
    portals: array<vec4<f32>, 6>,
    lights: array<BlobLight, 3>,
    // x: lights in use
    light_count: vec4<f32>,
}

// How far behind a portal's plane a blob still shows (its quad hides the
// sliver), and how deep the space behind its opening reaches.
const CLIP_BEHIND: f32 = 0.5;
const TUNNEL_DEPTH: f32 = 128.0;
const RIM: f32 = 4.0;

// `$uvscale` of every blob material.
const UV_SCALE: f32 = 0.006;
// Shared by every blob material.
const PHONG_EXPONENT: f32 = 40.0;
const PHONG_BOOST: f32 = 2.0;
const PHONG_EXPONENT2: f32 = 400.0;
const PHONG_BOOST2: f32 = 7.0;
// Opaque paint: `$bumpstrength` 0.4, the shader's default rim light.
const OPAQUE_BUMP: f32 = 0.4;
const OPAQUE_RIM_EXPONENT: f32 = 10.0;
const OPAQUE_RIM_BOOST: f32 = 1.0;
// Erase paint (blob_surface_erase.vmt).
const ERASE_BUMP: f32 = 0.2;
const ERASE_RIM_EXPONENT: f32 = 8.0;
const ERASE_RIM_BOOST: f32 = 300.0;
// blob_surface_opacity_erase.vtf (one flat value, 21/255).
const ERASE_OPACITY: f32 = 0.0824;
const ERASE_REFRACT: f32 = 0.1;
const ERASE_BACKGROUND_BOOST: f32 = 0.0;
const ERASE_AMBIENT_SCALE: f32 = 0.1;
const ERASE_BACKLIGHT_SCALE: f32 = 0.1;
// The reflection: 0.67 × the cube map scale (c30.z) × 1/16. Cube maps
// already come out at their stored value times c30.z, as on models, so the
// blob's reflection is a sixteenth of what the cube map holds.
const ENV_SCALE: f32 = 0.67 / 16.0;

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> blob: BlobParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var env_tex: texture_cube<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var env_samp: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(3) var normal_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(4) var normal_samp: sampler;

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

fn ambient_cube(n: vec3<f32>) -> vec3<f32> {
    let sq = n * n;
    let x = select(blob.ambient[1].rgb, blob.ambient[0].rgb, n.x >= 0.0);
    let y = select(blob.ambient[3].rgb, blob.ambient[2].rgb, n.y >= 0.0);
    let z = select(blob.ambient[5].rgb, blob.ambient[4].rgb, n.z >= 0.0);
    return sq.x * x + sq.y * y + sq.z * z;
}

fn ambient_average() -> vec3<f32> {
    var sum = vec3<f32>(0.0);
    for (var i = 0; i < 6; i++) {
        sum += blob.ambient[i].rgb;
    }
    return sum / 6.0;
}

// (direction to the light, attenuation), as for models.
fn light_falloff(light: BlobLight, p: vec3<f32>) -> vec4<f32> {
    let kind = u32(light.position.w);
    if kind == 2u {
        return vec4<f32>(-light.direction.xyz, 1.0);
    }
    let d = light.position.xyz - p;
    let dist = max(length(d), 0.001);
    let l = d / dist;
    let a = light.attenuation;
    var atten = 1.0 / max(a.x + a.y * dist + a.z * dist * dist, 1e-6);
    if kind == 1u {
        let cos_angle = dot(-l, light.direction.xyz);
        let inner = light.direction.w;
        let outer = a.w;
        let edge = max((cos_angle - outer) / max(inner - outer, 1e-4), 1e-4);
        atten = atten * min(pow(edge, light.color.w), 1.0);
    }
    return vec4<f32>(l, atten);
}

// The scene already drawn behind the blob (Bevy's copy of the opaque pass,
// already exposed; the game scales its copy of the frame by the tonemap
// scale once more, like everything the shader outputs, so `fragment` does
// too). Views without the copy see the light inside the blob instead.
@if(VIEW_TRANSMISSION_TEXTURE)
fn scene_behind(uv: vec2<f32>, fallback: vec3<f32>) -> vec3<f32> {
    return textureSampleLevel(view_bindings::view_transmission_texture, view_bindings::view_transmission_sampler, uv, 0.0).rgb;
}

@if(!VIEW_TRANSMISSION_TEXTURE)
fn scene_behind(uv: vec2<f32>, fallback: vec3<f32>) -> vec3<f32> {
    return fallback;
}

// A normal map texel bent around `n` along the tangent `t`.
fn bend(bump: vec3<f32>, t: vec3<f32>, n: vec3<f32>) -> vec3<f32> {
    return normalize(bump.x * t + bump.y * cross(t, n) + bump.z * n);
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    // Blobs stay out of projected-texture shadow maps.
    if fog.mode == 2u {
        discard;
    }
    let p = in.world_position.xyz;
    if in_portal(p) {
        discard;
    }
    var base = vec3<f32>(1.0);
    var opaque = 1.0;
    @if(VERTEX_COLORS) {
        base = in.color.rgb;
        opaque = in.color.a;
    }
    let n = normalize(in.world_normal);

    // Triplanar normal map: one projection per axis, weighted by how
    // squarely the surface faces it.
    let uv = p * UV_SCALE;
    let bump_x = textureSample(normal_tex, normal_samp, uv.yz).xyz * 2.0 - 1.0;
    let bump_y = textureSample(normal_tex, normal_samp, uv.zx).xyz * 2.0 - 1.0;
    let bump_z = textureSample(normal_tex, normal_samp, uv.xy).xyz * 2.0 - 1.0;
    var w = max((abs(n) - 0.2) * 7.0, vec3<f32>(0.0));
    w = w / max(w.x + w.y + w.z, 1e-4);
    let t_y = normalize(vec3<f32>(0.0, 1.0, 0.0) - n.y * n + vec3<f32>(1e-5, 0.0, 0.0));
    let t_x = normalize(vec3<f32>(1.0, 0.0, 0.0) - n.x * n + vec3<f32>(0.0, 1e-5, 0.0));
    let bumped = w.x * bend(bump_x, t_y, n) + w.y * bend(bump_y, t_x, n) + w.z * bend(bump_z, t_x, n);
    let bump_xy = w.x * bump_x.xy + w.y * bump_y.xy + w.z * bump_z.xy;
    // The bump's tangential part, scaled by `$bumpstrength`.
    let tangential = bumped - dot(bumped, n) * n;
    let n_opaque = normalize(bumped + (OPAQUE_BUMP - 1.0) * tangential);
    let n_erase = normalize(bumped + (ERASE_BUMP - 1.0) * tangential);

    let to_eye = view.world_position - p;
    let dist = max(length(to_eye), 1.0);
    let v = to_eye / dist;
    // `$fresnelbumpstrength` 0: the Fresnel term sees the plain normal.
    let n_dot_v = saturate(dot(v, n));
    let fresnel = 1.0 - n_dot_v;
    let fresnel4 = fresnel * fresnel * fresnel * fresnel;
    let r_opaque = reflect(-v, n_opaque);
    let r_erase = reflect(-v, n_erase);

    var diffuse_o = vec3<f32>(0.0);
    var diffuse_e = vec3<f32>(0.0);
    var spec_o = vec3<f32>(0.0);
    var spec_e = vec3<f32>(0.0);
    var glint_o = vec3<f32>(0.0);
    var glint_e = vec3<f32>(0.0);
    var rim_o = vec3<f32>(0.0);
    var rim_e = vec3<f32>(0.0);
    let count = u32(blob.light_count.x);
    for (var i = 0u; i < 3u; i++) {
        if i >= count {
            break;
        }
        let light = blob.lights[i];
        let f = light_falloff(light, p);
        let l = f.xyz;
        let c = light.color.rgb * f.w;
        let ndl_o = dot(n_opaque, l);
        let ndl_e = dot(n_erase, l);
        let hl_o = saturate(ndl_o * 0.5 + 0.5);
        let hl_e = saturate(ndl_e * 0.5 + 0.5);
        diffuse_o += hl_o * hl_o * c;
        diffuse_e += hl_e * hl_e * c;
        let rdl_o = max(saturate(dot(r_opaque, l)), 1e-6);
        let rdl_e = max(saturate(dot(r_erase, l)), 1e-6);
        let lo = saturate(ndl_o) * c;
        let le = saturate(ndl_e) * c;
        spec_o += pow(rdl_o, PHONG_EXPONENT) * lo;
        spec_e += pow(rdl_e, PHONG_EXPONENT) * le;
        glint_o += pow(rdl_o, PHONG_EXPONENT2) * lo;
        glint_e += pow(rdl_e, PHONG_EXPONENT2) * le;
        rim_o += pow(rdl_o, OPAQUE_RIM_EXPONENT) * lo;
        rim_e += pow(rdl_e, ERASE_RIM_EXPONENT) * le;
    }

    // The ambient light at the surface's own normal (the game's per-vertex
    // ambient).
    let ambient = ambient_cube(n);
    let dim = vec3<f32>(0.8, 0.85, 1.0);

    // Opaque paint.
    let env_o = textureSample(env_tex, env_samp, r_opaque).rgb * fresnel * ENV_SCALE;
    let lit_o = base * ((diffuse_o * 0.82 + 0.04) * dim + ambient)
        + spec_o * PHONG_BOOST + glint_o * PHONG_BOOST2
        + rim_o * fresnel4 * OPAQUE_RIM_BOOST
        + env_o * (1.0 + diffuse_o + ambient);

    // Erase paint: what is behind it, shifted by the bumps the more the
    // nearer it is, fading at the silhouette to the light inside it.
    let screen = in.position.xy / view.viewport.zw;
    let shift = (2.0 - fresnel) * ERASE_REFRACT * 80.0 / dist;
    let inside = max(ambient_average() * ERASE_AMBIENT_SCALE, ambient_cube(-normalize(v + n)) * ERASE_BACKLIGHT_SCALE);
    var behind = scene_behind(screen + shift * bump_xy, inside * view.exposure);
    let lum = dot(behind, vec3<f32>(0.3, 0.59, 0.11));
    behind = behind * (1.0 + lum * lum * ERASE_BACKGROUND_BOOST);
    let env_e = textureSample(env_tex, env_samp, r_erase).rgb * fresnel * ENV_SCALE;
    let through = mix(inside, behind, sqrt(n_dot_v)) * base + env_e * (1.0 + diffuse_e + ambient) + spec_e * PHONG_BOOST;
    let lit_e = base * ((diffuse_e * 0.82 + 0.04) * dim + ambient)
        + glint_e * PHONG_BOOST2 + rim_e * fresnel4 * ERASE_RIM_BOOST;
    let water = mix(through, lit_e, ERASE_OPACITY) * view.exposure;

    var c = mix(water, lit_o * view.exposure, opaque);
    if fog.mode == 1u {
        let forward = -view.world_from_view[2].xyz;
        let depth = dot(p - view.world_position, forward);
        let f = saturate(min(fog.directional_light_exponent, (depth - fog.be.x) / max(fog.be.y - fog.be.x, 1.0)));
        c = mix(c, fog.base_color.rgb, f);
    }
    return vec4<f32>(c, 1.0);
}
