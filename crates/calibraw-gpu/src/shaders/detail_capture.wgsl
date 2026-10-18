#import calibraw::common as Common
#import calibraw::detail_utils as DetailUtils

// Some Mali Vulkan compilers crash when image operands are function parameters.
@group(0) @binding(22) var adjustment_base_tex: texture_2d<f32>;

fn adjustment_base_at(pos: vec2<i32>) -> vec3<f32> {
    return textureLoad(adjustment_base_tex, Common::clamp_pos(pos), 0).xyz;
}

fn capture_detail_scale() -> f32 {
    let tuning = Common::effects_uniforms.capture_scale_sigma;
    return clamp(sqrt(DetailUtils::presence_reference_scale()), tuning.x, tuning.y);
}

fn capture_sharpen_blur_ev(
    pos: vec2<i32>,
    radius_pixels: f32,
    step: i32,
) -> f32 {
    let center_ev = Common::log_luminance(adjustment_base_at(pos));
    let sigma_samples = clamp(
        radius_pixels / max(f32(step), 1.0),
        Common::effects_uniforms.capture_scale_sigma.z,
        Common::effects_uniforms.capture_scale_sigma.w
    );
    var weighted_sum = 0.0;
    var weight_sum = 0.0;

    for (var dy = -2; dy <= 2; dy = dy + 1) {
        for (var dx = -2; dx <= 2; dx = dx + 1) {
            let sample_ev = Common::log_luminance(
                adjustment_base_at(pos + vec2<i32>(dx * step, dy * step)),
            );
            let distance_squared = f32(dx * dx + dy * dy);
            let spatial = exp(-0.5 * distance_squared / (sigma_samples * sigma_samples));
            let delta = sample_ev - center_ev;
            let range = exp(-3.4 * delta * delta);
            let weight = spatial * range;
            weighted_sum = weighted_sum + sample_ev * weight;
            weight_sum = weight_sum + weight;
        }
    }
    return weighted_sum / max(weight_sum, 1e-6);
}

fn capture_sharpen_edge_strength(
    pos: vec2<i32>,
    step: i32,
) -> f32 {
    let left = Common::log_luminance(adjustment_base_at(pos + vec2<i32>(-step, 0)));
    let right = Common::log_luminance(adjustment_base_at(pos + vec2<i32>(step, 0)));
    let up = Common::log_luminance(adjustment_base_at(pos + vec2<i32>(0, -step)));
    let down = Common::log_luminance(adjustment_base_at(pos + vec2<i32>(0, step)));
    return length(vec2<f32>(right - left, down - up));
}

fn capture_local_ev_bounds(pos: vec2<i32>) -> vec2<f32> {
    var low = 1e20;
    var high = -1e20;
    for (var dy = -1; dy <= 1; dy = dy + 1) {
        for (var dx = -1; dx <= 1; dx = dx + 1) {
            let value = Common::log_luminance(adjustment_base_at(pos + vec2<i32>(dx, dy)));
            low = min(low, value);
            high = max(high, value);
        }
    }
    return vec2<f32>(low, high);
}

fn capture_impulse_coherence(
    pos: vec2<i32>,
    center_ev: f32,
) -> f32 {
    let left = Common::log_luminance(adjustment_base_at(pos + vec2<i32>(-1, 0)));
    let right = Common::log_luminance(adjustment_base_at(pos + vec2<i32>(1, 0)));
    let up = Common::log_luminance(adjustment_base_at(pos + vec2<i32>(0, -1)));
    let down = Common::log_luminance(adjustment_base_at(pos + vec2<i32>(0, 1)));
    let horizontal = min(abs(center_ev - left), abs(center_ev - right));
    let vertical = min(abs(center_ev - up), abs(center_ev - down));
    let support = min(horizontal, vertical);
    let tuning = Common::effects_uniforms.capture_mask_coherence;
    return 1.0 - smoothstep(tuning.z, tuning.w, support);
}

fn capture_noise_ev_sigma(rgb: vec3<f32>) -> f32 {
    let signal = max(dot(max(rgb, vec3<f32>(0.0)), vec3<f32>(0.25, 0.50, 0.25)), 1e-5);
    let channel_variance = max(
        Common::camera_uniforms.noise_read.rgb + Common::camera_uniforms.noise_shot.rgb * vec3<f32>(signal),
        vec3<f32>(1e-12),
    );
    let signal_variance = dot(
        channel_variance,
        vec3<f32>(0.25 * 0.25, 0.50 * 0.50, 0.25 * 0.25),
    );
    let relative_sigma = sqrt(max(signal_variance, 0.0)) / signal;
    let residual = mix(1.0, 0.58, clamp(Common::camera_uniforms.noise_options.x, 0.0, 1.0));
    return log2(1.0 + relative_sigma * residual);
}

