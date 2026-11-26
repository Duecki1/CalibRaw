#import calibraw::common as Common
#import calibraw::color as Color
#import calibraw::basic_adjustments as BasicAdjustments
#import calibraw::profile as Profile
#import calibraw::scene_source as SceneSource
#import calibraw::scene_adjustments as SceneAdjustments
#import calibraw::detail_scale_space as DetailScaleSpace
#import calibraw::tone_common as ToneCommon
#import calibraw::tonemap as Tonemap
#import calibraw::scene::camera as Camera
#import calibraw::scene::bake_common as Bake
#import calibraw::scene::geometry as Geometry
#import calibraw::scene::lights_common as Lights

fn glow_work_at(pos: vec2<i32>) -> vec3<f32> {
    return textureLoad(SceneAdjustments::glow_work_tex, Common::clamp_pos(pos), 0).xyz;
}

fn glow_stage_step(stage: u32) -> i32 {
    // The 96-pixel maximum support is mirrored by GLOW_SUPPORT in processing.rs.
    var reference_step = 1.0;
    if stage == 2u {
        reference_step = 2.0;
    } else if stage == 3u {
        reference_step = 4.0;
    } else if stage == 4u {
        reference_step = 8.0;
    }
    let scale = Common::resolution_scale(0.45, 3.0);
    return max(i32(round(reference_step * scale)), 1);
}

fn glow_stage_mix(stage: u32) -> f32 {
    let radius = clamp(Common::effects_uniforms.glow_diffusion.x / 100.0, 0.0, 1.0);
    if stage == 0u { return 1.0; }
    if stage == 1u { return smoothstep(0.0, 0.20, radius); }
    if stage == 2u { return smoothstep(0.15, 0.45, radius); }
    if stage == 3u { return smoothstep(0.40, 0.75, radius); }
    return smoothstep(0.70, 1.0, radius);
}

// Normalized diffusion of Glowing Area emission. Alpha is unused.
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

// Adds the diffused halo of Glowing Area masks.
fn apply_glow(pos: vec2<i32>, rgb: vec3<f32>) -> vec3<f32> {
    if !mask_glow_self_illuminating_active() {
        return rgb;
    }

    let bloom = glow_work_at(pos);

    let current_luma = Common::safe_luma(rgb);
    let core_protection = 1.0 - 0.72 * smoothstep(1.0, 3.2, current_luma);
    return rgb + bloom * 2.8 * core_protection;
}

