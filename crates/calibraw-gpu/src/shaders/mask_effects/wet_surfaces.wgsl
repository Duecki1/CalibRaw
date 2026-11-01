// Wet Surfaces: wet materials, the sheen of wet ground, standing water and what
// they reflect.
//
// Water darkens a rough surface because light its surface scatters back is
// partly reflected down again by the water film, and absorbed. With dry albedo
// a, the wet albedo is (1 - p) a / (1 - p a) (Lekner and Dorf, "Why some things
// are darker when wet", 1988, equation 1 without its surface reflection, which
// the reflections here add). Dark materials darken most, and each channel
// darkens by its own albedo, so wet colours deepen by themselves.
//
// The weather map says where the ground is and how far each part of it lies
// below what stands on it (weather_analysis.wgsl). A vertical object mirrors
// about the row it stands on: a pixel at some distance below it shows the object
// the same distance above it. Reflections are read from the environment map,
// which holds the whole photo for every export tile, and spread along the view
// on rough ground, which elongates them toward the viewer.
//
// Standing water has a level surface, so its edge is where that level meets the
// ground: a contour line. Everything that limits a puddle therefore raises the
// ground rather than fading the water: the terrain in the weather map, rough
// surfaces and what is not ground. The photo's own detail is the last height: its
// lighter grains stand clear of shallow water and its dark joints and pits fill
// first, so an edge breaks up on the texture that is really there. Ground beside
// the water has drawn it in, and stays dark past the edge.

// Packed parameter lanes (effect_lanes.rs).
const WET_SURFACES_WETNESS_LANE: u32 = 0u;
const WET_SURFACES_PUDDLES_LANE: u32 = 1u;
const WET_SURFACES_PUDDLE_SIZE_LANE: u32 = 2u;
const WET_SURFACES_GLOSS_LANE: u32 = 3u;
const WET_SURFACES_REFLECTIONS_LANE: u32 = 4u;
const WET_SURFACES_RIPPLES_LANE: u32 = 5u;
const WET_SURFACES_SEED_LANE: u32 = 6u;

// Share of light the water film reflects back to the surface (p above, for a
// refractive index of 4/3: Lekner and Dorf's equation 9).
const WET_INTERNAL_REFLECTION: f32 = 0.475;
// Share of the ground's darkening that walls and other surfaces take: rain runs
// off them.
const WET_OTHER_SURFACES: f32 = 0.72;
// Depth difference at which a ground cell stops contributing to a pixel, in
// normalized depth.
const WET_DEPTH_SIGMA: f32 = 0.025;
// Depths beyond which nothing is wet: the sky and far scenery.
const WET_FAR_START: f32 = 0.9;
const WET_FAR_END: f32 = 0.97;
// Surface roughness (weather_cell_roughness) from which a surface starts to lose
// its sheen, and at which it holds no standing water: gravel and grass drink it.
const WET_ROUGHNESS_RANGE: vec2<f32> = vec2<f32>(0.018, 0.04);
// How far rough surfaces and what is not ground rise above the water, in the units
// of the puddle field: past its whole range.
const WET_NOT_GROUND_RISE: f32 = 0.6;
// Octaves of the puddle field.
const WET_UNDULATION_OCTAVES: u32 = 3u;
// Height of the photo's detail at the smallest Puddle size, in units of the puddle
// field per standard deviation of the local guide, and how many deviations count.
const WET_DETAIL_RELIEF: f32 = 0.04;
const WET_DETAIL_LIMIT: f32 = 2.5;
// Distance of the outer taps of the detail's footprint, in pixels.
const WET_DETAIL_REACH: i32 = 2;
// Smallest relative deviation of the guide to divide by: smoother than this is
// noise, not relief.
const WET_DETAIL_MIN_DEVIATION: f32 = 0.02;
// Height below the water's level over which the ground beside it has soaked it up,
// at the smallest Puddle size.
const WET_SOAK_REACH: f32 = 0.05;
// Half the height over which a pixel goes from dry to under water at the smallest
// Puddle size, in units of the puddle field. A pixel spans many grains, some already under water and some not,
// so its coverage is partial rather than a hard flip that would read as dither.
const WET_WATER_EDGE: f32 = 0.035;
// Taps of a reflection along the view.
const WET_REFLECTION_TAPS: i32 = 3;
// How far a rough reflection spreads along the view, relative to its distance
// from its contact point.
const WET_REFLECTION_SPREAD: f32 = 0.9;
// Reflectance of water at normal incidence.
const WET_F0: f32 = 0.02;
// Camera height units per image point below the horizon, and puddles per
// camera height at Puddle size 0 and 100.
const WET_GROUND_SCALE: f32 = 1.25;
const WET_PUDDLE_FREQUENCY: vec2<f32> = vec2<f32>(2.4, 0.5);
// Ripple cells per camera height.
const WET_RIPPLE_FREQUENCY: f32 = 7.0;
// Largest image offset of a reflection in a ripple, relative to its distance
// from its contact point.
const WET_RIPPLE_DISTORTION: f32 = 0.25;
// Scene-light highlight strength relative to the ambient level.
const WET_GLINT_GAIN: f32 = 2.5;
const WET_GLINT_LIMIT: f32 = 24.0;

