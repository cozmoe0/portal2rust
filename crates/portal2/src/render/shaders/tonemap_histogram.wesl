// Luminance histogram of the finished 3D view for the dynamic tone map:
// sixteen buckets even in gamma space (edges (i / 16)^2.5 in linear
// luminance) and one for pixels at or above 1.

@group(0) @binding(0) var scene: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> counts: array<atomic<u32>>;

@compute @workgroup_size(16, 16, 1)
fn histogram(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(scene);
    // Every other pixel in each direction is plenty.
    let p = id.xy * 2u;
    if p.x >= size.x || p.y >= size.y {
        return;
    }
    let c = textureLoad(scene, vec2<i32>(p), 0).rgb;
    let lum = max(dot(c, vec3<f32>(0.299, 0.587, 0.114)), 0.0);
    var bucket = 16u;
    if lum < 1.0 {
        bucket = min(u32(floor(pow(lum, 0.4) * 16.0)), 15u);
    }
    atomicAdd(&counts[bucket], 1u);
}
