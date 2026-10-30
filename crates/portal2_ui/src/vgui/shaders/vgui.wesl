// Interface drawing (see vgui/render.rs): gamma colours and texels are
// decoded to linear and blended into the linear HDR view.

@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

fn decode_srgb(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

struct UiIn {
    @location(0) pos: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
}

struct UiOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
}

@vertex
fn ui_vertex(v: UiIn) -> UiOut {
    return UiOut(vec4<f32>(v.pos, 0.0, 1.0), v.uv, v.color);
}

@fragment
fn ui_fragment(v: UiOut) -> @location(0) vec4<f32> {
    let t = textureSample(tex, samp, v.uv);
    return vec4<f32>(decode_srgb(t.rgb) * decode_srgb(v.color.rgb), t.a * v.color.a);
}

// The paused-game backdrop: the scene's luminance, blurred and darkened
// (fitted to the game: about 0.48 of a few-pixel blur at 900 lines).
@fragment
fn backdrop(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let size = vec2<f32>(textureDimensions(tex));
    let uv = pos.xy / size;
    let step = vec2<f32>(size.y * 0.0018) / size;
    var sum = 0.0;
    var weight = 0.0;
    for (var j = -4; j <= 4; j++) {
        for (var i = -4; i <= 4; i++) {
            let w = exp(-f32(i * i + j * j) / 8.0);
            let c = textureSampleLevel(tex, samp, uv + vec2<f32>(f32(i), f32(j)) * step, 0.0).rgb;
            sum += dot(c, vec3<f32>(0.2126, 0.7152, 0.0722)) * w;
            weight += w;
        }
    }
    return vec4<f32>(vec3<f32>(sum / weight * 0.48), 1.0);
}
