// The overhaul's post-processing on the finished 3D view (see
// visuals/fx/postfx.rs): a bloom chain over the parts brighter than the
// display shows, then a tone curve that keeps everything below its knee as
// it was and rolls brighter values off, and a light colour grade.

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var aux: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;
@group(0) @binding(3) var<uniform> post: FxPost;

struct FxPost {
    // x: threshold, y: soft knee, z: intensity, w: 1 / mip count
    bloom: vec4<f32>,
    // rgb: shadow tint (luminance 1), w: amount
    shadow_tint: vec4<f32>,
    // rgb: highlight tint (luminance 1), w: amount
    highlight_tint: vec4<f32>,
    // x: tone knee, y: white point, z: saturation, w: contrast
    tone: vec4<f32>,
    // x: bloom on, y: grade on, z: frame number, w: unused
    flags: vec4<f32>,
}

struct FsIn {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

fn luminance(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

fn tap(uv: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(src, samp, uv, 0.0).rgb;
}

// Brightness above the threshold, with a quadratic knee below it.
fn prefilter(c: vec3<f32>) -> vec3<f32> {
    let b = max(c.r, max(c.g, c.b));
    let t = post.bloom.x;
    let k = max(post.bloom.y, 1e-4);
    var soft = clamp(b - t + k, 0.0, 2.0 * k);
    soft = soft * soft / (4.0 * k);
    return c * (max(soft, b - t) / max(b, 1e-4));
}

// The 13-tap downsample (Jimenez 2014): five overlapping 2x2 boxes.
fn down13(uv: vec2<f32>, karis: bool) -> vec3<f32> {
    let t = 1.0 / vec2<f32>(textureDimensions(src));
    let a = tap(uv + t * vec2<f32>(-2.0, -2.0));
    let b = tap(uv + t * vec2<f32>(0.0, -2.0));
    let c = tap(uv + t * vec2<f32>(2.0, -2.0));
    let d = tap(uv + t * vec2<f32>(-1.0, -1.0));
    let e = tap(uv + t * vec2<f32>(1.0, -1.0));
    let f = tap(uv + t * vec2<f32>(-2.0, 0.0));
    let g = tap(uv);
    let h = tap(uv + t * vec2<f32>(2.0, 0.0));
    let i = tap(uv + t * vec2<f32>(-1.0, 1.0));
    let j = tap(uv + t * vec2<f32>(1.0, 1.0));
    let k = tap(uv + t * vec2<f32>(-2.0, 2.0));
    let l = tap(uv + t * vec2<f32>(0.0, 2.0));
    let m = tap(uv + t * vec2<f32>(2.0, 2.0));
    let boxes = array<vec3<f32>, 5>((d + e + i + j) * 0.25, (a + b + f + g) * 0.25, (b + c + g + h) * 0.25, (f + g + k + l) * 0.25, (g + h + l + m) * 0.25);
    let weights = array<f32, 5>(0.5, 0.125, 0.125, 0.125, 0.125);
    var sum = vec3<f32>(0.0);
    var total = 0.0;
    for (var n = 0; n < 5; n++) {
        // The first pass weights each box down by its brightness so single
        // hot pixels do not flicker into big blobs.
        var w = weights[n];
        if karis {
            w = w / (1.0 + luminance(boxes[n]));
        }
        sum += boxes[n] * w;
        total += w;
    }
    return sum / total;
}

@fragment
fn bloom_prefilter(in: FsIn) -> @location(0) vec4<f32> {
    return vec4<f32>(prefilter(down13(in.uv, true)), 1.0);
}

@fragment
fn bloom_down(in: FsIn) -> @location(0) vec4<f32> {
    return vec4<f32>(down13(in.uv, false), 1.0);
}

// A 3x3 tent over the smaller mip, added onto the larger one by blending.
@fragment
fn bloom_up(in: FsIn) -> @location(0) vec4<f32> {
    let t = 1.0 / vec2<f32>(textureDimensions(src));
    var sum = tap(in.uv) * 4.0;
    sum += (tap(in.uv + vec2<f32>(t.x, 0.0)) + tap(in.uv - vec2<f32>(t.x, 0.0)) + tap(in.uv + vec2<f32>(0.0, t.y)) + tap(in.uv - vec2<f32>(0.0, t.y))) * 2.0;
    sum += tap(in.uv + t) + tap(in.uv - t) + tap(in.uv + vec2<f32>(t.x, -t.y)) + tap(in.uv + vec2<f32>(-t.x, t.y));
    return vec4<f32>(sum / 16.0, 1.0);
}

// Identity up to the knee, then an extended-Reinhard shoulder that reaches
// 1 at the white point (mirrored by `postfx::shoulder` for the tests).
fn shoulder(x: f32) -> f32 {
    let k = post.tone.x;
    if x <= k {
        return x;
    }
    let t = (x - k) / (1.0 - k);
    let tw = max((post.tone.y - k) / (1.0 - k), 1e-3);
    let g = t * (1.0 + t / (tw * tw)) / (1.0 + t);
    return min(k + (1.0 - k) * g, 1.0);
}

fn grade(c_in: vec3<f32>) -> vec3<f32> {
    var c = max(c_in, vec3<f32>(0.0));
    // Contrast around middle grey, in log space.
    let pivot = 0.18;
    c = pivot * pow(c / pivot + vec3<f32>(1e-6), vec3<f32>(post.tone.w));
    // Cool shadows, warm highlights.
    let lum = luminance(c);
    let s = (1.0 - smoothstep(0.0, 0.3, lum)) * post.shadow_tint.w;
    let h = smoothstep(0.35, 1.2, lum) * post.highlight_tint.w;
    c = c * mix(vec3<f32>(1.0), post.shadow_tint.rgb, s) * mix(vec3<f32>(1.0), post.highlight_tint.rgb, h);
    let l2 = luminance(c);
    return max(mix(vec3<f32>(l2), c, post.tone.z), vec3<f32>(0.0));
}

fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    return select(1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055, c * 12.92, c <= vec3<f32>(0.0031308));
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    return select(pow((c + 0.055) / 1.055, vec3<f32>(2.4)), c / 12.92, c <= vec3<f32>(0.04045));
}

// Interleaved gradient noise (Jimenez 2014), animated by frame.
fn ign(p: vec2<f32>) -> f32 {
    let q = p + vec2<f32>(5.588238 * post.flags.z);
    return fract(52.9829189 * fract(dot(q, vec2<f32>(0.06711056, 0.00583715))));
}

@fragment
fn composite(in: FsIn) -> @location(0) vec4<f32> {
    let texel = vec2<i32>(in.pos.xy);
    let scene = textureLoad(src, texel, 0);
    var c = scene.rgb;
    if post.flags.x > 0.5 {
        c += textureSampleLevel(aux, samp, in.uv, 0.0).rgb * (post.bloom.z * post.bloom.w);
    }
    if post.flags.y > 0.5 {
        c = grade(c);
        c = vec3<f32>(shoulder(c.r), shoulder(c.g), shoulder(c.b));
        // Half a step of noise in display space hides banding in the
        // dark gradients haze and shafts leave.
        let d = (ign(in.pos.xy) - 0.5) / 255.0;
        c = srgb_to_linear(clamp(linear_to_srgb(c) + vec3<f32>(d), vec3<f32>(0.0), vec3<f32>(1.0)));
    }
    return vec4<f32>(c, scene.a);
}
