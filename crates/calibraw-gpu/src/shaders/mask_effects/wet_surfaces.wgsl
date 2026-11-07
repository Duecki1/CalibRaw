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
// The scene bake (scene/geometry.wgsl) gives every pixel up to two surfaces,
// the ground plane fitted to the photo (B8) and the working image. A surface is
// ground where its normal lies within about 20 degrees of the plane's and it lies
// on the plane: each surface of an edge pixel is judged and shaded on its own, so
// only what is ground gets wet. A reflection is traced (`scene_trace`): the view
// ray reflected about the plane's normal, tilted by ripples, is followed through
// the visible surfaces of the working image, and what it meets is read from the
// photo with a footprint that grows with the surface's roughness and the
// distance travelled. A ray that leaves the image reads the wide-footprint
// surroundings where it left.
//
// Standing water has a level surface, so its edge is where that level meets the
// ground: a contour line. Everything that limits a puddle therefore raises the
// ground rather than fading the water: rough surfaces and what is not ground. The
// photo's own detail is the last height: its lighter grains stand clear of shallow
// water and its dark joints and pits fill first, so an edge breaks up on the texture
// that is really there. Ground beside the water has drawn it in, and stays dark past
// the edge. The puddle field lives on the plane, in camera heights, so puddles
// keep their ground size with distance and dissolve into the far sheen when a pixel
// covers more of them than they hold.
//
// Water is photographed through the same lens as the ground it lies on. Out of focus
// its edge and ripples blur as much as the ground does (the bake's lens blur), and
// the photo there has no grain left for the edge to break up on.

// Packed parameter lanes: `WET_SURFACES_<NAME>_LANE` (generated from the effect
// registry, effect_wgsl.rs).

// Share of light the water film reflects back to the surface (p above, for a
// refractive index of 4/3: Lekner and Dorf's equation 9).
const WET_INTERNAL_REFLECTION: f32 = 0.475;
// Share of the ground's darkening that walls and other surfaces take: rain runs
// off them.
const WET_OTHER_SURFACES: f32 = 0.72;
// Cosines of the angle between a surface's normal and the plane's over which a
// surface starts and ends to count as ground, and its distance from the plane
// relative to its depth over which it stops: B8's inliers (scene/ground.rs) lie
// within 0.94 and 0.05.
const WET_GROUND_COSINE: vec2<f32> = vec2<f32>(0.90, 0.97);
const WET_GROUND_DISTANCE: vec2<f32> = vec2<f32>(0.04, 0.10);
// A surface's roughness (`wet_roughness`) from which it starts to lose its sheen,
// and at which it holds no standing water: gravel and grass drink it.
const WET_ROUGHNESS_RANGE: vec2<f32> = vec2<f32>(0.018, 0.04);
// The guide's samples spacing in pixels and how far the windows of the smoothest
// neighbourhood lie from the pixel, in pixels.
const WET_ROUGHNESS_SPACING: i32 = 2;
const WET_ROUGHNESS_REACH: i32 = 4;
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
// The deviation of the footprint a rough surface reads its reflection with,
// relative to the image distance the ray travelled, at roughness 1.
const WET_REFLECTION_SPREAD: f32 = 0.45;
// The deviation of what a ray that leaves the image reads, in shorter edges.
const WET_REFLECTION_EXIT: f32 = 0.12;
// Lift of a reflection ray's origin off the surface, relative to its depth.
const WET_REFLECTION_LIFT: f32 = 0.004;
// Reflectance of water at normal incidence.
const WET_F0: f32 = 0.02;
// Puddles per camera height at Puddle size 0 and 100.
const WET_PUDDLE_FREQUENCY: vec2<f32> = vec2<f32>(2.4, 0.5);
// Ripple cells per camera height.
const WET_RIPPLE_FREQUENCY: f32 = 7.0;
// How far a ripple at full strength tilts the water's normal.
const WET_RIPPLE_TILT: f32 = 0.12;
// Scene-light highlight strength relative to the ambient level.
const WET_GLINT_GAIN: f32 = 2.5;
const WET_GLINT_LIMIT: f32 = 24.0;
// GGX alpha of the highlight of emitted light that arrives evenly from all
// around (scene_emission.wgsl); light from one direction keeps the ground's.
const WET_EMISSION_SPREAD_ALPHA: f32 = 0.6;

