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


const MASK_BLUR_STAGE_COUNT: u32 = 5u;

fn mask_blur_stage_step(stage: u32) -> i32 {
    var reference_step = 1.0;
    switch stage {
        case 2u: { reference_step = 2.0; }
        case 3u: { reference_step = 3.0; }
        case 4u: { reference_step = 5.0; }
        default: {}
    }
    return SceneAdjustments::presence_step(reference_step, 15);
}

fn mask_blur_stage_mix(radius: f32, stage: u32) -> f32 {
    let normalized_radius = clamp(radius / 16.0, 0.0, 1.0);
    switch stage {
        case 0u: { return smoothstep(0.0, 0.12, normalized_radius); }
        case 1u: { return smoothstep(0.08, 0.30, normalized_radius); }
        case 2u: { return smoothstep(0.22, 0.50, normalized_radius); }
        case 3u: { return smoothstep(0.42, 0.75, normalized_radius); }
        default: { return smoothstep(0.68, 1.0, normalized_radius); }
    }
}

fn mask_blur_stage_mix_sum(radius: f32) -> f32 {
    var sum = 0.0;
    for (var stage = 0u; stage < MASK_BLUR_STAGE_COUNT; stage = stage + 1u) {
        sum = sum + mask_blur_stage_mix(radius, stage);
    }
    return sum;
}

// Taps are weighted by how much this stage blurs them relative to the pixel
// itself (see `mask_blur_tap_weight`), so a masked Blur gathers only what it
// blurs and leaves no halo from outside its mask. Global Blurs and full masks
// blur every tap equally, so each weighs exactly 1.
//
// The first stage computes each tap's amount. Every stage stores the next
// stage's amount at its pixel in the output's alpha (`store_mask_blur_stage`),
// so later stages read their taps' amounts with their colours; `amount` is
// then the pixel's own stored amount, so equal amounts weigh exactly 1 at any
// storage precision.
fn mask_blur_diffused_at(pos: vec2<i32>, stage: u32, amount: f32, source_rgb: vec3<f32>) -> vec3<f32> {
    let step = mask_blur_stage_step(stage);
    var sum = vec3<f32>(0.0);
    var total_weight = 0.0;
    var kernel_weight = 0.0;
    for (var y = -2; y <= 2; y = y + 1) {
        for (var x = -2; x <= 2; x = x + 1) {
            let weight = Common::binomial5_weight(x)
                * Common::binomial5_weight(y);
            let tap = pos + vec2<i32>(x * step, y * step);
            let input = SceneAdjustments::local_effects_rgba_at(tap);
            var masked = weight;
            if x != 0 || y != 0 {
                var tap_amount = input.w;
                if stage == 0u {
                    tap_amount = mask_blur_stage_amount(tap, stage);
                }
                masked = weight * min(tap_amount / amount, 1.0);
            }
            sum = sum + input.xyz * masked;
            total_weight = total_weight + masked;
            kernel_weight = kernel_weight + weight;
        }
    }
    return mask_blur_masked_mean(sum, total_weight, kernel_weight, source_rgb);
}

// How much this stage of the Blur pyramid mixes the pixel at `pos` toward its
// diffused neighbourhood, combining every Blur mask that covers it.
fn mask_blur_stage_amount(pos: vec2<i32>, stage: u32) -> f32 {
    var retained_source = 1.0;
    let count = Common::mask_slot_count();
    for (var index = 0u; index < count; index = index + 1u) {
        let state = Common::mask_data[index].metadata;
        if !Common::mask_slot_applies(state, MASK_EFFECT_BLUR_ID) {
            continue;
        }

        let params = mask_effect_params(index);
        let radius = mask_effect_lane(params, BLUR_RADIUS_LANE);
        let stage_mix = mask_blur_stage_mix(radius, stage);
        if stage_mix <= 1e-6 { continue; }
        let coverage = SceneAdjustments::local_mask_weight(pos, index);
        let amount = mask_effect_percent(params, BLUR_AMOUNT_LANE) * coverage;
        if amount <= 1e-6 { continue; }

        let mix_sum = mask_blur_stage_mix_sum(radius);
        // Keep the first stage's radius ramp: normalizing by a tiny sum would
        // turn any positive radius into a full one-pixel diffusion.
        let stage_share = stage_mix / max(mix_sum, 1.0);
        let distributed_amount = min(amount, 0.995);
        let stage_amount = 1.0 - pow(1.0 - distributed_amount, stage_share);
        retained_source = retained_source * (1.0 - stage_amount);
    }
    return 1.0 - retained_source;
}

