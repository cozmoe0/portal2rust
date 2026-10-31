// Brush surfaces: base texture(s) lit by baked lightmaps, with optional
// tangent-space bump mapping against three directional lightmaps, cube map
// reflections, detail textures and self-illumination.

import bevy_pbr::render::forward_io::VertexOutput;
import bevy_pbr::render::mesh_view_bindings::view;
import bevy_pbr::render::mesh_view_bindings::fog;
import bevy_pbr::render::mesh_view_bindings::globals;
import portal2_render::shaders::projected::{is_shadow_view, projected_lighting};
import portal2_render::shaders::fx::{FX_LEAF_TRANSMISSION, FX_REFLECTIONS, FX_SSR, FxSurface, fx_emissive_boost, fx_fresnel, fx_haze, fx_is_foliage, fx_lighting, fx_on, fx_probe, fx_wetness};

struct WorldParams {
    color: vec4<f32>,
    envmap_tint: vec4<f32>,
    selfillum_tint: vec4<f32>,
    // x: detail scale, y: detail blend factor, z: detail blend mode
    detail: vec4<f32>,
    flags: u32,
    alpha_ref: f32,
    lightmap_scale: f32,
    envmap_contrast: f32,
    envmap_saturation: f32,
    envmap_light_scale: f32,
    _pad1: f32,
    _pad2: f32,
    // The overhaul's surface profile: roughness, f0, wet, flags
    fx: vec4<f32>,
    // x: self-illumination boost share
    fx2: vec4<f32>,
}

const FLAG_BASE2: u32 = 1u;
const FLAG_BUMP: u32 = 2u;
const FLAG_SSBUMP: u32 = 4u;
const FLAG_ENVMAP: u32 = 8u;
const FLAG_ENVMAPMASK: u32 = 16u;
const FLAG_BASEALPHA_ENVMAPMASK: u32 = 32u;
const FLAG_NORMALALPHA_ENVMAPMASK: u32 = 64u;
const FLAG_SELFILLUM: u32 = 128u;
const FLAG_ALPHATEST: u32 = 256u;
const FLAG_UNLIT: u32 = 512u;
const FLAG_DETAIL: u32 = 1024u;
const FLAG_LIGHTMAPPED: u32 = 2048u;
const FLAG_TRANSLUCENT: u32 = 4096u;
const FLAG_VERTEX_BLEND: u32 = 8192u;
const FLAG_BUMP2: u32 = 16384u;
const FLAG_BLEND_MODULATE: u32 = 32768u;
const FLAG_ADDITIVE: u32 = 65536u;
const FLAG_WATER: u32 = 131072u;
const FLAG_WATER_BASE: u32 = 262144u;
const FLAG_SOLID_ENERGY: u32 = 524288u;
const FLAG_PAINT: u32 = 1048576u;
const FLAG_MOVIE: u32 = 2097152u;

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> material: WorldParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var base_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var base_samp: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(3) var base2_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(4) var base2_samp: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(5) var bump_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(6) var bump_samp: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(7) var lightmap_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(8) var lightmap_samp: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(9) var envmap_tex: texture_cube<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(10) var envmap_samp: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(11) var envmask_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(12) var envmask_samp: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(13) var detail_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(14) var detail_samp: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(15) var bump2_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(16) var bump2_samp: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(17) var modulate_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(18) var modulate_samp: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(19) var paint_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(20) var paint_samp: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(21) var splat_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(22) var splat_samp: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(23) var bubble_layout_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(24) var bubble_layout_samp: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(25) var bubble_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(26) var bubble_samp: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(27) var paint_env_tex: texture_cube<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(28) var paint_env_samp: sampler;

// Tangent-space basis used for directional lightmaps (x: S, y: T, z: normal).
const BUMP_BASIS_0 = vec3<f32>(0.81649658, 0.0, 0.57735027);
const BUMP_BASIS_1 = vec3<f32>(-0.40824829, 0.70710678, 0.57735027);
const BUMP_BASIS_2 = vec3<f32>(-0.40824829, -0.70710678, 0.57735027);

fn has(flag: u32) -> bool {
    return (material.flags & flag) != 0u;
}