// Mask Saturation, Texture and Clarity. Mask Dehaze runs with the global one
// in the scene preparation (SceneAdjustments::dehaze_amount_at).
fn apply_local_scene_effect_nodes(pos: vec2<i32>, input_rgb: vec3<f32>) -> vec3<f32> {
    var rgb = input_rgb;
    let count = Common::mask_slot_count();
    for (var index = 0u; index < count; index = index + 1u) {
        let state = Common::mask_metadata(index);
        if !Common::mask_slot_applies(state, 0u) { continue; }
        let local = Common::mask_adjust_2_field(index);
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
    let count = Common::mask_slot_count();
    for (var index = 0u; index < count; index = index + 1u) {
        let state = Common::mask_metadata(index);
        if !Common::mask_slot_applies(state, MASK_EFFECT_NEON_ID) { continue; }
        let weight = SceneAdjustments::local_mask_weight(pos, index);
        if weight <= 1e-5 { continue; }
        let adjusted = apply_neon(pos, rgb, mask_effect_params(index));
        rgb = mix(rgb, adjusted, weight);
    }
    return rgb;
}

// The creative pass's per-slot effects, in card order (`EffectPass::Creative`); the
// registry generates their dispatch (effect_wgsl.rs).
fn apply_local_creative_mask_effect_nodes(pos: vec2<i32>, input_rgb: vec3<f32>) -> vec3<f32> {
    var rgb = input_rgb;
    let count = Common::mask_slot_count();
    for (var index = 0u; index < count; index = index + 1u) {
        let state = Common::mask_metadata(index);
        if !Common::mask_slot_active(state) { continue; }
        let effect_id = Common::mask_effect_id(state);
        if !is_creative_slot_effect(effect_id) { continue; }
        let weight = SceneAdjustments::local_mask_weight(pos, index);
        if weight <= 1e-5 { continue; }
        let adjusted = apply_creative_slot_effect(effect_id, pos, rgb, mask_effect_params(index));
        rgb = mix(rgb, adjusted, weight);
    }
    return rgb;
}

@compute @workgroup_size(8, 8, 1)
fn build_detail_level_0(@builtin(global_invocation_id) dispatch_gid: vec3<u32>) {
    let gid = Common::dispatch_id(dispatch_gid);
    DetailScaleSpace::store_detail_pyramid_level(gid, 0u);
}

@compute @workgroup_size(8, 8, 1)
fn build_detail_level_1(@builtin(global_invocation_id) dispatch_gid: vec3<u32>) {
    let gid = Common::dispatch_id(dispatch_gid);
    DetailScaleSpace::store_detail_pyramid_level(gid, 1u);
}

@compute @workgroup_size(8, 8, 1)
fn build_detail_level_2(@builtin(global_invocation_id) dispatch_gid: vec3<u32>) {
    let gid = Common::dispatch_id(dispatch_gid);
    DetailScaleSpace::store_detail_pyramid_level(gid, 2u);
}

@compute @workgroup_size(8, 8, 1)
fn build_detail_level_3(@builtin(global_invocation_id) dispatch_gid: vec3<u32>) {
    let gid = Common::dispatch_id(dispatch_gid);
    DetailScaleSpace::store_detail_pyramid_level(gid, 3u);
}

@compute @workgroup_size(8, 8, 1)
fn build_detail_level_4(@builtin(global_invocation_id) dispatch_gid: vec3<u32>) {
    let gid = Common::dispatch_id(dispatch_gid);
    DetailScaleSpace::store_detail_pyramid_level(gid, 4u);
}

@compute @workgroup_size(8, 8, 1)
fn build_detail_level_5(@builtin(global_invocation_id) dispatch_gid: vec3<u32>) {
    let gid = Common::dispatch_id(dispatch_gid);
    DetailScaleSpace::store_detail_pyramid_level(gid, 5u);
}

@compute @workgroup_size(8, 8, 1)
fn build_detail_level_6(@builtin(global_invocation_id) dispatch_gid: vec3<u32>) {
    let gid = Common::dispatch_id(dispatch_gid);
    DetailScaleSpace::store_detail_pyramid_level(gid, 6u);
}

@compute @workgroup_size(8, 8, 1)
fn apply_scene_effects_node(@builtin(global_invocation_id) dispatch_gid: vec3<u32>) {
    let gid = Common::dispatch_id(dispatch_gid);
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
fn copy_scene_effects_node(@builtin(global_invocation_id) dispatch_gid: vec3<u32>) {
    let gid = Common::dispatch_id(dispatch_gid);
    if gid.x >= Common::camera_uniforms.width || gid.y >= Common::camera_uniforms.height { return; }
    let pos = vec2<i32>(i32(gid.x), i32(gid.y));
    textureStore(SceneAdjustments::local_effects_out, pos, vec4<f32>(SceneAdjustments::adjustment_base_at(pos), 1.0));
}

@compute @workgroup_size(8, 8, 1)
fn prepare_glow_source(@builtin(global_invocation_id) dispatch_gid: vec3<u32>) {
    let gid = Common::dispatch_id(dispatch_gid);
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
fn diffuse_glow_0(@builtin(global_invocation_id) dispatch_gid: vec3<u32>) {
    let gid = Common::dispatch_id(dispatch_gid);
    store_glow_stage(gid, 0u);
}

@compute @workgroup_size(8, 8, 1)
fn diffuse_glow_1(@builtin(global_invocation_id) dispatch_gid: vec3<u32>) {
    let gid = Common::dispatch_id(dispatch_gid);
    store_glow_stage(gid, 1u);
}

@compute @workgroup_size(8, 8, 1)
fn diffuse_glow_2(@builtin(global_invocation_id) dispatch_gid: vec3<u32>) {
    let gid = Common::dispatch_id(dispatch_gid);
    store_glow_stage(gid, 2u);
}

@compute @workgroup_size(8, 8, 1)
fn diffuse_glow_3(@builtin(global_invocation_id) dispatch_gid: vec3<u32>) {
    let gid = Common::dispatch_id(dispatch_gid);
    store_glow_stage(gid, 3u);
}

@compute @workgroup_size(8, 8, 1)
fn diffuse_glow_4(@builtin(global_invocation_id) dispatch_gid: vec3<u32>) {
    let gid = Common::dispatch_id(dispatch_gid);
    store_glow_stage(gid, 4u);
}

@compute @workgroup_size(8, 8, 1)
fn apply_creative_effects(@builtin(global_invocation_id) dispatch_gid: vec3<u32>) {
    let gid = Common::dispatch_id(dispatch_gid);
    if gid.x >= Common::camera_uniforms.width || gid.y >= Common::camera_uniforms.height { return; }
    let pos = vec2<i32>(i32(gid.x), i32(gid.y));
    var rgb = SceneAdjustments::local_effects_at(pos);
    // Each effect below is compiled only into the programs of an effect set
    // that holds it (effect_programs.rs). Without an active slot of its
    // effect, each call leaves `rgb` as it is.
    // Light that Glow and Neon cast on the surfaces lands first, so every
    // creative effect sees the lit scene whatever the order of the cards.
#ifdef EFFECTS_SCENE_EMISSION
    rgb = apply_scene_emission_light(pos, rgb);
#endif
    rgb = apply_local_creative_mask_effect_nodes(pos, rgb);
#ifdef EFFECT_GLOW
    rgb = apply_glow(pos, rgb);
#endif
#ifdef EFFECTS_GLOW_CORES
    rgb = apply_mask_glow_cores(pos, rgb);
#endif
#ifdef EFFECT_STYLED_GLOW
    rgb = apply_styled_glow(pos, rgb);
#endif
#ifdef EFFECT_LIGHT_RAYS
    rgb = apply_light_rays(pos, rgb);
#endif
#ifdef EFFECT_LIGHT_SHAFTS
    rgb = apply_light_shafts(pos, rgb);
#endif
    textureStore(SceneAdjustments::creative_effects_out, pos, vec4<f32>(rgb, 1.0));
}
