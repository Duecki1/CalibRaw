// Neon runs before the blur stage: sample only the adjustment input here.

fn neon_edge_sample(pos: vec2<f32>) -> f32 {
    let base = vec2<i32>(floor(pos));
    let f = pos - vec2<f32>(base);
    let rgb = mix(
        mix(SceneAdjustments::adjustment_base_at(base),
        SceneAdjustments::adjustment_base_at(base + vec2<i32>(1, 0)), f.x),
        mix(SceneAdjustments::adjustment_base_at(base + vec2<i32>(0, 1)),
        SceneAdjustments::adjustment_base_at(base + vec2<i32>(1, 1)), f.x), f.y,
    );
    // A soft black floor rejects shadow noise without singular log(black) edges.
    return log2(1.0 + max(Common::safe_luma(rgb), 0.0) / 0.08);
}

fn neon_edge_energy(pos: vec2<f32>, radius: f32) -> f32 {
    let x = vec2<f32>(radius, 0.0);
    let y = vec2<f32>(0.0, radius);
    let tl = neon_edge_sample(pos - x - y);
    let tc = neon_edge_sample(pos - y);
    let tr = neon_edge_sample(pos + x - y);
    let ml = neon_edge_sample(pos - x);
    let mr = neon_edge_sample(pos + x);
    let bl = neon_edge_sample(pos - x + y);
    let bc = neon_edge_sample(pos + y);
    let br = neon_edge_sample(pos + x + y);
    // Pairwise differences guarantee zero DC response, even for HDR flat fields.
    let gradient = vec2<f32>(
        (tr - tl) + 2.0 * (mr - ml) + (br - bl),
        (bl - tl) + 2.0 * (bc - tc) + (br - tr),
    ) * 0.125;
    return length(gradient);
}

fn neon_edge_profile(pos: vec2<i32>, params: MaskEffectParams) -> vec2<f32> {
    let edge_width = clamp(mask_effect_lane(params, NEON_EDGE_WIDTH_LANE), 0.5, 8.0);
    let width = mask_focus_blur_radius(edge_width, 24.0);
    let detail = mask_effect_percent(params, NEON_DETAIL_LANE);
    let threshold = mix(0.16, 0.012, detail);
    let p = vec2<f32>(pos);
    let energy = neon_edge_energy(p, width);
    let core = smoothstep(threshold, threshold + 0.32, energy);
    var halo = core * 0.4;
    // Diffuse the same line instead of thresholding a second, larger Sobel.
    // This prevents disconnected parallel outlines and rectangular wide halos.
    for (var y = -1; y <= 1; y += 1) {
        for (var x = -1; x <= 1; x += 1) {
            if x == 0 && y == 0 { continue; }
            let offset = vec2<f32>(f32(x), f32(y)) * width;
            let neighbor = neon_edge_energy(p + offset, width);
            let weight = select(0.1, 0.05, x != 0 && y != 0);
            halo += smoothstep(threshold, threshold + 0.32, neighbor) * weight;
        }
    }
    return vec2<f32>(core, halo);
}


fn apply_neon(pos: vec2<i32>, source_rgb: vec3<f32>, params: MaskEffectParams) -> vec3<f32> {
    let amount = mask_effect_percent(params, NEON_AMOUNT_LANE);
    if amount <= 1e-6 { return source_rgb; }
    let glow = mask_effect_percent(params, NEON_GLOW_LANE);
    let background = mask_effect_percent(params, NEON_BACKGROUND_LANE);
    let profile = neon_edge_profile(pos, params);
    let color = mask_effect_picker_color_to_working(mask_effect_color(params, NEON_COLOR_LANE));
    // Saturated diffuse emission surrounds a subtly whiter hot tube. Both are
    // derived from the same DC-rejecting contour, not competing edge detectors.
    let hot_color = mix(color, vec3<f32>(1.0), 0.22 * profile.x);
    let emitted = hot_color * (0.58 * profile.x) + color * (0.32 * glow * profile.y);
    return mix(source_rgb, source_rgb * background + emitted, amount);
}

// Scene light: Neon's lines are an area light (scene_emission.wgsl). The grid of
// emitters is built from full-image inputs only, so the lines are not traced
// there: the contour statistics give the share of the area on a line
// (`scene_contour_line_share`) with Neon's threshold, ramp and soft black floor
// (`neon_edge_sample`, `neon_edge_profile`). A line emits its mean light: the
// tube's 0.58 with about half of its 0.22 toward white, and the glow's 0.32,
// whose halo averages to the line's own share (its weights sum to one).
const NEON_BLACK_FLOOR: f32 = 0.08;
const NEON_EDGE_RAMP: f32 = 0.32;

fn neon_lights_scene(params: MaskEffectParams) -> bool {
    return mask_effect_percent(params, NEON_AMOUNT_LANE) > 1e-6
        && mask_effect_percent(params, NEON_SCENE_LIGHT_LANE) > 1e-6;
}

// The radiance a Neon slot emits into the scene around full-image `uv` where its
// mask is full.
fn neon_scene_emission(params: MaskEffectParams, uv: vec2<f32>) -> vec3<f32> {
    if !neon_lights_scene(params) { return vec3<f32>(0.0); }
    let edge_width = clamp(mask_effect_lane(params, NEON_EDGE_WIDTH_LANE), 0.5, 8.0);
    let width = mask_focus_blur_radius(edge_width, 24.0);
    let detail = mask_effect_percent(params, NEON_DETAIL_LANE);
    let threshold = mix(0.16, 0.012, detail);
    let share = scene_contour_line_share(uv, threshold, NEON_EDGE_RAMP, NEON_BLACK_FLOOR, width);
    if share <= 0.0 { return vec3<f32>(0.0); }
    let glow = mask_effect_percent(params, NEON_GLOW_LANE);
    let color = mask_effect_picker_color_to_working(mask_effect_color(params, NEON_COLOR_LANE));
    let tube = mix(color, vec3<f32>(1.0), 0.11);
    return (tube * 0.58 + color * (0.32 * glow)) * share
        * (mask_effect_percent(params, NEON_AMOUNT_LANE)
            * mask_effect_percent(params, NEON_SCENE_LIGHT_LANE));
}
