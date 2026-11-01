// Rain: falling drops and the mist of unresolved drops beyond them.
//
// A drop is transparent and refracts about 165 degrees of its surroundings, so
// its brightness hardly depends on what lies behind it (Garg and Nayar,
// "Photometric Model of a Rain Drop", 2003). A streak therefore replaces a
// share of the background with the light around the drop, rather than adding
// light: it shows light against a dark wall and dark against a bright sky. That
// share is the part of the exposure the drop covers a pixel for, width / (length
// + width), so long streaks are fainter, as with a slow shutter.
//
// Drops lie in layers at fixed distances. Layers inside the scene are hidden
// behind nearer surfaces; layers in the open air before the nearest surface
// are larger and defocused. Each layer is a grid of cells holding at most one
// streak each, placed by hashing the cell. Cells stretch with the streak length
// and a cell's hash decides whether it holds a drop, so changing a control moves
// or fades the same streaks rather than reshuffling them. Sizes are in
// shorter-edge units, and a streak thinner than a pixel widens and fades by the
// same factor, so a reduced preview has the mean brightness of the export.

// Packed parameter lanes (effect_lanes.rs).
const RAIN_AMOUNT_LANE: u32 = 0u;
const RAIN_LENGTH_LANE: u32 = 1u;
const RAIN_FALL_X_LANE: u32 = 2u;
const RAIN_FALL_Y_LANE: u32 = 3u;
const RAIN_DROP_SIZE_LANE: u32 = 4u;
const RAIN_MIST_LANE: u32 = 5u;
const RAIN_LIGHT_GLOW_LANE: u32 = 6u;
const RAIN_GUSTS_LANE: u32 = 7u;
const RAIN_PERSPECTIVE_LANE: u32 = 8u;
const RAIN_FOCUS_LANE: u32 = 9u;
const RAIN_SEED_LANE: u32 = 10u;
const RAIN_DEPTH_LANE: u32 = 11u;

const RAIN_LAYERS: u32 = 6u;
// Layer distances, far to near, as Relight's light depth: 0 at the nearest
// surface and 1 at the farthest, negative in the open air before the nearest
// surface down to -1 at the camera.
const RAIN_LAYER_DEPTHS: array<f32, 6> = array<f32, 6>(0.8, 0.45, 0.12, -0.25, -0.6, -0.85);
// Streaks per square shorter edge at the longest streak length and full Amount.
const RAIN_LAYER_DENSITIES: array<f32, 6> = array<f32, 6>(5000.0, 3000.0, 1400.0, 240.0, 30.0, 5.0);
// Streak width and length at the nearest surface, in shorter edges, at the
// lowest and highest Drop size and Streak length.
const RAIN_WIDTH_RANGE: vec2<f32> = vec2<f32>(0.0005, 0.0024);
const RAIN_LENGTH_RANGE: vec2<f32> = vec2<f32>(0.006, 0.26);
// Longest streak in shorter edges, for the nearest layers.
const RAIN_MAX_LENGTH: f32 = 1.4;
// Share of the background a streak replaces, relative to width / (length + width).
const RAIN_OPACITY: f32 = 14.0;
const RAIN_MAX_OPACITY: f32 = 0.55;
const RAIN_NEAR_OPACITY: f32 = 0.45;
// Defocus radius in shorter edges per unit of difference in inverse distance.
const RAIN_DEFOCUS: f32 = 0.0016;
// Share of the depth range over which a layer passes behind a surface.
const RAIN_OCCLUSION_SOFTNESS: f32 = 0.03;
// Forward scattering of drops: rain shows best looking toward a light.
const RAIN_LIGHT_ANISOTROPY: f32 = 0.72;
// A drop's light relative to the coarse light around it: it also sees the sky
// and everything else beyond the frame.
const RAIN_ENVIRONMENT_GAIN: f32 = 1.5;
// Light a drop refracts from Relight and Light Rays lights at Light glow 100.
const RAIN_LIGHT_GAIN: f32 = 2.4;
// Image light a drop refracts at Light glow 100, relative to the halo's mean.
const RAIN_IMAGE_LIGHT_GAIN: f32 = 40.0;
// Mist optical depth at the farthest surface at full Amount and Mist.
const RAIN_MIST_DEPTH: f32 = 1.1;
// Mist before the nearest surface, relative to the depth range.
const RAIN_MIST_NEAR: f32 = 0.12;
const RAIN_MIST_GAIN: f32 = 1.1;
const RAIN_MIST_GLOW_GAIN: f32 = 4.0;
// How far gusts bend the rain, in shorter edges.
const RAIN_GUST_BEND: f32 = 0.05;

fn rain_layer_depth(layer: u32) -> f32 {
    var depths = RAIN_LAYER_DEPTHS;
    return depths[layer];
}

fn rain_layer_density(layer: u32) -> f32 {
    var densities = RAIN_LAYER_DENSITIES;
    return densities[layer];
}