// Everything about the ground at one surface of a pixel.
struct WetPixel {
    // The surface, and how much of it is ground.
    surface: Geometry::SceneSurface,
    on_ground: f32,
    // Image point of the pixel (shorter edges from the image centre) and the unit
    // vector in image points toward the horizon.
    point: vec2<f32>,
    up: vec2<f32>,
    // 1 on smooth surfaces, 0 on rough ones.
    smooth_surface: f32,
    // The guide's mean around the pixel.
    guide_mean: f32,
    // The lens blur at the surface in shorter edges.
    blur: f32,
}

// How much a surface at `position` with `normal`, sky share `sky`, is ground:
// its normal lies within about 20 degrees of the plane's and it lies on the plane
// (both in the frame of relief scale 1). Without a fitted ground, none.
fn wet_ground_of(normal: vec3<f32>, position: vec3<f32>, sky: f32) -> f32 {
    if !Geometry::scene_has_ground() { return 0.0; }
    let plane = Geometry::scene_ground_plane();
    let cosine = dot(normal, plane.xyz);
    let offset = abs(dot(plane.xyz, position) - plane.w) / max(position.z, 1e-3);
    return (1.0 - sky) * smoothstep(WET_GROUND_COSINE.x, WET_GROUND_COSINE.y, cosine)
        * (1.0 - smoothstep(WET_GROUND_DISTANCE.x, WET_GROUND_DISTANCE.y, offset));
}

// How much of the bake's surface near full-image `uv` is ground, from the one
// working texel there and the surface of it whose depth lies nearer the
// texel's own: without the pixel's coverage, for the soft estimates around a
// pixel (`wet_level_near`).
fn wet_ground_near(uv: vec2<f32>) -> f32 {
    if !Geometry::scene_has_ground() { return 0.0; }
    let camera = Bake::scene_bake.camera;
    let size = vec2<f32>(Bake::scene_bake_size());
    let texel = vec2<i32>(floor(clamp(uv, vec2<f32>(0.0), vec2<f32>(0.9999)) * size));
    let geometry = Geometry::scene_layer(texel, Bake::SCENE_LAYER_GEOMETRY);
    let normals = Geometry::scene_layer(texel, Bake::SCENE_LAYER_NORMALS);
    let far = abs(geometry.x - geometry.w) < abs(geometry.x - geometry.z);
    let d = select(geometry.z, geometry.w, far);
    let encoded = select(normals.xy, normals.zw, far);
    let point = Camera::scene_image_point(uv, Geometry::scene_aspect());
    let position = Camera::scene_frame_position(camera, point, Camera::scene_frame_depth(camera, d, 1.0));
    return wet_ground_of(
        Bake::scene_from_octahedral(encoded),
        position,
        select(0.0, geometry.y, far),
    );
}

// The mean and relative variance of the guide over three samples either way
// around `pos`, WET_ROUGHNESS_SPACING pixels apart.
fn wet_guide_window(pos: vec2<i32>) -> vec2<f32> {
    var sum = 0.0;
    var square = 0.0;
    for (var dy = -1; dy <= 1; dy = dy + 1) {
        for (var dx = -1; dx <= 1; dx = dx + 1) {
            let guide = SceneSource::scene_guide_at(pos + vec2<i32>(dx, dy) * WET_ROUGHNESS_SPACING);
            sum += guide;
            square += guide * guide;
        }
    }
    let mean = sum / 9.0;
    return vec2<f32>(mean, max(square / 9.0 - mean * mean, 0.0) / max(mean * mean, 1e-6));
}