// What the focus-blur loop reads of a slot before it blurs: the effect's Amount,
// 0 to 1, and how far it blurs in reference pixels (Lens Blur's and Tilt-Shift's
// Radius, Motion Blur's Distance, Radial Blur's Strength). Zero for any other effect.
struct MaskFocusBlur {
    amount: f32,
    size: f32,
}

fn mask_focus_blur(effect_id: u32, params: MaskEffectParams) -> MaskFocusBlur {
    switch effect_id {
        case MASK_EFFECT_LENS_BLUR_ID: {
            return MaskFocusBlur(
                mask_effect_percent(params, LENS_BLUR_AMOUNT_LANE),
                mask_effect_lane(params, LENS_BLUR_RADIUS_LANE),
            );
        }
        case MASK_EFFECT_MOTION_BLUR_ID: {
            return MaskFocusBlur(
                mask_effect_percent(params, MOTION_BLUR_AMOUNT_LANE),
                mask_effect_lane(params, MOTION_BLUR_DISTANCE_LANE),
            );
        }
        case MASK_EFFECT_RADIAL_BLUR_ID: {
            return MaskFocusBlur(
                mask_effect_percent(params, RADIAL_BLUR_AMOUNT_LANE),
                mask_effect_lane(params, RADIAL_BLUR_STRENGTH_LANE),
            );
        }
        case MASK_EFFECT_TILT_SHIFT_ID: {
            return MaskFocusBlur(
                mask_effect_percent(params, TILT_SHIFT_AMOUNT_LANE),
                mask_effect_lane(params, TILT_SHIFT_RADIUS_LANE),
            );
        }
        default: {
            return MaskFocusBlur(0.0, 0.0);
        }
    }
}

// `stored_amount` is the amount the previous stage stored at `pos` (unused
// by the first stage).
fn apply_mask_blur_stage(
    pos: vec2<i32>,
    source_rgb: vec3<f32>,
    stored_amount: f32,
    stage: u32,
) -> vec3<f32> {
    let combined_amount = mask_blur_stage_amount(pos, stage);
    var rgb = source_rgb;
    if combined_amount > 1e-6 {
        var tap_reference = combined_amount;
        if stage != 0u {
            tap_reference = max(stored_amount, 1e-6);
        }
        rgb = mix(
            source_rgb,
            mask_blur_diffused_at(pos, stage, tap_reference, source_rgb),
            combined_amount,
        );
    }
    return rgb;
}

