// Halation, the film halo before the Glow's Halation style replaced it
// (styled_glow.wgsl): highlights inside the mask scatter scene-linear energy
// into a warm halo before the display transform. Nearby highlights outside
// the selection can cast a halo into it.
//
// legacy: remove in v2.0.0

fn mask_halation_emission(rgb: vec3<f32>, threshold: f32) -> f32 {
    let luminance = max(Common::safe_luma(rgb), 0.0);
    let cutoff = pow(mix(0.08, 0.98, threshold), 2.2);
    let excess = max(luminance - cutoff, 0.0);
    let gate = smoothstep(cutoff, cutoff + max(0.12, cutoff * 0.6), luminance);
    return gate * excess / (0.35 + excess);
}

fn apply_halation(pos: vec2<i32>, rgb: vec3<f32>, params: MaskEffectParams) -> vec3<f32> {
    let amount = mask_effect_percent(params, HALATION_AMOUNT_LANE);
    let radius_control = mask_effect_lane(params, HALATION_RADIUS_LANE);
    if amount <= 1e-6 || radius_control <= 1e-6 { return rgb; }
    let scale = Common::resolution_scale(0.45, 3.0);
    // Finite Gaussian support, at most 96 pixels (GLOW_SUPPORT in processing.rs).
    // Keep radius continuous so tiny halos fade smoothly instead of snapping.
    let radius = clamp(radius_control, 0.0, 32.0) * scale;
    let threshold = mask_effect_percent(params, HALATION_THRESHOLD_LANE);
    let source = mask_halation_emission(SceneAdjustments::local_effects_at(pos), threshold);
    var scattered = 0.0;
    // Equal-energy samples from a Gaussian disk. Antipodal pairs avoid a
    // directional fringe; bilinear filtering resolves subpixel light sources.
    const PAIRS: u32 = 80u;
    for (var i = 0u; i < PAIRS; i += 1u) {
        let unit = (f32(i) + 0.5) / f32(PAIRS);
        let r = sqrt(-2.0 * log(1.0 - unit * 0.988891)) / 3.0;
        let angle = f32(i) * 2.39996323;
        let offset = vec2<f32>(cos(angle), sin(angle)) * (radius * r);
        scattered += mask_halation_emission(mask_effect_source_linear_at(vec2<f32>(pos) + offset), threshold);
        scattered += mask_halation_emission(mask_effect_source_linear_at(vec2<f32>(pos) - offset), threshold);
    }
    scattered /= f32(PAIRS * 2u);
    // Remove the undiffused core: a flat field never develops a color cast.
    let halo = max(scattered - source, 0.0);
    let warmth = mask_effect_percent(params, HALATION_WARMTH_LANE);
    let tint = Common::SRGB_TO_REC2020 * mix(vec3<f32>(1.0, 0.52, 0.20),
        vec3<f32>(1.0, 0.10, 0.01), warmth);
    return rgb + tint * (2.0 * amount * halo);
}
