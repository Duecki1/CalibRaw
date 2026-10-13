
const MASK_TILT_SHIFT_PI: f32 = 3.14159265;

fn mask_tilt_shift_weight(
    pos: vec2<i32>,
    primary: vec4<f32>,
    secondary: vec4<f32>,
) -> f32 {
    let full_size = vec2<f32>(
        f32(max(Common::camera_uniforms.full_width, 1u)),
        f32(max(Common::camera_uniforms.full_height, 1u)),
    );
    let short_edge = max(min(full_size.x, full_size.y), 1.0);
    let center = primary.zw / 100.0 * full_size;
    let global_pos = vec2<f32>(pos + Common::tile_origin()) + vec2<f32>(0.5);
    let angle = secondary.x * MASK_TILT_SHIFT_PI / 180.0;
    let normal = vec2<f32>(-sin(angle), cos(angle));
    let distance_percent = abs(dot(global_pos - center, normal)) / short_edge * 100.0;
    let sharp_half_width = secondary.y * 0.5;
    return smoothstep(
        sharp_half_width,
        sharp_half_width + max(secondary.z, 0.1),
        distance_percent,
    );
}

// The caller scales the circle of confusion by distance from the focus band.
// A Gaussian falloff gives a soft defocus shoulder, avoiding a sharp/blurred
// double image in the transition. The support remains bounded by 144 pixels.
fn mask_tilt_shift_at(pos: vec2<i32>, primary: vec4<f32>, mask: MaskBlurCoverage) -> vec3<f32> {
    let radius = mask_focus_blur_radius(primary.y, 144.0);
    let center = SceneAdjustments::local_effects_at(pos);
    if radius <= 1e-6 { return center; }
    let rings = u32(clamp(ceil(radius), 2.0, 12.0));
    var sum = vec3<f32>(0.0);
    var total_weight = 0.0;
    var kernel_weight = 0.0;
    for (var ring = 0u; ring < 12u; ring += 1u) {
        if ring >= rings { break; }
        let radial = (f32(ring) + 0.5) / f32(rings);
        let pairs = 2u * ring + 2u;
        let weight = radial * exp(-3.5 * radial * radial) / f32(pairs);
        for (var pair = 0u; pair < 24u; pair += 1u) {
            if pair >= pairs { break; }
            let angle = (f32(pair) + fract(f32(ring) * 0.381966))
                * MASK_TILT_SHIFT_PI / f32(pairs);
            let offset = vec2<f32>(cos(angle), sin(angle)) * (radius * radial);
            let ahead = vec2<f32>(pos) + offset;
            let behind = vec2<f32>(pos) - offset;
            let ahead_weight = weight * mask_blur_tap_weight(mask, ahead);
            let behind_weight = weight * mask_blur_tap_weight(mask, behind);
            sum += mask_effect_source_linear_at(ahead) * ahead_weight
                + mask_effect_source_linear_at(behind) * behind_weight;
            total_weight += ahead_weight + behind_weight;
            kernel_weight += 2.0 * weight;
        }
    }
    return mask_blur_masked_mean(sum, total_weight, kernel_weight, center);
}