// Everything about the ground at one pixel.
struct WetPixel {
    // Edge-accurate scene depth.
    depth: f32,
    // How much the pixel is ground.
    on_ground: f32,
    // Distance from the pixel along the ground to what stands on it.
    contact: f32,
    // Distance from the pixel down from the horizon.
    below: f32,
    // Image point of the pixel.
    point: vec2<f32>,
    // Unit vector in image points toward the horizon.
    up: vec2<f32>,
    // 1 on smooth surfaces, 0 on rough ones.
    smooth_surface: f32,
    // The ground's height (weather_terrain_height).
    terrain: f32,
    // The guide's mean and standard deviation around the pixel.
    guide_mean: f32,
    guide_deviation: f32,
}

// The pixel's ground: the surrounding map cells, each weighted by how near its
// depth lies to the pixel's, which keeps the ground's edge on the edge-accurate
// depth rather than on the coarse cells. The contact distance comes only from
// cells on the ground.
fn wet_pixel(pos: vec2<i32>) -> WetPixel {
    let map = weather_map_at(pos);
    let depth = select(0.0, weather_depth_from(map, pos), weather_depth_present());
    let ground = Common::effects_uniforms.weather_ground;
    let point = CreativeEffects::full_image_uv(pos) * Common::weather_image_size();
    let below = ground.z - dot(point, ground.xy);
    let smooth_surface = 1.0 - smoothstep(WET_ROUGHNESS_RANGE.x, WET_ROUGHNESS_RANGE.y, map.z);
    let guide_deviation = max(sqrt(max(map.z, 0.0)), WET_DETAIL_MIN_DEVIATION) * max(map.w, 1e-3);
    var pixel = WetPixel(depth, 0.0, 0.0, below, point, ground.xy, smooth_surface, 0.0, map.w, guide_deviation);
    if ground.w < 0.5 || !weather_depth_present() || below <= 0.0 { return pixel; }

    let grid = vec2<i32>(Common::weather_grid());
    let position = CreativeEffects::full_image_uv(pos) * vec2<f32>(grid) - vec2<f32>(0.5);
    let base = vec2<i32>(floor(position)) - vec2<i32>(1);
    var score = 0.0;
    var weights = 0.0;
    var contact = 0.0;
    var terrain = 0.0;
    var on_ground_weights = 0.0;
    for (var y = 0; y < 4; y = y + 1) {
        for (var x = 0; x < 4; x = x + 1) {
            let cell = base + vec2<i32>(x, y);
            let offset = abs(vec2<f32>(cell) - position);
            let tent = max(1.0 - 0.5 * offset.x, 0.0) * max(1.0 - 0.5 * offset.y, 0.0);
            if tent <= 0.0 { continue; }
            let texel = textureLoad(
                weather_ground_tex,
                clamp(cell, vec2<i32>(0), grid - vec2<i32>(1)),
                0,
            );
            let separation = (texel.z - depth) / WET_DEPTH_SIGMA;
            let weight = tent * max(exp(-separation * separation), 1e-4);
            score += texel.x * weight;
            weights += weight;
            contact += texel.y * texel.x * weight;
            terrain += texel.w * texel.x * weight;
            on_ground_weights += texel.x * weight;
        }
    }
    pixel.on_ground = score / max(weights, 1e-6);
    pixel.contact = contact / max(on_ground_weights, 1e-6);
    pixel.terrain = terrain / max(on_ground_weights, 1e-6);
    return pixel;
}