// Everything about one Rain effect that is the same for every layer at a pixel.
struct RainField {
    // The pixel in the warped plane where streaks are straight and parallel:
    // x across the fall, y along it.
    warped: vec2<f32>,
    // Image points per warped unit at this pixel.
    scale: f32,
    // Streak width and length at the nearest surface.
    width: f32,
    length: f32,
    amount: f32,
    focus: f32,
    seed: u32,
}

// Coverage of the streaks of `layer` at a pixel: the sum over the cells whose
// streak can reach it of the share of the background they replace.
fn rain_layer_coverage(field: RainField, layer: u32, distance: f32) -> f32 {
    let size = 1.0 / distance;
    let width = field.width * size;
    let length = min(field.length * size, RAIN_MAX_LENGTH);
    let longest = min(RAIN_LENGTH_RANGE.y * size, RAIN_MAX_LENGTH);
    let density = rain_layer_density(layer);
    let cell_size = vec2<f32>(1.0 / (density * longest * 1.3), length * 1.3);
    let presence = field.amount * length / longest;
    let pixel = scene_pixel_extent() / field.scale;
    let defocus = abs(size - 1.0 / field.focus) * RAIN_DEFOCUS;
    let sigma = sqrt(0.25 * width * width + defocus * defocus + 0.25 * pixel * pixel);
    let peak = 0.5 * width / sigma;
    // Near the lens a drop's image is so wide that it hardly covers a pixel.
    let nearness = mix(RAIN_NEAR_OPACITY, 1.0, smoothstep(0.2, 1.0, distance));
    let opacity = min(RAIN_OPACITY * width / (length + width), RAIN_MAX_OPACITY) * peak * nearness;
    let reach = 3.0 * sigma;
    let cell = vec2<i32>(floor(field.warped / cell_size));
    let salt = field.seed * 1048583u + layer * 7919u;
    var coverage = 0.0;
    for (var dy = -1; dy <= 0; dy = dy + 1) {
        for (var dx = -1; dx <= 1; dx = dx + 1) {
            let candidate = cell + vec2<i32>(dx, dy);
            let random = effect_cell_random(candidate, salt);
            if random.x >= presence { continue; }
            // Larger drops fall faster, so their streaks are longer and wider.
            let drop = 0.55 + 0.9 * random.w;
            let start = (vec2<f32>(candidate) + vec2<f32>(random.y, random.z)) * cell_size;
            let offset = field.warped - start;
            let along = offset.y / (length * mix(0.7, 1.0, random.w));
            if along < -0.02 || along > 1.02 { continue; }
            let across = offset.x - offset.y * (random.y - 0.5) * 0.015;
            if abs(across) > reach * drop { continue; }
            let scaled = across / (sigma * drop);
            // Ends taper and a drop's oscillation beads the streak faintly.
            let ends = smoothstep(0.0, 0.1, along) * (1.0 - smoothstep(0.82, 1.0, along));
            let beads = 1.0 + 0.16 * sin(along * (18.0 + 30.0 * random.y) + 6.28 * random.z);
            coverage += opacity * ends * beads * exp(-0.5 * scaled * scaled);
        }
    }
    return min(coverage, 1.0);
}

// Light from scene lights and the photograph's light sources that a drop at
// `depth` refracts toward the camera.
fn rain_drop_light(pos: vec2<i32>, params: MaskEffectParams, depth: f32) -> vec3<f32> {
    let glow = mask_effect_percent(params, RAIN_LIGHT_GLOW_LANE);
    if glow <= 1e-6 { return vec3<f32>(0.0); }
    var result = scene_light_received(pos, depth, RAIN_LIGHT_ANISOTROPY)
        * (glow * RAIN_LIGHT_GAIN * scene_ambient_level());
    if medium_image_lights_enabled(params) {
        result += image_light_at(pos) * (glow * RAIN_IMAGE_LIGHT_GAIN);
    }
    return result;
}

