virtual fn finish_reference_at(_pos: vec2<i32>) -> vec3<f32> {
    return vec3<f32>(0.0);
}

#import calibraw::common as Common
#import calibraw::noise as Noise

// Keep the virtual declaration at byte zero: naga_oil 0.22 mishandles it after
// leading line comments. CFA entrypoints override this fallback.
fn finish_reference_bilinear(pos: vec2<f32>) -> vec3<f32> {
    let base = floor(pos);
    let p0 = vec2<i32>(i32(base.x), i32(base.y));
    let p1 = p0 + vec2<i32>(1, 1);
    let f = fract(pos);
    let a = finish_reference_at(p0);
    let b = finish_reference_at(vec2<i32>(p1.x, p0.y));
    let c = finish_reference_at(vec2<i32>(p0.x, p1.y));
    let d = finish_reference_at(p1);
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

fn finish_apply_ca(pos: vec2<i32>, rgb: vec3<f32>) -> vec3<f32> {
    var out = rgb;
    if abs(Common::camera_uniforms.ca_red) > 1e-6 {
        out.r = finish_reference_bilinear(Common::ca_warped_pos(pos, Common::camera_uniforms.ca_red)).r;
    }
    if abs(Common::camera_uniforms.ca_blue) > 1e-6 {
        out.b = finish_reference_bilinear(Common::ca_warped_pos(pos, Common::camera_uniforms.ca_blue)).b;
    }
    return out;
}

// Inverse of the CFA finish passes' opponent transform: Rec.2020 luma `y`
// with (B - Y) * 0.56433 and (R - Y) * 0.67815 opponents back to camera RGB.
fn finish_from_yuv(y: f32, uv: vec2<f32>) -> vec3<f32> {
    let b = y + uv.x / 0.56433;
    let r = y + uv.y / 0.67815;
    let g = (y - 0.2627 * r - 0.0593 * b) / 0.6780;
    return vec3<f32>(r, g, b);
}

fn finish_reference_luma(pos: vec2<i32>) -> f32 {
    return dot(finish_reference_at(pos), vec3<f32>(0.25, 0.50, 0.25));
}

// Scharr gradient magnitude of the reference luma, normalised by 32.
fn finish_scharr_detail_at(pos: vec2<i32>) -> f32 {
    let nw = finish_reference_luma(pos + vec2<i32>(-1, -1));
    let n  = finish_reference_luma(pos + vec2<i32>( 0, -1));
    let ne = finish_reference_luma(pos + vec2<i32>( 1, -1));
    let w  = finish_reference_luma(pos + vec2<i32>(-1,  0));
    let e  = finish_reference_luma(pos + vec2<i32>( 1,  0));
    let sw = finish_reference_luma(pos + vec2<i32>(-1,  1));
    let ss = finish_reference_luma(pos + vec2<i32>( 0,  1));
    let se = finish_reference_luma(pos + vec2<i32>( 1,  1));
    let gx = 3.0 * (ne - nw) + 10.0 * (e - w) + 3.0 * (se - sw);
    let gy = 3.0 * (sw - nw) + 10.0 * (ss - n) + 3.0 * (se - ne);
    return sqrt(gx * gx + gy * gy) / 32.0;
}

// Adapted from darktable 5.6.0 dual demosaicing (GPL-3.0-or-later), shared by
// the Bayer and X-Trans finish passes.
// Dual demosaic blend weight of the high-detail `reference` over `low`
// (whose alpha is its confidence). `opponent_delta` is the distance between
// their colour opponents, computed by the caller's own opponent transform.
fn finish_dual_high_weight(
    pos: vec2<i32>,
    reference: vec3<f32>,
    low: vec4<f32>,
    opponent_delta: f32,
) -> f32 {
    var detail = 0.0;
    for (var dy = -2; dy <= 2; dy = dy + 1) {
        let wy = Common::binomial5_weight(dy);
        for (var dx = -2; dx <= 2; dx = dx + 1) {
            detail += wy * Common::binomial5_weight(dx)
                * finish_scharr_detail_at(Common::clamp_pos(pos + vec2<i32>(dx, dy)));
        }
    }
    detail /= 256.0;

    let threshold = 0.005 * pow(max(Common::camera_uniforms.dual_threshold, 0.0), 1.1);
    if threshold <= 1e-7 { return 1.0; }

    let variance = Noise::nr_component_variance(0.5 * (reference + low.rgb));
    let noise_floor = 2.25 * sqrt(max(variance.x, 1e-10));
    let detail_signal = max(detail - noise_floor, 0.0);
    let edge_confidence = smoothstep(
        threshold,
        max(4.0 * threshold, threshold + 1e-5),
        detail_signal,
    );

    let opponent_sigma = max(sqrt(max(variance.y, 1e-10)), 0.0015);
    let disagreement = smoothstep(3.0 * opponent_sigma, 8.0 * opponent_sigma, opponent_delta);
    let low_confidence = clamp(low.a, 0.0, 1.0);
    let alias_penalty = 0.45 * disagreement * (1.0 - 0.35 * edge_confidence);
    let high_confidence = clamp(edge_confidence * (1.0 - alias_penalty), 0.0, 1.0);
    return clamp(1.0 - low_confidence * (1.0 - high_confidence), 0.0, 1.0);
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