// Mirrors MaskEffect::Sharpen.shader_id(). A Sharpen slot's `adjust_0_field`
// holds its controls in the units of the Detail card's `sharpen` uniform.
const MASK_EFFECT_SHARPEN_ID: u32 = 17u;

// Capture-sharpening controls, normalized.
struct CaptureSharpening {
    // Strength in 0..1 (Amount 0..150).
    amount: f32,
    // Blur radius in reference pixels, 0.5..3; `capture_detail_scale` maps it
    // to working pixels.
    radius: f32,
    // Weight of the finest band and noise threshold in 0..1 (Detail 0..100).
    detail: f32,
    // Restriction to strong edges in 0..1 (Masking 0..100).
    masking: f32,
}

// `controls` = (Amount 0..150, Radius 0.5..3, Detail 0..100, Masking 0..100),
// as the Detail card and Sharpen effect slots store them.
fn capture_sharpening(controls: vec4<f32>) -> CaptureSharpening {
    return CaptureSharpening(
        clamp(controls.x / 150.0, 0.0, 1.0),
        clamp(controls.y, 0.5, 3.0),
        clamp(controls.z / 100.0, 0.0, 1.0),
        clamp(controls.w / 100.0, 0.0, 1.0),
    );
}

fn global_capture_sharpening() -> CaptureSharpening {
    return capture_sharpening(Common::effects_uniforms.sharpen);
}

// Unlimited log2 luminance gain of `settings` at `pos` for a detail-band gain
// of `strength`: the selected band minus its noise threshold, gated by
// impulse coherence, edge masking and the shadow and highlight ranges. `rgb` is
// the unsharpened scene-linear pixel at `pos`; neighbours come from the
// adjustment base, so contributions of several settings do not compound.
fn capture_sharpening_response(
    pos: vec2<i32>,
    rgb: vec3<f32>,
    settings: CaptureSharpening,
    strength: f32,
) -> f32 {
    let radius = settings.radius;
    let detail = settings.detail;
    let masking = settings.masking;
    let radius_pixels = radius * capture_detail_scale();
    let step = clamp(i32(round(max(radius_pixels * 0.48, 1.0))), 1, 3);

    let center_ev = Common::log_luminance(rgb);
    let base_ev = capture_sharpen_blur_ev(pos, radius_pixels, step);
    let acutance_ev = center_ev - base_ev;

    let micro_left = Common::log_luminance(adjustment_base_at(pos + vec2<i32>(-1, 0)));
    let micro_right = Common::log_luminance(adjustment_base_at(pos + vec2<i32>(1, 0)));
    let micro_up = Common::log_luminance(adjustment_base_at(pos + vec2<i32>(0, -1)));
    let micro_down = Common::log_luminance(adjustment_base_at(pos + vec2<i32>(0, 1)));
    let micro_base_ev = 0.25 * (micro_left + micro_right + micro_up + micro_down);
    let micro_ev = center_ev - micro_base_ev;
    let selected_band = mix(acutance_ev, mix(acutance_ev, micro_ev, 0.42), detail * detail);

    let shadow_noise = 1.0 - smoothstep(-8.2, -3.3, center_ev);
    let fixed_threshold = mix(
        Common::effects_uniforms.capture_thresholds.x,
        Common::effects_uniforms.capture_thresholds.y,
        detail
    ) * mix(1.0, 2.3, shadow_noise);
    let edge_strength = capture_sharpen_edge_strength(pos, 1);
    let edge_noise_relief = smoothstep(
        Common::effects_uniforms.capture_thresholds.z,
        Common::effects_uniforms.capture_thresholds.w,
        edge_strength
    );
    let sensor_threshold = capture_noise_ev_sigma(rgb)
        * mix(0.52, 0.34, detail)
        * mix(1.0, 0.12, edge_noise_relief);
    let detail_threshold = max(fixed_threshold, sensor_threshold);
    let thresholded = DetailUtils::soft_detail_threshold(selected_band, detail_threshold);
    let coherence = mix(1.0, capture_impulse_coherence(pos, center_ev), 0.72 + 0.20 * detail);

    var edge_mask = 1.0;
    if masking > 1e-6 {
        let edge_threshold = mix(
            Common::effects_uniforms.capture_mask_coherence.x,
            Common::effects_uniforms.capture_mask_coherence.y,
            pow(masking, 1.35)
        );
        edge_mask = smoothstep(edge_threshold * 0.72, edge_threshold + 0.16, edge_strength);
    }

    let shadow_gate = smoothstep(-9.2, -4.3, center_ev);
    let highlight_gate = 1.0 - 0.62 * smoothstep(2.6, 6.0, center_ev);
    return thresholded * strength * coherence * edge_mask * shadow_gate * highlight_gate;
}

