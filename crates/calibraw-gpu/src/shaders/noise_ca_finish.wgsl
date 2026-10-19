virtual fn finish_reference_at(_pos: vec2<i32>) -> vec3<f32> {
    return vec3<f32>(0.0);
}

#import calibraw::common as Common
#import calibraw::noise as Noise

// Keep the virtual declaration at byte zero: naga_oil 0.22 mishandles it after
// leading line comments. CFA entrypoints override this fallback.

// Inverse of the CFA finish passes' opponent transform: Rec.2020 luma `y`
// with (B - Y) * 0.56433 and (R - Y) * 0.67815 opponents back to camera RGB.
fn finish_from_yuv(y: f32, uv: vec2<f32>) -> vec3<f32> {
    let b = y + uv.x / 0.56433;
    let r = y + uv.y / 0.67815;
    let g = (y - 0.2627 * r - 0.0593 * b) / 0.6780;
    return vec3<f32>(r, g, b);
}

// Signal level at which the false-colour guard ramp spans 0.006..0.055 in
// opponent units, the absolute range it was tuned with.
const FALSE_COLOR_REFERENCE_LEVEL: f32 = 0.18;
const FALSE_COLOR_RAMP_START: f32 = 0.006 / FALSE_COLOR_REFERENCE_LEVEL;
const FALSE_COLOR_RAMP_END: f32 = 0.055 / FALSE_COLOR_REFERENCE_LEVEL;

// Blend of the Reference demosaic's centre opponents toward the five-tap cross
// median, up to `max_strength`. `disagreement` is the distance between the two
// in the opponent units of `finish_from_yuv`. Demosaic chroma outliers scale
// with the signal, so the ramp follows the brightest channel of camera RGB
// `rgb` (white-balanced, sensor white 1) and keeps saturated detail judged by
// its own level. It never starts below 3 sigma, or ends below 8 sigma, of the
// opponent noise the sensor model predicts, which belongs to sensor denoise.
fn finish_false_color_strength(rgb: vec3<f32>, disagreement: f32, max_strength: f32) -> f32 {
    let level = max(max(rgb.r, max(rgb.g, rgb.b)), 0.0);
    let variance = Noise::nr_rgb_variance(rgb);
    let u_variance = 0.318468 * dot(variance, vec3<f32>(0.069011, 0.459684, 0.884916));
    let v_variance = 0.459887 * dot(variance, vec3<f32>(0.543611, 0.459684, 0.003516));
    let sigma = sqrt(u_variance + v_variance);
    let start = max(FALSE_COLOR_RAMP_START * level, 3.0 * sigma);
    let end = max(FALSE_COLOR_RAMP_END * level, 8.0 * sigma);
    return max_strength * smoothstep(start, end, disagreement);
}

fn finish_apply_sensor_denoise(pos: vec2<i32>, rgb: vec3<f32>) -> vec3<f32> {
    let signal_strength = Noise::nr_perceptual_strength(Common::camera_uniforms.noise_options.x, 3.2);
    if signal_strength <= 1e-6 { return rgb; }

    let center_signal = Noise::nr_signal(rgb);
    let center_variance = Noise::nr_component_variance(rgb);
    var signal_sum = center_signal;
    var signal_weights = 1.0;

    let scale_count = Noise::nr_scale_count();
    for (var scale = 0; scale < 5; scale = scale + 1) {
        if scale >= scale_count { break; }
        let radius = Noise::nr_scale_radius(scale);
        for (var direction_index = 0; direction_index < 8; direction_index = direction_index + 1) {
            let direction = Noise::NR_DIRECTIONS[direction_index];
            let sample = finish_reference_at(pos + direction * radius);
            let spatial = Noise::nr_scale_spatial_weight(radius, direction);
            let sample_signal = Noise::nr_signal(sample);
            let sample_variance = Noise::nr_component_variance(sample);
            let range_weight = Noise::nr_signal_range_weight(
                center_signal,
                center_variance,
                sample_signal,
                sample_variance,
                spatial,
            );
            signal_sum += sample_signal * range_weight;
            signal_weights += range_weight;
        }
        if scale == 1 {
            for (var direction_index = 0; direction_index < 8; direction_index = direction_index + 1) {
                let offset = Noise::NR_KNIGHT_DIRECTIONS[direction_index];
                let sample = finish_reference_at(pos + offset);
                let spatial = Noise::nr_offset_spatial_weight(offset);
                let sample_signal = Noise::nr_signal(sample);
                let sample_variance = Noise::nr_component_variance(sample);
                let range_weight = Noise::nr_signal_range_weight(
                    center_signal,
                    center_variance,
                    sample_signal,
                    sample_variance,
                    spatial,
                );
                signal_sum += sample_signal * range_weight;
                signal_weights += range_weight;
            }
        }
    }
    return Noise::nr_finish_signal(rgb, signal_sum, signal_weights);
}
