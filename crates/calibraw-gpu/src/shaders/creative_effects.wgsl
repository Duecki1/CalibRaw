#import calibraw::common as Common
#import calibraw::color as Color
#import calibraw::basic_adjustments as BasicAdjustments
#import calibraw::scene_adjustments as SceneAdjustments
#import calibraw::detail_scale_space as DetailScaleSpace
#import calibraw::tone_common as ToneCommon
#import calibraw::tonemap as Tonemap

fn glow_work_at(pos: vec2<i32>) -> vec3<f32> {
    return textureLoad(SceneAdjustments::glow_work_tex, Common::clamp_pos(pos), 0).xyz;
}

fn glow_stage_step(stage: u32) -> i32 {
    // The 96-pixel maximum support is mirrored by GLOW_SUPPORT in processing.rs.
    var reference_step = 1.0;
    switch stage {
        case 2u: { reference_step = 2.0; }
        case 3u: { reference_step = 4.0; }
        case 4u: { reference_step = 8.0; }
        default: {}
    }
    let scale = Common::resolution_scale(0.45, 3.0);
    return max(i32(round(reference_step * scale)), 1);
}

fn glow_stage_mix(stage: u32) -> f32 {
    let radius = clamp(Common::effects_uniforms.glow_diffusion.x / 100.0, 0.0, 1.0);
    switch stage {
        case 0u: { return 1.0; }
        case 1u: { return smoothstep(0.0, 0.20, radius); }
        case 2u: { return smoothstep(0.15, 0.45, radius); }
        case 3u: { return smoothstep(0.40, 0.75, radius); }
        default: { return smoothstep(0.70, 1.0, radius); }
    }
}

// Normalized diffusion of self-illuminating Glow emission. Alpha is unused.
fn glow_diffuse_at(pos: vec2<i32>, stage: u32) -> vec4<f32> {
    let center = textureLoad(SceneAdjustments::glow_work_tex, Common::clamp_pos(pos), 0);
    let stage_mix = glow_stage_mix(stage);
    if stage_mix < 1e-6 {
        return center;
    }

    let step = glow_stage_step(stage);
    var sum = vec4<f32>(0.0);
    var sum_weight = 0.0;
    for (var ky = -2; ky <= 2; ky = ky + 1) {
        for (var kx = -2; kx <= 2; kx = kx + 1) {
            let weight = Common::binomial5_weight(kx) * Common::binomial5_weight(ky);
            let sample_pos = Common::clamp_pos(pos + vec2<i32>(kx * step, ky * step));
            sum = sum + textureLoad(SceneAdjustments::glow_work_tex, sample_pos, 0) * weight;
            sum_weight = sum_weight + weight;
        }
    }
    return mix(center, sum / max(sum_weight, 1e-6), stage_mix);
}

// Adds the diffused halo of self-illuminating Glow masks.
fn apply_glow(pos: vec2<i32>, rgb: vec3<f32>) -> vec3<f32> {
    if !mask_glow_self_illuminating_active() {
        return rgb;
    }

    let bloom = glow_work_at(pos);

    let current_luma = Common::safe_luma(rgb);
    let core_protection = 1.0 - 0.72 * smoothstep(1.0, 3.2, current_luma);
    return rgb + bloom * 2.8 * core_protection;
}

fn full_image_uv(pos: vec2<i32>) -> vec2<f32> {
    let dimensions = max(
        vec2<f32>(f32(Common::camera_uniforms.full_width), f32(Common::camera_uniforms.full_height)),
        vec2<f32>(1.0),
    );
    let global_pos = clamp(pos + Common::tile_origin(), vec2<i32>(0), Common::full_image_max());
    return (vec2<f32>(global_pos) + vec2<f32>(0.5)) / dimensions;
}

fn vignette_distance_from_center(pos: vec2<i32>, roundness: f32, center: vec2<f32>) -> f32 {
    let dimensions = max(Common::effects_uniforms.vignette_frame.xy, vec2<f32>(1.0));
    // The output position of this source position. The divisor is positive across the source,
    // whose perspective correction keeps its vanishing line outside the frame.
    let source = vec3<f32>(full_image_uv(pos), 1.0);
    let projection = Common::effects_uniforms.vignette_projection;
    let frame_uv = vec2<f32>(dot(projection[0].xyz, source), dot(projection[1].xyz, source))
        / dot(projection[2].xyz, source);
    let p = abs((frame_uv - center) * 2.0);
    let frame_ellipse = length(p);
    let frame_rectangle = pow(pow(p.x, 8.0) + pow(p.y, 8.0), 1.0 / 8.0);
    let short_dimension = max(min(dimensions.x, dimensions.y), 1.0);
    let image_circle = length(vec2<f32>(
        p.x * dimensions.x / short_dimension,
        p.y * dimensions.y / short_dimension,
    ));

    if abs(roundness) < 1e-6 {
        return frame_ellipse;
    }
    if roundness < 0.0 {
        return mix(frame_ellipse, frame_rectangle, -roundness);
    }
    return mix(frame_ellipse, image_circle, roundness);
}

