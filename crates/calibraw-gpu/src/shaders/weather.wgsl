#import calibraw::common as Common
#import calibraw::scene_source as SceneSource
#import calibraw::scene_adjustments as SceneAdjustments
#import calibraw::creative_effects as CreativeEffects

// Rain and Wet Surfaces, in two passes around the creative effects, so that
// the result does not depend on the order of their cards:
//   apply_wet_surfaces  before the creative effects: wet ground darkens and
//                       reflects the photo, then Relight's light lands on it
//                       and Fog veils it, and Relight's Ambient dims what it
//                       reflects along with everything else already there.
//   apply_weather_air   after them: scene lights' highlights on the wet
//                       ground, then falling rain in front of everything.
//
// Everything is in scene-linear Rec.2020 and depends only on full-image
// coordinates, the weather map (weather_analysis.wgsl), scene depth and global
// tone statistics, and reads no neighbouring pixels, so export tiles match the
// preview and need no extra halo. Image points are in shorter-edge units with
// (0, 0) at the top-left corner. Effects keep their own mask weight.

// These values must match MaskEffect::shader_id on the Rust side.
const MASK_EFFECT_RAIN_ID: u32 = 18u;
const MASK_EFFECT_WET_SURFACES_ID: u32 = 19u;

@group(0) @binding(69) var weather_environment_tex: texture_2d<f32>;
@group(0) @binding(70) var weather_environment_blur_tex: texture_2d<f32>;
@group(0) @binding(71) var weather_depth_map_tex: texture_2d<f32>;
@group(0) @binding(73) var weather_ground_tex: texture_2d<f32>;

// Shorter-edge units per full-image pixel.
fn weather_pixel_extent() -> f32 {
    return 1.0 / max(
        f32(min(Common::camera_uniforms.full_width, Common::camera_uniforms.full_height)),
        1.0,
    );
}

fn weather_depth_present() -> bool {
    return Common::scene_tone_uniforms.scene_depth_present != 0u;
}

// The weather map interpolated to a pixel: the guided filter's slope and offset,
// and the surface's roughness.
fn weather_map_at(pos: vec2<i32>) -> vec3<f32> {
    let grid = vec2<i32>(Common::weather_grid());
    let last = grid - vec2<i32>(1);
    let position = clamp(
        CreativeEffects::full_image_uv(pos) * vec2<f32>(grid) - vec2<f32>(0.5),
        vec2<f32>(0.0),
        vec2<f32>(last),
    );
    let base = vec2<i32>(floor(position));
    let next = min(base + vec2<i32>(1), last);
    let f = position - vec2<f32>(base);
    let top = mix(
        textureLoad(weather_depth_map_tex, base, 0).xyz,
        textureLoad(weather_depth_map_tex, vec2<i32>(next.x, base.y), 0).xyz,
        f.x,
    );
    let bottom = mix(
        textureLoad(weather_depth_map_tex, vec2<i32>(base.x, next.y), 0).xyz,
        textureLoad(weather_depth_map_tex, next, 0).xyz,
        f.x,
    );
    return mix(top, bottom, f.y);
}

// Scene depth at a pixel, edge-accurate, from the weather map's values there:
// its guided filter evaluated against the photo. Near 0, far 1.
fn weather_depth_from(map: vec3<f32>, pos: vec2<i32>) -> f32 {
    return clamp(map.x * SceneSource::weather_guide_at(pos) + map.y, 0.0, 1.0);
}

// The photo's light at full-image `uv` at the current exposure: its coarse
// colour, a few percent of the longer edge across.
fn weather_environment_at(uv: vec2<f32>) -> vec3<f32> {
    let grid = vec2<f32>(Common::weather_grid());
    let cell = clamp(uv * grid, vec2<f32>(0.5), grid - vec2<f32>(0.5));
    return textureSampleLevel(
        weather_environment_tex,
        SceneAdjustments::local_mask_sampler,
        cell / f32(Common::WEATHER_GRID_LONG),
        0.0,
    ).xyz * exp2(Common::scene_tone_uniforms.exposure);
}

// The same, spread over a wide part of the photo.
fn weather_environment_blurred_at(uv: vec2<f32>) -> vec3<f32> {
    let grid = vec2<f32>(Common::weather_blur_grid());
    let cell = clamp(uv * grid, vec2<f32>(0.5), grid - vec2<f32>(0.5));
    return textureSampleLevel(
        weather_environment_blur_tex,
        SceneAdjustments::local_mask_sampler,
        cell / f32(Common::WEATHER_BLUR_GRID_LONG),
        0.0,
    ).xyz * exp2(Common::scene_tone_uniforms.exposure);
}

fn weather_slots() -> u32 {
    return min(Common::scene_tone_uniforms.mask_counts.x, Common::MAX_RENDER_MASK_SLOTS);
}

@compute @workgroup_size(8, 8, 1)
fn apply_wet_surfaces(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= Common::camera_uniforms.width || gid.y >= Common::camera_uniforms.height { return; }
    let pos = vec2<i32>(i32(gid.x), i32(gid.y));
    var rgb = SceneAdjustments::local_effects_at(pos);
    for (var index = 0u; index < weather_slots(); index = index + 1u) {
        if !Common::mask_slot_applies(Common::mask_data[index].metadata, MASK_EFFECT_WET_SURFACES_ID) {
            continue;
        }
        let weight = SceneAdjustments::local_mask_weight(pos, index);
        if weight <= 1e-5 { continue; }
        rgb = mix(rgb, wet_surface(pos, rgb, CreativeEffects::mask_effect_params(index)), weight);
    }
    textureStore(SceneAdjustments::creative_effects_out, pos, vec4<f32>(rgb, 1.0));
}

@compute @workgroup_size(8, 8, 1)
fn apply_weather_air(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= Common::camera_uniforms.width || gid.y >= Common::camera_uniforms.height { return; }
    let pos = vec2<i32>(i32(gid.x), i32(gid.y));
    var rgb = SceneAdjustments::local_effects_at(pos);
    for (var index = 0u; index < weather_slots(); index = index + 1u) {
        let state = Common::mask_data[index].metadata;
        if !Common::mask_slot_active(state) { continue; }
        let effect_id = Common::mask_effect_id(state);
        if effect_id != MASK_EFFECT_WET_SURFACES_ID { continue; }
        let weight = SceneAdjustments::local_mask_weight(pos, index);
        if weight <= 1e-5 { continue; }
        rgb += wet_glints(pos, CreativeEffects::mask_effect_params(index)) * weight;
    }
    for (var index = 0u; index < weather_slots(); index = index + 1u) {
        if !Common::mask_slot_applies(Common::mask_data[index].metadata, MASK_EFFECT_RAIN_ID) {
            continue;
        }
        let weight = SceneAdjustments::local_mask_weight(pos, index);
        if weight <= 1e-5 { continue; }
        rgb = mix(rgb, apply_rain(pos, rgb, CreativeEffects::mask_effect_params(index)), weight);
    }
    textureStore(SceneAdjustments::creative_effects_out, pos, vec4<f32>(rgb, 1.0));
}
