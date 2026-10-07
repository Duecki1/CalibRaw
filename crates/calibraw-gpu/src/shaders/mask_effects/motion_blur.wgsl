
fn mask_motion_blur_at(pos: vec2<i32>, primary: vec4<f32>, mask: MaskBlurCoverage) -> vec3<f32> {
    let distance = mask_focus_blur_radius(primary.y, 288.0);
    let center = SceneAdjustments::local_effects_at(pos);
    if distance <= 1e-6 { return center; }
    let direction = vec2<f32>(cos(radians(primary.z)), sin(radians(primary.z)));
    // At most one pixel between midpoint taps, even at maximum shutter length.
    // A normalized, gently tapered shutter avoids periodic gaps and hard ends.
    let sample_count = u32(clamp(ceil(distance * 1.5), 4.0, 432.0));
    var sum = vec3<f32>(0.0);
    var total_weight = 0.0;
    var kernel_weight = 0.0;
    for (var index = 0u; index < 432u; index += 1u) {
        if index >= sample_count { break; }
        let unit = (f32(index) + 0.5) / f32(sample_count) - 0.5;
        let weight = 1.0 - 0.4 * pow(abs(unit) * 2.0, 4.0);
        let tap = vec2<f32>(pos) + direction * (unit * distance);
        let masked = weight * mask_blur_tap_weight(mask, tap);
        sum += mask_effect_source_linear_at(tap) * masked;
        total_weight += masked;
        kernel_weight += weight;
    }
    return mask_blur_masked_mean(sum, total_weight, kernel_weight, center);
}
