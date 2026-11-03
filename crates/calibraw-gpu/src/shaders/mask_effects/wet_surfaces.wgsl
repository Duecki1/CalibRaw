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
// The scene map says where the ground is and how far each part of it lies
// below what stands on it (scene_analysis.wgsl). A vertical object mirrors
// about the row it stands on: a pixel at some distance below it shows the object
// the same distance above it. Reflections are read from the environment map,
// which holds the whole photo for every export tile, and spread along the view
// on rough ground, which elongates them toward the viewer.
//
// Standing water has a level surface, so its edge is where that level meets the
// ground: a contour line. Everything that limits a puddle therefore raises the
// ground rather than fading the water: the terrain in the scene map, rough
// surfaces and what is not ground. The photo's own detail is the last height: its
// lighter grains stand clear of shallow water and its dark joints and pits fill
// first, so an edge breaks up on the texture that is really there. Ground beside
// the water has drawn it in, and stays dark past the edge.
//
// Water is photographed through the same lens as the ground it lies on. Out of focus
// its edge and ripples blur as much as the ground does (scene_focus_blur), and the
// photo there has no grain left for the edge to break up on.

// Packed parameter lanes: `WET_SURFACES_<NAME>_LANE` (generated from the effect
// registry, effect_wgsl.rs).

// Share of light the water film reflects back to the surface (p above, for a
// refractive index of 4/3: Lekner and Dorf's equation 9).
const WET_INTERNAL_REFLECTION: f32 = 0.475;
// Share of the ground's darkening that walls and other surfaces take: rain runs
// off them.
const WET_OTHER_SURFACES: f32 = 0.72;
// Depths beyond which nothing is wet: the sky and far scenery.
const WET_FAR_START: f32 = 0.9;
const WET_FAR_END: f32 = 0.97;
// Surface roughness (scene_map_cell_roughness) from which a surface starts to lose
// its sheen, and at which it holds no standing water: gravel and grass drink it.
const WET_ROUGHNESS_RANGE: vec2<f32> = vec2<f32>(0.018, 0.04);
// How far rough surfaces and what is not ground rise above the water, in the units
// of the puddle field: past its whole range.
const WET_NOT_GROUND_RISE: f32 = 0.6;
// Octaves of the puddle field.
const WET_UNDULATION_OCTAVES: u32 = 3u;
// Height of the photo's grain at the smallest Puddle size, in units of the puddle
// field per WET_DETAIL_CONTRAST of it, and how many of those count.
const WET_DETAIL_RELIEF: f32 = 0.04;
const WET_DETAIL_LIMIT: f32 = 2.5;
// Relative difference of the guide that makes one unit of grain: well above noise.
const WET_DETAIL_CONTRAST: f32 = 0.08;
// Distances in pixels of the outer taps of the grain's core and of the ring around it.
const WET_DETAIL_CORE: i32 = 2;
const WET_DETAIL_REACH: i32 = 6;
// Radius of the lens blur's footprint relative to the measured deviation: past a
// Gaussian's 10 to 90 percent rise (1.3), as noise makes the photo's edges measure
// narrower than they look beside the water.
const WET_EDGE_PER_DEVIATION: f32 = 2.2;
// Pixels of ground a ripple's ring blurs over per pixel of the lens blur's deviation.
const WET_RIPPLE_FOCUS: f32 = 2.0;
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
// GGX alpha of the highlight of emitted light that arrives evenly from all
// around (scene_emission.wgsl); light from one direction keeps the ground's.
const WET_EMISSION_SPREAD_ALPHA: f32 = 0.6;

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
    // The ground's height (scene_terrain_height).
    terrain: f32,
    // The guide's mean around the pixel.
    guide_mean: f32,
    // The lens blur here in shorter edges.
    blur: f32,
}