// Position on the ground in camera heights (x across the view, y along it), and
// in its z the ground distance one pixel covers here.
fn wet_ground_position(pixel: WetPixel) -> vec3<f32> {
    let right = vec2<f32>(-pixel.up.y, pixel.up.x);
    let lateral = dot(pixel.point - 0.5 * Common::weather_image_size(), right);
    let clamped = max(pixel.below, 0.01);
    return vec3<f32>(
        lateral * WET_GROUND_SCALE / clamped,
        WET_GROUND_SCALE / clamped,
        WET_GROUND_SCALE * weather_pixel_extent() / (clamped * clamped),
    );
}

// Height of the photo's detail at a pixel, in standard deviations of the local guide.
// It is read over a few pixels: grains span several, and single pixels would add
// sensor noise that reads as dither along the water's edge. WET_SURFACES_SUPPORT in
// tiles.rs reserves their reach.
fn wet_detail(pos: vec2<i32>, pixel: WetPixel) -> f32 {
    let reach = WET_DETAIL_REACH;
    let guide = (2.0 * SceneSource::weather_guide_at(pos)
        + SceneSource::weather_guide_at(pos + vec2<i32>(reach, 0))
        + SceneSource::weather_guide_at(pos - vec2<i32>(reach, 0))
        + SceneSource::weather_guide_at(pos + vec2<i32>(0, reach))
        + SceneSource::weather_guide_at(pos - vec2<i32>(0, reach))) / 6.0;
    return clamp((guide - pixel.guide_mean) / pixel.guide_deviation, -WET_DETAIL_LIMIT, WET_DETAIL_LIMIT);
}

// The ground's gentle undulation, 0 to 1: the broad forms that make basins. Finer
// forms would only draw tendrils; the photo's detail breaks up the edge instead.
fn wet_undulation(position: vec2<f32>) -> f32 {
    var point = position;
    var amplitude = 0.5;
    var value = 0.0;
    var normalization = 0.0;
    for (var octave = 0u; octave < WET_UNDULATION_OCTAVES; octave = octave + 1u) {
        value += CreativeEffects::atmosphere_noise(point) * amplitude;
        normalization += amplitude;
        point = vec2<f32>(point.x * 1.62 + point.y * 1.17, point.x * -1.17 + point.y * 1.62)
            + vec2<f32>(13.7, 9.2);
        amplitude *= 0.45;
    }
    return value / normalization;
}

// Water at a pixel: how much of it is under standing water and how much the ground
// has soaked up beside it. Detail too fine to resolve near the horizon fades to its
// mean, so puddles dissolve into the far sheen instead of aliasing.
struct WetWater {
    standing: f32,
    soaked: f32,
}