fn apply_rain(pos: vec2<i32>, input_rgb: vec3<f32>, params: MaskEffectParams) -> vec3<f32> {
    let amount = mask_effect_percent(params, RAIN_AMOUNT_LANE);
    if amount <= 1e-6 { return input_rgb; }
    let wants_depth = mask_effect_lane(params, RAIN_DEPTH_LANE) > 0.5;
    let uses_depth = wants_depth && scene_depth_present();
    var surface_depth = select(1.0, SCENE_DEPTH_UNKNOWN, wants_depth);
    if uses_depth {
        // Mist builds up over real distance and the layers lie at fixed distances.
        surface_depth = scene_distance_at(pos);
    }
    let uv = full_image_uv(pos);
    let camera = scene_camera(SCENE_DEFAULT_RELIEF_CONTROL);

    // The light around the drops. Light reaches them from above as well.
    let around = 0.6 * scene_environment_blurred_at(uv)
        + 0.4 * scene_environment_blurred_at(vec2<f32>(uv.x, uv.y * 0.3));

    let seed = u32(clamp(mask_effect_lane(params, RAIN_SEED_LANE), 0.0, 1000.0));
    let seed_offset = vec2<f32>(f32(seed) * 0.071 + 5.3, f32(seed) * -0.113 + 41.7);
    let image_size = Common::scene_image_size();
    let center = 0.5 * image_size;
    let point = uv * image_size;
    let fall = mask_effect_lane_pair(params, RAIN_FALL_X_LANE);
    let across = vec2<f32>(fall.y, -fall.x);
    let relative = point - center;
    let plane = vec2<f32>(dot(relative, across), dot(relative, fall));
    // A homography: straight lines stay straight, and streaks parallel in the
    // warped plane meet at a vanishing point on the fall axis.
    let perspective = clamp(mask_effect_lane(params, RAIN_PERSPECTIVE_LANE) / 100.0, -1.0, 1.0);
    let denominator = max(1.0 + perspective * 0.7 * plane.y / max(image_size.x, image_size.y), 0.15);
    var warped = plane / denominator;

    // Gusts: the rain gathers into uneven curtains and bends along them.
    let gusts = mask_effect_percent(params, RAIN_GUSTS_LANE);
    let curtain_point = vec2<f32>(warped.x * 2.1, warped.y * 0.55) + seed_offset;
    let curtain = effect_fbm(curtain_point);
    warped.x += gusts * RAIN_GUST_BEND
        * (effect_noise(curtain_point * 0.7 + vec2<f32>(9.2, 3.1)) - 0.5);
    let strength = mix(1.0, smoothstep(0.28, 0.72, curtain) * 1.7, gusts);

    let size = mask_effect_percent(params, RAIN_DROP_SIZE_LANE);
    let travel = mask_effect_percent(params, RAIN_LENGTH_LANE);
    let focus_control = mask_effect_percent(params, RAIN_FOCUS_LANE);
    let field = RainField(
        warped,
        denominator,
        mix(RAIN_WIDTH_RANGE.x, RAIN_WIDTH_RANGE.y, size * size),
        mix(RAIN_LENGTH_RANGE.x, RAIN_LENGTH_RANGE.y, travel * travel),
        pow(amount, 1.2),
        mix(0.3, 3.0, focus_control * focus_control),
        seed,
    );

    // The mist of drops too small and many to resolve: extinction grows with
    // the rain rate as R^0.67 (Tremblay et al., "Rain Rendering for Evaluating
    // and Improving Robustness to Bad Weather", 2021), and it scatters scene
    // lights as Fog does.
    let mist = mask_effect_percent(params, RAIN_MIST_LANE);
    let extinction = RAIN_MIST_DEPTH * mist * pow(amount, 0.67) * mix(1.0, strength, 0.5);
    let distance = max(surface_depth, 0.0);
    let optical_depth = extinction * (RAIN_MIST_NEAR + distance);
    let transmission = exp(-optical_depth);
    var rgb = input_rgb * transmission + around * RAIN_MIST_GAIN * (1.0 - transmission);
    let glow = mask_effect_percent(params, RAIN_LIGHT_GLOW_LANE);
    if extinction > 1e-6 && glow > 1e-6 {
        let ray = vec3<f32>(effect_image_point(pos) * 1.25, 1.0);
        let volume = MediumVolume(ray, 0.0, 0.1, 1.0, vec3<f32>(0.0), 0.0, 0.5);
        let scattered = medium_light_scattering(pos, volume, distance, extinction);
        rgb += scattered * (glow * RAIN_MIST_GLOW_GAIN * scene_ambient_level());
        rgb += medium_image_light(pos, params, glow, 1.0, 1.0 - transmission);
    }

    // Streaks, far to near, each layer veiled by the mist in front of it.
    let around_light = around * RAIN_ENVIRONMENT_GAIN;
    for (var layer = 0u; layer < RAIN_LAYERS; layer = layer + 1u) {
        let depth = rain_layer_depth(layer);
        var visible = 1.0;
        if depth > 0.0 && uses_depth {
            visible = smoothstep(
                depth - RAIN_OCCLUSION_SOFTNESS,
                depth + RAIN_OCCLUSION_SOFTNESS,
                surface_depth,
            );
            if visible <= 1e-4 { continue; }
        }
        let layer_distance = scene_depth_z(camera, depth);
        let coverage = rain_layer_coverage(field, layer, layer_distance) * visible * min(strength, 1.0);
        if coverage <= 1e-4 { continue; }
        let veil = exp(-extinction * (RAIN_MIST_NEAR + max(depth, 0.0)));
        let drop = around_light + rain_drop_light(pos, params, depth);
        rgb = mix(rgb, drop, coverage * veil);
    }
    return rgb;
}