// The pixel's ground: the surrounding map cells, each weighted by how near its
// depth lies to the pixel's, which keeps the ground's edge on the edge-accurate
// depth rather than on the coarse cells. The contact distance comes only from
// cells on the ground.
fn wet_pixel(pos: vec2<i32>) -> WetPixel {
    let map = scene_depth_map_at(pos);
    let depth = select(0.0, scene_depth_from(map, pos), scene_depth_present());
    let ground = Common::effects_uniforms.scene_ground;
    let point = full_image_uv(pos) * Common::scene_image_size();
    let below = ground.z - dot(point, ground.xy);
    let smooth_surface = 1.0 - smoothstep(WET_ROUGHNESS_RANGE.x, WET_ROUGHNESS_RANGE.y, map.z);
    let blur = select(0.0, scene_focus_blur(depth), scene_depth_present());
    var pixel = WetPixel(depth, 0.0, 0.0, below, point, ground.xy, smooth_surface, 0.0, map.w, blur);
    if ground.w < 0.5 || !scene_depth_present() || below <= 0.0 { return pixel; }

    let grid = vec2<i32>(Common::scene_map_grid());
    let position = full_image_uv(pos) * vec2<f32>(grid) - vec2<f32>(0.5);
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
                scene_ground_tex,
                clamp(cell, vec2<i32>(0), grid - vec2<i32>(1)),
                0,
            );
            let weight = tent * scene_depth_closeness(texel.z, depth);
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

// Position on the ground of an image point in camera heights (x across the view, y
// along it), and in its z the ground distance one pixel covers there.
fn wet_ground_position(point: vec2<f32>) -> vec3<f32> {
    let ground = Common::effects_uniforms.scene_ground;
    let right = vec2<f32>(-ground.y, ground.x);
    let lateral = dot(point - 0.5 * Common::scene_image_size(), right);
    let clamped = max(ground.z - dot(point, ground.xy), 0.01);
    return vec3<f32>(
        lateral * WET_GROUND_SCALE / clamped,
        WET_GROUND_SCALE / clamped,
        WET_GROUND_SCALE * scene_pixel_extent() / (clamped * clamped),
    );
}

// Height of the photo's grain at a pixel: how far the guide of a few pixels lies from
// the ring around them, relative to the mean guide. Only grain counts: painted lines
// and tile patterns are not relief. Out of focus the photo has no grain left, so the
// water's edge there follows nothing but the blurred field. WET_SURFACES_SUPPORT in
// tiles.rs reserves the ring's reach.
fn wet_detail(pos: vec2<i32>, pixel: WetPixel) -> f32 {
    let core = (2.0 * SceneSource::scene_guide_at(pos)
        + SceneSource::scene_guide_at(pos + vec2<i32>(WET_DETAIL_CORE, 0))
        + SceneSource::scene_guide_at(pos - vec2<i32>(WET_DETAIL_CORE, 0))
        + SceneSource::scene_guide_at(pos + vec2<i32>(0, WET_DETAIL_CORE))
        + SceneSource::scene_guide_at(pos - vec2<i32>(0, WET_DETAIL_CORE))) / 6.0;
    let ring = (SceneSource::scene_guide_at(pos + vec2<i32>(WET_DETAIL_REACH, 0))
        + SceneSource::scene_guide_at(pos - vec2<i32>(WET_DETAIL_REACH, 0))
        + SceneSource::scene_guide_at(pos + vec2<i32>(0, WET_DETAIL_REACH))
        + SceneSource::scene_guide_at(pos - vec2<i32>(0, WET_DETAIL_REACH))) / 4.0;
    return clamp(
        (core - ring) / (WET_DETAIL_CONTRAST * max(pixel.guide_mean, 1e-3)),
        -WET_DETAIL_LIMIT,
        WET_DETAIL_LIMIT,
    );
}

// The ground's gentle undulation, 0 to 1: the broad forms that make basins. Finer
// forms would only draw tendrils; the photo's detail breaks up the edge instead.
fn wet_undulation(position: vec2<f32>) -> f32 {
    return effect_fbm_octaves(position, WET_UNDULATION_OCTAVES, 0.45);
}

