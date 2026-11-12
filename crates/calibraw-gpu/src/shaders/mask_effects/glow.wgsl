// Glowing Area: the mask itself emits the picked colour, so the glow shows on
// dark surfaces too. Emission joins the shared Glow diffusion
// (`mask_glow_source_at`), whose radius is the widest active Glowing Area's,
// and a hot core is added inside the mask. Edits saved before the Glow with
// styles called it a Glow with Self-illuminating on; one with it off renders
// as the legacy highlight Glow (legacy/highlight_glow.wgsl).

fn mask_glow_self_illuminating_active() -> bool {
    let count = Common::mask_slot_count();
    for (var index = 0u; index < count; index = index + 1u) {
        if Common::mask_slot_applies(Common::mask_data[index].metadata, MASK_EFFECT_GLOW_ID) {
            return true;
        }
    }
    return false;
}

// Scene-linear emission of Glowing Area masks, diffused by the shared Glow
// passes.
fn mask_glow_source_at(pos: vec2<i32>) -> vec3<f32> {
    var emission = vec3<f32>(0.0);
    let count = Common::mask_slot_count();
    for (var index = 0u; index < count; index = index + 1u) {
        let state = Common::mask_data[index].metadata;
        if !Common::mask_slot_applies(state, MASK_EFFECT_GLOW_ID) { continue; }
        let params = mask_effect_params(index);
        let weight = SceneAdjustments::local_mask_weight(pos, index);
        if weight <= 1e-5 { continue; }

        let amount = mask_effect_percent(params, GLOW_AMOUNT_LANE);
        let color = mask_effect_picker_color_to_working(mask_effect_color(params, GLOW_COLOR_LANE));
        emission = emission + color * weight * amount * 0.8;
    }
    return emission;
}

// The hot cores of Glowing Areas; their halo comes from the shared diffusion
// (`apply_glow`).
fn mask_glow_area_cores(pos: vec2<i32>) -> vec3<f32> {
    var self_lit = vec3<f32>(0.0);
    let count = Common::mask_slot_count();
    for (var index = 0u; index < count; index += 1u) {
        let state = Common::mask_data[index].metadata;
        if !Common::mask_slot_applies(state, MASK_EFFECT_GLOW_ID) { continue; }
        let params = mask_effect_params(index);
        let amount = mask_effect_percent(params, GLOW_AMOUNT_LANE);
        let core = mask_effect_percent(params, GLOW_CORE_LANE);
        let tint = mask_effect_picker_color_to_working(mask_effect_color(params, GLOW_COLOR_LANE));
        let weight = SceneAdjustments::local_mask_weight(pos, index);
        if weight <= 1e-5 { continue; }
        let hot_color = mix(tint, vec3<f32>(1.0), smoothstep(0.15, 1.0, core));
        self_lit += hot_color * weight * amount * core * 2.1;
    }
    return self_lit;
}

// Adds the Glowing Areas' cores and the legacy highlight Glows' blooms. The
// legacy blooms ease off over the brightest input, which the cores do not.
fn apply_mask_glow_cores(pos: vec2<i32>, input_rgb: vec3<f32>) -> vec3<f32> {
    let self_lit = mask_glow_area_cores(pos);
    let emitted = legacy_highlight_glow_emission(pos);
    let core_protection = 1.0 - 0.72 * smoothstep(1.0, 3.2, Common::safe_luma(input_rgb));
    return input_rgb + self_lit + emitted * core_protection;
}

// Scene light: a Glowing Area is an area light (scene_emission.wgsl). Where its
// mask covers, it emits what it adds to the image: the source of the shared
// diffusion (0.8 in `mask_glow_source_at`) as `apply_glow` scales it (2.8),
// which the diffusion spreads without changing its total, and the hot core of
// `mask_glow_area_cores`. Scene light scales it.
const GLOW_HALO_RADIANCE: f32 = 2.24;
const GLOW_CORE_RADIANCE: f32 = 2.1;

fn mask_glow_lights_scene(params: MaskEffectParams) -> bool {
    return mask_effect_percent(params, GLOW_AMOUNT_LANE) > 1e-6
        && mask_effect_percent(params, GLOW_SCENE_LIGHT_LANE) > 1e-6;
}

// The radiance a Glow slot emits into the scene where its mask is full.
fn mask_glow_scene_emission(params: MaskEffectParams) -> vec3<f32> {
    if !mask_glow_lights_scene(params) { return vec3<f32>(0.0); }
    let amount = mask_effect_percent(params, GLOW_AMOUNT_LANE);
    let core = mask_effect_percent(params, GLOW_CORE_LANE);
    let tint = mask_effect_picker_color_to_working(mask_effect_color(params, GLOW_COLOR_LANE));
    let hot_color = mix(tint, vec3<f32>(1.0), smoothstep(0.15, 1.0, core));
    return (tint * GLOW_HALO_RADIANCE + hot_color * (core * GLOW_CORE_RADIANCE))
        * (amount * mask_effect_percent(params, GLOW_SCENE_LIGHT_LANE));
}