fn calibrated_vignette_anchor(
    distance: f32,
    start: f32,
    end: f32,
    power: f32,
    edge_opacity: f32,
) -> f32 {
    return edge_opacity * pow(smoothstep(start, end, distance), power);
}

fn calibrated_vignette_opacity(
    distance: f32,
    amount: f32,
    midpoint: f32,
    feather: f32,
) -> f32 {
    let magnitude = clamp(abs(amount), 0.0, 1.0);
    let dark_half_fit = Common::effects_uniforms.vignette_dark_half_fit;
    let dark_full_fit = Common::effects_uniforms.vignette_dark_full_fit;
    let light_half_fit = Common::effects_uniforms.vignette_light_half_fit;
    let light_full_fit = Common::effects_uniforms.vignette_light_full_fit;
    var shaped_distance = distance;
    if abs(midpoint - 0.5) >= 1e-6 {
        let midpoint_power = exp2((midpoint - 0.5) * 1.4);
        let corner_distance = sqrt(2.0);
        shaped_distance = corner_distance
            * pow(max(distance / corner_distance, 0.0), midpoint_power);
    }
    var half_amount = 0.0;
    var full_amount = 0.0;

    if amount < 0.0 {
        half_amount = calibrated_vignette_anchor(
            shaped_distance,
            dark_half_fit.x,
            dark_half_fit.y,
            dark_half_fit.z,
            dark_half_fit.w
        );
        full_amount = calibrated_vignette_anchor(
            shaped_distance,
            dark_full_fit.x,
            dark_full_fit.y,
            dark_full_fit.z,
            dark_full_fit.w
        );
    } else {
        half_amount = calibrated_vignette_anchor(
            shaped_distance,
            light_half_fit.x,
            light_half_fit.y,
            light_half_fit.z,
            light_half_fit.w
        );
        full_amount = calibrated_vignette_anchor(
            shaped_distance,
            light_full_fit.x,
            light_full_fit.y,
            light_full_fit.z,
            light_full_fit.w
        );
    }

    var opacity = 0.0;
    if magnitude <= 0.5 {
        opacity = half_amount * (magnitude * 2.0);
    } else {
        opacity = mix(half_amount, full_amount, (magnitude - 0.5) * 2.0);
    }
    if abs(feather - 0.5) < 1e-6 {
        return opacity;
    }
    let feather_power = exp2((0.5 - feather) * 1.3);
    return pow(clamp(opacity, 0.0, 1.0), feather_power);
}

// Mask Saturation, Texture and Clarity. Mask Dehaze runs with the global one
// in the scene preparation (SceneAdjustments::dehaze_amount_at).
fn apply_local_scene_effect_nodes(pos: vec2<i32>, input_rgb: vec3<f32>) -> vec3<f32> {
    var rgb = input_rgb;
    let count = min(Common::scene_tone_uniforms.mask_counts.x, Common::MAX_RENDER_MASK_SLOTS);
    for (var index = 0u; index < count; index = index + 1u) {
        let state = Common::mask_data[index].metadata;
        if !Common::mask_slot_applies(state, 0u) { continue; }
        let local = Common::mask_data[index].adjust_2_field;
        if max(abs(local.x), max(abs(local.y), abs(local.z))) <= 1e-7 {
            continue;
        }
        let weight = SceneAdjustments::local_mask_weight(pos, index);
        if weight <= 1e-5 { continue; }
        var adjusted = rgb;
        adjusted = DetailScaleSpace::apply_texture_and_clarity_values(pos, adjusted, local.y, local.z);
        adjusted = BasicAdjustments::apply_saturation_value(adjusted, local.x);
        rgb = mix(rgb, adjusted, weight);
    }
    return rgb;
}

fn apply_local_mask_effect_nodes(pos: vec2<i32>, input_rgb: vec3<f32>) -> vec3<f32> {
    var rgb = input_rgb;
    let count = min(Common::scene_tone_uniforms.mask_counts.x, Common::MAX_RENDER_MASK_SLOTS);
    for (var index = 0u; index < count; index = index + 1u) {
        let state = Common::mask_data[index].metadata;
        if !Common::mask_slot_applies(state, MASK_EFFECT_NEON_ID) { continue; }
        let weight = SceneAdjustments::local_mask_weight(pos, index);
        if weight <= 1e-5 { continue; }
        let adjusted = apply_neon(
            pos,
            rgb,
            Common::mask_data[index].adjust_0_field,
            Common::mask_data[index].adjust_1_field,
        );
        rgb = mix(rgb, adjusted, weight);
    }
    return rgb;
}

