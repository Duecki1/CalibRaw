// Glow follows bright source pixels rather than painting a flat colored veil.
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
    let scale = clamp(f32(min(Common::camera_uniforms.full_width,
        Common::camera_uniforms.full_height)) / 1080.0, 0.45, 3.0);
    let count = min(Common::scene_tone_uniforms.mask_counts.x, Common::MAX_RENDER_MASK_SLOTS);
    for (var index = 0u; index < count; index += 1u) {
        let state = Common::mask_data[index].metadata;
        if state.x == 0u || state.y == 0u || Common::mask_effect_id(state) != MASK_EFFECT_GLOW_ID { continue; }
        let primary = Common::mask_data[index].adjust_0_field;
        let amount = clamp(primary.x / 100.0, 0.0, 1.0);
        if amount <= 1e-6 { continue; }
        let core = clamp(primary.z / 100.0, 0.0, 1.0);
        let tint = mask_effect_picker_color_to_working(Common::mask_data[index].adjust_1_field.xyz);
        let p = vec2<f32>(pos);
        let source = mask_glow_emitter(p, index, tint);
        // Each component integrates its own source and radius. A second Glow
        // card cannot broaden the first card or the global adjustment's bloom.
        // Truncated Gaussian support stays within the 96-pixel export halo.
        let radius = clamp(primary.y / 100.0, 0.0, 1.0) * 32.0 * scale;
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
    return input_rgb + emitted * core_protection;
}
