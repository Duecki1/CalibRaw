#import calibraw::common as Common
#import calibraw::color as Color
#import calibraw::profile as Profile
#import calibraw::look_lut as LookLut
#import calibraw::scene_adjustments as SceneAdjustments
#import calibraw::creative_effects as CreativeEffects
#import calibraw::tone_common as ToneCommon
#import calibraw::tonemap as Tonemap

const MIXER_HUE_RED: f32 = 0.0812052;
const MIXER_HUE_ORANGE: f32 = 0.1465993;
const MIXER_HUE_YELLOW: f32 = 0.3049145;
const MIXER_HUE_GREEN: f32 = 0.3958204;
const MIXER_HUE_AQUA: f32 = 0.5410248;
const MIXER_HUE_BLUE: f32 = 0.7334778;
const MIXER_HUE_PURPLE: f32 = 0.8160390;
const MIXER_HUE_MAGENTA: f32 = 0.9121206;

const MIXER_WIDTH_RED: f32 = 0.160;
const MIXER_WIDTH_ORANGE: f32 = 0.150;
const MIXER_WIDTH_YELLOW: f32 = 0.150;
const MIXER_WIDTH_GREEN: f32 = 0.140;
const MIXER_WIDTH_AQUA: f32 = 0.180;
const MIXER_WIDTH_BLUE: f32 = 0.180;
const MIXER_WIDTH_PURPLE: f32 = 0.100;
const MIXER_WIDTH_MAGENTA: f32 = 0.160;

struct MixerSample {
    lab: vec3<f32>,
    chroma: f32,
    hue_vector: vec2<f32>,
    confidence: f32,
}

struct MixerBandWeights {
    first: vec4<f32>,
    second: vec4<f32>,
    total: f32,
}

fn max_abs_vec4(value: vec4<f32>) -> f32 {
    return max(max(abs(value.x), abs(value.y)), max(abs(value.z), abs(value.w)));
}

fn smooth_hue_bell(hue: f32, anchor: f32, width: f32) -> f32 {
    let t = clamp(1.0 - Color::circular_hue_distance(hue, anchor) / width, 0.0, 1.0);
    let feather = t * t * (3.0 - 2.0 * t);
    return feather * feather;
}

fn mixer_band_weights(hue: f32) -> MixerBandWeights {
    let first = vec4<f32>(
        smooth_hue_bell(hue, MIXER_HUE_RED, MIXER_WIDTH_RED),
        smooth_hue_bell(hue, MIXER_HUE_ORANGE, MIXER_WIDTH_ORANGE),
        smooth_hue_bell(hue, MIXER_HUE_YELLOW, MIXER_WIDTH_YELLOW),
        smooth_hue_bell(hue, MIXER_HUE_GREEN, MIXER_WIDTH_GREEN),
    );
    let second = vec4<f32>(
        smooth_hue_bell(hue, MIXER_HUE_AQUA, MIXER_WIDTH_AQUA),
        smooth_hue_bell(hue, MIXER_HUE_BLUE, MIXER_WIDTH_BLUE),
        smooth_hue_bell(hue, MIXER_HUE_PURPLE, MIXER_WIDTH_PURPLE),
        smooth_hue_bell(hue, MIXER_HUE_MAGENTA, MIXER_WIDTH_MAGENTA),
    );
    return MixerBandWeights(first, second, dot(first, vec4<f32>(1.0)) + dot(second, vec4<f32>(1.0)));
}

fn mixer_band_value(weights: MixerBandWeights, first: vec4<f32>, second: vec4<f32>) -> f32 {
    return (dot(weights.first, first) + dot(weights.second, second)) / max(weights.total, 1e-6);
}

fn directed_hue_shift(value: f32, backward_span: f32, forward_span: f32) -> f32 {
    let amount = clamp(value / 100.0, -2.0, 2.0);
    let span = select(backward_span, forward_span, amount >= 0.0);
    return amount * span * 0.90;
}

fn mixer_hue_shift_values(weights: MixerBandWeights, first_values: vec4<f32>, second_values: vec4<f32>) -> f32 {
    let first = vec4<f32>(
        directed_hue_shift(first_values.x, 0.1690846, 0.0653940),
        directed_hue_shift(first_values.y, 0.0653940, 0.1583152),
        directed_hue_shift(first_values.z, 0.1583152, 0.0909059),
        directed_hue_shift(first_values.w, 0.0909059, 0.1452044),
    );
    let second = vec4<f32>(
        directed_hue_shift(second_values.x, 0.1452044, 0.1924530),
        directed_hue_shift(second_values.y, 0.1924530, 0.0825612),
        directed_hue_shift(second_values.z, 0.0825612, 0.0960816),
        directed_hue_shift(second_values.w, 0.0960816, 0.1690846),
    );
    return (dot(weights.first, first) + dot(weights.second, second)) / max(weights.total, 1e-6);
}

// How colourful an OKLab colour of chroma `chroma` and lightness `lightness`
// is, the way the colour mixer and the black-and-white mix measure it: chroma
// relative to what a deep colour of that lightness reaches, so 1 or more is a
// deep colour and near 0 is near-neutral.
fn mixer_relative_chroma(chroma: f32, lightness: f32) -> f32 {
    return chroma / max(0.028 + 0.095 * max(lightness, 0.0), 0.028);
}

// 0 in the deepest shadows, where colour is mostly noise, rising to 1 by
// OKLab lightness 0.115.
fn mixer_signal_confidence(lightness: f32) -> f32 {
    return smoothstep(0.035, 0.115, max(lightness, 0.0));
}