// How rough the photo's surface is at `pos`: the guide's variance relative to its
// squared mean, high on gravel and grass and low on asphalt or paving, in the
// smoothest of three windows so that an object standing on smooth ground leaves no
// rough ring around it; and the guide's mean there.
fn wet_roughness(pos: vec2<i32>) -> vec2<f32> {
    let centre = wet_guide_window(pos);
    let reach = vec2<i32>(WET_ROUGHNESS_REACH, 0);
    let roughness = min(
        centre.y,
        min(wet_guide_window(pos + reach).y, wet_guide_window(pos - reach).y),
    );
    return vec2<f32>(roughness, centre.x);
}

// The lens blur at a surface, a Gaussian's deviation in shorter edges.
fn wet_blur(surface: Geometry::SceneSurface) -> f32 {
    let size = Bake::scene_bake_size();
    return Bake::scene_lens_blur(surface.position.z) / f32(min(size.x, size.y));
}

// The ground at `surface`, one of the pixel at tile position `pos` and
// full-image `uv`.
fn wet_pixel(pos: vec2<i32>, uv: vec2<f32>, surface: Geometry::SceneSurface) -> WetPixel {
    let on_ground = wet_ground_of(surface.normal, surface.position, surface.sky);
    var pixel = WetPixel(
        surface,
        on_ground,
        Camera::scene_image_point(uv, Geometry::scene_aspect()),
        Geometry::scene_horizon_up(),
        0.0,
        0.0,
        wet_blur(surface),
    );
    if on_ground <= 1e-4 { return pixel; }
    let roughness = wet_roughness(pos);
    pixel.smooth_surface = 1.0 - smoothstep(WET_ROUGHNESS_RANGE.x, WET_ROUGHNESS_RANGE.y, roughness.x);
    pixel.guide_mean = roughness.y;
    return pixel;
}

