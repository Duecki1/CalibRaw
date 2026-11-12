// Highlight Glow, the Glow with Self-illuminating off before the Glow was
// rebuilt with styles (styled_glow.wgsl). Bright source pixels inside the mask
// bloom with their own colour, tinted toward the picked colour, over the
// slot's own radius. `apply_mask_glow_cores` (glow.wgsl) adds the result.
//
// legacy: remove in v2.0.0

// Highlights follow bright source pixels rather than painting a flat colored veil.
fn legacy_highlight_glow_highlight(rgb: vec3<f32>, tint: vec3<f32>) -> vec3<f32> {
    let positive = max(rgb, vec3<f32>(0.0));
    let luma = max(Common::safe_luma(positive), 0.0);
    let gate = smoothstep(0.18, 0.85, luma);
    let colored = positive * mix(vec3<f32>(1.0), tint, 0.72);
    return colored * (gate / (1.0 + 0.75 * luma));
}

fn legacy_highlight_glow_emitter(pos: vec2<f32>, index: u32, tint: vec3<f32>) -> vec3<f32> {
    let coverage = SceneAdjustments::local_mask_weight(vec2<i32>(round(pos)), index);
    return legacy_highlight_glow_highlight(mask_effect_source_linear_at(pos), tint) * coverage;
}

// What every highlight Glow slot emits at `pos`, before the core protection
// `apply_mask_glow_cores` applies.
fn legacy_highlight_glow_emission(pos: vec2<i32>) -> vec3<f32> {
    var emitted = vec3<f32>(0.0);
    let scale = Common::resolution_scale(0.45, 3.0);
    let count = Common::mask_slot_count();
    for (var index = 0u; index < count; index += 1u) {
        let state = Common::mask_data[index].metadata;
        if !Common::mask_slot_applies(state, MASK_EFFECT_LEGACY_HIGHLIGHT_GLOW_ID) { continue; }
        let params = mask_effect_params(index);
        let amount = mask_effect_percent(params, LEGACY_HIGHLIGHT_GLOW_AMOUNT_LANE);
        let core = mask_effect_percent(params, LEGACY_HIGHLIGHT_GLOW_CORE_LANE);
        let tint = mask_effect_picker_color_to_working(
            mask_effect_color(params, LEGACY_HIGHLIGHT_GLOW_COLOR_LANE),
        );
        if amount <= 1e-6 { continue; }
        let p = vec2<f32>(pos);
        let source = legacy_highlight_glow_emitter(p, index, tint);
        // Each component integrates its own source and radius. A second Glow
        // card cannot broaden the first card or the shared diffusion.
        // Truncated Gaussian support stays within the 96-pixel export halo.
        let radius = mask_effect_percent(params, LEGACY_HIGHLIGHT_GLOW_RADIUS_LANE) * 32.0 * scale;
        var bloom = source;
        if radius > 1e-6 {
            bloom = vec3<f32>(0.0);
            const PAIRS: u32 = 80u;
            for (var i = 0u; i < PAIRS; i += 1u) {
                let unit = (f32(i) + 0.5) / f32(PAIRS);
                let r = sqrt(-2.0 * log(1.0 - unit * 0.988891)) / 3.0;
                let angle = f32(i) * 2.39996323;
                let offset = vec2<f32>(cos(angle), sin(angle)) * (radius * r);
                bloom += legacy_highlight_glow_emitter(p + offset, index, tint);
                bloom += legacy_highlight_glow_emitter(p - offset, index, tint);
            }
            bloom /= f32(PAIRS * 2u);
        }
        emitted += amount * (bloom * 0.67 + source * core * 0.38);
    }
    return emitted;
}