// The focus blurs (Lens Blur, Motion Blur, Radial Blur, Tilt-Shift) of the
// first stage, over `diffused`, the stage's diffused image, each blurring
// the neighbourhood of `source_rgb`, the stage's input. Only the first stage's program calls this:
// Lens Blur reads the scene bake, which only that program binds (the creative
// layout), and WGSL checks the bindings of every function a program calls.
fn apply_mask_focus_blurs(pos: vec2<i32>, source_rgb: vec3<f32>, diffused: vec3<f32>) -> vec3<f32> {
    var rgb = diffused;
    let count = Common::mask_slot_count();
    for (var index = 0u; index < count; index = index + 1u) {
        let state = Common::mask_data[index].metadata;
        if !Common::mask_slot_active(state) { continue; }
        let effect_id = Common::mask_effect_id(state);
        let params = mask_effect_params(index);
        let blur = mask_focus_blur(effect_id, params);
        if blur.size <= 1e-6 { continue; }
        let coverage = SceneAdjustments::local_mask_weight(pos, index);
        let amount = blur.amount * coverage;
        if amount <= 1e-6 { continue; }
        let mask = MaskBlurCoverage(index, coverage);

        var adjusted = source_rgb;
        if effect_id == MASK_EFFECT_LENS_BLUR_ID {
            adjusted = mask_lens_blur_at(pos, params, mask);
        } else if effect_id == MASK_EFFECT_MOTION_BLUR_ID {
            adjusted = mask_motion_blur_at(pos, params, mask);
        } else if effect_id == MASK_EFFECT_RADIAL_BLUR_ID {
            adjusted = mask_radial_blur_at(pos, params, mask);
        } else if effect_id == MASK_EFFECT_TILT_SHIFT_ID {
            let defocus = mask_tilt_shift_weight(pos, params);
            if defocus <= 1e-6 { continue; }
            adjusted = mask_tilt_shift_at(pos, blur.size * defocus, mask);
        }
        rgb = mix(rgb, adjusted, amount);
    }
    return rgb;
}

fn mask_blur_stage_in_bounds(gid: vec3<u32>) -> bool {
    return gid.x < Common::camera_uniforms.width && gid.y < Common::camera_uniforms.height;
}

// The next stage's amount at `pos`, for its taps (`mask_blur_diffused_at`).
// Nothing after the stages reads alpha.
fn mask_blur_next_amount(pos: vec2<i32>, stage: u32) -> f32 {
    if stage + 1u < MASK_BLUR_STAGE_COUNT {
        return mask_blur_stage_amount(pos, stage + 1u);
    }
    return 1.0;
}

fn store_mask_blur_stage(gid: vec3<u32>, stage: u32) {
    if !mask_blur_stage_in_bounds(gid) { return; }
    let pos = vec2<i32>(i32(gid.x), i32(gid.y));
    let source = SceneAdjustments::local_effects_rgba_at(pos);
    textureStore(
        SceneAdjustments::creative_effects_out,
        pos,
        vec4<f32>(apply_mask_blur_stage(pos, source.xyz, source.w, stage), mask_blur_next_amount(pos, stage)),
    );
}

fn store_first_mask_blur_stage(gid: vec3<u32>) {
    if !mask_blur_stage_in_bounds(gid) { return; }
    let pos = vec2<i32>(i32(gid.x), i32(gid.y));
    let source = SceneAdjustments::local_effects_rgba_at(pos);
    let diffused = apply_mask_blur_stage(pos, source.xyz, source.w, 0u);
    textureStore(
        SceneAdjustments::creative_effects_out,
        pos,
        vec4<f32>(apply_mask_focus_blurs(pos, source.xyz, diffused), mask_blur_next_amount(pos, 0u)),
    );
}

@compute @workgroup_size(8, 8, 1)
fn diffuse_mask_blur_0(@builtin(global_invocation_id) gid: vec3<u32>) {
    store_first_mask_blur_stage(gid);
}

@compute @workgroup_size(8, 8, 1)
fn diffuse_mask_blur_1(@builtin(global_invocation_id) gid: vec3<u32>) {
    store_mask_blur_stage(gid, 1u);
}

@compute @workgroup_size(8, 8, 1)
fn diffuse_mask_blur_2(@builtin(global_invocation_id) gid: vec3<u32>) {
    store_mask_blur_stage(gid, 2u);
}

@compute @workgroup_size(8, 8, 1)
fn diffuse_mask_blur_3(@builtin(global_invocation_id) gid: vec3<u32>) {
    store_mask_blur_stage(gid, 3u);
}

@compute @workgroup_size(8, 8, 1)
fn diffuse_mask_blur_4(@builtin(global_invocation_id) gid: vec3<u32>) {
    store_mask_blur_stage(gid, 4u);
}
