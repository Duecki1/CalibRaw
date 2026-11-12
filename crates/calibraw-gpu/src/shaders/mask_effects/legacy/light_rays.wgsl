// Light Rays before the rebuilt Light Rays (light_shafts.wgsl) replaced it:
// procedural shafts drawn in 2D from each source, through the openings its mask
// paints in the full-image mask atlas, shown only where the mask covers. They
// are the shafts of Light Rays' 2D style (light_shafts_flat.wgsl), with a Fade
// and a colour of their own.
//
// legacy: remove in v2.0.0

// Packed parameter lanes: `LIGHT_RAYS_<NAME>_LANE` (generated from the effect
// registry, effect_wgsl.rs).

fn apply_light_rays(pos: vec2<i32>, input_rgb: vec3<f32>) -> vec3<f32> {
    var scattered = vec3<f32>(0.0);
    let output_uv = full_image_uv(pos);
    let count = Common::mask_slot_count();
    for (var index = 0u; index < count; index = index + 1u) {
        let state = Common::mask_data[index].metadata;
        if !Common::mask_slot_applies(state, MASK_EFFECT_LIGHT_RAYS_ID) { continue; }
        let params = mask_effect_params(index);
        let amount = clamp(mask_effect_lane(params, LIGHT_RAYS_AMOUNT_LANE) / 100.0, 0.0, 1.0);
        let reach = clamp(mask_effect_lane(params, LIGHT_RAYS_LENGTH_LANE) / 100.0, 0.0, 2.0);
        if amount <= 1e-6 || reach <= 1e-6 { continue; }
        let source_uv = mask_effect_lane_pair(params, LIGHT_RAYS_SOURCE_X_LANE) / 100.0;
        let look = LightShaftsFlatLook(
            clamp(mask_effect_lane(params, LIGHT_RAYS_SPREAD_LANE), 0.0, 45.0),
            clamp(mask_effect_lane(params, LIGHT_RAYS_FADE_LANE) / 100.0, 0.0, 1.0),
            clamp(mask_effect_lane(params, LIGHT_RAYS_SOFTNESS_LANE) / 100.0, 0.0, 1.0),
            clamp(mask_effect_lane(params, LIGHT_RAYS_VARIATION_LANE) / 100.0, 0.0, 1.0),
            mask_effect_lane(params, LIGHT_RAYS_RAY_COUNT_LANE),
        );
        let ray = light_shafts_flat_ray(pos, index, output_uv, source_uv, reach, look, 0.0);
        if ray.aperture <= 1e-6 { continue; }
        let color = mask_effect_picker_color_to_working(mask_effect_color(params, LIGHT_RAYS_COLOR_LANE));
        scattered += color * ray.shaft * ray.aperture * amount * 0.32;
    }
    return input_rgb + scattered * light_shafts_flat_headroom(input_rgb);
}