// 1 up to OKLab lightness 1 (diffuse white), easing off above it so HDR
// highlights are not pushed further.
fn mixer_hdr_guard(lightness: f32) -> f32 {
    return 1.0 / (1.0 + 0.20 * max(lightness - 1.0, 0.0));
}

fn mixer_sample_from_rgb(rgb: vec3<f32>) -> MixerSample {
    let lab = Color::linear_srgb_to_oklab(Common::REC2020_TO_SRGB * rgb);
    let chroma = length(lab.yz);
    var hue_vector = vec2<f32>(1.0, 0.0);
    if chroma > 1e-9 {
        hue_vector = lab.yz / chroma;
    }

    let relative_chroma = mixer_relative_chroma(chroma, lab.x);
    let chroma_confidence = smoothstep(0.10, 0.62, relative_chroma);
    let signal_confidence = mixer_signal_confidence(lab.x);
    let confidence = chroma_confidence * signal_confidence;
    return MixerSample(lab, chroma, hue_vector, confidence);
}

fn stabilized_mixer_sample(pos: vec2<i32>, center_rgb: vec3<f32>) -> MixerSample {
    let center = mixer_sample_from_rgb(center_rgb);
    if center.confidence < 1e-5 {
        return center;
    }

    var vector_sum = center.hue_vector * center.confidence * 4.0;
    var weight_sum = center.confidence * 4.0;
    for (var dy = -2; dy <= 2; dy = dy + 1) {
        for (var dx = -2; dx <= 2; dx = dx + 1) {
            if dx == 0 && dy == 0 {
                continue;
            }
            let neighbour_rgb = textureLoad(SceneAdjustments::final_adjustment_tex, Common::clamp_pos(pos + vec2<i32>(dx, dy)), 0).xyz;
            let neighbour = mixer_sample_from_rgb(neighbour_rgb);
            if neighbour.confidence < 1e-5 {
                continue;
            }

            let distance_squared = f32(dx * dx + dy * dy);
            let spatial = 1.0 / (1.0 + 0.65 * distance_squared);
            let lightness_delta = neighbour.lab.x - center.lab.x;
            let chroma_delta = neighbour.chroma - center.chroma;
            let hue_agreement = clamp(dot(center.hue_vector, neighbour.hue_vector), -1.0, 1.0);
            let range_weight = 1.0 / (
                1.0
                + 72.0 * lightness_delta * lightness_delta
                + 34.0 * chroma_delta * chroma_delta
                + 8.0 * (1.0 - hue_agreement)
            );
            let weight = spatial * range_weight * neighbour.confidence;
            vector_sum = vector_sum + neighbour.hue_vector * weight;
            weight_sum = weight_sum + weight;
        }
    }

    var stable_hue = center.hue_vector;
    let vector_length = length(vector_sum);
    if weight_sum > 1e-5 && vector_length > 1e-5 {
        stable_hue = vector_sum / vector_length;
    }
    return MixerSample(center.lab, center.chroma, stable_hue, center.confidence);
}

fn mixer_saturation_factor(amount: f32) -> f32 {
    let value = clamp(amount, -1.0, 1.0);
    if value >= 0.0 {
        return exp2(value * 0.85);
    }
    return max(1.0 + value, 0.0);
}

fn mixer_luminance_ev(amount: f32, lightness: f32) -> f32 {
    let value = clamp(amount, -1.0, 1.0);
    let endpoint_ev = select(1.45, 1.20, value >= 0.0);
    let signal = smoothstep(0.040, 0.135, max(lightness, 0.0));
    return value * endpoint_ev * signal * mixer_hdr_guard(lightness);
}

fn apply_hue_rotation_value(input_rgb: vec3<f32>, degrees: f32) -> vec3<f32> {
    let rotation = clamp(degrees, -180.0, 180.0);
    if abs(rotation) < 1e-7 {
        return input_rgb;
    }

    let lab = Color::rec2020_to_oklab(input_rgb);
    let chroma = length(lab.yz);
    if chroma < 1e-7 {
        return input_rgb;
    }

    let angle = atan2(lab.z, lab.y) + rotation * 3.14159265359 / 180.0;
    let target_hue = vec2<f32>(cos(angle), sin(angle));
    return Color::perceptual_rec2020_from_oklab_nonnegative(
        lab.x,
        target_hue,
        chroma,
    );
}

fn apply_local_hue_rotations(pos: vec2<i32>, input_rgb: vec3<f32>) -> vec3<f32> {
    var rgb = input_rgb;
    let count = Common::mask_slot_count();
    for (var index = 0u; index < count; index = index + 1u) {
        let state = Common::mask_metadata(index);
        if state.x == 0u || Common::mask_effect_id(state) != 0u || (state.w & 4u) == 0u { continue; }
        let degrees = Common::mask_grade_options(index).z;
        if abs(degrees) < 1e-7 { continue; }
        let weight = SceneAdjustments::local_mask_weight(pos, index);
        if weight <= 1e-5 { continue; }

        rgb = apply_hue_rotation_value(rgb, degrees * weight);
    }
    return rgb;
}

fn color_grade_strength(
    shadows: vec4<f32>,
    midtones: vec4<f32>,
    highlights: vec4<f32>,
    global: vec4<f32>,
) -> f32 {
    return max(
        max(max(abs(shadows.y), abs(shadows.z)), max(abs(midtones.y), abs(midtones.z))),
        max(max(abs(highlights.y), abs(highlights.z)), max(abs(global.y), abs(global.z))),
    );
}

fn color_grade_vector(wheel: vec4<f32>) -> vec2<f32> {
    let angle = wheel.x * 2.0 * 3.14159265359;
    return vec2<f32>(cos(angle), sin(angle)) * clamp(wheel.y, 0.0, 1.0);
}

