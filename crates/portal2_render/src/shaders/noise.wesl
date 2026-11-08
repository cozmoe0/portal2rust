// Procedural noise for the visual overhaul's shaders: an integer hash, 3D
// gradient noise and fractal sums of it. Everything is computed, nothing is
// read from textures, so the patterns have no repeat and no resolution.

// Rotation between fractal octaves, so their lattices never line up (and
// time, on the third axis, drifts into the others: higher octaves move).
const NOISE_OCTAVE_ROTATION: mat3x3<f32> = mat3x3<f32>(
    vec3<f32>(0.00, 0.80, 0.60),
    vec3<f32>(-0.80, 0.36, -0.48),
    vec3<f32>(-0.60, -0.48, 0.64),
);

// Three rounds of multiply-and-mix over a lattice point (a 3D PCG style
// hash): every output bit depends on every input bit.
fn noise_hash3(c: vec3<u32>) -> vec3<u32> {
    var v = c * 1664525u + vec3<u32>(1013904223u);
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    v = v ^ (v >> vec3<u32>(16u));
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    return v;
}

// A pseudo-random gradient (each component -1..1) at a lattice point.
fn noise_gradient_at(c: vec3<u32>) -> vec3<f32> {
    let h = noise_hash3(c) >> vec3<u32>(8u);
    return vec3<f32>(h) * (2.0 / 16777215.0) - vec3<f32>(1.0);
}

// Gradient noise: the corner gradients' ramps, blended with a quintic
// curve so the result is smooth to the second derivative. Roughly -1..1,
// mostly within -0.5..0.5.
fn noise_gradient3(p: vec3<f32>) -> f32 {
    let cell = floor(p);
    let f = p - cell;
    let i = bitcast<vec3<u32>>(vec3<i32>(cell));
    let u = f * f * f * (f * (f * 6.0 - vec3<f32>(15.0)) + vec3<f32>(10.0));
    let n000 = dot(noise_gradient_at(i), f);
    let n100 = dot(noise_gradient_at(i + vec3<u32>(1u, 0u, 0u)), f - vec3<f32>(1.0, 0.0, 0.0));
    let n010 = dot(noise_gradient_at(i + vec3<u32>(0u, 1u, 0u)), f - vec3<f32>(0.0, 1.0, 0.0));
    let n110 = dot(noise_gradient_at(i + vec3<u32>(1u, 1u, 0u)), f - vec3<f32>(1.0, 1.0, 0.0));
    let n001 = dot(noise_gradient_at(i + vec3<u32>(0u, 0u, 1u)), f - vec3<f32>(0.0, 0.0, 1.0));
    let n101 = dot(noise_gradient_at(i + vec3<u32>(1u, 0u, 1u)), f - vec3<f32>(1.0, 0.0, 1.0));
    let n011 = dot(noise_gradient_at(i + vec3<u32>(0u, 1u, 1u)), f - vec3<f32>(0.0, 1.0, 1.0));
    let n111 = dot(noise_gradient_at(i + vec3<u32>(1u, 1u, 1u)), f - vec3<f32>(1.0, 1.0, 1.0));
    let x00 = mix(n000, n100, u.x);
    let x10 = mix(n010, n110, u.x);
    let x01 = mix(n001, n101, u.x);
    let x11 = mix(n011, n111, u.x);
    return mix(mix(x00, x10, u.y), mix(x01, x11, u.y), u.z) * 1.4;
}

// Fractal sum of `octaves` gradient noises, each twice the frequency and
// half the amplitude of the one before. Mostly within -0.5..0.5.
fn noise_fbm3(p: vec3<f32>, octaves: i32) -> f32 {
    var sum = 0.0;
    var amp = 0.5;
    var q = p;
    for (var i = 0; i < octaves; i++) {
        sum += amp * noise_gradient3(q);
        q = NOISE_OCTAVE_ROTATION * q * 2.03 + vec3<f32>(17.1, 3.7, 9.3);
        amp *= 0.5;
    }
    return sum;
}
