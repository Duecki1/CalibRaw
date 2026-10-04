const LIGHT_RAY_PI: f32 = 3.141592653589793;

fn light_ray_emission_at(uv: vec2<f32>, mask_index: u32) -> f32 {
    let layer = Common::mask_data[mask_index].point_color_meta.z;
    if layer == 0xffffffffu { return 1.0; }
    // Extending boundary coverage keeps an off-frame manual source continuous.
    let atlas_size = vec2<f32>(textureDimensions(SceneAdjustments::light_rays_mask_tex));
    let half_texel = vec2<f32>(0.5) / atlas_size;
    return textureSampleLevel(
        SceneAdjustments::light_rays_mask_tex,
        SceneAdjustments::local_mask_sampler,
        clamp(uv, half_texel, vec2<f32>(1.0) - half_texel),
        i32(layer),
        0.0,
    ).x;
}

fn light_ray_hash(cell: u32, seed: u32) -> f32 {
    return mask_effect_hash_unit(cell * 1597334677u ^ seed * 3812015801u);
}

fn light_ray_circular_noise(turn: f32, frequency: u32, seed: u32) -> f32 {
    let point = fract(turn) * f32(frequency);
    let cell = u32(floor(point));
    let f = fract(point);
    let blend = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    return mix(
        light_ray_hash(cell % frequency, seed),
        light_ray_hash((cell + 1u) % frequency, seed),
        blend,
    );
}

fn light_ray_beams(turn: f32, count: u32, seed: u32, softness: f32) -> f32 {
    // Circular noise varies the spacing and strength of individual shafts. The
    // low-frequency warp avoids the repeating spokes of a sinusoidal pattern.
    let broad = light_ray_circular_noise(turn, max(count / 4u, 2u), seed + 11u);
    let warped_turn = turn + (broad - 0.5) * 1.7 / f32(count);
    let primary = light_ray_circular_noise(warped_turn, count, seed);
    let detail = light_ray_circular_noise(warped_turn, count * 2u + 1u, seed + 37u);
    let aperture = smoothstep(0.16, 0.88, primary * 0.82 + detail * 0.18);
    return 0.06 + 1.4 * pow(aperture, mix(3.8, 1.15, softness));
}

fn light_ray_angular_pattern(
    radial_pixels: vec2<f32>,
    seed: u32,
    ray_count: f32,
    variation: f32,
    softness: f32,
    spread: f32,
) -> f32 {
    let count = u32(floor(clamp(ray_count, 4.0, 96.0) + 0.5));
    let turn = atan2(radial_pixels.y, radial_pixels.x) / (2.0 * LIGHT_RAY_PI) + 0.5;
    let footprint = 0.75 / max(length(radial_pixels), 1.0);
    let width = radians(spread) * mix(0.08, 0.28, softness) / (2.0 * LIGHT_RAY_PI);
    let offset = max(width, footprint / (2.0 * LIGHT_RAY_PI));
    let pattern = light_ray_beams(turn, count, seed, softness) * 0.5
        + light_ray_beams(turn - offset, count, seed, softness) * 0.25
        + light_ray_beams(turn + offset, count, seed, softness) * 0.25;
    let resolved = 1.0 - smoothstep(0.35, 1.0, f32(count) * footprint / (2.0 * LIGHT_RAY_PI));
    return mix(1.0, pattern, variation * resolved);
}

fn light_ray_path_energy(
    output_uv: vec2<f32>,
    source_uv: vec2<f32>,
    full_size: vec2<f32>,
    mask_index: u32,
    spread: f32,
    softness: f32,
) -> f32 {
    if Common::mask_data[mask_index].point_color_meta.z == 0xffffffffu { return 1.0; }
    let radial_pixels = (output_uv - source_uv) * full_size;
    let radial_length = length(radial_pixels);
    let path = source_uv - output_uv;
    var visible_end = 1.0;
    for (var axis = 0u; axis < 2u; axis = axis + 1u) {
        if path[axis] > 1e-6 {
            visible_end = min(visible_end, (1.0 - output_uv[axis]) / path[axis]);
        } else if path[axis] < -1e-6 {
            visible_end = min(visible_end, -output_uv[axis] / path[axis]);
        }
    }
    visible_end = clamp(visible_end, 0.0, 1.0);
    if visible_end <= 1e-6 { return 0.0; }
    let perpendicular = vec2<f32>(-radial_pixels.y, radial_pixels.x) / max(radial_length, 1.0);
    let cone_slope = tan(radians(spread * 0.5));
    let atlas_size = vec2<f32>(textureDimensions(SceneAdjustments::light_rays_mask_tex));
    let path_texels = length(path * visible_end * atlas_size);
    let tap_count = u32(clamp(ceil(path_texels), 32.0, 192.0));
    var energy = 0.0;
    var weights = 0.0;
    // Integrate source coverage along the visible path, emphasizing its source
    // end. Midpoint taps and an analytic frame intersection avoid edge bands.
    for (var tap = 0u; tap < 192u; tap = tap + 1u) {
        if tap >= tap_count { break; }
        let t = (f32(tap) + 0.5) / f32(tap_count);
        let progress = visible_end * t;
        let base_uv = mix(output_uv, source_uv, progress);
        let side = perpendicular * radial_length * (1.0 - progress)
            * cone_slope * sin(LIGHT_RAY_PI * t) * 0.25 / full_size;
        let center = light_ray_emission_at(base_uv, mask_index);
        let sides = 0.5 * (light_ray_emission_at(base_uv - side, mask_index)
            + light_ray_emission_at(base_uv + side, mask_index));
        let weight = 0.35 + t * t * 1.65;
        energy += mix(center, sides, softness * 0.55) * weight;
        weights += weight;
    }
    return energy / weights;
}