fn apply_local_creative_mask_effect_nodes(pos: vec2<i32>, input_rgb: vec3<f32>) -> vec3<f32> {
    var rgb = input_rgb;
    let count = min(Common::scene_tone_uniforms.mask_counts.x, Common::MAX_RENDER_MASK_SLOTS);
    for (var index = 0u; index < count; index = index + 1u) {
        let state = Common::mask_data[index].metadata;
        if !Common::mask_slot_active(state) { continue; }
        let effect_id = Common::mask_effect_id(state);
        if effect_id != MASK_EFFECT_EDGE_GLOW_ID
            && effect_id != MASK_EFFECT_PIXELATE_ID
            && effect_id != MASK_EFFECT_FOG_ID
            && effect_id != MASK_EFFECT_SMOKE_ID
            && effect_id != MASK_EFFECT_HALATION_ID
            && effect_id != MASK_EFFECT_RELIGHT_ID {
            continue;
        }
        let weight = SceneAdjustments::local_mask_weight(pos, index);
        if weight <= 1e-5 { continue; }

        let primary = Common::mask_data[index].adjust_0_field;
        let secondary = Common::mask_data[index].adjust_1_field;
        var adjusted = rgb;
        if effect_id == MASK_EFFECT_EDGE_GLOW_ID {
            adjusted = apply_edge_glow(pos, rgb, primary, secondary);
        } else if effect_id == MASK_EFFECT_PIXELATE_ID {
            adjusted = apply_pixelate(pos, rgb, primary);
        } else if effect_id == MASK_EFFECT_FOG_ID {
            adjusted = apply_fog(pos, rgb, mask_effect_params(index));
        } else if effect_id == MASK_EFFECT_SMOKE_ID {
            adjusted = apply_smoke(pos, rgb, mask_effect_params(index));
        } else if effect_id == MASK_EFFECT_HALATION_ID {
            adjusted = apply_mask_halation(pos, rgb, primary);
        } else if effect_id == MASK_EFFECT_RELIGHT_ID {
            adjusted = apply_relight(pos, rgb, mask_effect_params(index));
        }
        rgb = mix(rgb, adjusted, weight);
    }
    return rgb;
}

@compute @workgroup_size(8, 8, 1)
fn build_detail_level_0(@builtin(global_invocation_id) gid: vec3<u32>) {
    DetailScaleSpace::store_detail_pyramid_level(gid, 0u);
}

@compute @workgroup_size(8, 8, 1)
fn build_detail_level_1(@builtin(global_invocation_id) gid: vec3<u32>) {
    DetailScaleSpace::store_detail_pyramid_level(gid, 1u);
}

@compute @workgroup_size(8, 8, 1)
fn build_detail_level_2(@builtin(global_invocation_id) gid: vec3<u32>) {
    DetailScaleSpace::store_detail_pyramid_level(gid, 2u);
}

@compute @workgroup_size(8, 8, 1)
fn build_detail_level_3(@builtin(global_invocation_id) gid: vec3<u32>) {
    DetailScaleSpace::store_detail_pyramid_level(gid, 3u);
}

@compute @workgroup_size(8, 8, 1)
fn build_detail_level_4(@builtin(global_invocation_id) gid: vec3<u32>) {
    DetailScaleSpace::store_detail_pyramid_level(gid, 4u);
}

@compute @workgroup_size(8, 8, 1)
fn build_detail_level_5(@builtin(global_invocation_id) gid: vec3<u32>) {
    DetailScaleSpace::store_detail_pyramid_level(gid, 5u);
}

@compute @workgroup_size(8, 8, 1)
fn build_detail_level_6(@builtin(global_invocation_id) gid: vec3<u32>) {
    DetailScaleSpace::store_detail_pyramid_level(gid, 6u);
}

@compute @workgroup_size(8, 8, 1)
fn apply_scene_effects_node(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= Common::camera_uniforms.width || gid.y >= Common::camera_uniforms.height { return; }
    let pos = vec2<i32>(i32(gid.x), i32(gid.y));
    var rgb = SceneAdjustments::adjustment_base_at(pos);
    rgb = DetailScaleSpace::apply_texture_and_clarity_values(pos, rgb, Common::effects_uniforms.presence.x, Common::effects_uniforms.presence.y);
    rgb = BasicAdjustments::apply_saturation_vibrance(rgb);
    rgb = apply_local_scene_effect_nodes(pos, rgb);
    rgb = apply_local_mask_effect_nodes(pos, rgb);
    textureStore(SceneAdjustments::local_effects_out, pos, vec4<f32>(rgb, 1.0));
}

