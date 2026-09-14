@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var output: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(2) var<uniform> enabled: vec4<u32>;

@compute @workgroup_size(16, 16)
fn preview_clipping(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id.xy >= textureDimensions(source)) {
        return;
    }
    let position = vec2<i32>(id.xy);
    let pixel = textureLoad(source, position, 0);
    var color = pixel;
    // Test developed, display-encoded RGB before preview resampling.
    // A zero channel alone is common in saturated colors, so shadows require all three.
    if enabled.x != 0u && all(pixel.rgb <= vec3<f32>(0.0)) {
        color = vec4<f32>(0.0, 0.0, 1.0, pixel.a);
    }
    if enabled.y != 0u && any(pixel.rgb >= vec3<f32>(1.0)) {
        color = vec4<f32>(1.0, 0.0, 0.0, pixel.a);
    }
    textureStore(output, position, color);
}