fn color_grade_tonal_weights(luminance: f32, options: vec4<f32>) -> vec3<f32> {
    let ev = log2(max(luminance, 1e-7) / ToneCommon::SCENE_MIDDLE_GREY);
    let width = mix(0.60, 2.80, clamp(options.x, 0.0, 1.0));
    let pivot = -clamp(options.y, -1.0, 1.0) * 1.5;
    let shadows = 1.0 - smoothstep(
        -1.25 + pivot - 0.5 * width,
        -1.25 + pivot + 0.5 * width,
        ev,
    );
    let highlights = smoothstep(
        1.25 + pivot - 0.5 * width,
        1.25 + pivot + 0.5 * width,
        ev,
    );
    let midtones = max(1.0 - shadows - highlights, 0.0);
    let total = max(shadows + midtones + highlights, 1e-6);
    return vec3<f32>(shadows, midtones, highlights) / total;
}

fn apply_color_grading_wheels(
    input_rgb: vec3<f32>,
    shadows: vec4<f32>,
    midtones: vec4<f32>,
    highlights: vec4<f32>,
    global: vec4<f32>,
    options: vec4<f32>,
) -> vec3<f32> {
    if color_grade_strength(shadows, midtones, highlights, global) < 1e-7 {
        return input_rgb;
    }

    let rgb = input_rgb;
    let luminance = max(dot(rgb, Common::LUMA), 0.0);
    let weights = color_grade_tonal_weights(luminance, options);
    let lab = Color::linear_srgb_to_oklab(Common::REC2020_TO_SRGB * rgb);

    let grade_vector = color_grade_vector(shadows) * weights.x
        + color_grade_vector(midtones) * weights.y
        + color_grade_vector(highlights) * weights.z
        + color_grade_vector(global);

    var adjusted = rgb;
    if dot(grade_vector, grade_vector) > 1e-12 {
        let signal = smoothstep(0.025, 0.115, max(lab.x, 0.0));
        let hdr_guard = 1.0 / (1.0 + 0.25 * max(lab.x - 1.0, 0.0));
        let existing_chroma = length(lab.yz);
        let saturation_guard = 1.0 / (1.0 + 1.8 * existing_chroma);
        let target_ab = lab.yz + grade_vector * (0.135 * signal * hdr_guard * saturation_guard);
        let target_chroma = length(target_ab);
        if target_chroma > 1e-8 {
            adjusted = Color::perceptual_rec2020_from_oklab_nonnegative(
                lab.x,
                target_ab / target_chroma,
                target_chroma,
            );
        }
    }

    let luminance_grade = shadows.z * weights.x
        + midtones.z * weights.y
        + highlights.z * weights.z
        + global.z;
    if abs(luminance_grade) > 1e-7 {
        adjusted = adjusted * exp2(mixer_luminance_ev(luminance_grade, lab.x));
    }
    return adjusted;
}

fn apply_local_color_grading(pos: vec2<i32>, input_rgb: vec3<f32>) -> vec3<f32> {
    var rgb = input_rgb;
    let count = Common::mask_slot_count();
    for (var index = 0u; index < count; index = index + 1u) {
        let state = Common::mask_metadata(index);
        if state.x == 0u || Common::mask_effect_id(state) != 0u || (state.w & 2u) == 0u { continue; }
        let weight = SceneAdjustments::local_mask_weight(pos, index);
        if weight <= 1e-5 { continue; }
        let adjusted = apply_color_grading_wheels(
            rgb,
            Common::mask_grade_shadows(index),
            Common::mask_grade_midtones(index),
            Common::mask_grade_highlights(index),
            Common::mask_grade_global(index),
            Common::mask_grade_options(index),
        );
        rgb = mix(rgb, adjusted, weight);
    }
    return rgb;
}

fn apply_color_mixer_values(
    pos: vec2<i32>,
    rgb: vec3<f32>,
    hue_0: vec4<f32>,
    hue_1: vec4<f32>,
    saturation_0: vec4<f32>,
    saturation_1: vec4<f32>,
    luminance_0: vec4<f32>,
    luminance_1: vec4<f32>,
) -> vec3<f32> {
    let strengths = vec3<f32>(
        max(max_abs_vec4(hue_0), max_abs_vec4(hue_1)),
        max(max_abs_vec4(saturation_0), max_abs_vec4(saturation_1)),
        max(max_abs_vec4(luminance_0), max_abs_vec4(luminance_1)),
    );
    if max(strengths.x, max(strengths.y, strengths.z)) < 1e-6 {
        return rgb;
    }

    let sample = stabilized_mixer_sample(pos, rgb);
    if sample.confidence < 1e-5 {
        return rgb;
    }

    let selector_hue = fract(atan2(sample.hue_vector.y, sample.hue_vector.x) / (2.0 * 3.14159265359) + 1.0);
    let weights = mixer_band_weights(selector_hue);
    let hue_shift = mixer_hue_shift_values(weights, hue_0, hue_1) * sample.confidence;
    let saturation_amount = mixer_band_value(weights, saturation_0, saturation_1)
        / 100.0 * sample.confidence;
    let luminance_amount = mixer_band_value(weights, luminance_0, luminance_1)
        / 100.0 * sample.confidence;

    if max(abs(hue_shift), max(abs(saturation_amount), abs(luminance_amount))) < 1e-7 {
        return rgb;
    }

    var adjusted = rgb;
    if abs(hue_shift) > 1e-7 || abs(saturation_amount) > 1e-7 {
        let center_hue = sample.lab.yz / max(sample.chroma, 1e-9);
        let center_angle = atan2(center_hue.y, center_hue.x);
        let target_angle = center_angle + hue_shift * 2.0 * 3.14159265359;
        let target_hue = vec2<f32>(cos(target_angle), sin(target_angle));
        let target_chroma = sample.chroma * mixer_saturation_factor(saturation_amount);
        adjusted = Color::perceptual_rec2020_from_oklab_nonnegative(
            sample.lab.x,
            target_hue,
            target_chroma,
        );
    }

    if abs(luminance_amount) > 1e-7 {
        adjusted = adjusted * exp2(mixer_luminance_ev(luminance_amount, sample.lab.x));
        adjusted = Color::perceptual_gamut_compress_nonnegative_rec2020(adjusted);
    }
    return adjusted;
}

