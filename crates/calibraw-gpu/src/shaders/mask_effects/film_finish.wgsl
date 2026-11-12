// Film effects use the same full-image coordinates in previews and export tiles.
// Grain and vignette are applied after the display transform. Every module
// retains its own controls. Grain and Vignette read the photo only through the
// effect API (effect_api.wgsl); Vignette's calibration (`vignette_*_fit`) is its
// own input.

// Film-like grain uses correlated, zero-mean noise in perceptual lightness.
// Design reference (multiscale noise / midtone response):
// https://github.com/darktable-org/darktable/blob/master/src/iop/grain.c
// This implementation uses an integer hash, with no frame/time-dependent seed.
fn grain_hash(value: u32) -> u32 {
    var hash = value;
    hash = (hash ^ (hash >> 16u)) * 0x7feb352du;
    hash = (hash ^ (hash >> 15u)) * 0x846ca68bu;
    return hash ^ (hash >> 16u);
}

fn grain_noise_at(cell: vec2<i32>) -> f32 {
    let seed = grain_hash(bitcast<u32>(cell.x) ^ grain_hash(bitcast<u32>(cell.y) + 0x9e3779b9u));
    let first = grain_hash(seed);
    let second = grain_hash(seed + 0x68bc21ebu);
    let u = (f32(first & 0xffffu) + 0.5) / 65536.0;
    let v = (f32(second & 0xffffu) + 0.5) / 65536.0;
    return sqrt(-2.0 * log(u)) * cos(6.28318530718 * v);
}

fn grain_field(point: vec2<f32>) -> f32 {
    let cell = vec2<i32>(floor(point));
    let f = point - vec2<f32>(cell);
    let blend = f * f * (vec2<f32>(3.0) - 2.0 * f);
    let top = mix(grain_noise_at(cell), grain_noise_at(cell + vec2<i32>(1, 0)), blend.x);
    let bottom = mix(grain_noise_at(cell + vec2<i32>(0, 1)), grain_noise_at(cell + vec2<i32>(1, 1)), blend.x);
    // Normalize variance so grid intersections don't appear as heavier grain.
    let variance = (blend * blend + (1.0 - blend) * (1.0 - blend));
    return mix(top, bottom, blend.y) * inverseSqrt(variance.x * variance.y);
}

fn mask_grain_noise(point: vec2<f32>, roughness: f32) -> f32 {
    let coarse = mix(0.12, 0.75, roughness);
    let fine = grain_field(point);
    let clumps = grain_field(point * 0.48 + vec2<f32>(37.1, 91.7));
    return (fine + coarse * clumps) * inverseSqrt(1.0 + coarse * coarse);
}

fn apply_grain(pos: vec2<i32>, rgb: vec3<f32>, params: MaskEffectParams) -> vec3<f32> {
    let amount = mask_effect_percent(params, GRAIN_AMOUNT_LANE);
    let luminance = max(dot(rgb, vec3<f32>(0.2627, 0.6780, 0.0593)), 0.0);
    if amount <= 1e-6 || luminance <= 1e-8 || luminance >= 1.0 { return rgb; }
    let short_edge = effect_short_edge();
    let size = clamp(mask_effect_lane(params, GRAIN_SIZE_LANE), 0.5, 4.0);
    let point = (vec2<f32>(effect_image_pixel(pos)) + vec2<f32>(0.5)) * (2160.0 / short_edge);
    let seed = mask_effect_lane(params, GRAIN_SEED_LANE);
    let seed_hash = grain_hash(u32(round(max(seed, 0.0))) + 17u);
    let offset = vec2<f32>(f32(seed_hash & 0xffffu), f32(seed_hash >> 16u)) * 0.03125;
    let rotated = vec2<f32>(0.8 * point.x - 0.6 * point.y, 0.6 * point.x + 0.8 * point.y)
        / (1.35 * size) + offset;
    let roughness = mask_effect_percent(params, GRAIN_ROUGHNESS_LANE);
    let noise = mask_grain_noise(rotated, roughness);
    let lightness = pow(luminance, 1.0 / 3.0);
    let envelope = 4.0 * lightness * (1.0 - lightness);
    // Attenuate frequencies a small preview cannot resolve rather than aliasing
    // full-strength noise. Size stays relative to the film plane, not the tile.
    let footprint = min(short_edge * size / 2160.0, 1.0);
    let strength = 0.04 * amount * envelope * footprint;
    let grained = clamp(lightness + strength * noise, 0.0, 1.0);
    let monochrome = rgb * (grained * grained * grained / luminance);
    let color_amount = mask_effect_percent(params, GRAIN_COLOR_LANE);
    if color_amount <= 1e-6 { return monochrome; }
    var chroma = vec3<f32>(
        mask_grain_noise(rotated + vec2<f32>(113.7, 59.3), roughness),
        mask_grain_noise(rotated + vec2<f32>(277.1, 311.9), roughness),
        mask_grain_noise(rotated + vec2<f32>(419.3, 173.1), roughness),
    );
    chroma -= vec3<f32>(dot(chroma, vec3<f32>(0.2627, 0.6780, 0.0593)));
    return max(monochrome + chroma * (strength * color_amount * lightness * 0.5), vec3<f32>(0.0));
}

