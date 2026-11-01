// Fog: a medium that builds with distance from the camera to the visible surface.

// Packed parameter lanes (effect_lanes.rs).
const FOG_AMOUNT_LANE: u32 = 0u;
const FOG_DENSITY_LANE: u32 = 1u;
const FOG_SCALE_LANE: u32 = 2u;
const FOG_SOFTNESS_LANE: u32 = 3u;
const FOG_COLOR_LANE: u32 = 4u;
const FOG_VARIATION_LANE: u32 = 7u;
const FOG_SEED_LANE: u32 = 8u;
const FOG_START_LANE: u32 = 9u;
const FOG_DEPTH_INFLUENCE_LANE: u32 = 10u;
const FOG_LIGHT_GLOW_LANE: u32 = 11u;

// Scattered scene light at Light glow 100, relative to the single-scattering
// estimate: lights in photographs read brighter in haze than their surfaces.
const FOG_LIGHT_GLOW_GAIN: f32 = 4.0;

fn apply_fog(pos: vec2<i32>, input_rgb: vec3<f32>, params: MaskEffectParams) -> vec3<f32> {
    let amount = mask_effect_percent(params, FOG_AMOUNT_LANE);
    let density = mask_effect_percent(params, FOG_DENSITY_LANE);
    if amount <= 1e-6 || density <= 1e-6 {
        return input_rgb;
    }
    let influence = mask_effect_percent(params, FOG_DEPTH_INFLUENCE_LANE);
    var distance = SCENE_DEPTH_UNKNOWN;
    if scene_depth_present() && influence > 1e-6 {
        distance = scene_depth_at(pos);
    }
    distance = mix(1.0, distance, influence);
    let start = clamp(mask_effect_lane(params, FOG_START_LANE) / 100.0, 0.0, 0.95);
    if distance <= start { return input_rgb; }

    let scale = clamp(mask_effect_lane(params, FOG_SCALE_LANE) / 100.0, 0.01, 1.0);
    let softness = mask_effect_percent(params, FOG_SOFTNESS_LANE);
    let variation = mask_effect_percent(params, FOG_VARIATION_LANE);
    let seed = clamp(mask_effect_lane(params, FOG_SEED_LANE), 0.0, 1000.0);
    let offset = vec3<f32>(seed * 0.071 + 19.3, seed * -0.113 + 47.1, seed * 0.053 + 11.7);
    let frequency = mix(7.0, 1.6, scale);
    let image_point = effect_image_point(pos);
    // A perspective volume in relative scene units. Integrate only to the
    // visible surface; foreground objects truncate the same volume as the
    // background. Fixed world-space intervals preserve shared ray prefixes.
    let ray = vec3<f32>(image_point * 1.25, 1.0);
    let step = 1.0 / f32(MEDIUM_CELLS);
    let onset_width = mix(0.025, 0.18, softness);
    let volume = MediumVolume(ray, start, onset_width, frequency, offset, variation, softness);
    var optical_length = medium_onset_integral(distance, start, onset_width);
    if variation > 1e-6 {
        optical_length = 0.0;
        for (var i = 0u; i < MEDIUM_CELLS; i = i + 1u) {
            let lo = max(f32(i) * step, start);
            let cell_end = f32(i + 1u) * step;
            let hi = min(cell_end, distance);
            if hi <= lo { continue; }
            // A partial last interval uses the same density as the full interval,
            // so increasing surface distance can never remove accumulated fog.
            let bank_density = medium_bank_density(volume, lo, cell_end);
            let segment = medium_onset_integral(hi, start, onset_width)
                - medium_onset_integral(lo, start, onset_width);
            optical_length += segment * bank_density;
        }
    }
    // Beer-Lambert extinction and constant-environment single scattering in
    // scene-linear Rec.2020. Amount changes concentration, not a screen overlay.
    // https://pbr-book.org/4ed/Volume_Scattering/Transmittance
    let optical_depth = 6.0 * density * density * amount * optical_length * length(ray);
    let transmission = exp(-optical_depth);
    // Match airlight to scene illumination, avoiding white self-luminous fog
    // in dark photographs.
    let ambient = scene_ambient_level() * 1.15;
    let color = mask_effect_picker_color_to_working(mask_effect_color(params, FOG_COLOR_LANE));
    let airlight = color * ambient;
    let fogged = input_rgb * transmission + airlight * (1.0 - transmission);
    // Light glow: the fog also scatters scene lights (Relight, Light Rays),
    // brightest looking toward a light. The fog's brightness is its albedo,
    // so the glow keeps the light's colour.
    let glow = mask_effect_percent(params, FOG_LIGHT_GLOW_LANE);
    if glow <= 1e-6 { return fogged; }
    let albedo = Common::safe_luma(color);
    let extinction = 6.0 * density * density * amount * length(ray);
    let scattered = medium_light_scattering(pos, volume, distance, extinction);
    return fogged + scattered * (glow * FOG_LIGHT_GLOW_GAIN * ambient * albedo)
        + medium_image_light(pos, params, glow, albedo, 1.0 - transmission);
}