fn apply_color_mixer(pos: vec2<i32>, rgb: vec3<f32>) -> vec3<f32> {
    return apply_color_mixer_values(
        pos,
        rgb,
        Common::scene_tone_uniforms.hsl_hue_0_field,
        Common::scene_tone_uniforms.hsl_hue_1_field,
        Common::scene_tone_uniforms.hsl_saturation_0_field,
        Common::scene_tone_uniforms.hsl_saturation_1_field,
        Common::scene_tone_uniforms.hsl_luminance_0_field,
        Common::scene_tone_uniforms.hsl_luminance_1_field,
    );
}

// Black and white (Rust `Treatment::Monochrome`). The view node turns the
// photo grey after the colour mixers: each pixel keeps its OKLab lightness,
// the grey of Saturation -100. The mixer's Luminance values are the mix: each
// brightens or darkens the grey of the colours of its band, as a colour
// filter does on black-and-white film (Lightroom's B&W mix). The mix moves a
// pixel by how colourful it is, so neutral greys stay where they are and a
// deep colour moves more than a pale one of the same hue.

// The gain at ±100 on a fully colourful pixel, in EV. Further than the colour
// mixer's Luminance (mixer_luminance_ev): without colour there is no hue to
// break, and a red filter takes a blue sky down by two to three stops.
const MONOCHROME_MIX_DARKEN_EV: f32 = 3.0;
const MONOCHROME_MIX_BRIGHTEN_EV: f32 = 2.0;
// The colour estimate (monochrome_mix_colour) weighs a neighbour by
// exp(-(d² / SPATIAL + (ΔL / LIGHTNESS)² + |Δab|² / COLOUR²)), d in pixels and
// ΔL, Δab in OKLab.
const MONOCHROME_MIX_SPATIAL: f32 = 4.5;
const MONOCHROME_MIX_LIGHTNESS: f32 = 0.05;
const MONOCHROME_MIX_COLOUR: f32 = 0.05;

// The OKLab colour (a, b) the mix reads at `pos`: the pixel's own averaged
// over its 5×5 neighbourhood with weights that fall with distance and with the
// difference in lightness and colour from the pixel (a joint bilateral
// filter). Averaging the ab vector, not chroma, lets chroma noise cancel: a
// neutral pixel's noise averages towards grey and the mix leaves it, and a
// flat colour such as a noisy blue sky gets one gain instead of turning its
// chroma noise into grain when the mix moves it by stops. A neighbour across
// an edge differs in lightness or colour and barely counts, so the mix keeps
// to its side of the edge. `center_lab` is the pixel's OKLab. Reads the same
// taps of final_adjustment_tex as the colour mixer (stabilized_mixer_sample),
// which COLOR_MIXER_SUPPORT keeps in an export tile's halo.
fn monochrome_mix_colour(pos: vec2<i32>, center_lab: vec3<f32>) -> vec2<f32> {
    var ab_sum = center_lab.yz;
    var weight_sum = 1.0;
    for (var dy = -2; dy <= 2; dy = dy + 1) {
        for (var dx = -2; dx <= 2; dx = dx + 1) {
            if dx == 0 && dy == 0 {
                continue;
            }
            let neighbour_rgb = textureLoad(SceneAdjustments::final_adjustment_tex, Common::clamp_pos(pos + vec2<i32>(dx, dy)), 0).xyz;
            let neighbour = Color::rec2020_to_oklab(neighbour_rgb);
            let lightness_delta = (neighbour.x - center_lab.x) / MONOCHROME_MIX_LIGHTNESS;
            let colour_delta = (neighbour.yz - center_lab.yz) / MONOCHROME_MIX_COLOUR;
            let distance_squared = f32(dx * dx + dy * dy);
            let weight = exp(-(
                distance_squared / MONOCHROME_MIX_SPATIAL
                + lightness_delta * lightness_delta
                + dot(colour_delta, colour_delta)
            ));
            ab_sum = ab_sum + neighbour.yz * weight;
            weight_sum = weight_sum + weight;
        }
    }
    return ab_sum / weight_sum;
}

// The mix's gain in EV for a pixel of OKLab lightness `lightness` and colour
// `ab`; `luminance_0` and `luminance_1` are the eight bands' values (±100).
// How colourful the pixel is, its chroma relative to its lightness as the
// colour mixer measures it, ramps the gain in: from nothing at near-neutral
// chroma, which noise reaches, to all of it at the chroma of a deep colour.
// Like the colour mixer's Luminance, it fades out in the deepest shadows,
// where colour is mostly noise, and eases off above diffuse white.
fn monochrome_mix_ev(
    lightness: f32,
    ab: vec2<f32>,
    luminance_0: vec4<f32>,
    luminance_1: vec4<f32>,
) -> f32 {
    let chroma = length(ab);
    if chroma <= 1e-9 {
        return 0.0;
    }
    let hue = fract(atan2(ab.y, ab.x) / (2.0 * 3.14159265359) + 1.0);
    let amount = clamp(mixer_band_value(mixer_band_weights(hue), luminance_0, luminance_1) / 100.0, -1.0, 1.0);
    let colourfulness = smoothstep(0.10, 1.60, mixer_relative_chroma(chroma, lightness));
    let signal = mixer_signal_confidence(lightness);
    let hdr_guard = mixer_hdr_guard(lightness);
    let endpoint_ev = select(MONOCHROME_MIX_DARKEN_EV, MONOCHROME_MIX_BRIGHTEN_EV, amount >= 0.0);
    return amount * endpoint_ev * colourfulness * signal * hdr_guard;
}