// Position on the ground of an image point in camera heights (x across the view, y
// along it), and in its z the ground distance one pixel covers there.
fn wet_ground_position(point: vec2<f32>) -> vec3<f32> {
    return Geometry::scene_ground_position(point, scene_pixel_extent());
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

// How far the ground rises above the water's level where a surface is ground
// by `on_ground` and smooth by `smooth_surface`: rough surfaces and what is not
// ground lift it past the puddle field's range.
fn wet_rise(on_ground: f32, smooth_surface: f32) -> f32 {
    return (1.0 - on_ground * smooth_surface) * WET_NOT_GROUND_RISE;
}

fn wet_field(point: vec2<f32>, basins: WetBasins) -> f32 {
    return wet_undulation(wet_ground_position(point).xy * basins.frequency + basins.offset);
}

// How far the water's level lies above the ground at an image point near a
// pixel: from the bake's surface beside it, without the pixel's coverage, and
// the pixel's own roughness.
fn wet_level_near(point: vec2<f32>, smooth_surface: f32, basins: WetBasins) -> f32 {
    let uv = point / Geometry::scene_aspect() + vec2<f32>(0.5);
    return wet_field(point, basins) - basins.threshold - wet_rise(wet_ground_near(uv), smooth_surface);
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
    let rise = wet_rise(pixel.on_ground, pixel.smooth_surface);
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
            wet_level_near(pixel.point + along, pixel.smooth_surface, basins),
            wet_level_near(pixel.point - along, pixel.smooth_surface, basins),
            wet_level_near(pixel.point + across, pixel.smooth_surface, basins),
            wet_level_near(pixel.point - across, pixel.smooth_surface, basins),
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

// What the ground reflects at a surface: the view ray reflected about `normal`
// (the plane's, tilted by ripples) traced through the scene, and what it meets
// read from the photo with a footprint that widens with the roughness and the
// distance the ray travelled, and with the lens blur at the ground. A ray that
// leaves the image reads the wide surroundings where it left.
fn wet_reflected(pixel: WetPixel, normal: vec3<f32>, roughness: f32) -> vec3<f32> {
    let position = pixel.surface.position;
    let direction = reflect(normalize(position), normal);
    let origin = position + normal * (WET_REFLECTION_LIFT * position.z);
    let hit = Geometry::scene_trace(origin, direction);
    let spread = roughness * WET_REFLECTION_SPREAD * hit.travel;
    var footprint = sqrt(spread * spread + pixel.blur * pixel.blur);
    if !hit.found {
        footprint = max(footprint, WET_REFLECTION_EXIT);
    }
    return Geometry::scene_image(hit.uv, footprint) * exp2(Common::scene_tone_uniforms.exposure);
}

// A surface of the pixel wet: darkened as water does, and where it is ground,
// with its puddles and the reflection it shows.
fn wet_shade(
    pos: vec2<i32>,
    uv: vec2<f32>,
    surface: Geometry::SceneSurface,
    input_rgb: vec3<f32>,
    params: MaskEffectParams,
) -> vec3<f32> {
    let wetness = mask_effect_percent(params, WET_SURFACES_WETNESS_LANE);
    let pixel = wet_pixel(pos, uv, surface);
    // The sky is not wet.
    let reachable = 1.0 - surface.sky;

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
    let plane = Geometry::scene_ground_plane();
    let view_cosine = clamp(-dot(plane.xyz, normalize(surface.position)), 0.0, 1.0);
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
    var normal = plane.xyz;
    var ripple_light = 0.0;
    let ripples = mask_effect_percent(params, WET_SURFACES_RIPPLES_LANE)
        * wet_rain_strength() * puddle;
    if ripples > 1e-4 {
        let seed = u32(clamp(mask_effect_lane(params, WET_SURFACES_SEED_LANE), 0.0, 1000.0));
        let tilt = wet_ripples(position, pixel.blur, seed) * ripples;
        let axes = Geometry::scene_ground_axes();
        normal = normalize(plane.xyz + (axes[0] * tilt.x + axes[1] * tilt.y) * WET_RIPPLE_TILT);
        ripple_light = length(tilt);
    }
    let shimmer = min(reflectance * (1.0 + ripple_light), 1.0);
    return rgb * (1.0 - shimmer) + wet_reflected(pixel, normal, roughness) * shimmer;
}

// Wet Surfaces on a pixel: each surface of the scene bake wet on its own, and the
// two blended by coverage. Without scene depth only the darkening applies, as
// on any surface that is not ground.
fn wet_surface(pos: vec2<i32>, input_rgb: vec3<f32>, params: MaskEffectParams) -> vec3<f32> {
    if !Geometry::scene_geometry_present() {
        let ambient = scene_ambient_level();
        let albedo = clamp(0.18 * max(input_rgb, vec3<f32>(0.0)) / ambient, vec3<f32>(1e-4), vec3<f32>(1.0));
        let darkened = input_rgb * ((1.0 - WET_INTERNAL_REFLECTION) / (vec3<f32>(1.0) - WET_INTERNAL_REFLECTION * albedo));
        return mix(input_rgb, darkened, mask_effect_percent(params, WET_SURFACES_WETNESS_LANE) * WET_OTHER_SURFACES);
    }
    let uv = full_image_uv(pos);
    let scene_rgb = Geometry::scene_unedited_at(pos);
    let pixel = Geometry::scene_pixel(uv, scene_rgb, 1.0);
    let colors = Geometry::scene_split(pixel, scene_rgb, input_rgb);
    let near = wet_shade(pos, uv, pixel.near, colors[0], params);
    if !pixel.in_band {
        return near;
    }
    return Geometry::scene_blend(pixel, near, wet_shade(pos, uv, pixel.far, colors[1], params));
}

// Highlights of scene lights on one surface of the wet ground: a GGX lobe, which
// stretches into a long streak toward the viewer at grazing angles by itself.
// Light the scene's surfaces shadow does not reach it (the light bake).
fn wet_glint_surface(
    pos: vec2<i32>,
    uv: vec2<f32>,
    pixel: Geometry::ScenePixel,
    surface: Geometry::SceneSurface,
    params: MaskEffectParams,
) -> vec3<f32> {
    let reflections = mask_effect_percent(params, WET_SURFACES_REFLECTIONS_LANE);
    let wetness = mask_effect_percent(params, WET_SURFACES_WETNESS_LANE);
    let ground = wet_pixel(pos, uv, surface);
    if ground.on_ground <= 1e-4 { return vec3<f32>(0.0); }
    let position = wet_ground_position(ground.point);
    let puddle = wet_water(pos, ground, position, params).standing;
    let gloss = mask_effect_percent(params, WET_SURFACES_GLOSS_LANE);
    let roughness = mix(mix(0.55, 0.16, gloss), 0.05, puddle);
    let alpha = roughness * roughness;
    let strength = max(wetness * mix(0.3, 1.0, gloss) * mix(0.25, 1.0, ground.smooth_surface), puddle)
        * ground.on_ground;

    let camera = Bake::scene_bake.camera;
    let plane = Geometry::scene_ground_plane();
    var glint = vec3<f32>(0.0);
    for (var index = 0u; index < Common::mask_slot_count(); index = index + 1u) {
        let light = scene_light_at(index);
        if !light.emits { continue; }
        let coverage = scene_light_coverage(light, pos);
        if coverage <= 1e-6 { continue; }
        // The surface and the ground's normal in the light's own frame.
        let z = Camera::scene_frame_depth(camera, surface.d, light.relief_scale);
        let at = Camera::scene_frame_position(camera, ground.point, z);
        let normal = Camera::scene_frame_normal(camera, plane.xyz, ground.point, z, light.relief_scale);
        let offset = light.position - at;
        let specular = wet_glint_specular(normal, normalize(offset), normalize(-at), alpha);
        if specular <= 0.0 { continue; }
        var visibility = 1.0;
        if light.channel != 0u {
            visibility = Geometry::scene_visibility(pixel, surface, light.channel - 1u);
        }
        let falloff = 1.0 / (1.0 + dot(offset, offset) / (light.reach * light.reach));
        glint += light.intensity * (specular * falloff * coverage * visibility);
    }
    if scene_emission_present() {
        let arrival = scene_emission_surface_at(uv, surface.d);
        let spread = length(arrival.direction);
        if spread > 1e-4 {
            // Light from many directions spreads the highlight as a rougher
            // surface would.
            let broadened = mix(WET_EMISSION_SPREAD_ALPHA, alpha, spread);
            glint += arrival.light * wet_glint_specular(
                plane.xyz, arrival.direction / spread, normalize(-surface.position), broadened,
            );
        }
    }
    return glint * (WET_GLINT_GAIN * scene_ambient_level() * reflections * strength);
}

// Highlights of scene lights on the wet ground, added after the creative
// effects, so Relight's Ambient does not dim them.
fn wet_glints(pos: vec2<i32>, params: MaskEffectParams) -> vec3<f32> {
    let reflections = mask_effect_percent(params, WET_SURFACES_REFLECTIONS_LANE);
    if reflections <= 1e-4 || !scene_lights_present() || !Geometry::scene_geometry_present() {
        return vec3<f32>(0.0);
    }
    let uv = full_image_uv(pos);
    let pixel = Geometry::scene_pixel(uv, Geometry::scene_unedited_at(pos), 1.0);
    let near = wet_glint_surface(pos, uv, pixel, pixel.near, params);
    if !pixel.in_band {
        return near;
    }
    return Geometry::scene_blend(pixel, near, wet_glint_surface(pos, uv, pixel, pixel.far, params));
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