@compute @workgroup_size(8, 8, 1)
fn copy_scene_effects_node(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= Common::camera_uniforms.width || gid.y >= Common::camera_uniforms.height { return; }
    let pos = vec2<i32>(i32(gid.x), i32(gid.y));
    textureStore(SceneAdjustments::local_effects_out, pos, vec4<f32>(SceneAdjustments::adjustment_base_at(pos), 1.0));
}

@compute @workgroup_size(8, 8, 1)
fn prepare_glow_source(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= Common::camera_uniforms.width || gid.y >= Common::camera_uniforms.height { return; }
    let pos = vec2<i32>(i32(gid.x), i32(gid.y));
    textureStore(SceneAdjustments::glow_work_out, pos, vec4<f32>(mask_glow_source_at(pos), 0.0));
}

fn store_glow_stage(gid: vec3<u32>, stage: u32) {
    if gid.x >= Common::camera_uniforms.width || gid.y >= Common::camera_uniforms.height { return; }
    let pos = vec2<i32>(i32(gid.x), i32(gid.y));
    textureStore(SceneAdjustments::glow_work_out, pos, glow_diffuse_at(pos, stage));
}

@compute @workgroup_size(8, 8, 1)
fn diffuse_glow_0(@builtin(global_invocation_id) gid: vec3<u32>) {
    store_glow_stage(gid, 0u);
}

@compute @workgroup_size(8, 8, 1)
fn diffuse_glow_1(@builtin(global_invocation_id) gid: vec3<u32>) {
    store_glow_stage(gid, 1u);
}

@compute @workgroup_size(8, 8, 1)
fn diffuse_glow_2(@builtin(global_invocation_id) gid: vec3<u32>) {
    store_glow_stage(gid, 2u);
}

@compute @workgroup_size(8, 8, 1)
fn diffuse_glow_3(@builtin(global_invocation_id) gid: vec3<u32>) {
    store_glow_stage(gid, 3u);
}

@compute @workgroup_size(8, 8, 1)
fn diffuse_glow_4(@builtin(global_invocation_id) gid: vec3<u32>) {
    store_glow_stage(gid, 4u);
}

@compute @workgroup_size(8, 8, 1)
fn apply_creative_effects(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= Common::camera_uniforms.width || gid.y >= Common::camera_uniforms.height { return; }
    let pos = vec2<i32>(i32(gid.x), i32(gid.y));
    var rgb = SceneAdjustments::local_effects_at(pos);
    rgb = apply_local_creative_mask_effect_nodes(pos, rgb);
    rgb = apply_glow(pos, rgb);
    rgb = apply_mask_glow_cores(pos, rgb);
    rgb = apply_light_rays(pos, rgb);
    textureStore(SceneAdjustments::creative_effects_out, pos, vec4<f32>(rgb, 1.0));
}

// Film-like grain uses correlated, zero-mean noise in perceptual lightness.
// Design reference (multiscale noise / midtone response):
// https://github.com/darktable-org/darktable/blob/master/src/iop/grain.c
// This implementation uses an integer hash, with no frame/time-dependent seed.
fn grain_hash(value: u32) -> u32 {
    var hash = value;
    hash = (hash ^ (hash >> 16u)) * 0x7feb352du;
    hash = (hash ^ (hash >> 15u)) * 0x846ca68bu;
    return hash ^ (hash >> 16u);
}

fn grain_noise_at(cell: vec2<i32>) -> f32 {
    let seed = grain_hash(bitcast<u32>(cell.x) ^ grain_hash(bitcast<u32>(cell.y) + 0x9e3779b9u));
    let first = grain_hash(seed);
    let second = grain_hash(seed + 0x68bc21ebu);
    let u = (f32(first & 0xffffu) + 0.5) / 65536.0;
    let v = (f32(second & 0xffffu) + 0.5) / 65536.0;
    return sqrt(-2.0 * log(u)) * cos(6.28318530718 * v);
}

fn grain_field(point: vec2<f32>) -> f32 {
    let cell = vec2<i32>(floor(point));
    let f = point - vec2<f32>(cell);
    let blend = f * f * (vec2<f32>(3.0) - 2.0 * f);
    let top = mix(grain_noise_at(cell), grain_noise_at(cell + vec2<i32>(1, 0)), blend.x);
    let bottom = mix(grain_noise_at(cell + vec2<i32>(0, 1)), grain_noise_at(cell + vec2<i32>(1, 1)), blend.x);
    // Normalize variance so grid intersections don't appear as heavier grain.
    let variance = (blend * blend + (1.0 - blend) * (1.0 - blend));
    return mix(top, bottom, blend.y) * inverseSqrt(variance.x * variance.y);
}
