
const MASK_RADIAL_BLUR_SAMPLE_COUNT: u32 = 432u;

fn mask_radial_blur_at(
    pos: vec2<i32>,
    primary: vec4<f32>,
    secondary: vec4<f32>,
    mask: MaskBlurCoverage,
) -> vec3<f32> {
    let full_size = vec2<f32>(
        f32(max(Common::camera_uniforms.full_width, 1u)),
        f32(max(Common::camera_uniforms.full_height, 1u)),
    );
    let center = primary.zw / 100.0 * full_size;
    let global_pos = vec2<f32>(pos + Common::tile_origin()) + vec2<f32>(0.5);
    let delta = global_pos - center;
    let center_distance = length(delta);
    let radial_direction = delta / max(center_distance, 1e-4);
    let edge_factor = clamp(center_distance / max(length(full_size) * 0.5, 1.0), 0.0, 1.0);
    let extent = mask_focus_blur_radius(primary.y, 288.0) * edge_factor;
    let center_rgb = SceneAdjustments::local_effects_at(pos);
    if extent <= 1e-6 { return center_rgb; }
    let sample_count = u32(clamp(ceil(extent * 1.5), 4.0, f32(MASK_RADIAL_BLUR_SAMPLE_COUNT)));
    var sum = vec3<f32>(0.0);
    var total_weight = 0.0;
    var kernel_weight = 0.0;

    for (var index = 0u; index < MASK_RADIAL_BLUR_SAMPLE_COUNT; index = index + 1u) {
        if index >= sample_count { break; }
        let unit = (f32(index) + 0.5) / f32(sample_count) - 0.5;
        var offset = radial_direction * (unit * extent);
        if secondary.x >= 0.5 {
            // Spin samples lie on an arc centered on the chosen point.
            let angle = unit * extent / max(center_distance, 1.0);
            let cosine_delta = -2.0 * sin(angle * 0.5) * sin(angle * 0.5);
            offset = vec2<f32>(
                cosine_delta * delta.x - sin(angle) * delta.y,
                sin(angle) * delta.x + cosine_delta * delta.y,
            );
        }
        let weight = 1.0 - 0.4 * pow(abs(unit) * 2.0, 4.0);
        let tap = vec2<f32>(pos) + offset;
        let masked = weight * mask_blur_tap_weight(mask, tap);
        sum = sum + mask_effect_source_linear_at(tap) * masked;
        total_weight = total_weight + masked;
        kernel_weight = kernel_weight + weight;
    }
    return mask_blur_masked_mean(sum, total_weight, kernel_weight, center_rgb);
}