// The global mix at `pos` applied to `rgb`, the pixel there in scene-linear
// Rec.2020: scaled by the mix's gain, still in colour so that the masks' colour
// mixers can mix it further before monochrome_grey. Scaling linear RGB by g
// scales its OKLab lightness by g^(1/3), so the grey moves by the same g.
fn apply_monochrome_mix(pos: vec2<i32>, rgb: vec3<f32>) -> vec3<f32> {
    let luminance_0 = Common::scene_tone_uniforms.hsl_luminance_0_field;
    let luminance_1 = Common::scene_tone_uniforms.hsl_luminance_1_field;
    if max(max_abs_vec4(luminance_0), max_abs_vec4(luminance_1)) < 1e-6 {
        return rgb;
    }
    let lab = Color::rec2020_to_oklab(rgb);
    let ab = monochrome_mix_colour(pos, lab);
    let ev = monochrome_mix_ev(lab.x, ab, luminance_0, luminance_1);
    if abs(ev) < 1e-7 {
        return rgb;
    }
    return rgb * exp2(ev);
}

// The grey of `rgb` (scene-linear Rec.2020) that keeps its OKLab lightness L:
// L³ in every channel, which is OKLab (L, 0, 0) exactly and stays neutral
// through the view transform. Rec.2020 and sRGB share D65 white.
fn monochrome_grey(rgb: vec3<f32>) -> vec3<f32> {
    let lightness = max(Color::rec2020_to_oklab(rgb).x, 0.0);
    return vec3<f32>(lightness * lightness * lightness);
}