// Water at a pixel: how much of it is under standing water and how much the ground
// has soaked up beside it. Detail too fine to resolve near the horizon fades to its
// mean, so puddles dissolve into the far sheen instead of aliasing.
struct WetWater {
    standing: f32,
    soaked: f32,
}

// What sets where puddles lie: their field, seed and how high the water stands in it.
struct WetBasins {
    frequency: f32,
    offset: vec2<f32>,
    threshold: f32,
}

// How far the ground rises from a surface's terrain and whether it can hold water.
fn wet_rise(terrain: f32, on_ground: f32, smooth_surface: f32) -> f32 {
    return terrain + (1.0 - on_ground * smooth_surface) * WET_NOT_GROUND_RISE;
}

fn wet_field(point: vec2<f32>, basins: WetBasins) -> f32 {
    return wet_undulation(wet_ground_position(point).xy * basins.frequency + basins.offset);
}

// How far the water's level lies above the ground at an image point near a pixel:
// from the interpolated maps, without the pixel's edge-accurate depth.
fn wet_level_near(point: vec2<f32>, basins: WetBasins) -> f32 {
    let uv = point / Common::scene_image_size();
    let ground = scene_ground_near(uv);
    let roughness = scene_depth_map_near(uv).z;
    let smooth_surface = 1.0 - smoothstep(WET_ROUGHNESS_RANGE.x, WET_ROUGHNESS_RANGE.y, roughness);
    return wet_field(point, basins) - basins.threshold - wet_rise(ground.w, ground.x, smooth_surface);
}

fn wet_water(pos: vec2<i32>, pixel: WetPixel, position: vec3<f32>, params: MaskEffectParams) -> WetWater {
    let amount = mask_effect_percent(params, WET_SURFACES_PUDDLES_LANE);
    if amount <= 1e-5 { return WetWater(0.0, 0.0); }
    let size_control = mask_effect_percent(params, WET_SURFACES_PUDDLE_SIZE_LANE);
    let frequency = mix(WET_PUDDLE_FREQUENCY.x, WET_PUDDLE_FREQUENCY.y, size_control);
    let seed = clamp(mask_effect_lane(params, WET_SURFACES_SEED_LANE), 0.0, 1000.0);
    let basins = WetBasins(
        frequency,
        vec2<f32>(seed * 0.113 + 3.7, seed * -0.071 + 17.1),
        mix(0.72, 0.36, amount),
    );
    let rise = wet_rise(pixel.terrain, pixel.on_ground, pixel.smooth_surface);
    // Above zero the ground lies below the water's level.
    let level = wet_field(pixel.point, basins) - basins.threshold - rise;
    // Broader forms have gentler slopes in the field, so the edge's heights shrink
    // with them to keep the same width on the ground.
    let edge_scale = frequency / WET_PUDDLE_FREQUENCY.x;
    let relief = WET_DETAIL_RELIEF * edge_scale;
    let resolved = 1.0 - smoothstep(0.4, 1.2, position.z * frequency);
    let mean = (1.0 - smoothstep(0.3, 0.75, basins.threshold)) * amount * (1.0 - smoothstep(0.0, 0.1, rise));
    let edge = WET_WATER_EDGE * edge_scale + position.z * frequency;

    // Through the lens the water's edge spreads as far as the ground's does. Each
    // point of the blur's footprint adds its own coverage, its ramp as wide as the
    // level changes across the footprint, so even a steep edge leaves no steps.
    let reach = pixel.blur * WET_EDGE_PER_DEVIATION;
    var ramp = edge;
    var around = vec4<f32>(level);
    if reach > scene_pixel_extent() {
        let along = pixel.up * reach;
        let across = vec2<f32>(-along.y, along.x);
        around = vec4<f32>(
            wet_level_near(pixel.point + along, basins),
            wet_level_near(pixel.point - along, basins),
            wet_level_near(pixel.point + across, basins),
            wet_level_near(pixel.point - across, basins),
        );
        ramp = max(edge, 0.5 * length(around.xz - around.yw));
    }
    // Far from the water's edge the grain changes nothing, and costs reads.
    var grain = 0.0;
    let highest = max(level, max(max(around.x, around.y), max(around.z, around.w)));
    let lowest = min(level, min(min(around.x, around.y), min(around.z, around.w)));
    if lowest < WET_DETAIL_LIMIT * relief + ramp && highest > -WET_DETAIL_LIMIT * relief - ramp {
        grain = wet_detail(pos, pixel) * relief;
    }
    let standing = (smoothstep(-ramp, ramp, level - grain)
        + smoothstep(-ramp, ramp, around.x - grain)
        + smoothstep(-ramp, ramp, around.y - grain)
        + smoothstep(-ramp, ramp, around.z - grain)
        + smoothstep(-ramp, ramp, around.w - grain)) / 5.0;
    let mean_level = (level + around.x + around.y + around.z + around.w) / 5.0;
    let soaked = smoothstep(-WET_SOAK_REACH * edge_scale - ramp, ramp, mean_level);
    return WetWater(mix(mean, standing, resolved), mix(mean, soaked, resolved));
}