fn decode_srgb(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

fn luminance(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2125, 0.7154, 0.0721));
}

fn apply_detail(albedo: vec3<f32>, detail: vec4<f32>) -> vec3<f32> {
    let mode = u32(material.detail.z);
    let f = material.detail.y;
    switch mode {
        case 1u: { return albedo + detail.rgb * f; }
        case 7u, 8u: { return mix(albedo, albedo * detail.rgb * 2.0, f); }
        default: { return mix(albedo, albedo * detail.rgb * 2.0, f); }
    }
}

// Source-style distance fog by view depth: min(max density, (depth - start)
// / (end - start)), clamped. The maximum density rides in the fog uniform's
// `directional_light_exponent` (see render/fog.rs).
fn apply_source_fog(color: vec3<f32>, world_position: vec3<f32>) -> vec3<f32> {
    if fog.mode != 1u {
        return color;
    }
    let forward = -view.world_from_view[2].xyz;
    let depth = dot(world_position - view.world_position, forward);
    let f = saturate(min(fog.directional_light_exponent, (depth - fog.be.x) / max(fog.be.y - fog.be.x, 1.0)));
    return mix(color, fog.base_color.rgb, f);
}

// Water and goo (see `water_material` in render/materials.rs).
fn water(in: VertexOutput, uv: vec2<f32>, n: vec3<f32>) -> vec4<f32> {
    var light = vec3<f32>(1.0);
    if has(FLAG_LIGHTMAPPED) {
        var lm_uv = vec2<f32>(0.0);
        @if(VERTEX_UVS_B) {
            lm_uv = in.uv_b;
        }
        light = textureSampleLevel(lightmap_tex, lightmap_samp, lm_uv, 0.0).rgb * material.lightmap_scale;
    }
    var color = material.color.rgb * light;
    if has(FLAG_WATER_BASE) {
        var flow = vec2<f32>(0.0);
        if has(FLAG_BASE2) {
            flow = textureSampleLevel(base2_tex, base2_samp, fract(uv), 0.0).rg * 2.0 - 1.0;
        }
        // Two phases half a cycle apart, each restarting while unseen.
        let t = globals.time / material.detail.y;
        let p0 = fract(t);
        let p1 = fract(t + 0.5);
        let base_uv = in.world_position.xy / material.detail.x;
        let c0 = textureSample(base_tex, base_samp, base_uv + flow * (p0 * material.detail.z));
        let c1 = textureSample(base_tex, base_samp, base_uv + flow * (p1 * material.detail.z) + vec2<f32>(0.5));
        let goo = mix(c0, c1, abs(p0 * 2.0 - 1.0));
        color = mix(color, goo.rgb * light, goo.a);
    }
    if has(FLAG_ENVMAP) {
        let v = normalize(view.world_position - in.world_position.xyz);
        color = color + textureSample(envmap_tex, envmap_samp, reflect(-v, n)).rgb * material.envmap_tint.rgb;
    }
    return vec4<f32>(apply_source_fog(color * view.exposure, in.world_position.xyz), 1.0);
}

// Emancipation fields (see `solid_energy_material` in render/materials.rs).
fn solid_energy(in: VertexOutput, uv: vec2<f32>) -> vec4<f32> {
    var flow = vec2<f32>(0.0);
    if has(FLAG_BASE2) {
        flow = textureSample(base2_tex, base2_samp, uv).rg * 2.0 - 1.0;
    }
    let t = globals.time / material.detail.x;
    let p0 = fract(t);
    let p1 = fract(t + 0.5);
    let r0 = textureSample(base_tex, base_samp, uv + flow * (p0 * material.detail.y)).a;
    let r1 = textureSample(base_tex, base_samp, uv + flow * (p1 * material.detail.y) + vec2<f32>(0.5)).a;
    var mask = mix(r0, r1, abs(p0 * 2.0 - 1.0));
    if has(FLAG_DETAIL) {
        mask = mask * textureSample(detail_tex, detail_samp, uv).r;
    }
    let c = material.color.rgb * mask * view.exposure;
    // Additive: fog toward black, premultiplied with zero alpha.
    return vec4<f32>(fade_with_fog(c, in.world_position.xyz), 0.0);
}