fn wet_water(pos: vec2<i32>, pixel: WetPixel, position: vec3<f32>, params: CreativeEffects::MaskEffectParams) -> WetWater {
    let amount = clamp(CreativeEffects::mask_effect_lane(params, WET_SURFACES_PUDDLES_LANE) / 100.0, 0.0, 1.0);
    if amount <= 1e-5 { return WetWater(0.0, 0.0); }
    let size_control = clamp(CreativeEffects::mask_effect_lane(params, WET_SURFACES_PUDDLE_SIZE_LANE) / 100.0, 0.0, 1.0);
    let frequency = mix(WET_PUDDLE_FREQUENCY.x, WET_PUDDLE_FREQUENCY.y, size_control);
    let seed = clamp(CreativeEffects::mask_effect_lane(params, WET_SURFACES_SEED_LANE), 0.0, 1000.0);
    let offset = vec2<f32>(seed * 0.113 + 3.7, seed * -0.071 + 17.1);
    let field = wet_undulation(position.xy * frequency + offset);
    let threshold = mix(0.72, 0.36, amount);
    let rise = pixel.terrain + (1.0 - pixel.on_ground * pixel.smooth_surface) * WET_NOT_GROUND_RISE;
    // Above zero the ground lies below the water's level.
    let level = field - threshold - rise;
    // Broader forms have gentler slopes in the field, so the edge's heights shrink
    // with them to keep the same width on the ground.
    let edge_scale = frequency / WET_PUDDLE_FREQUENCY.x;
    let relief = WET_DETAIL_RELIEF * edge_scale;
    let edge = WET_WATER_EDGE * edge_scale + position.z * frequency;
    let resolved = 1.0 - smoothstep(0.4, 1.2, position.z * frequency);
    let mean = (1.0 - smoothstep(0.3, 0.75, threshold)) * amount * (1.0 - smoothstep(0.0, 0.1, rise));
    // Far from the water's edge the detail changes nothing, and costs reads.
    var surface = level;
    if abs(level) < WET_DETAIL_LIMIT * relief + edge {
        surface -= wet_detail(pos, pixel) * relief;
    }
    return WetWater(
        mix(mean, smoothstep(-edge, edge, surface), resolved),
        mix(mean, smoothstep(-WET_SOAK_REACH * edge_scale, 0.0, level), resolved),
    );
}

// How hard any Rain effect in the edit falls.
fn wet_rain_strength() -> f32 {
    var strength = 0.0;
    for (var index = 0u; index < weather_slots(); index = index + 1u) {
        if !Common::mask_slot_applies(Common::mask_data[index].metadata, MASK_EFFECT_RAIN_ID) {
            continue;
        }
        let params = CreativeEffects::mask_effect_params(index);
        strength = max(strength, CreativeEffects::mask_effect_lane(params, RAIN_AMOUNT_LANE) / 100.0);
    }
    return clamp(strength, 0.0, 1.0);
}

// The tilt of the water surface from rain's ripples, in ground units: expanding
// rings around random points, fading as they spread. It fades where a ring would
// be smaller than a pixel.
fn wet_ripples(position: vec3<f32>, seed: u32) -> vec2<f32> {
    let frequency = WET_RIPPLE_FREQUENCY;
    let resolved = 1.0 - smoothstep(0.08, 0.3, position.z * frequency);
    if resolved <= 1e-4 { return vec2<f32>(0.0); }
    let point = position.xy * frequency;
    let cell = vec2<i32>(floor(point));
    var tilt = vec2<f32>(0.0);
    for (var dy = -1; dy <= 1; dy = dy + 1) {
        for (var dx = -1; dx <= 1; dx = dx + 1) {
            let candidate = cell + vec2<i32>(dx, dy);
            let random = rain_cell_random(candidate, seed * 2097593u + 31u);
            if random.w < 0.25 { continue; }
            let toward = point - (vec2<f32>(candidate) + random.xy);
            let radius = length(toward);
            let age = random.z;
            let ring = (radius - age * 1.1) / 0.09;
            if abs(ring) > 3.0 { continue; }
            let wave = sin(ring * 2.2) * exp(-0.5 * ring * ring) * (1.0 - age) * (1.0 - age);
            tilt += toward / max(radius, 1e-4) * wave;
        }
    }
    return tilt * resolved;
}

