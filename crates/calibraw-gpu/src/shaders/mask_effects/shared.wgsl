
// These values must match MaskEffect::shader_id on the Rust side.
const MASK_EFFECT_NEON_ID: u32 = 1u;
const MASK_EFFECT_GLOW_ID: u32 = 2u;
const MASK_EFFECT_LIGHT_RAYS_ID: u32 = 3u;
const MASK_EFFECT_BLUR_ID: u32 = 4u;
const MASK_EFFECT_EDGE_GLOW_ID: u32 = 5u;
const MASK_EFFECT_PIXELATE_ID: u32 = 6u;
const MASK_EFFECT_LENS_BLUR_ID: u32 = 7u;
const MASK_EFFECT_MOTION_BLUR_ID: u32 = 8u;
const MASK_EFFECT_RADIAL_BLUR_ID: u32 = 9u;
const MASK_EFFECT_TILT_SHIFT_ID: u32 = 10u;
const MASK_EFFECT_FOG_ID: u32 = 11u;
const MASK_EFFECT_SMOKE_ID: u32 = 12u;
const MASK_EFFECT_GRAIN_ID: u32 = 13u;
const MASK_EFFECT_HALATION_ID: u32 = 14u;
const MASK_EFFECT_VIGNETTE_ID: u32 = 15u;
const MASK_EFFECT_RELIGHT_ID: u32 = 16u;

// An effect slot's packed parameters (effect_lanes.rs). Lane n is component
// n % 4 of `primary`, `secondary` or `tertiary` for n / 4 = 0, 1, 2; effects
// name their lanes `<EFFECT>_<NAME>_LANE`, and a colour takes three lanes.
// `options` holds per-slot switches, named `<NAME>_OPTION`. The names match
// the Rust packing (layout_contract_tests).
struct MaskEffectParams {
    primary: vec4<f32>,
    secondary: vec4<f32>,
    tertiary: vec4<f32>,
    options: vec4<f32>,
}

fn mask_effect_params(index: u32) -> MaskEffectParams {
    return MaskEffectParams(
        Common::mask_data[index].adjust_0_field,
        Common::mask_data[index].adjust_1_field,
        Common::mask_data[index].adjust_2_field,
        Common::mask_data[index].film_effects,
    );
}

fn mask_effect_lane(params: MaskEffectParams, lane: u32) -> f32 {
    let row = select(
        select(params.tertiary, params.secondary, lane < 8u),
        params.primary,
        lane < 4u,
    );
    return row[lane % 4u];
}

// Two consecutive lanes, such as a position's x and y.
fn mask_effect_lane_pair(params: MaskEffectParams, lane: u32) -> vec2<f32> {
    return vec2<f32>(mask_effect_lane(params, lane), mask_effect_lane(params, lane + 1u));
}

// The three lanes of a colour.
fn mask_effect_color(params: MaskEffectParams, lane: u32) -> vec3<f32> {
    return vec3<f32>(
        mask_effect_lane(params, lane),
        mask_effect_lane(params, lane + 1u),
        mask_effect_lane(params, lane + 2u),
    );
}

// Integer hash finalizer shared by the procedural effects: mixes `seed` and
// maps its low 24 bits to [0, 1].
fn mask_effect_hash_unit(seed: u32) -> f32 {
    var h = (seed ^ (seed >> 16u)) * 2246822519u;
    h = (h ^ (h >> 13u)) * 3266489917u;
    h = h ^ (h >> 16u);
    return f32(h & 0x00ffffffu) / 16777215.0;
}

fn mask_effect_srgb_component_to_linear(value: f32) -> f32 {
    let encoded = clamp(value, 0.0, 1.0);
    if encoded <= 0.04045 {
        return encoded / 12.92;
    }
    return pow((encoded + 0.055) / 1.055, 2.4);
}

fn mask_effect_picker_color_to_working(color: vec3<f32>) -> vec3<f32> {
    let linear_srgb = vec3<f32>(
        mask_effect_srgb_component_to_linear(color.r),
        mask_effect_srgb_component_to_linear(color.g),
        mask_effect_srgb_component_to_linear(color.b),
    );
    return max(Common::SRGB_TO_REC2020 * linear_srgb, vec3<f32>(0.0));
}

// The mask a blur is confined to: its slot and its coverage at the pixel being
// blurred, which is above zero.
struct MaskBlurCoverage {
    index: u32,
    coverage: f32,
}

// How much a blur tap at `pos` (tile pixels, as for
// `mask_effect_source_linear_at`) may contribute: fully where the mask covers
// it at least as much as the blurred pixel, in proportion where it covers it
// less. A masked blur then gathers only what it blurs itself (normalized
// convolution), so blurring a background around a masked-out subject leaves
// no halo of the subject's colours, while a feathered edge still blends.
// Without a mask layer, or where the mask is full, every tap weighs exactly 1.
fn mask_blur_tap_weight(mask: MaskBlurCoverage, pos: vec2<f32>) -> f32 {
    let tap = SceneAdjustments::local_mask_weight_at(pos, mask.index);
    return min(tap / mask.coverage, 1.0);
}

// Mean of coverage-weighted taps: `weight` sums the weights including
// coverage and `kernel_weight` without it. Where hardly any tap is covered (a
// mask much thinner than the blur), the result fades back to `center`.
fn mask_blur_masked_mean(
    sum: vec3<f32>,
    weight: f32,
    kernel_weight: f32,
    center: vec3<f32>,
) -> vec3<f32> {
    let mean = sum / max(weight, 1e-6);
    let covered = weight / max(kernel_weight, 1e-6);
    if covered >= 0.01 { return mean; }
    return mix(center, mean, covered / 0.01);
}

fn mask_effect_source_linear_at(pos: vec2<f32>) -> vec3<f32> {
    // Manual bilinear filtering also works with the non-filterable 32-bit
    // working texture used for high-quality processing.
    let base = vec2<i32>(floor(pos));
    let fraction = fract(pos);
    let top = mix(
        SceneAdjustments::local_effects_at(base),
        SceneAdjustments::local_effects_at(base + vec2<i32>(1, 0)),
        fraction.x,
    );
    let bottom = mix(
        SceneAdjustments::local_effects_at(base + vec2<i32>(0, 1)),
        SceneAdjustments::local_effects_at(base + vec2<i32>(1, 1)),
        fraction.x,
    );
    return mix(top, bottom, fraction.y);
}