// How hard any Rain effect in the edit falls.
fn wet_rain_strength() -> f32 {
    var strength = 0.0;
    for (var index = 0u; index < Common::mask_slot_count(); index = index + 1u) {
        if !Common::mask_slot_applies(Common::mask_data[index].metadata, MASK_EFFECT_RAIN_ID) {
            continue;
        }
        let params = mask_effect_params(index);
        strength = max(strength, mask_effect_lane(params, RAIN_AMOUNT_LANE) / 100.0);
    }
    return clamp(strength, 0.0, 1.0);
}

// The tilt of the water surface from rain's ripples, in ground units: expanding
// rings around random points, fading as they spread. It fades where a ring would
// be smaller than a pixel or the lens blur.
fn wet_ripples(position: vec3<f32>, blur: f32, seed: u32) -> vec2<f32> {
    let frequency = WET_RIPPLE_FREQUENCY;
    let footprint = position.z * (1.0 + WET_RIPPLE_FOCUS * blur / scene_pixel_extent());
    let resolved = 1.0 - smoothstep(0.08, 0.3, footprint * frequency);
    if resolved <= 1e-4 { return vec2<f32>(0.0); }
    let point = position.xy * frequency;
    let cell = vec2<i32>(floor(point));
    var tilt = vec2<f32>(0.0);
    for (var dy = -1; dy <= 1; dy = dy + 1) {
        for (var dx = -1; dx <= 1; dx = dx + 1) {
            let candidate = cell + vec2<i32>(dx, dy);
            let random = effect_cell_random(candidate, seed * 2097593u + 31u);
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

// What the ground reflects at an image point that lies `contact` below what stands
// on it, spread along the view by `roughness`.
fn wet_reflected(point: vec2<f32>, contact: f32, up: vec2<f32>, roughness: f32, distortion: vec2<f32>) -> vec3<f32> {
    let image_size = Common::scene_image_size();
    let mirrored = point + up * (2.0 * contact) + distortion;
    let spread = roughness * WET_REFLECTION_SPREAD * (contact + 0.02);
    var sum = vec3<f32>(0.0);
    var weights = 0.0;
    for (var tap = -WET_REFLECTION_TAPS; tap <= WET_REFLECTION_TAPS; tap = tap + 1) {
        let offset = f32(tap) / f32(WET_REFLECTION_TAPS);
        let weight = exp(-2.0 * offset * offset);
        let at = mirrored + up * (offset * spread);
        sum += scene_environment_at(at / image_size) * weight;
        weights += weight;
    }
    // A rough surface also scatters sideways, by less.
    return mix(
        sum / weights,
        scene_environment_blurred_at(mirrored / image_size),
        smoothstep(0.35, 0.9, roughness) * 0.5,
    );
}

// The same seen through the lens: over the footprint of its blur. Where the contact
// changes quickly the mirror squeezes a light into a thin line, sharper than anything
// around it unless it blurs with the rest.
fn wet_reflected_through_lens(pixel: WetPixel, roughness: f32, distortion: vec2<f32>) -> vec3<f32> {
    var sum = wet_reflected(pixel.point, pixel.contact, pixel.up, roughness, distortion);
    let reach = pixel.blur * WET_EDGE_PER_DEVIATION;
    if reach <= scene_pixel_extent() { return sum; }
    let along = pixel.up * reach;
    let across = vec2<f32>(-along.y, along.x);
    let image_size = Common::scene_image_size();
    for (var index = 0; index < 4; index = index + 1) {
        let sign_of = f32(1 - (index & 2));
        let point = pixel.point + select(across, along, index < 2) * sign_of;
        let contact = scene_ground_near(point / image_size).y;
        sum += wet_reflected(point, contact, pixel.up, roughness, distortion);
    }
    return sum / 5.0;
}

fn wet_surface(pos: vec2<i32>, input_rgb: vec3<f32>, params: MaskEffectParams) -> vec3<f32> {
    let wetness = mask_effect_percent(params, WET_SURFACES_WETNESS_LANE);
    let pixel = wet_pixel(pos);
    let reachable = 1.0 - smoothstep(WET_FAR_START, WET_FAR_END, pixel.depth);

    // Darkening, by each channel's albedo estimated against the ambient level,
    // where middle grey is 18% reflectance. Lamps and sky are past albedo 1 and
    // stay as they are.
    let ambient = scene_ambient_level();
    let positive = max(input_rgb, vec3<f32>(0.0));
    let albedo = clamp(0.18 * positive / ambient, vec3<f32>(1e-4), vec3<f32>(1.0));
    let p = WET_INTERNAL_REFLECTION;
    let darkened = input_rgb * ((1.0 - p) / (vec3<f32>(1.0) - p * albedo));

    var water = WetWater(0.0, 0.0);
    var position = vec3<f32>(0.0);
    if pixel.on_ground > 1e-4 {
        position = wet_ground_position(pixel.point);
        water = wet_water(pos, pixel, position, params);
    }
    let puddle = water.standing;
    let soaked = max(wetness * mix(WET_OTHER_SURFACES, 1.0, pixel.on_ground), max(water.soaked, puddle));
    let rgb = mix(input_rgb, darkened, soaked * reachable);
    if pixel.on_ground <= 1e-4 { return rgb; }

    // What the ground reflects: stronger toward the horizon, as for all water.
    let reflections = mask_effect_percent(params, WET_SURFACES_REFLECTIONS_LANE);
    let gloss = mask_effect_percent(params, WET_SURFACES_GLOSS_LANE);
    let view_cosine = sin(atan(pixel.below * 2.0 * SCENE_FOCAL_TAN));
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
    let ripples = mask_effect_percent(params, WET_SURFACES_RIPPLES_LANE)
        * wet_rain_strength() * puddle;
    if ripples > 1e-4 {
        let seed = u32(clamp(mask_effect_lane(params, WET_SURFACES_SEED_LANE), 0.0, 1000.0));
        let tilt = wet_ripples(position, pixel.blur, seed) * ripples;
        let right = vec2<f32>(-pixel.up.y, pixel.up.x);
        distortion = (right * tilt.x + pixel.up * tilt.y) * WET_RIPPLE_DISTORTION * (pixel.contact + 0.05);
        ripple_light = length(tilt);
    }
    let shimmer = min(reflectance * (1.0 + ripple_light), 1.0);
    return rgb * (1.0 - shimmer) + wet_reflected_through_lens(pixel, roughness, distortion) * shimmer;
}

// Highlights of scene lights on the wet ground, added after the creative
// effects, so Relight's Ambient does not dim them. A GGX lobe stretches into a
// long streak toward the viewer at grazing angles by itself. Surfaces between
// the ground and a light shadow it (scene_shadows.wgsl).
fn wet_glints(pos: vec2<i32>, params: MaskEffectParams) -> vec3<f32> {
    let reflections = mask_effect_percent(params, WET_SURFACES_REFLECTIONS_LANE);
    let wetness = mask_effect_percent(params, WET_SURFACES_WETNESS_LANE);
    if reflections <= 1e-4 || !scene_lights_present() {
        return vec3<f32>(0.0);
    }
    let pixel = wet_pixel(pos);
    if pixel.on_ground <= 1e-4 { return vec3<f32>(0.0); }
    let position = wet_ground_position(pixel.point);
    let puddle = wet_water(pos, pixel, position, params).standing;
    let gloss = mask_effect_percent(params, WET_SURFACES_GLOSS_LANE);
    let roughness = mix(mix(0.55, 0.16, gloss), 0.05, puddle);
    let alpha = roughness * roughness;
    let strength = max(wetness * mix(0.3, 1.0, gloss) * mix(0.25, 1.0, pixel.smooth_surface), puddle)
        * pixel.on_ground;

    // The ground's normal in the camera frame: perpendicular to every view ray
    // through the horizon.
    let image_center = 0.5 * Common::scene_image_size();
    let horizon_offset = Common::effects_uniforms.scene_ground.z - dot(image_center, pixel.up);
    let normal = normalize(vec3<f32>(
        pixel.up,
        -horizon_offset * 2.0 * SCENE_FOCAL_TAN,
    ));

    let uv = full_image_uv(pos);
    let shadows = scene_shadow_surface_at(pos);
    var glint = vec3<f32>(0.0);
    for (var index = 0u; index < Common::mask_slot_count(); index = index + 1u) {
        let light = scene_light_at(index);
        if !light.emits { continue; }
        let coverage = scene_light_coverage(light, pos);
        if coverage <= 1e-6 { continue; }
        let surface = scene_camera_point(
            light.camera,
            uv * light.camera.image_size,
            scene_depth_z(light.camera, pixel.depth),
        );
        let specular = wet_glint_specular(
            normal, normalize(light.position - surface), normalize(-surface), alpha,
        );
        if specular <= 0.0 { continue; }
        glint += light.intensity
            * (specular * scene_light_falloff(light, surface) * coverage * scene_shadow_of(shadows, light));
    }
    if scene_emission_present() {
        let arrival = scene_emission_surface_at(uv, pixel.depth);
        let spread = length(arrival.direction);
        if spread > 1e-4 {
            let camera = scene_camera(SCENE_DEFAULT_RELIEF_CONTROL);
            let surface = scene_camera_point(
                camera, uv * camera.image_size, scene_depth_z(camera, pixel.depth),
            );
            // Light from many directions spreads the highlight as a rougher
            // surface would.
            let broadened = mix(WET_EMISSION_SPREAD_ALPHA, alpha, spread);
            glint += arrival.light * wet_glint_specular(
                normal, arrival.direction / spread, normalize(-surface), broadened,
            );
        }
    }
    return glint * (WET_GLINT_GAIN * scene_ambient_level() * reflections * strength);
}

// The GGX highlight toward the camera of light arriving along `to_light` on
// ground with `normal` and GGX `alpha`; zero from behind or below.
fn wet_glint_specular(
    normal: vec3<f32>,
    to_light: vec3<f32>,
    to_camera: vec3<f32>,
    alpha: f32,
) -> f32 {
    let halfway = normalize(to_light + to_camera);
    let n_l = dot(normal, to_light);
    let n_v = dot(normal, to_camera);
    if n_l <= 0.0 || n_v <= 0.0 { return 0.0; }
    let n_h = max(dot(normal, halfway), 0.0);
    let denominator = n_h * n_h * (alpha * alpha - 1.0) + 1.0;
    let distribution = alpha * alpha / (3.14159265 * denominator * denominator);
    let v_h = max(dot(to_camera, halfway), 0.0);
    let fresnel = WET_F0 + (1.0 - WET_F0) * pow(1.0 - v_h, 5.0);
    return min(distribution * fresnel * n_l / max(4.0 * v_h * v_h, 0.1), WET_GLINT_LIMIT);
}