// What the ground reflects at a pixel, spread along the view by `roughness`.
fn wet_reflected(pixel: WetPixel, roughness: f32, distortion: vec2<f32>) -> vec3<f32> {
    let image_size = Common::weather_image_size();
    let mirrored = pixel.point + pixel.up * (2.0 * pixel.contact) + distortion;
    let spread = roughness * WET_REFLECTION_SPREAD * (pixel.contact + 0.02);
    var sum = vec3<f32>(0.0);
    var weights = 0.0;
    for (var tap = -WET_REFLECTION_TAPS; tap <= WET_REFLECTION_TAPS; tap = tap + 1) {
        let offset = f32(tap) / f32(WET_REFLECTION_TAPS);
        let weight = exp(-2.0 * offset * offset);
        let at = mirrored + pixel.up * (offset * spread);
        sum += weather_environment_at(at / image_size) * weight;
        weights += weight;
    }
    // A rough surface also scatters sideways, by less.
    return mix(
        sum / weights,
        weather_environment_blurred_at(mirrored / image_size),
        smoothstep(0.35, 0.9, roughness) * 0.5,
    );
}

fn wet_surface(pos: vec2<i32>, input_rgb: vec3<f32>, params: CreativeEffects::MaskEffectParams) -> vec3<f32> {
    let wetness = clamp(CreativeEffects::mask_effect_lane(params, WET_SURFACES_WETNESS_LANE) / 100.0, 0.0, 1.0);
    let pixel = wet_pixel(pos);
    let reachable = 1.0 - smoothstep(WET_FAR_START, WET_FAR_END, pixel.depth);

    // Darkening, by each channel's albedo estimated against the ambient level,
    // where middle grey is 18% reflectance. Lamps and sky are past albedo 1 and
    // stay as they are.
    let ambient = CreativeEffects::scene_ambient_level();
    let positive = max(input_rgb, vec3<f32>(0.0));
    let albedo = clamp(0.18 * positive / ambient, vec3<f32>(1e-4), vec3<f32>(1.0));
    let p = WET_INTERNAL_REFLECTION;
    let darkened = input_rgb * ((1.0 - p) / (vec3<f32>(1.0) - p * albedo));

    var water = WetWater(0.0, 0.0);
    var position = vec3<f32>(0.0);
    if pixel.on_ground > 1e-4 {
        position = wet_ground_position(pixel);
        water = wet_water(pos, pixel, position, params);
    }
    let puddle = water.standing;
    let soaked = max(wetness * mix(WET_OTHER_SURFACES, 1.0, pixel.on_ground), max(water.soaked, puddle));
    let rgb = mix(input_rgb, darkened, soaked * reachable);
    if pixel.on_ground <= 1e-4 { return rgb; }

    // What the ground reflects: stronger toward the horizon, as for all water.
    let reflections = clamp(CreativeEffects::mask_effect_lane(params, WET_SURFACES_REFLECTIONS_LANE) / 100.0, 0.0, 1.0);
    let gloss = clamp(CreativeEffects::mask_effect_lane(params, WET_SURFACES_GLOSS_LANE) / 100.0, 0.0, 1.0);
    let view_cosine = sin(atan(pixel.below * 2.0 * CreativeEffects::SCENE_FOCAL_TAN));
    let fresnel = WET_F0 + (1.0 - WET_F0) * pow(1.0 - view_cosine, 5.0);
    let sheen = wetness * mix(0.18, 0.75, gloss) * mix(0.25, 1.0, pixel.smooth_surface);
    let reflectance = fresnel * max(sheen, puddle) * pixel.on_ground * reflections * 1.6;
    if reflectance <= 1e-4 { return rgb; }

    let film_roughness = mix(
        mix(1.0, 0.85, pixel.smooth_surface),
        mix(0.6, 0.28, pixel.smooth_surface),
        gloss,
    );
    let roughness = mix(film_roughness, 0.05, puddle);
    var distortion = vec2<f32>(0.0);
    var ripple_light = 0.0;
    let ripples = clamp(CreativeEffects::mask_effect_lane(params, WET_SURFACES_RIPPLES_LANE) / 100.0, 0.0, 1.0)
        * wet_rain_strength() * puddle;
    if ripples > 1e-4 {
        let seed = u32(clamp(CreativeEffects::mask_effect_lane(params, WET_SURFACES_SEED_LANE), 0.0, 1000.0));
        let tilt = wet_ripples(position, seed) * ripples;
        let right = vec2<f32>(-pixel.up.y, pixel.up.x);
        distortion = (right * tilt.x + pixel.up * tilt.y) * WET_RIPPLE_DISTORTION * (pixel.contact + 0.05);
        ripple_light = length(tilt);
    }
    let shimmer = min(reflectance * (1.0 + ripple_light), 1.0);
    return rgb * (1.0 - shimmer) + wet_reflected(pixel, roughness, distortion) * shimmer;
}

