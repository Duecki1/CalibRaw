// Display histogram, separate from the scene EV histogram used by tone mapping.
// Workgroup-local bins avoid global atomic contention on flat/clipped images.
struct Histogram { bins: array<atomic<u32>, 1024> }
struct Sampling {
    origin: vec4<f32>,
    step_x: vec4<f32>,
    step_y: vec4<f32>,
    extent: vec4<u32>,
}
@group(0) @binding(0) var preview: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> histogram: Histogram;
@group(0) @binding(2) var<uniform> sampling: Sampling;
var<workgroup> local_bins: array<atomic<u32>, 1024>;

fn bin(value: f32) -> u32 {
    return u32(round(clamp(value, 0.0, 1.0) * 255.0));
}

fn decode_srgb(rgb: vec3<f32>) -> vec3<f32> {
    return select(pow((rgb + 0.055) / 1.055, vec3<f32>(2.4)),
                  rgb / 12.92, rgb <= vec3<f32>(0.04045));
}

fn encode_srgb(value: f32) -> f32 {
    return select(1.055 * pow(value, 1.0 / 2.4) - 0.055,
                  12.92 * value, value <= 0.0031308);
}

@compute @workgroup_size(16, 16)
fn preview_histogram(@builtin(global_invocation_id) gid: vec3<u32>,
                     @builtin(local_invocation_index) lane: u32) {
    for (var i = lane; i < 1024u; i += 256u) {
        atomicStore(&local_bins[i], 0u);
    }
    workgroupBarrier();
    if all(gid.xy < sampling.extent.xy) {
        let position = sampling.origin.xy + f32(gid.x) * sampling.step_x.xy
                                            + f32(gid.y) * sampling.step_y.xy;
        let size = vec2<f32>(textureDimensions(preview));
        // Exclude the blank corners introduced by rotation/geometry.
        if all(position >= vec2<f32>(-0.5)) && all(position < size - 0.5) {
            let rgb = textureLoad(preview, vec2<i32>(floor(position + 0.5)), 0).rgb;
            // Relative sRGB luminance, encoded onto the same display axis as RGB.
            let luminance = encode_srgb(dot(decode_srgb(rgb), vec3<f32>(0.2126, 0.7152, 0.0722)));
            atomicAdd(&local_bins[bin(rgb.r)], 1u);
            atomicAdd(&local_bins[256u + bin(rgb.g)], 1u);
            atomicAdd(&local_bins[512u + bin(rgb.b)], 1u);
            atomicAdd(&local_bins[768u + bin(luminance)], 1u);
        }
    }
    workgroupBarrier();
    for (var i = lane; i < 1024u; i += 256u) {
        let count = atomicLoad(&local_bins[i]);
        if count != 0u { atomicAdd(&histogram.bins[i], count); }
    }
}