fn vignette_distance_from_center(pos: vec2<i32>, roundness: f32, center: vec2<f32>) -> f32 {
    let dimensions = effect_output_size();
    let p = abs((effect_output_uv(pos) - center) * 2.0);
    let frame_ellipse = length(p);
    let frame_rectangle = pow(pow(p.x, 8.0) + pow(p.y, 8.0), 1.0 / 8.0);
    let short_dimension = max(min(dimensions.x, dimensions.y), 1.0);
    let image_circle = length(vec2<f32>(
        p.x * dimensions.x / short_dimension,
        p.y * dimensions.y / short_dimension,
    ));

    if abs(roundness) < 1e-6 {
        return frame_ellipse;
    }
    if roundness < 0.0 {
        return mix(frame_ellipse, frame_rectangle, -roundness);
    }
    return mix(frame_ellipse, image_circle, roundness);
}

fn calibrated_vignette_anchor(
    distance: f32,
    start: f32,
    end: f32,
    power: f32,
    edge_opacity: f32,
) -> f32 {
    return edge_opacity * pow(smoothstep(start, end, distance), power);
}

fn calibrated_vignette_opacity(
    distance: f32,
    amount: f32,
    midpoint: f32,
    feather: f32,
) -> f32 {
    let magnitude = clamp(abs(amount), 0.0, 1.0);
    let dark_half_fit = Common::effects_uniforms.vignette_dark_half_fit;
    let dark_full_fit = Common::effects_uniforms.vignette_dark_full_fit;
    let light_half_fit = Common::effects_uniforms.vignette_light_half_fit;
    let light_full_fit = Common::effects_uniforms.vignette_light_full_fit;
    var shaped_distance = distance;
    if abs(midpoint - 0.5) >= 1e-6 {
        let midpoint_power = exp2((midpoint - 0.5) * 1.4);
        let corner_distance = sqrt(2.0);
        shaped_distance = corner_distance
            * pow(max(distance / corner_distance, 0.0), midpoint_power);
    }
    var half_amount = 0.0;
    var full_amount = 0.0;

    if amount < 0.0 {
        half_amount = calibrated_vignette_anchor(
            shaped_distance,
            dark_half_fit.x,
            dark_half_fit.y,
            dark_half_fit.z,
            dark_half_fit.w
        );
        full_amount = calibrated_vignette_anchor(
            shaped_distance,
            dark_full_fit.x,
            dark_full_fit.y,
            dark_full_fit.z,
            dark_full_fit.w
        );
    } else {
        half_amount = calibrated_vignette_anchor(
            shaped_distance,
            light_half_fit.x,
            light_half_fit.y,
            light_half_fit.z,
            light_half_fit.w
        );
        full_amount = calibrated_vignette_anchor(
            shaped_distance,
            light_full_fit.x,
            light_full_fit.y,
            light_full_fit.z,
            light_full_fit.w
        );
    }

    var opacity = 0.0;
    if magnitude <= 0.5 {
        opacity = half_amount * (magnitude * 2.0);
    } else {
        opacity = mix(half_amount, full_amount, (magnitude - 0.5) * 2.0);
    }
    if abs(feather - 0.5) < 1e-6 {
        return opacity;
    }
    let feather_power = exp2((0.5 - feather) * 1.3);
    return pow(clamp(opacity, 0.0, 1.0), feather_power);
}

fn apply_vignette(pos: vec2<i32>, rgb: vec3<f32>, params: MaskEffectParams) -> vec3<f32> {
    let amount = clamp(mask_effect_lane(params, VIGNETTE_AMOUNT_LANE) / 100.0, -1.0, 1.0);
    if abs(amount) <= 1e-6 { return rgb; }
    let roundness = clamp(mask_effect_lane(params, VIGNETTE_ROUNDNESS_LANE) / 100.0, -1.0, 1.0);
    let center = clamp(
        mask_effect_lane_pair(params, VIGNETTE_CENTER_X_LANE) / 100.0,
        vec2<f32>(0.0),
        vec2<f32>(1.0),
    );
    let distance = vignette_distance_from_center(pos, roundness, center);
    var opacity = calibrated_vignette_opacity(distance, amount,
        mask_effect_percent(params, VIGNETTE_MIDPOINT_LANE),
        mask_effect_percent(params, VIGNETTE_FEATHER_LANE));
    if amount < 0.0 {
        opacity *= 1.0 - mask_effect_percent(params, VIGNETTE_HIGHLIGHTS_LANE)
            * smoothstep(0.35, 1.0, effect_luma(rgb));
        return rgb * (1.0 - opacity);
    }
    return mix(rgb, vec3<f32>(1.0), opacity);
}

fn apply_film_finish_modules(pos: vec2<i32>, input_rgb: vec3<f32>) -> vec3<f32> {
    var rgb = input_rgb;
    let count = Common::mask_slot_count();
    // Finish the light falloff before adding texture, independent of card order.
    for (var stage = 0u; stage < 2u; stage += 1u) {
        let effect_kind = select(MASK_EFFECT_VIGNETTE_ID, MASK_EFFECT_GRAIN_ID, stage == 1u);
        for (var index = 0u; index < count; index += 1u) {
            let state = Common::mask_data[index].metadata;
            if !Common::mask_slot_applies(state, effect_kind) { continue; }
            let weight = SceneAdjustments::local_mask_weight(pos, index);
            if weight <= 1e-6 { continue; }
            let params = mask_effect_params(index);
            var adjusted = rgb;
            if effect_kind == MASK_EFFECT_GRAIN_ID {
                adjusted = apply_grain(pos, rgb, params);
            } else {
                adjusted = apply_vignette(pos, rgb, params);
            }
            rgb = mix(rgb, adjusted, weight);
        }
    }
    return rgb;
}