fn light_ray_scattering(
    distance: f32,
    reach: f32,
    radius: f32,
    fade: f32,
    angular_pattern: f32,
) -> f32 {
    let travel = distance / max(reach, 1e-6);
    // A finite source and gradual extinction leave a small halo at the light,
    // then reveal the shafts. The terminal fade has no hard circular boundary.
    let onset = 1.0 - exp(-distance / max(radius * 1.8 + 0.018, 0.018));
    let extinction = exp(-travel * mix(0.8, 4.2, fade));
    let terminal = 1.0 - smoothstep(0.55, 1.0, travel);
    let dilution = 1.0 / (1.0 + distance * 2.5);
    let halo_width = max(radius * 1.25, 0.012);
    let halo = exp(-distance * distance / (halo_width * halo_width)) * 0.10;
    return terminal * (onset * extinction * dilution * angular_pattern + halo);
}

fn apply_light_rays(pos: vec2<i32>, input_rgb: vec3<f32>) -> vec3<f32> {
    var scattered = vec3<f32>(0.0);
    let output_uv = full_image_uv(pos);
    let full_size = max(vec2<f32>(
        f32(Common::camera_uniforms.full_width),
        f32(Common::camera_uniforms.full_height),
    ), vec2<f32>(1.0));
    let short_edge = min(full_size.x, full_size.y);
    let count = min(Common::scene_tone_uniforms.mask_counts.x, Common::MAX_RENDER_MASK_SLOTS);
    for (var index = 0u; index < count; index = index + 1u) {
        let state = Common::mask_data[index].metadata;
        if state.x == 0u || state.y == 0u
            || Common::mask_effect_id(state) != MASK_EFFECT_LIGHT_RAYS_ID { continue; }
        let primary = Common::mask_data[index].adjust_0_field;
        let secondary = Common::mask_data[index].adjust_1_field;
        let tertiary = Common::mask_data[index].adjust_2_field;
        let amount = clamp(primary.x / 100.0, 0.0, 1.0);
        let reach = clamp(primary.y / 100.0, 0.0, 2.0);
        if amount <= 1e-6 || reach <= 1e-6 { continue; }
        let source_uv = primary.zw / 100.0;
        let radial_pixels = (output_uv - source_uv) * full_size;
        let distance = length(radial_pixels) / short_edge;
        if distance >= reach { continue; }
        let fade = clamp(secondary.w / 100.0, 0.0, 1.0);
        let softness = clamp(tertiary.w / 100.0, 0.0, 1.0);
        let spread = clamp(tertiary.x, 0.0, 45.0);
        let aperture = light_ray_path_energy(output_uv, source_uv, full_size, index, spread, softness);
        if aperture <= 1e-6 { continue; }
        // Placement translates the same shafts without reshuffling their
        // texture on every slider movement.
        let variation = clamp(tertiary.z / 100.0, 0.0, 1.0);
        let pattern = light_ray_angular_pattern(radial_pixels, 19u, tertiary.y,
            variation, softness, spread);
        let image_point = output_uv * full_size / short_edge;
        let density = mix(1.0, 0.80 + 0.32 * atmosphere_noise(
            image_point * 7.0 + vec2<f32>(13.1, 7.9),
        ), variation);
        let shaft = light_ray_scattering(distance, reach, 0.006, fade, pattern) * density;
        let color = mask_effect_picker_color_to_working(secondary.xyz);
        scattered += color * shaft * aperture * amount * 0.32;
    }
    // Restrained scene-linear scattering retains texture in bright areas; tone
    // mapping later rolls the added light into the existing scene highlights.
    let highlight_headroom = 1.0 / (1.0 + max(Common::safe_luma(input_rgb), 0.0) * 0.35);
    return input_rgb + scattered * highlight_headroom;
}
