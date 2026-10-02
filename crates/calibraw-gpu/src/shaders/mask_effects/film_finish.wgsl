// Film effects use the same full-image coordinates in previews and export tiles.
// Grain and vignette are applied after the display transform; halation scatters
// scene-linear highlight energy before it. Every module retains its own controls.

fn mask_halation_emission(rgb: vec3<f32>, threshold: f32) -> f32 {
    let luminance = max(Common::safe_luma(rgb), 0.0);
    let cutoff = pow(mix(0.08, 0.98, threshold), 2.2);
    let excess = max(luminance - cutoff, 0.0);
    let gate = smoothstep(cutoff, cutoff + max(0.12, cutoff * 0.6), luminance);
    return gate * excess / (0.35 + excess);
}

fn apply_mask_halation(pos: vec2<i32>, rgb: vec3<f32>, primary: vec4<f32>) -> vec3<f32> {
    let amount = clamp(primary.x / 100.0, 0.0, 1.0);
    if amount <= 1e-6 || primary.y <= 1e-6 { return rgb; }
    let scale = clamp(f32(min(Common::camera_uniforms.full_width,
        Common::camera_uniforms.full_height)) / 1080.0, 0.45, 3.0);
    // Finite Gaussian support, at most 96 pixels (GLOW_SUPPORT in processing.rs).
    // Keep radius continuous so tiny halos fade smoothly instead of snapping.
    let radius = clamp(primary.y, 0.0, 32.0) * scale;
    let threshold = clamp(primary.z / 100.0, 0.0, 1.0);
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
    let warmth = clamp(primary.w / 100.0, 0.0, 1.0);
    let tint = Common::SRGB_TO_REC2020 * mix(vec3<f32>(1.0, 0.52, 0.20),
        vec3<f32>(1.0, 0.10, 0.01), warmth);
    return rgb + tint * (2.0 * amount * halo);
}

fn mask_grain_noise(point: vec2<f32>, roughness: f32) -> f32 {
    let coarse = mix(0.12, 0.75, roughness);
    let fine = grain_field(point);
    let clumps = grain_field(point * 0.48 + vec2<f32>(37.1, 91.7));
    return (fine + coarse * clumps) * inverseSqrt(1.0 + coarse * coarse);
}

fn apply_mask_grain(pos: vec2<i32>, rgb: vec3<f32>, primary: vec4<f32>, seed: f32) -> vec3<f32> {
    let amount = clamp(primary.x / 100.0, 0.0, 1.0);
    let luminance = max(dot(rgb, vec3<f32>(0.2627, 0.6780, 0.0593)), 0.0);
    if amount <= 1e-6 || luminance <= 1e-8 || luminance >= 1.0 { return rgb; }
    let short_edge = max(f32(min(Common::camera_uniforms.full_width,
        Common::camera_uniforms.full_height)), 1.0);
    let size = clamp(primary.y, 0.5, 4.0);
    let global_pos = clamp(pos + Common::tile_origin(), vec2<i32>(0), Common::full_image_max());
    let point = (vec2<f32>(global_pos) + vec2<f32>(0.5)) * (2160.0 / short_edge);
    let seed_hash = grain_hash(u32(round(max(seed, 0.0))) + 17u);
    let offset = vec2<f32>(f32(seed_hash & 0xffffu), f32(seed_hash >> 16u)) * 0.03125;
    let rotated = vec2<f32>(0.8 * point.x - 0.6 * point.y, 0.6 * point.x + 0.8 * point.y)
        / (1.35 * size) + offset;
    let roughness = clamp(primary.z / 100.0, 0.0, 1.0);
    let noise = mask_grain_noise(rotated, roughness);
    let lightness = pow(luminance, 1.0 / 3.0);
    let envelope = 4.0 * lightness * (1.0 - lightness);
    // Attenuate frequencies a small preview cannot resolve rather than aliasing
    // full-strength noise. Size stays relative to the film plane, not the tile.
    let footprint = min(short_edge * size / 2160.0, 1.0);
    let strength = 0.04 * amount * envelope * footprint;
    let grained = clamp(lightness + strength * noise, 0.0, 1.0);
    let monochrome = rgb * (grained * grained * grained / luminance);
    let color_amount = clamp(primary.w / 100.0, 0.0, 1.0);
    if color_amount <= 1e-6 { return monochrome; }
    var chroma = vec3<f32>(
        mask_grain_noise(rotated + vec2<f32>(113.7, 59.3), roughness),
        mask_grain_noise(rotated + vec2<f32>(277.1, 311.9), roughness),
        mask_grain_noise(rotated + vec2<f32>(419.3, 173.1), roughness),
    );
    chroma -= vec3<f32>(dot(chroma, vec3<f32>(0.2627, 0.6780, 0.0593)));
    return max(monochrome + chroma * (strength * color_amount * lightness * 0.5), vec3<f32>(0.0));
}

fn apply_mask_vignette(pos: vec2<i32>, rgb: vec3<f32>, primary: vec4<f32>, secondary: vec4<f32>) -> vec3<f32> {
    let amount = clamp(primary.x / 100.0, -1.0, 1.0);
    if abs(amount) <= 1e-6 { return rgb; }
    let distance = vignette_distance_from_center(pos, clamp(primary.z / 100.0, -1.0, 1.0),
        clamp(secondary.yz / 100.0, vec2<f32>(0.0), vec2<f32>(1.0)));
    var opacity = calibrated_vignette_opacity(distance, amount,
        clamp(primary.y / 100.0, 0.0, 1.0), clamp(primary.w / 100.0, 0.0, 1.0));
    if amount < 0.0 {
        opacity *= 1.0 - clamp(secondary.x / 100.0, 0.0, 1.0)
            * smoothstep(0.35, 1.0, Common::safe_luma(rgb));
        return rgb * (1.0 - opacity);
    }
    return mix(rgb, vec3<f32>(1.0), opacity);
}

fn apply_film_finish_modules(pos: vec2<i32>, input_rgb: vec3<f32>) -> vec3<f32> {
    var rgb = input_rgb;
    let count = min(Common::scene_tone_uniforms.mask_counts.x, Common::MAX_RENDER_MASK_SLOTS);
    // Finish the light falloff before adding texture, independent of card order.
    for (var stage = 0u; stage < 2u; stage += 1u) {
        let effect_kind = select(MASK_EFFECT_VIGNETTE_ID, MASK_EFFECT_GRAIN_ID, stage == 1u);
        for (var index = 0u; index < count; index += 1u) {
            let state = Common::mask_data[index].metadata;
            if state.x == 0u || state.y == 0u || Common::mask_effect_id(state) != effect_kind { continue; }
            let weight = SceneAdjustments::local_mask_weight(pos, index);
            if weight <= 1e-6 { continue; }
            let primary = Common::mask_data[index].adjust_0_field;
            let secondary = Common::mask_data[index].adjust_1_field;
            var adjusted = rgb;
            if effect_kind == MASK_EFFECT_GRAIN_ID {
                adjusted = apply_mask_grain(pos, rgb, primary, secondary.x);
            } else {
                adjusted = apply_mask_vignette(pos, rgb, primary, secondary);
            }
            rgb = mix(rgb, adjusted, weight);
        }
    }
    return rgb;
}