// Highlights of scene lights on the wet ground, added after the creative
// effects, so Relight's Ambient does not dim them. A GGX lobe stretches into a
// long streak toward the viewer at grazing angles by itself.
fn wet_glints(pos: vec2<i32>, params: CreativeEffects::MaskEffectParams) -> vec3<f32> {
    let reflections = clamp(CreativeEffects::mask_effect_lane(params, WET_SURFACES_REFLECTIONS_LANE) / 100.0, 0.0, 1.0);
    let wetness = clamp(CreativeEffects::mask_effect_lane(params, WET_SURFACES_WETNESS_LANE) / 100.0, 0.0, 1.0);
    if reflections <= 1e-4 || !CreativeEffects::scene_lights_present() {
        return vec3<f32>(0.0);
    }
    let pixel = wet_pixel(pos);
    if pixel.on_ground <= 1e-4 { return vec3<f32>(0.0); }
    let position = wet_ground_position(pixel);
    let puddle = wet_water(pos, pixel, position, params).standing;
    let gloss = clamp(CreativeEffects::mask_effect_lane(params, WET_SURFACES_GLOSS_LANE) / 100.0, 0.0, 1.0);
    let roughness = mix(mix(0.55, 0.16, gloss), 0.05, puddle);
    let alpha = roughness * roughness;
    let strength = max(wetness * mix(0.3, 1.0, gloss) * mix(0.25, 1.0, pixel.smooth_surface), puddle)
        * pixel.on_ground;

    // The ground's normal in the camera frame: perpendicular to every view ray
    // through the horizon.
    let image_center = 0.5 * Common::weather_image_size();
    let horizon_offset = Common::effects_uniforms.weather_ground.z - dot(image_center, pixel.up);
    let normal = normalize(vec3<f32>(
        pixel.up,
        -horizon_offset * 2.0 * CreativeEffects::SCENE_FOCAL_TAN,
    ));

    let uv = CreativeEffects::full_image_uv(pos);
    var glint = vec3<f32>(0.0);
    for (var index = 0u; index < CreativeEffects::scene_light_slots(); index = index + 1u) {
        let light = CreativeEffects::scene_light_at(index);
        if !light.emits { continue; }
        let coverage = CreativeEffects::scene_light_coverage(light, pos);
        if coverage <= 1e-6 { continue; }
        let surface = CreativeEffects::scene_camera_point(
            light.camera,
            uv * light.camera.image_size,
            CreativeEffects::scene_depth_z(light.camera, pixel.depth),
        );
        let to_light = normalize(light.position - surface);
        let to_camera = normalize(-surface);
        let halfway = normalize(to_light + to_camera);
        let n_l = dot(normal, to_light);
        let n_v = dot(normal, to_camera);
        if n_l <= 0.0 || n_v <= 0.0 { continue; }
        let n_h = max(dot(normal, halfway), 0.0);
        let denominator = n_h * n_h * (alpha * alpha - 1.0) + 1.0;
        let distribution = alpha * alpha / (3.14159265 * denominator * denominator);
        let v_h = max(dot(to_camera, halfway), 0.0);
        let fresnel = WET_F0 + (1.0 - WET_F0) * pow(1.0 - v_h, 5.0);
        let specular = min(distribution * fresnel * n_l / max(4.0 * v_h * v_h, 0.1), WET_GLINT_LIMIT);
        glint += light.intensity
            * (specular * CreativeEffects::scene_light_falloff(light, surface) * coverage);
    }
    return glint * (WET_GLINT_GAIN * CreativeEffects::scene_ambient_level() * reflections * strength);
}