fn point_color_hsl(rgb: vec3<f32>) -> vec3<f32> {
    let linear_srgb = clamp(Common::REC2020_TO_SRGB * rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    let srgb = Color::srgb_oetf(linear_srgb);
    let cmax = max(srgb.r, max(srgb.g, srgb.b));
    let cmin = min(srgb.r, min(srgb.g, srgb.b));
    let delta = cmax - cmin;
    let luminance = 0.5 * (cmax + cmin);
    if delta < 1e-7 {
        return vec3<f32>(0.0, 0.0, luminance);
    }
    var hue = 0.0;
    if cmax == srgb.r {
        hue = (srgb.g - srgb.b) / delta;
        if hue < 0.0 { hue = hue + 6.0; }
    } else if cmax == srgb.g {
        hue = (srgb.b - srgb.r) / delta + 2.0;
    } else {
        hue = (srgb.r - srgb.g) / delta + 4.0;
    }
    hue = hue / 6.0;
    let saturation = delta / max(1.0 - abs(2.0 * luminance - 1.0), 1e-7);
    return vec3<f32>(fract(hue + 1.0), saturation, luminance);
}

fn point_color_range_weight(value: f32, range: vec4<f32>) -> f32 {
    let distance_min = value - range.y;
    let distance_max = range.z - value;
    if distance_min >= 0.0 && distance_max >= 0.0 { return 1.0; }
    if distance_min < 0.0 {
        let span = max(range.y - range.x, 1e-5);
        return smoothstep(0.0, 1.0, 1.0 + distance_min / span);
    }
    let span = max(range.w - range.z, 1e-5);
    return smoothstep(0.0, 1.0, 1.0 + distance_max / span);
}

fn point_color_hue_weight(value: f32, range: vec4<f32>, scale: f32) -> f32 {
    let wrapped = fract(value + 1.0);
    var best = 0.0;
    for (var offset = -1; offset <= 1; offset = offset + 1) {
        best = max(best, point_color_range_weight((wrapped + f32(offset)) / scale, range));
    }
    return best;
}

fn point_color_hsl_to_rgb(hsl: vec3<f32>) -> vec3<f32> {
    let h = fract(hsl.x + 1.0) * 6.0;
    let s = clamp(hsl.y, 0.0, 1.0);
    let l = clamp(hsl.z, 0.0, 1.0);
    let chroma = (1.0 - abs(2.0 * l - 1.0)) * s;
    let x = chroma * (1.0 - abs(fract(h / 2.0) * 2.0 - 1.0));
    var rgb = vec3<f32>(0.0);
    if h < 1.0 { rgb = vec3<f32>(chroma, x, 0.0); }
    else if h < 2.0 { rgb = vec3<f32>(x, chroma, 0.0); }
    else if h < 3.0 { rgb = vec3<f32>(0.0, chroma, x); }
    else if h < 4.0 { rgb = vec3<f32>(0.0, x, chroma); }
    else if h < 5.0 { rgb = vec3<f32>(x, 0.0, chroma); }
    else { rgb = vec3<f32>(chroma, 0.0, x); }
    let m = l - 0.5 * chroma;
    let encoded = rgb + vec3<f32>(m);
    let magnitude = abs(encoded);
    let lo = encoded / 12.92;
    let hi = sign(encoded) * pow((magnitude + 0.055) / 1.055, vec3<f32>(2.4));
    let cutoff = step(vec3<f32>(0.04045), magnitude);
    return Common::SRGB_TO_REC2020 * mix(lo, hi, cutoff);
}

fn point_color_weight(sample: vec3<f32>, point: Common::PointColor) -> f32 {
    let range_scale = 0.2 + 1.6 * clamp(point.sample_range.w, 0.0, 100.0) / 100.0;
    let selected_hue = point_color_hue_weight(sample.x - point.sample_range.x, point.hue_range, range_scale);
    let saturation_weight = point_color_range_weight((sample.y - point.sample_range.y) / range_scale, point.saturation_range);
    let luminance_weight = point_color_range_weight((sample.z - point.sample_range.z) / range_scale, point.luminance_range);
    // Hue on nearly neutral pixels is mostly noise. Suppress those pixels
    // when the selected target has a meaningful chroma, while preserving
    // neutral targets whose hue is intentionally irrelevant.
    let sample_chroma = (1.0 - abs(2.0 * sample.z - 1.0)) * sample.y;
    let target_chroma = (1.0 - abs(2.0 * point.sample_range.z - 1.0)) * point.sample_range.y;
    let colored_target = smoothstep(0.04, 0.12, target_chroma);
    let reliable_sample = smoothstep(0.008, 0.06, sample_chroma);
    let hue_weight = mix(1.0, selected_hue, colored_target);
    return hue_weight * saturation_weight * luminance_weight
        * mix(1.0, reliable_sample, colored_target);
}

fn point_color_selection_weight(sample: vec3<f32>, index: u32) -> f32 {
    if index >= min(Common::scene_tone_uniforms.point_color_meta.x, 8u) { return 0.0; }
    return point_color_weight(sample, Common::scene_tone_uniforms.point_colors[index]);
}

fn point_color_guided_delta(pos: vec2<i32>, center: vec3<f32>, display_rgb: vec3<f32>) -> vec3<f32> {
    // Select from a small, edge-aware neighborhood. Keep the original pixel
    // for the color transform so texture and fine detail remain intact.
    let scale = 1.0 / max(dot(center, Common::LUMA), 0.04);
    var total = 1.0;
    var sum = center * total;
    for (var dy = -1; dy <= 1; dy = dy + 1) {
        for (var dx = -1; dx <= 1; dx = dx + 1) {
            if dx == 0 && dy == 0 { continue; }
            let neighbor = textureLoad(SceneAdjustments::final_adjustment_tex, Common::clamp_pos(pos + vec2<i32>(dx, dy)), 0).xyz;
            let difference = (neighbor - center) * scale;
            let lightness_difference = dot(difference, Common::LUMA);
            let chroma_difference = difference - vec3<f32>(lightness_difference);
            let spatial = 1.0 / (1.0 + 0.5 * f32(dx * dx + dy * dy));
            let edge = 1.0 / (1.0 + 24.0 * lightness_difference * lightness_difference
                + 6.0 * dot(chroma_difference, chroma_difference));
            let weight = spatial * edge;
            sum = sum + neighbor * weight;
            total = total + weight;
        }
    }
    let display_scale = clamp(dot(display_rgb, Common::LUMA) * scale, 0.0, 8.0);
    return (sum / total - center) * display_scale;
}

fn apply_point_color_values(input_rgb: vec3<f32>, sample: vec3<f32>, shifts: vec3<f32>) -> vec3<f32> {
    if max(abs(shifts.x), max(abs(shifts.y), abs(shifts.z))) < 1e-7 { return input_rgb; }
    let residual = input_rgb - point_color_hsl_to_rgb(sample);
    // A saturation increase should strengthen existing color, not inject a
    // saturated arbitrary hue into grayscale noise.
    let adjusted = vec3<f32>(sample.x + shifts.x, sample.y * max(1.0 + shifts.y, 0.0), sample.z + shifts.z);
    return residual + point_color_hsl_to_rgb(adjusted);
}

fn apply_point_colors(input_rgb: vec3<f32>, selection_sample: vec3<f32>) -> vec3<f32> {
    let count = min(Common::scene_tone_uniforms.point_color_meta.x, 8u);
    if count == 0u { return input_rgb; }
    let sample = point_color_hsl(input_rgb);
    var color_delta = vec3<f32>(0.0);
    for (var index = 0u; index < count; index = index + 1u) {
        let point = Common::scene_tone_uniforms.point_colors[index];
        let weight = point_color_selection_weight(selection_sample, index);
        if weight <= 1e-5 { continue; }
        let adjusted = apply_point_color_values(input_rgb, sample, point.shifts.xyz);
        color_delta = color_delta + (adjusted - input_rgb) * weight;
    }
    if max(abs(color_delta.x), max(abs(color_delta.y), abs(color_delta.z))) < 1e-7 { return input_rgb; }
    return Color::perceptual_gamut_compress_nonnegative_rec2020(input_rgb + color_delta);
}

fn apply_local_point_colors(pos: vec2<i32>, input_rgb: vec3<f32>, guided_delta: vec3<f32>) -> vec3<f32> {
    if (Common::scene_tone_uniforms.point_color_meta.w & 1u) == 0u { return input_rgb; }
    var rgb = input_rgb;
    let count = Common::mask_slot_count();
    for (var index = 0u; index < count; index = index + 1u) {
        let state = Common::mask_metadata(index);
        if state.x == 0u || Common::mask_effect_id(state) != 0u { continue; }
        let color_count = min(Common::mask_point_color_meta(index).x, 8u);
        if color_count == 0u { continue; }
        let mask_weight = SceneAdjustments::local_mask_weight(pos, index);
        if mask_weight <= 1e-5 { continue; }
        let sample = point_color_hsl(rgb);
        let selection_sample = point_color_hsl(max(rgb + guided_delta, vec3<f32>(0.0)));
        var color_delta = vec3<f32>(0.0);
        for (var color_index = 0u; color_index < color_count; color_index = color_index + 1u) {
            let point = Common::mask_point_colors(index, color_index);
            let weight = point_color_weight(selection_sample, point);
            if weight <= 1e-5 { continue; }
            let adjusted = apply_point_color_values(rgb, sample, point.shifts.xyz);
            color_delta = color_delta + (adjusted - rgb) * weight;
        }
        if max(abs(color_delta.x), max(abs(color_delta.y), abs(color_delta.z))) > 1e-7 {
            let adjusted = Color::perceptual_gamut_compress_nonnegative_rec2020(rgb + color_delta);
            rgb = mix(rgb, adjusted, mask_weight);
        }
    }
    return rgb;
}

fn local_point_color_visualization(pos: vec2<i32>, selection_sample: vec3<f32>) -> f32 {
    if (Common::scene_tone_uniforms.point_color_meta.w & 2u) == 0u { return 0.0; }
    let count = Common::mask_slot_count();
    for (var index = 0u; index < count; index = index + 1u) {
        let state = Common::mask_metadata(index);
        let color_meta = Common::mask_point_color_meta(index);
        let selected = color_meta.y;
        if selected == 0u || selected > color_meta.x || state.x == 0u || Common::mask_effect_id(state) != 0u { continue; }
        let mask_weight = SceneAdjustments::local_mask_weight(pos, index);
        return mask_weight * point_color_weight(selection_sample, Common::mask_point_colors(index, selected - 1u));
    }
    return 0.0;
}

fn apply_local_color_mixer(pos: vec2<i32>, input_rgb: vec3<f32>) -> vec3<f32> {
    var rgb = input_rgb;
    let count = Common::mask_slot_count();
    for (var index = 0u; index < count; index = index + 1u) {
        let state = Common::mask_metadata(index);
        if state.x == 0u || Common::mask_effect_id(state) != 0u || (state.w & 1u) == 0u { continue; }
        let weight = SceneAdjustments::local_mask_weight(pos, index);
        if weight <= 1e-5 { continue; }

        let adjusted = apply_color_mixer_values(
            pos,
            rgb,
            Common::mask_hsl_hue_0_field(index),
            Common::mask_hsl_hue_1_field(index),
            Common::mask_hsl_saturation_0_field(index),
            Common::mask_hsl_saturation_1_field(index),
            Common::mask_hsl_luminance_0_field(index),
            Common::mask_hsl_luminance_1_field(index),
        );
        rgb = mix(rgb, adjusted, weight);
    }
    return rgb;
}

// VIEW_TRANSFORM_PROFILE_TONE: the camera profile's tone curve renders the
// scene as the DNG SDK applies a ProfileToneCurve (RefBaselineRGBTone): in
// linear ProPhoto RGB, the largest and the smallest channel take the curve's
// values (behind its shoulder, Profile::profile_tone_value) and the middle
// channel keeps its share of the distance between them, which locks the RGB
// hue. `view_input` is non-negative scene-linear Rec.2020; the result is
// display-linear Rec.2020, with ProPhoto colours outside Rec.2020 projected
// into it.
fn apply_profile_tone_view_transform(view_input: vec3<f32>) -> vec3<f32> {
    let profile_signed = Profile::REC2020_TO_PROPHOTO * view_input;
    let profile_linear = Color::gamut_project_nonnegative(
        profile_signed,
        dot(profile_signed, Profile::PROPHOTO_LUMA),
    );
    let range = Tonemap::channel_range(profile_linear);
    let toned = Tonemap::hue_locked_tone(
        profile_linear,
        range,
        vec2<f32>(Profile::profile_tone_value(range.x), Profile::profile_tone_value(range.y)),
    );
    return Color::gamut_project_nonnegative_rec2020(Profile::PROPHOTO_TO_REC2020 * toned);
}

fn apply_view_transform(scene_rgb: vec3<f32>) -> vec3<f32> {
    let view_input = Color::gamut_project_nonnegative_rec2020(scene_rgb);

    // Comparisons, not `switch` (see common.wgsl).
    let mode = Common::camera_uniforms.view_transform_mode;
    if mode == Common::VIEW_TRANSFORM_SCENE_SIGMOID {
        return Tonemap::apply_sigmoid_view_transform(view_input);
    }
    if mode == Common::VIEW_TRANSFORM_DISPLAY_RETONE {
        return Tonemap::apply_display_retone(view_input);
    }
    if mode == Common::VIEW_TRANSFORM_PROFILE_TONE {
        return apply_profile_tone_view_transform(view_input);
    }
    // The profile's rendering takes the place of a developed raster's: Contrast
    // and the sigmoid settings retone it relative to the reference sigmoid.
    if mode == Common::VIEW_TRANSFORM_PROFILE_TONE_RETONE {
        return Tonemap::apply_display_retone(apply_profile_tone_view_transform(view_input));
    }
    if mode == Common::VIEW_TRANSFORM_PASSTHROUGH {
        return view_input;
    }
    // A mode this shader does not know passes through as well.
    return view_input;
}

// The display-referred tone after the global Blacks: each mask's Blacks in
// turn, then Brightness. Brightness amounts add up (see
// Tonemap::apply_display_brightness_amount), the global one and each mask's
// weighted by its coverage, so they are applied once. `grade_options.w` holds
// Brightness, -100 to 100.
fn apply_local_display_tone(pos: vec2<i32>, input_rgb: vec3<f32>) -> vec3<f32> {
    var rgb = input_rgb;
    var brightness = Common::scene_tone_uniforms.grade_options.w / 100.0;
    let count = Common::mask_slot_count();
    for (var index = 0u; index < count; index = index + 1u) {
        let state = Common::mask_metadata(index);
        if !Common::mask_slot_applies(state, 0u) { continue; }
        let blacks = Common::mask_adjust_1_field(index).y;
        let mask_brightness = Common::mask_grade_options(index).w;
        if abs(blacks) < 1e-7 && abs(mask_brightness) < 1e-7 { continue; }
        let weight = SceneAdjustments::local_mask_weight(pos, index);
        if weight <= 1e-5 { continue; }
        if abs(blacks) >= 1e-7 {
            let amount = Tonemap::basic_low_tone_control(blacks) * weight;
            // migration: remove in v2.0.0, with LEGACY_LUMINANCE_BLACKS.
            let legacy_luminance = (state.w & Common::LEGACY_LUMINANCE_BLACKS) != 0u;
            rgb = Tonemap::apply_display_blacks_toe_amount(rgb, amount, legacy_luminance);
        }
        brightness = brightness + mask_brightness / 100.0 * weight;
    }
    return Tonemap::apply_display_brightness_amount(rgb, brightness);
}

@compute @workgroup_size(8, 8, 1)
fn apply_view_node(@builtin(global_invocation_id) dispatch_gid: vec3<u32>) {
    let gid = Common::dispatch_id(dispatch_gid);
    if gid.x >= Common::camera_uniforms.width || gid.y >= Common::camera_uniforms.height { return; }
    let pos = vec2<i32>(i32(gid.x), i32(gid.y));
    let rgb = textureLoad(SceneAdjustments::final_adjustment_tex, pos, 0).xyz;
    // Black and white mixes the grey from the colours, through the masks'
    // colour mixers too, then drops them. The global mixer's Hue and
    // Saturation have nothing left to change.
    let monochrome = Common::scene_tone_uniforms.treatment.x == Common::TREATMENT_MONOCHROME;
    var mixed: vec3<f32>;
    if monochrome {
        mixed = monochrome_grey(apply_local_color_mixer(pos, apply_monochrome_mix(pos, rgb)));
    } else {
        mixed = apply_local_color_mixer(pos, apply_color_mixer(pos, rgb));
    }
    let globally_hue_rotated = apply_hue_rotation_value(
        mixed,
        Common::scene_tone_uniforms.grade_options.z,
    );
    let hue_rotated = apply_local_hue_rotations(pos, globally_hue_rotated);
    let globally_graded = apply_color_grading_wheels(
        hue_rotated,
        Common::scene_tone_uniforms.grade_shadows,
        Common::scene_tone_uniforms.grade_midtones,
        Common::scene_tone_uniforms.grade_highlights,
        Common::scene_tone_uniforms.grade_global,
        Common::scene_tone_uniforms.grade_options,
    );
    let graded = apply_local_color_grading(pos, globally_graded);
    var display_linear = apply_view_transform(graded);
    display_linear = Tonemap::apply_display_blacks_toe_value(display_linear, Common::scene_tone_uniforms.basic_tone.w);
    display_linear = apply_local_display_tone(pos, display_linear);
    // Sampling modes capture the input to global or local point color.
    if Common::scene_tone_uniforms.point_color_meta.z == 1u {
        textureStore(SceneAdjustments::display_linear_out, pos, vec4<f32>(display_linear, 1.0));
        return;
    }
    // Keep the global range preview anchored to the unadjusted color.
    var point_color_delta = vec3<f32>(0.0);
    if Common::scene_tone_uniforms.point_color_meta.y > 0u || (Common::scene_tone_uniforms.point_color_meta.w & 7u) != 0u {
        point_color_delta = point_color_guided_delta(pos, rgb, display_linear);
    }
    let point_color_sample = point_color_hsl(max(display_linear + point_color_delta, vec3<f32>(0.0)));
    display_linear = apply_point_colors(display_linear, point_color_sample);
    if Common::scene_tone_uniforms.point_color_meta.z == 2u {
        textureStore(SceneAdjustments::display_linear_out, pos, vec4<f32>(display_linear, 1.0));
        return;
    }
    let local_point_color_sample = point_color_hsl(max(display_linear + point_color_delta, vec3<f32>(0.0)));
    let local_selection = local_point_color_visualization(pos, local_point_color_sample);
    display_linear = apply_local_point_colors(pos, display_linear, point_color_delta);
    display_linear = LookLut::apply_look_lut(display_linear);
    display_linear = CreativeEffects::apply_film_finish_modules(pos, display_linear);
    textureStore(SceneAdjustments::display_linear_out, pos, vec4<f32>(display_linear, 1.0));

    var output_rgb = Profile::apply_output_encoding(display_linear);
    let visualize_index = Common::scene_tone_uniforms.point_color_meta.y;
    if visualize_index > 0u || local_selection > 0.0 {
        let selected_weight = max(
            select(0.0, point_color_selection_weight(point_color_sample, visualize_index - 1u), visualize_index > 0u),
            local_selection,
        );
        // Match the mask overlay after output encoding.
        let overlay_rgb = vec3<f32>(78.0 / 255.0, 163.0 / 255.0, 1.0);
        let overlay_alpha = selected_weight * (92.0 / 255.0);
        output_rgb = mix(output_rgb, overlay_rgb, overlay_alpha);
    }
    textureStore(SceneAdjustments::out_tex, pos, vec4<f32>(output_rgb, 1.0));
}
