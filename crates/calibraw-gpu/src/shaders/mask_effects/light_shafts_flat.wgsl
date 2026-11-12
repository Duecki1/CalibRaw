// Light Rays in its 2D style (light_shafts.rs `LightShaftsMode::Flat`), and the
// legacy Light Rays (legacy/light_rays.wgsl), which draw the same shafts: rays
// fanning out from a source in the image, without scene depth.
//
// Per source and pixel, in full-image coordinates so export tiles match the
// preview: procedural rays around the source (circular value noise over the
// angle, `light_shafts_flat_angular_pattern`), the share of the path from the
// pixel back to the source that the mask opens (`light_shafts_flat_path_energy`,
// read from the full-image mask atlas, emphasising the source end), and a
// falloff with distance that leaves a small halo at the source
// (`light_shafts_flat_scattering`). Distances are in shorter edges of the full
// image.

// The look of a source's rays, as its effect's controls set them: Spread in
// degrees (0 to 45); Fade, Softness and Variation in [0, 1]; Ray count as set
// (clamped to 4 to 96 where it is used).
struct LightShaftsFlatLook {
    spread: f32,
    fade: f32,
    softness: f32,
    variation: f32,
    ray_count: f32,
}

// A source's shafts at a pixel: their strength along the ray, and the
// aperture, the mask's coverage at the pixel times the openings along the path.
// Both 0 where the source does not reach the pixel.
struct LightShaftsFlatRay {
    shaft: f32,
    aperture: f32,
}

const LIGHT_SHAFTS_FLAT_PI: f32 = 3.141592653589793;

fn light_shafts_flat_hash(cell: u32, seed: u32) -> f32 {
    return mask_effect_hash_unit(cell * 1597334677u ^ seed * 3812015801u);
}

fn light_shafts_flat_circular_noise(turn: f32, frequency: u32, seed: u32) -> f32 {
    let point = fract(turn) * f32(frequency);
    let cell = u32(floor(point));
    let f = point - f32(cell);
    let blend = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    return mix(
        light_shafts_flat_hash(cell % frequency, seed),
        light_shafts_flat_hash((cell + 1u) % frequency, seed),
        blend,
    );
}

fn light_shafts_flat_beams(turn: f32, count: u32, seed: u32, softness: f32) -> f32 {
    // Circular noise varies the spacing and strength of individual shafts. The
    // low-frequency warp avoids the repeating spokes of a sinusoidal pattern.
    let broad = light_shafts_flat_circular_noise(turn, max(count / 4u, 2u), seed + 11u);
    let warped_turn = turn + (broad - 0.5) * 1.7 / f32(count);
    let primary = light_shafts_flat_circular_noise(warped_turn, count, seed);
    let detail = light_shafts_flat_circular_noise(warped_turn, count * 2u + 1u, seed + 37u);
    let aperture = smoothstep(0.16, 0.88, primary * 0.82 + detail * 0.18);
    return 0.06 + 1.4 * pow(aperture, mix(3.8, 1.15, softness));
}

fn light_shafts_flat_angular_pattern(
    radial_pixels: vec2<f32>,
    seed: u32,
    ray_count: f32,
    variation: f32,
    softness: f32,
    spread: f32,
) -> f32 {
    let count = u32(floor(clamp(ray_count, 4.0, 96.0) + 0.5));
    let turn = atan2(radial_pixels.y, radial_pixels.x) / (2.0 * LIGHT_SHAFTS_FLAT_PI) + 0.5;
    let footprint = 0.75 / max(length(radial_pixels), 1.0);
    let width = radians(spread) * mix(0.08, 0.28, softness) / (2.0 * LIGHT_SHAFTS_FLAT_PI);
    let offset = max(width, footprint / (2.0 * LIGHT_SHAFTS_FLAT_PI));
    let pattern = light_shafts_flat_beams(turn, count, seed, softness) * 0.5
        + light_shafts_flat_beams(turn - offset, count, seed, softness) * 0.25
        + light_shafts_flat_beams(turn + offset, count, seed, softness) * 0.25;
    let resolved = 1.0 - smoothstep(0.35, 1.0, f32(count) * footprint / (2.0 * LIGHT_SHAFTS_FLAT_PI));
    return mix(1.0, pattern, variation * resolved);
}

fn light_shafts_flat_path_energy(
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
    let atlas_size = vec2<f32>(textureDimensions(SceneAdjustments::full_image_mask_tex));
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
            * cone_slope * sin(LIGHT_SHAFTS_FLAT_PI * t) * 0.25 / full_size;
        let center = SceneAdjustments::full_image_mask_weight(base_uv, mask_index);
        let sides = 0.5 * (SceneAdjustments::full_image_mask_weight(base_uv - side, mask_index)
            + SceneAdjustments::full_image_mask_weight(base_uv + side, mask_index));
        let weight = 0.35 + t * t * 1.65;
        energy += mix(center, sides, softness * 0.55) * weight;
        weights += weight;
    }
    return energy / weights;
}

fn light_shafts_flat_scattering(
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

// The shafts of the source at full-image `source_uv` (reach `reach` in shorter
// edges) of the effect in slot `index` at the pixel `pos`, at full-image
// `output_uv`. `penetration` in [0, 1] lets that share of the light pass the
// closed parts of the mask on its way.
fn light_shafts_flat_ray(
    pos: vec2<i32>,
    index: u32,
    output_uv: vec2<f32>,
    source_uv: vec2<f32>,
    reach: f32,
    look: LightShaftsFlatLook,
    penetration: f32,
) -> LightShaftsFlatRay {
    let none = LightShaftsFlatRay(0.0, 0.0);
    let full_size = max(vec2<f32>(
        f32(Common::camera_uniforms.full_width),
        f32(Common::camera_uniforms.full_height),
    ), vec2<f32>(1.0));
    let short_edge = min(full_size.x, full_size.y);
    let radial_pixels = (output_uv - source_uv) * full_size;
    let distance = length(radial_pixels) / short_edge;
    if distance >= reach { return none; }
    // The shafts show only where the mask covers this pixel, as every
    // effect's result does; the mask's openings also shape them below.
    let coverage = SceneAdjustments::local_mask_weight(pos, index);
    if coverage <= 1e-6 { return none; }
    var openings = light_shafts_flat_path_energy(output_uv, source_uv, full_size, index, look.spread, look.softness);
    if penetration > 0.0 {
        openings = mix(openings, 1.0, penetration);
    }
    let aperture = coverage * openings;
    if aperture <= 1e-6 { return none; }
    // Placement translates the same shafts without reshuffling their
    // texture on every slider movement.
    let pattern = light_shafts_flat_angular_pattern(radial_pixels, 19u,
        look.ray_count, look.variation, look.softness, look.spread);
    let image_point = output_uv * full_size / short_edge;
    let density = mix(1.0, 0.80 + 0.32 * effect_noise(
        image_point * 7.0 + vec2<f32>(13.1, 7.9),
    ), look.variation);
    let shaft = light_shafts_flat_scattering(distance, reach, 0.006, look.fade, pattern) * density;
    return LightShaftsFlatRay(shaft, aperture);
}

// The added light the rays of a pixel whose colour is `input_rgb` receive:
// restrained scene-linear scattering keeps texture in bright areas, and tone
// mapping later rolls it into the scene's highlights.
fn light_shafts_flat_headroom(input_rgb: vec3<f32>) -> f32 {
    return 1.0 / (1.0 + max(Common::safe_luma(input_rgb), 0.0) * 0.35);
}