fn smooth01(x: f32) -> f32 {
    let t = saturate(x);
    return t * t * (3.0 - 2.0 * t);
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    return select(pow((c + 0.055) / 1.055, vec3<f32>(2.4)), c / 12.92, c <= vec3<f32>(0.04045));
}

// The paint over a surface (docs/re/paint.md): rgb before tone mapping and
// fog, a the coverage. The paint map holds the paint's sRGB colour and
// amount per luxel. Its thickness (amount plus the splat texture's height)
// picks bands: thin paint is steep-edged and greyed, thick paint carries
// bubbles that face the viewer. The game scales its splat and bubble
// textures by the luxel position on a 512 x 256 lightmap page.
fn paint_layer(in: VertexOutput, n: vec3<f32>, lm_uv: vec2<f32>, lm_step: f32) -> vec4<f32> {
    let p = textureSampleLevel(paint_tex, paint_samp, lm_uv, 0.0);
    let atlas = vec2<f32>(textureDimensions(paint_tex));
    let luxel = lm_uv * atlas;
    var t = vec3<f32>(1.0, 0.0, 0.0);
    var w = 1.0;
    @if(VERTEX_TANGENTS) {
        t = normalize(in.world_tangent.xyz);
        w = in.world_tangent.w;
    }
    let b = cross(n, t) * w;
    let to_eye = normalize(view.world_position - in.world_position.xyz);
    let vt = vec3<f32>(dot(to_eye, t), dot(to_eye, b), dot(to_eye, n));

    // Splat normal map with a parallax step by its own height.
    let splat_uv = luxel * 0.078125;
    let s0 = textureSample(splat_tex, splat_samp, splat_uv);
    let s1 = textureSample(splat_tex, splat_samp, splat_uv + vt.xy * s0.a * 0.01);
    let h = s0.a + p.a;
    let thin = smooth01((0.6 - h * 0.5) / 0.3);
    let cover = smooth01((h * 0.5 - 0.34) / 0.29);
    let deep = smooth01(h - 1.0);
    var ns = s1.xyz * 2.0 - 1.0;
    ns = vec3<f32>(ns.xy * (thin * cover * 3.9 + 0.1), ns.z);
    let nw = t * ns.x + b * ns.y + n * ns.z;
    let refraction = (view.view_from_world * vec4<f32>(nw, 0.0)).xy * (h * deep);

    // Two bubble layers; their spots are foreshortened toward the viewer.
    let layout_uv = luxel * 0.5078125 + vt.xy * 0.0014;
    let la = textureSample(bubble_layout_tex, bubble_layout_samp, layout_uv * 1.76 + vt.xy * 0.0007);
    let lb = textureSample(bubble_layout_tex, bubble_layout_samp, layout_uv + vt.xy * 0.03);
    let len_nt = max(length(vt.zx), 1e-4);
    let along = vt.z / len_nt;
    let across = vt.x / len_nt;
    let oa = la.xy - 0.5;
    let ob = lb.xy - 0.5;
    let qa = vec2<f32>(oa.x * along, oa.y * len_nt - oa.x * vt.y * across) + 0.5 + refraction * 0.2;
    let qb = vec2<f32>(ob.x * along, ob.y * len_nt - ob.x * vt.y * across) + 0.5 + refraction * 0.05;
    let ba = textureSample(bubble_tex, bubble_samp, qa);
    let bb = textureSample(bubble_tex, bubble_samp, qb);
    let b3 = textureSample(bubble_tex, bubble_samp, ob * 0.5 + 0.5);
    let pres_a = la.b * smooth01((h * 0.5 - 0.4) / 0.35);
    let pres_b = lb.b * smooth01((h * 0.5 - 0.75) * 20.0);
    let flat_bubble = vec4<f32>(0.5, 0.5, 1.0, 0.0);
    let ga = flat_bubble + pres_a * (ba - flat_bubble);
    let gb = flat_bubble + pres_b * (bb - flat_bubble);
    let g = mix(ga * vec4<f32>(1.0, 1.0, 1.0, 0.4), gb, gb.a);
    let gn = g.xyz * 2.0 - 1.0;
    let bubble4 = vec4<f32>(t * gn.x + b * gn.y + n * gn.z, g.a);
    let bubble = bubble4 / max(length(bubble4), 1e-4);

    // The surface normal also takes the small bubbles' bumps.
    let bumps = (b3.xyz * 2.0 - 1.0) * vec3<f32>(-2.0, -2.0, 1.0) * pow(pres_b, 3.5) * 0.4;
    let nt = normalize(ns + bumps);
    let surface = t * nt.x + b * nt.y + n * nt.z;

    // Colour, from a second read nearer the thick part, greyed where thin.
    let ahead = vec2<f32>(0.003 * (1.0 - h)) * vec2<f32>(512.0, 256.0) / atlas;
    let p2 = textureSampleLevel(paint_tex, paint_samp, lm_uv + ahead, 0.0);
    let rgb = srgb_to_linear(mix(p.rgb, p2.rgb, smooth01((p2.a - 0.9) * 10.0)));
    let lum = dot(rgb, vec3<f32>(0.299, 0.587, 0.114));
    let albedo = lum * 0.5 + cover * (rgb - lum * 0.5);

    // Reflections of the paint's own environment map, a sixteenth strong.
    let ndv = dot(surface, to_eye);
    let env_bubble = textureSample(paint_env_tex, paint_env_samp, reflect(-to_eye, normalize(bubble.xyz + vec3<f32>(0.0, 0.0, 1e-5)))).rgb;
    let env_surface = textureSample(paint_env_tex, paint_env_samp, reflect(-to_eye, surface)).rgb;
    let spec = env_bubble * (1.0 / 16.0) * albedo * bubble.w * ndv * 2.0 + env_surface * (1.0 / 16.0) * (1.5 - ndv) * 0.67;

    var light = textureSampleLevel(lightmap_tex, lightmap_samp, lm_uv, 0.0).rgb;
    if lm_step > 0.0 {
        let l1 = textureSampleLevel(lightmap_tex, lightmap_samp, lm_uv + vec2<f32>(lm_step, 0.0), 0.0).rgb;
        let l2 = textureSampleLevel(lightmap_tex, lightmap_samp, lm_uv + vec2<f32>(lm_step * 2.0, 0.0), 0.0).rgb;
        let l3 = textureSampleLevel(lightmap_tex, lightmap_samp, lm_uv + vec2<f32>(lm_step * 3.0, 0.0), 0.0).rgb;
        var dp = vec3<f32>(
            saturate(dot(nt, BUMP_BASIS_0)),
            saturate(dot(nt, BUMP_BASIS_1)),
            saturate(dot(nt, BUMP_BASIS_2)),
        );
        dp = dp * dp;
        light = (dp.x * l1 + dp.y * l2 + dp.z * l3) / max(dp.x + dp.y + dp.z, 1e-4);
    }
    light = light * material.lightmap_scale;

    let color = albedo * (light * 0.82 + 0.04) * vec3<f32>(0.8, 0.85, 1.0) + spec * (light + 1.0);
    let bubbles = ga.a * 0.2 + gb.a * 0.3;
    var alpha = saturate((smooth01((h * 0.5 - 0.4) * 20.0) - bubbles * 1.25) * 0.85);
    if p.a < 0.01 {
        alpha = 0.0;
    }
    return vec4<f32>(color, alpha);
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> @location(0) vec4<f32> {
    var uv = vec2<f32>(0.0);
    @if(VERTEX_UVS_A) {
        uv = in.uv;
    }
    if has(FLAG_SOLID_ENERGY) {
        if is_shadow_view() {
            discard;
        }
        return solid_energy(in, uv);
    }
    if has(FLAG_WATER) {
        if is_shadow_view() {
            return vec4<f32>(distance(view.world_position, in.world_position.xyz), 0.0, 0.0, 1.0);
        }
        return water(in, uv, normalize(in.world_normal));
    }
    if has(FLAG_MOVIE) {
        // Bink frames are gamma-encoded and the game's movie shader puts
        // them on screen as they are: no lighting and no tone mapping scale.
        if is_shadow_view() {
            return vec4<f32>(distance(view.world_position, in.world_position.xyz), 0.0, 0.0, 1.0);
        }
        let frame = textureSample(base_tex, base_samp, uv).rgb;
        return vec4<f32>(apply_source_fog(decode_srgb(frame), in.world_position.xyz), 1.0);
    }
    var base = textureSample(base_tex, base_samp, uv);
    var blend = 0.0;
    @if(VERTEX_COLORS) {
        blend = in.color.a;
    }
    if has(FLAG_VERTEX_BLEND) {
        // The modulate texture narrows the blend around a per-texel
        // midpoint (green) with a per-texel width (red).
        if has(FLAG_BLEND_MODULATE) {
            let m = textureSample(modulate_tex, modulate_samp, uv);
            let lo = saturate(m.g - m.r);
            let hi = saturate(m.g + m.r);
            let t = saturate((blend - lo) / max(hi - lo, 1e-4));
            blend = t * t * (3.0 - 2.0 * t);
        }
        let base2 = textureSample(base2_tex, base2_samp, uv);
        base = mix(base, base2, blend);
    }
    var albedo = base.rgb * material.color.rgb;
    if has(FLAG_DETAIL) {
        albedo = apply_detail(albedo, textureSample(detail_tex, detail_samp, uv * material.detail.x));
    }
    var alpha = material.color.a;
    if has(FLAG_TRANSLUCENT) || has(FLAG_ALPHATEST) {
        alpha = alpha * base.a;
    }
    // Alpha-tested surfaces are opaque wherever they pass. The main view is
    // drawn over the 3D sky box camera's picture, and a pixel left below
    // full alpha lets that picture through: foliage edges, where the
    // texture's alpha fades, showed the sky as dotted bright outlines.
    if has(FLAG_ALPHATEST) {
        if alpha < material.alpha_ref {
            discard;
        }
        alpha = 1.0;
    }
    if is_shadow_view() {
        // Shadow maps hold the distance to the nearest opaque surface.
        if has(FLAG_TRANSLUCENT) {
            discard;
        }
        return vec4<f32>(distance(view.world_position, in.world_position.xyz), 0.0, 0.0, 1.0);
    }

    var n = normalize(in.world_normal);
    if !is_front {
        n = -n;
    }
    var bump = vec4<f32>(0.5, 0.5, 1.0, 1.0);
    var tangent_normal = vec3<f32>(0.0, 0.0, 1.0);
    if has(FLAG_BUMP) {
        bump = textureSample(bump_tex, bump_samp, uv);
        if has(FLAG_BUMP2) {
            bump = mix(bump, textureSample(bump2_tex, bump2_samp, uv), blend);
        }
        tangent_normal = normalize(bump.xyz * 2.0 - 1.0);
    }

    // The overhaul's wet patches: darker, smoother and, in puddles, flat.
    var fx_rough = material.fx.x;
    var fx_wet = 0.0;
    @if(FX) {
        if !has(FLAG_UNLIT) && !has(FLAG_TRANSLUCENT) {
            let wet = fx_wetness(in.world_position.xyz, n, material.fx.z);
            fx_wet = max(wet.puddle, wet.damp * 0.3);
            albedo = albedo * mix(1.0, 0.6, wet.damp * material.fx.x);
            tangent_normal = normalize(mix(tangent_normal, vec3<f32>(0.0, 0.0, 1.0), wet.puddle));
            fx_rough = mix(mix(fx_rough, fx_rough * 0.6, wet.damp), 0.05, wet.puddle);
        }
    }

    var diffuse = vec3<f32>(1.0);
    if has(FLAG_LIGHTMAPPED) && !has(FLAG_UNLIT) {
        var lm_uv = vec2<f32>(0.0);
        @if(VERTEX_UVS_B) {
            lm_uv = in.uv_b;
        }
        var step = 0.0;
        @if(VERTEX_COLORS) {
            step = in.color.r;
        }
        if has(FLAG_BUMP) && step > 0.0 {
            let l1 = textureSampleLevel(lightmap_tex, lightmap_samp, lm_uv + vec2<f32>(step, 0.0), 0.0).rgb;
            let l2 = textureSampleLevel(lightmap_tex, lightmap_samp, lm_uv + vec2<f32>(step * 2.0, 0.0), 0.0).rgb;
            let l3 = textureSampleLevel(lightmap_tex, lightmap_samp, lm_uv + vec2<f32>(step * 3.0, 0.0), 0.0).rgb;
            if has(FLAG_SSBUMP) {
                diffuse = bump.x * l1 + bump.y * l2 + bump.z * l3;
            } else {
                var dp = vec3<f32>(
                    saturate(dot(tangent_normal, BUMP_BASIS_0)),
                    saturate(dot(tangent_normal, BUMP_BASIS_1)),
                    saturate(dot(tangent_normal, BUMP_BASIS_2)),
                );
                dp = dp * dp;
                let sum = max(dp.x + dp.y + dp.z, 1e-4);
                diffuse = (dp.x * l1 + dp.y * l2 + dp.z * l3) / sum;
            }
        } else {
            diffuse = textureSampleLevel(lightmap_tex, lightmap_samp, lm_uv, 0.0).rgb;
        }
        diffuse = diffuse * material.lightmap_scale;
    }

    var color = albedo * diffuse;
    if has(FLAG_UNLIT) {
        color = albedo;
    } else {
        // Projected textures light the bumped surface.
        var light_n = n;
        @if(VERTEX_TANGENTS) {
            if has(FLAG_BUMP) {
                var tn = tangent_normal;
                if has(FLAG_SSBUMP) {
                    tn = normalize(bump.x * BUMP_BASIS_0 + bump.y * BUMP_BASIS_1 + bump.z * BUMP_BASIS_2);
                }
                let t = normalize(in.world_tangent.xyz);
                let b = cross(n, t) * in.world_tangent.w;
                light_n = normalize(t * tn.x + b * tn.y + n * tn.z);
            }
        }
        color = color + albedo * projected_lighting(in.world_position.xyz, light_n);
        // The overhaul's lights (portals, lasers, fixture highlights).
        @if(FX) {
            if !has(FLAG_ADDITIVE) {
                var s: FxSurface;
                s.pos = in.world_position.xyz;
                s.n = light_n;
                s.geometric_n = n;
                s.v = normalize(view.world_position - in.world_position.xyz);
                s.roughness = fx_rough;
                s.f0 = vec3<f32>(material.fx.y);
                s.baked = diffuse;
                s.fixtures = true;
                s.foliage = fx_is_foliage(material.fx.w);
                let lit = fx_lighting(s);
                color = color + albedo * lit.diffuse + lit.specular;
                if s.foliage {
                    // Sunlight through the leaves from behind.
                    color = color + albedo * projected_lighting(in.world_position.xyz, -n) * FX_LEAF_TRANSMISSION;
                }
            }
        }
    }

    if has(FLAG_ENVMAP) {
        var wn = n;
        @if(VERTEX_TANGENTS) {
            if has(FLAG_BUMP) && !has(FLAG_SSBUMP) {
                let t = normalize(in.world_tangent.xyz);
                let b = cross(n, t) * in.world_tangent.w;
                wn = normalize(t * tangent_normal.x + b * tangent_normal.y + n * tangent_normal.z);
            }
        }
        let v = normalize(view.world_position - in.world_position.xyz);
        let r = reflect(-v, wn);
        var mask = vec3<f32>(1.0);
        if has(FLAG_ENVMAPMASK) {
            mask = textureSample(envmask_tex, envmask_samp, uv).rgb;
        }
        if has(FLAG_BASEALPHA_ENVMAPMASK) {
            mask = vec3<f32>(base.a);
        }
        if has(FLAG_NORMALALPHA_ENVMAPMASK) {
            mask = vec3<f32>(bump.a);
        }
        // As the game's shader: masked and tinted first, then the contrast
        // (toward the square) and saturation (around the luminance).
        var spec = textureSample(envmap_tex, envmap_samp, r).rgb * mask * material.envmap_tint.rgb;
        spec = mix(spec, spec * spec, material.envmap_contrast);
        let lum = dot(spec, vec3<f32>(0.299, 0.587, 0.114));
        spec = mix(vec3<f32>(lum), spec, material.envmap_saturation);
        // Reflections fade with the baked lighting so dark corners stay dark.
        spec = spec * mix(vec3<f32>(1.0), diffuse, saturate(material.envmap_light_scale));
        color = color + spec;
    }

    // The overhaul's reflections from the map's probes: wet patches, and
    // glossy materials the game draws without any.
    @if(FX) {
        if fx_on(FX_REFLECTIONS) && !has(FLAG_UNLIT) && !has(FLAG_ADDITIVE) {
            var gloss = 0.0;
            if !has(FLAG_ENVMAP) {
                let g = saturate(1.0 - material.fx.x);
                gloss = g * g * 0.5;
            }
            let weight = max(gloss, fx_wet);
            if weight > 0.0 {
                var wn = n;
                @if(VERTEX_TANGENTS) {
                    if has(FLAG_BUMP) && !has(FLAG_SSBUMP) {
                        let t = normalize(in.world_tangent.xyz);
                        let b = cross(n, t) * in.world_tangent.w;
                        wn = normalize(t * tangent_normal.x + b * tangent_normal.y + n * tangent_normal.z);
                    }
                }
                let v = normalize(view.world_position - in.world_position.xyz);
                let refl = fx_probe(in.world_position.xyz, reflect(-v, wn), fx_rough);
                let f = fx_fresnel(vec3<f32>(max(material.fx.y, 0.02)), saturate(dot(wn, v)));
                // Dimmer where the baked light is dim, as the game's own
                // reflections can be.
                let shade = mix(1.0, saturate(dot(diffuse, vec3<f32>(0.299, 0.587, 0.114)) * 2.5), 0.6);
                color = color + refl * f * (weight * shade);
            }
        }
    }

    if has(FLAG_SELFILLUM) {
        var tint = material.selfillum_tint.rgb;
        @if(FX) {
            tint = tint * (1.0 + (fx_emissive_boost() - 1.0) * material.fx2.x);
        }
        color = mix(color, albedo * tint, base.a);
    }

    if has(FLAG_PAINT) {
        var paint_uv = vec2<f32>(0.0);
        @if(VERTEX_UVS_B) {
            paint_uv = in.uv_b;
        }
        var paint_step = 0.0;
        @if(VERTEX_COLORS) {
            paint_step = in.color.r;
        }
        // Faces with directional lightmaps light their paint with them.
        let paint = paint_layer(in, n, paint_uv, paint_step);
        color = mix(color, paint.rgb, paint.a);
    }

    if has(FLAG_ADDITIVE) {
        // Premultiplied blending: added in full, fogging toward black.
        return vec4<f32>(fade_with_fog(color * view.exposure * alpha, in.world_position.xyz), 0.0);
    }
    @if(FX) {
        color = fx_haze(color, in.world_position.xyz, view.world_position);
        // How mirror-like the pixel is, for the screen-space reflections
        // (which put alpha back to 1); only opaque surfaces.
        if fx_on(FX_SSR) && !has(FLAG_TRANSLUCENT) {
            let puddle = saturate((fx_wet - 0.3) / 0.7);
            let glossy = saturate((0.2 - fx_rough) / 0.2) * 0.5;
            alpha = 1.0 - max(puddle, glossy);
        }
    }
    return vec4<f32>(apply_source_fog(color * view.exposure, in.world_position.xyz), alpha);
}

// Additive surfaces fade out in fog instead of taking on its colour.
fn fade_with_fog(color: vec3<f32>, world_position: vec3<f32>) -> vec3<f32> {
    if fog.mode != 1u {
        return color;
    }
    let forward = -view.world_from_view[2].xyz;
    let depth = dot(world_position - view.world_position, forward);
    let f = saturate(min(fog.directional_light_exponent, (depth - fog.be.x) / max(fog.be.y - fog.be.x, 1.0)));
    return color * (1.0 - f);
}
