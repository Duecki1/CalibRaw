// Glow has two modes, chosen per component by its Self-illuminating switch:
// - Self-illuminating (on): the mask itself emits the picked color, so
//   the glow shows on dark surfaces too. Emission joins the shared Glow
//   diffusion (`mask_glow_source_at`), whose radius is the widest active
//   self-illuminating radius, and a hot core is added inside the mask.
// - Highlight (off): bright source pixels inside the mask bloom with their
//   own color tinted toward the picked color, over this component's own radius.

fn mask_glow_self_illuminating(params: MaskEffectParams) -> bool {
    return mask_effect_lane(params, GLOW_SELF_ILLUMINATING_LANE) > 0.5;
}

fn mask_glow_self_illuminating_active() -> bool {
    let count = Common::mask_slot_count();
    for (var index = 0u; index < count; index = index + 1u) {
        let state = Common::mask_data[index].metadata;
        if Common::mask_slot_applies(state, MASK_EFFECT_GLOW_ID)
            && mask_glow_self_illuminating(mask_effect_params(index)) {
            return true;
        }
    }
    return false;
}

// Scene-linear emission of self-illuminating Glow masks, diffused by the shared
// Glow passes.
fn mask_glow_source_at(pos: vec2<i32>) -> vec3<f32> {
    var emission = vec3<f32>(0.0);
    let count = Common::mask_slot_count();
    for (var index = 0u; index < count; index = index + 1u) {
        let state = Common::mask_data[index].metadata;
        if !Common::mask_slot_applies(state, MASK_EFFECT_GLOW_ID) { continue; }
        let params = mask_effect_params(index);
        if !mask_glow_self_illuminating(params) { continue; }
        let weight = SceneAdjustments::local_mask_weight(pos, index);
        if weight <= 1e-5 { continue; }

        let amount = mask_effect_percent(params, GLOW_AMOUNT_LANE);
        let color = mask_effect_picker_color_to_working(mask_effect_color(params, GLOW_COLOR_LANE));
        emission = emission + color * weight * amount * 0.8;
    }
    return emission;
}

// Highlight mode follows bright source pixels rather than painting a flat colored veil.
fn mask_glow_highlight(rgb: vec3<f32>, tint: vec3<f32>) -> vec3<f32> {
    let positive = max(rgb, vec3<f32>(0.0));
    let luma = max(Common::safe_luma(positive), 0.0);
    let gate = smoothstep(0.18, 0.85, luma);
    let colored = positive * mix(vec3<f32>(1.0), tint, 0.72);
    return colored * (gate / (1.0 + 0.75 * luma));
}

fn mask_glow_emitter(pos: vec2<f32>, index: u32, tint: vec3<f32>) -> vec3<f32> {
    let coverage = SceneAdjustments::local_mask_weight(vec2<i32>(round(pos)), index);
    return mask_glow_highlight(mask_effect_source_linear_at(pos), tint) * coverage;
}

fn apply_mask_glow_cores(pos: vec2<i32>, input_rgb: vec3<f32>) -> vec3<f32> {
    var emitted = vec3<f32>(0.0);
    var self_lit = vec3<f32>(0.0);
    let scale = Common::resolution_scale(0.45, 3.0);
    let count = Common::mask_slot_count();
    for (var index = 0u; index < count; index += 1u) {
        let state = Common::mask_data[index].metadata;
        if !Common::mask_slot_applies(state, MASK_EFFECT_GLOW_ID) { continue; }
        let params = mask_effect_params(index);
        let amount = mask_effect_percent(params, GLOW_AMOUNT_LANE);
        let core = mask_effect_percent(params, GLOW_CORE_LANE);
        let tint = mask_effect_picker_color_to_working(mask_effect_color(params, GLOW_COLOR_LANE));
        if mask_glow_self_illuminating(params) {
            // The halo comes from the shared diffusion; add the hot core here.
            let weight = SceneAdjustments::local_mask_weight(pos, index);
            if weight <= 1e-5 { continue; }
            let hot_color = mix(tint, vec3<f32>(1.0), smoothstep(0.15, 1.0, core));
            self_lit += hot_color * weight * amount * core * 2.1;
            continue;
        }
        if amount <= 1e-6 { continue; }
        let p = vec2<f32>(pos);
        let source = mask_glow_emitter(p, index, tint);
        // Each component integrates its own source and radius. A second Glow
        // card cannot broaden the first card or the shared diffusion.
        // Truncated Gaussian support stays within the 96-pixel export halo.
        let radius = mask_effect_percent(params, GLOW_RADIUS_LANE) * 32.0 * scale;
        var bloom = source;
        if radius > 1e-6 {
            bloom = vec3<f32>(0.0);
            const PAIRS: u32 = 80u;
            for (var i = 0u; i < PAIRS; i += 1u) {
                let unit = (f32(i) + 0.5) / f32(PAIRS);
                let r = sqrt(-2.0 * log(1.0 - unit * 0.988891)) / 3.0;
                let angle = f32(i) * 2.39996323;
                let offset = vec2<f32>(cos(angle), sin(angle)) * (radius * r);
                bloom += mask_glow_emitter(p + offset, index, tint);
                bloom += mask_glow_emitter(p - offset, index, tint);
            }
            bloom /= f32(PAIRS * 2u);
        }
        emitted += amount * (bloom * 0.67 + source * core * 0.38);
    }
    let core_protection = 1.0 - 0.72 * smoothstep(1.0, 3.2, Common::safe_luma(input_rgb));
    return input_rgb + self_lit + emitted * core_protection;
}