fn capture_band_gain(settings: CaptureSharpening) -> f32 {
    return settings.amount * mix(4.20, 6.00, settings.detail);
}

// Log2 luminance gain of the Detail card's capture sharpening, in
// -0.28..0.32 EV, before the local-range limit of `apply_capture_sharpening_ev`.
fn capture_sharpening_ev(
    pos: vec2<i32>,
    rgb: vec3<f32>,
    settings: CaptureSharpening,
) -> f32 {
    return clamp(
        capture_sharpening_response(pos, rgb, settings, capture_band_gain(settings)),
        -0.28,
        0.32,
    );
}

// The Sharpen effect detects detail like capture sharpening but boosts it
// SHARPEN_EFFECT_GAIN times as much and lets edges overshoot their
// neighbourhood by up to SHARPEN_EFFECT_HALO_EV at full Amount: the visible
// acutance halo that capture sharpening deliberately suppresses. Tuned on
// photographs so Amount 40 roughly matches the Detail card's Amount 150 and
// Amount 150 is about 2.5 times as strong.
const SHARPEN_EFFECT_GAIN: f32 = 4.0;
const SHARPEN_EFFECT_HALO_EV: f32 = 0.5;
// Soft limit of one slot's gain, in EV.
const SHARPEN_EFFECT_MAX_EV: f32 = 1.0;

// Log2 luminance gain of a Sharpen effect slot, softly limited to
// ±SHARPEN_EFFECT_MAX_EV, before `apply_sharpen_effect_ev`.
fn sharpen_effect_ev(pos: vec2<i32>, rgb: vec3<f32>, settings: CaptureSharpening) -> f32 {
    let response = capture_sharpening_response(
        pos,
        rgb,
        settings,
        capture_band_gain(settings) * SHARPEN_EFFECT_GAIN,
    );
    return SHARPEN_EFFECT_MAX_EV * tanh(response / SHARPEN_EFFECT_MAX_EV);
}

// How far a Sharpen effect slot at `settings` may overshoot the local range.
fn sharpen_effect_halo_ev(settings: CaptureSharpening) -> f32 {
    return settings.amount * SHARPEN_EFFECT_HALO_EV;
}

// Applies a summed `sharpen_effect_ev` gain to `rgb` at `pos`. Overshoot past
// the 3x3 neighbourhood's luminance range of the adjustment base is
// compressed smoothly towards `halo_ev`. Negative channels are clipped to zero.
fn apply_sharpen_effect_ev(
    pos: vec2<i32>,
    rgb: vec3<f32>,
    sharpen_ev: f32,
    halo_ev: f32,
) -> vec3<f32> {
    let center_ev = Common::log_luminance(rgb);
    let bounds = capture_local_ev_bounds(pos);
    let halo = max(halo_ev, 1e-4);
    var target_ev = center_ev + sharpen_ev;
    if target_ev > bounds.y {
        target_ev = bounds.y + halo * tanh((target_ev - bounds.y) / halo);
    } else if target_ev < bounds.x {
        target_ev = bounds.x - halo * tanh((bounds.x - target_ev) / halo);
    }
    // Never undo the existing contrast: a pixel already outside the range
    // keeps at least its own value in the direction of the gain.
    if sharpen_ev > 0.0 {
        target_ev = max(target_ev, center_ev);
    } else {
        target_ev = min(target_ev, center_ev);
    }
    return max(rgb * exp2(target_ev - center_ev), vec3<f32>(0.0));
}

// Applies a summed `capture_sharpening_ev` gain to the unsharpened `rgb` at
// `pos`. The result stays within the 3x3 neighbourhood's luminance range plus
// a small overshoot, so overlapping sharpening cannot ring past local extremes.
// Negative channels are clipped to zero.
fn apply_capture_sharpening_ev(
    pos: vec2<i32>,
    rgb: vec3<f32>,
    sharpen_ev: f32,
) -> vec3<f32> {
    let center_ev = Common::log_luminance(rgb);
    let bounds = capture_local_ev_bounds(pos);
    let target_ev = clamp(center_ev + sharpen_ev, bounds.x - 0.018, bounds.y + 0.018);
    return max(rgb * exp2(target_ev - center_ev), vec3<f32>(0.0));
}
