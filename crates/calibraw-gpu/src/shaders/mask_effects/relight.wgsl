// Relight: a virtual point light shading the scene that scene depth describes.
//
// Geometry. `scene_depth_tex` (scene_depth.wgsl) covers the full source image.
// Channel y is dequantized smooth depth and z/w its edge-aware gradient per
// shorter-edge length; mip levels average them (scene_surface.rs). Positions
// are in the scene frame (scene_lights.wgsl), where the light is also a scene
// light other effects receive. Everything depends only on full-image
// coordinates, the shared depth texture and global tone statistics, so export
// tiles match the preview.
//
// Shading is scene-linear Rec.2020 before tone mapping. The added light is
// proportional to an estimate of surface reflectance (the pixel with part of
// its existing shading divided out), so lit shadows recover their texture and
// colour instead of receiving a flat wash.

// Packed parameter lanes and options (effect_lanes.rs).
const RELIGHT_AMOUNT_LANE: u32 = 0u;
const RELIGHT_REACH_LANE: u32 = 1u;
const RELIGHT_SOURCE_X_LANE: u32 = 2u;
const RELIGHT_SOURCE_Y_LANE: u32 = 3u;
const RELIGHT_COLOR_LANE: u32 = 4u;
const RELIGHT_DEPTH_LANE: u32 = 7u;
const RELIGHT_SIZE_LANE: u32 = 8u;
const RELIGHT_SHADOWS_LANE: u32 = 9u;
const RELIGHT_RELIEF_LANE: u32 = 10u;
const RELIGHT_AMBIENT_LANE: u32 = 11u;
const RELIGHT_SHADOW_CHANNEL_OPTION: u32 = 2u;
// Lights the shadow map holds, one per channel (RELIGHT_SHADOW_MAP_CHANNELS).
const RELIGHT_SHADOW_MAP_CHANNELS: u32 = 4u;

const RELIGHT_NORMAL_TAPS: u32 = 12u;
// Shadow-march steps: one per few depth texels of the ray's image path, within
// these bounds. Too few steps skip over a silhouette between them and leave
// banded copies of its shadow.
const RELIGHT_MIN_SHADOW_STEPS: f32 = 24.0;
const RELIGHT_MAX_SHADOW_STEPS: f32 = 96.0;
const RELIGHT_TEXELS_PER_SHADOW_STEP: f32 = 4.0;
// Narrowest shadow-sample footprint in level-0 texels: about one pixel of the
// depth model (roughly 700 across the image), whose silhouettes are blocky.
const RELIGHT_MIN_SHADOW_FOOTPRINT: f32 = 4.0;
const RELIGHT_GOLDEN_ANGLE: f32 = 2.399963229728653;
// Normalized depths where the light fades out and beyond which nothing is lit
// (`apply_relight`).
const RELIGHT_REACH_FADE_START: f32 = 0.92;
const RELIGHT_REACH_LIMIT: f32 = 0.96;
// A shadow-map texel whose 3×3 neighbourhood lies at least this deep is read
// only by unlit pixels: its resolve taps are four of those texels, so their
// weighted mean is beyond the reach limit too. The margin absorbs rounding.
const RELIGHT_UNLIT_TEXEL_DEPTH: f32 = RELIGHT_REACH_LIMIT + 0.001;
fn relight_surface(uv: vec2<f32>, level: f32) -> vec4<f32> {
    return textureSampleLevel(scene_depth_tex, SceneAdjustments::local_mask_sampler, uv, level);
}

// Mean extent of one level-0 texel in shorter-edge units.
fn relight_texel_extent(camera: SceneCamera) -> f32 {
    let texels = vec2<f32>(textureDimensions(scene_depth_tex));
    let extent = camera.image_size / texels;
    return sqrt(extent.x * extent.y);
}

// Averages the depth gradient over a disc, as a light of that size integrates
// the surface. Taps that leave the pixel's own surface (their depth departs
// from the tangent plane) are rejected, so a background slope does not tilt
// a foreground edge and the light leaves no halo around silhouettes.
fn relight_smoothed_gradient(
    camera: SceneCamera,
    uv: vec2<f32>,
    depth: f32,
    gradient: vec2<f32>,
    radius: f32,
) -> vec2<f32> {
    let spacing = radius * 0.5 / relight_texel_extent(camera);
    let levels = f32(textureNumLevels(scene_depth_tex));
    let level = clamp(log2(max(spacing, 1.0)), 0.0, levels - 1.0);
    let tolerance = 0.01 + radius * (0.25 + 0.5 * length(gradient));
    var sum = gradient;
    var weights = 1.0;
    for (var tap = 0u; tap < RELIGHT_NORMAL_TAPS; tap = tap + 1u) {
        let distance = radius * sqrt((f32(tap) + 0.5) / f32(RELIGHT_NORMAL_TAPS));
        let angle = f32(tap) * RELIGHT_GOLDEN_ANGLE;
        let offset = distance * vec2<f32>(cos(angle), sin(angle));
        let tap_surface = relight_surface(uv + offset / camera.image_size, level);
        let residual = (tap_surface.y - depth - dot(gradient, offset)) / tolerance;
        let weight = exp(-residual * residual);
        sum += tap_surface.zw * weight;
        weights += weight;
    }
    return sum / weights;
}

// Surface normal from the depth gradient (depth per shorter-edge length),
// facing the camera.
fn relight_normal(camera: SceneCamera, point: vec2<f32>, z: f32, gradient: vec2<f32>) -> vec3<f32> {
    let focal = 2.0 * SCENE_FOCAL_TAN;
    let lateral = scene_lateral(camera, point);
    let slope = camera.relief * gradient;
    let along_x = vec3<f32>(focal * z + lateral.x * slope.x, lateral.y * slope.x, slope.x);
    let along_y = vec3<f32>(lateral.x * slope.y, focal * z + lateral.y * slope.y, slope.y);
    let normal = normalize(cross(along_x, along_y));
    return select(normal, -normal, dot(normal, vec3<f32>(lateral * z, z)) > 0.0);
}

// How much the depth-map texels around `uv` block the ray at `position`.
// Each of the four texels is tested on its own and the results are blended
// bilinearly (percentage-closer filtering): thresholding interpolated depth
// would trace the texel grid as a hard staircase, while blending the tests
// gives an edge that ramps smoothly across one texel of `level`.
fn relight_blocked(
    camera: SceneCamera,
    position: vec3<f32>,
    uv: vec2<f32>,
    level: i32,
    width: f32,
) -> f32 {
    let size = vec2<i32>(textureDimensions(scene_depth_tex, level));
    let p = uv * vec2<f32>(size) - vec2<f32>(0.5);
    let base = vec2<i32>(floor(p));
    let f = fract(p);
    var blocked = 0.0;
    for (var y = 0; y < 2; y = y + 1) {
        for (var x = 0; x < 2; x = x + 1) {
            let cell = clamp(base + vec2<i32>(x, y), vec2<i32>(0), size - vec2<i32>(1));
            let occluder_z = scene_depth_z(camera, textureLoad(scene_depth_tex, cell, level).y);
            // Bias and minimum penumbra keep noisy depth (foliage) from speckling.
            let penetration = position.z - occluder_z - 0.006 * occluder_z;
            let thickness = 0.15 * occluder_z;
            let test = smoothstep(-0.5 * width, width, penetration)
                * (1.0 - smoothstep(thickness, thickness * 1.5, penetration));
            let weight = select(1.0 - f.x, f.x, x == 1) * select(1.0 - f.y, f.y, y == 1);
            blocked += test * weight;
        }
    }
    return blocked;
}

// Whether a shadow ray at depth `z` passes in front of every surface that
// one march step's occlusion tests read (`relight_blocked` around `uv` and
// `uv ± side` on `level`) by at least the test's ramp, so that every test
// reports exactly zero and the step can be skipped. Above level 0, channel x
// of the surface holds the nearest depth of each texel's footprint
// (scene_surface.rs), never farther than the depth any test reads there; the
// check reads 2×2 texels of the finest such level covering the tested cells.
fn relight_step_unoccluded(
    camera: SceneCamera,
    z: f32,
    width: f32,
    uv: vec2<f32>,
    side: vec2<f32>,
    level: i32,
) -> bool {
    let size = vec2<i32>(textureDimensions(scene_depth_tex, level));
    let last = size - vec2<i32>(1);
    let extent = vec2<f32>(size);
    // The cells `relight_blocked` reads for all three taps.
    let lo = clamp(
        vec2<i32>(floor(min(uv - side, uv + side) * extent - vec2<f32>(0.5))),
        vec2<i32>(0),
        last,
    );
    let hi = clamp(
        vec2<i32>(floor(max(uv - side, uv + side) * extent - vec2<f32>(0.5))) + vec2<i32>(1),
        vec2<i32>(0),
        last,
    );
    let top = i32(textureNumLevels(scene_depth_tex)) - 1;
    // Level 0 holds stored depth in channel x, not the nearest depth.
    var shift = select(0u, 1u, level == 0);
    while level + i32(shift) < top
        && any((hi >> vec2<u32>(shift)) - (lo >> vec2<u32>(shift)) > vec2<i32>(1)) {
        shift = shift + 1u;
    }
    let a = lo >> vec2<u32>(shift);
    let b = hi >> vec2<u32>(shift);
    if any(b - a > vec2<i32>(1)) { return false; }
    let coarse = level + i32(shift);
    let nearest = min(
        min(
            textureLoad(scene_depth_tex, a, coarse).x,
            textureLoad(scene_depth_tex, vec2<i32>(b.x, a.y), coarse).x,
        ),
        min(
            textureLoad(scene_depth_tex, vec2<i32>(a.x, b.y), coarse).x,
            textureLoad(scene_depth_tex, b, coarse).x,
        ),
    );
    let occluder_z = scene_depth_z(camera, nearest);
    // The largest penetration any test can see; at or below the ramp's start
    // its smoothstep is zero.
    return z - occluder_z - 0.006 * occluder_z <= -0.5 * width;
}

// Screen-space shadow: marches from the surface toward the light through the
// depth map. Depth maps hold only front surfaces, so an occluder is assumed to
// be solid for a thickness proportional to its distance; a ray passing farther
// behind it is lit.
//
// Each step is a cone section toward the light: its footprint widens with the
// distance from the receiver and with light size, as a real penumbra does, and
// is never narrower than twice the step spacing or a depth-model pixel, so
// consecutive steps cover the path without gaps. The step reads the mip level
// whose texels are half that footprint, filters the occlusion test over it and
// averages three taps across the ray, so silhouettes in the low-resolution
// depth map give soft shadow edges instead of texel staircases, and thin
// structures fade instead of striping. `jitter` in [0, 1) offsets the steps
// per pixel, so what aliasing remains becomes fine dither rather than
// contour-like copies of a silhouette's shadow. Steps where the ray provably
// passes in front of everything nearby are skipped
// (`relight_step_unoccluded`); they would leave the visibility unchanged.
fn relight_visibility(
    camera: SceneCamera,
    surface: vec3<f32>,
    light: vec3<f32>,
    size: f32,
    jitter: f32,
) -> f32 {
    let path = light - surface;
    let path_length = length(path);
    let start = scene_image_point(camera, surface);
    let end = scene_image_point(camera, light);
    let texel = relight_texel_extent(camera);
    let path_texels = length(end - start) / texel;
    if path_texels < 1.0 { return 1.0; }
    let direction = (end - start) / (path_texels * texel);
    let across = vec2<f32>(-direction.y, direction.x);
    // Leave the receiver's own texels; the tangent plane already shades them.
    let first = min(1.5 / path_texels, 0.5);
    let penumbra_rate = mix(0.01, 0.25, size);
    let top_level = f32(textureNumLevels(scene_depth_tex)) - 1.0;
    let steps = clamp(
        ceil(path_texels / RELIGHT_TEXELS_PER_SHADOW_STEP),
        RELIGHT_MIN_SHADOW_STEPS,
        RELIGHT_MAX_SHADOW_STEPS,
    );
    var visibility = 1.0;
    for (var index = 0.0; index < steps; index = index + 1.0) {
        let fraction = (index + jitter) / steps;
        // Denser near the receiver, where contact shadows are narrow.
        let t = mix(first, 1.0, fraction * fraction);
        let position = surface + path * t;
        if position.z <= 0.02 { break; }
        let point = scene_image_point(camera, position);
        let uv = point / camera.image_size;
        // Nothing is known beyond the frame: it casts no shadow.
        if any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) { break; }
        let travel = path_length * t;
        // Penumbra radius in the image at this step's distance.
        let cone = travel * penumbra_rate / (2.0 * SCENE_FOCAL_TAN * position.z);
        let spacing = path_texels * (1.0 - first) * 2.0 * fraction / steps;
        let footprint = max(max(cone / texel, 2.0 * spacing), RELIGHT_MIN_SHADOW_FOOTPRINT);
        // The filtered test ramps over one texel of the level, half the footprint.
        let level = i32(clamp(round(log2(footprint) - 1.0), 0.0, top_level));
        let width = 0.01 * position.z + travel * penumbra_rate;
        let side = across * (0.5 * footprint * texel) / camera.image_size;
        if relight_step_unoccluded(camera, position.z, width, uv, side, level) { continue; }
        let blocked = 0.5 * relight_blocked(camera, position, uv, level, width)
            + 0.25 * relight_blocked(camera, position, uv - side, level, width)
            + 0.25 * relight_blocked(camera, position, uv + side, level, width);
        visibility = min(visibility, 1.0 - blocked);
        if visibility <= 0.0 { break; }
    }
    return visibility;
}

// Interleaved gradient noise in [0, 1) per full-image pixel at full-image
// `uv`, so export tiles and the preview dither alike.
fn relight_jitter(uv: vec2<f32>) -> f32 {
    let full_size = vec2<f32>(
        f32(Common::camera_uniforms.full_width),
        f32(Common::camera_uniforms.full_height),
    );
    let p = floor(uv * full_size);
    return fract(52.9829189 * fract(dot(p, vec2<f32>(0.06711056, 0.00583715))));
}

// Shadow map. Shadows depend only on scene depth and the light, so they are
// traced once per level-0 scene-depth texel, for up to four lights (one per
// channel), instead of per image pixel. Pixels read the map with the joint
// upsampling taps of the depth itself (scene_depth.wgsl), so shadows
// keep following image edges. The map covers the full image, is shared by
// export tiles and is rebuilt only when depth or a shadowed light's position,
// size or relief changes (encode_relight_shadow_map in gpu.rs). Each texel
// traces from its own smooth depth and dithers with the jitter of the
// full-image pixel at its centre.
@group(0) @binding(46) var relight_shadow_map: texture_2d<f32>;
@group(0) @binding(47) var relight_shadow_map_out: texture_storage_2d<rgba16float, write>;

// The shadow-map channel of a Relight slot plus one, or zero when the slot
// traces its shadows per pixel instead (`assign_relight_shadow_channels`).
fn relight_shadow_channel(params: MaskEffectParams) -> u32 {
    let option = params.options[RELIGHT_SHADOW_CHANNEL_OPTION];
    return u32(clamp(option, 0.0, f32(RELIGHT_SHADOW_MAP_CHANNELS)) + 0.5);
}

fn relight_size(params: MaskEffectParams) -> f32 {
    return clamp(mask_effect_lane(params, RELIGHT_SIZE_LANE) / 100.0, 0.0, 1.0);
}

// Whether no lit pixel reads the shadow-map texel `cell`: its whole 3×3
// neighbourhood, clamped to the grid as the resolve taps are, lies beyond the
// reach limit (RELIGHT_UNLIT_TEXEL_DEPTH). Sky is often a large share of a
// photo, and its shadows would otherwise be traced for nothing.
fn relight_texel_unlit(cell: vec2<i32>, size: vec2<i32>) -> bool {
    for (var y = -1; y <= 1; y = y + 1) {
        for (var x = -1; x <= 1; x = x + 1) {
            let neighbour = clamp(cell + vec2<i32>(x, y), vec2<i32>(0), size - vec2<i32>(1));
            if textureLoad(scene_depth_tex, neighbour, 0).y < RELIGHT_UNLIT_TEXEL_DEPTH {
                return false;
            }
        }
    }
    return true;
}

@compute @workgroup_size(8, 8, 1)
fn build_relight_shadow_map(@builtin(global_invocation_id) gid: vec3<u32>) {
    let size = textureDimensions(scene_depth_tex);
    if gid.x >= size.x || gid.y >= size.y { return; }
    let cell = vec2<i32>(gid.xy);
    if relight_texel_unlit(cell, vec2<i32>(size)) {
        textureStore(relight_shadow_map_out, cell, vec4<f32>(1.0));
        return;
    }
    let uv = (vec2<f32>(gid.xy) + vec2<f32>(0.5)) / vec2<f32>(size);
    let depth = textureLoad(scene_depth_tex, cell, 0).y;
    let jitter = relight_jitter(uv);
    var visibility = vec4<f32>(1.0);
    for (var index = 0u; index < scene_light_slots(); index = index + 1u) {
        let params = mask_effect_params(index);
        let channel = relight_shadow_channel(params);
        if channel == 0u { continue; }
        let light = relight_scene_light(params);
        let camera = light.camera;
        let surface = scene_camera_point(camera, uv * camera.image_size, scene_depth_z(camera, depth));
        visibility[channel - 1u] = relight_visibility(
            camera, surface, light.position, relight_size(params), jitter,
        );
    }
    textureStore(relight_shadow_map_out, cell, visibility);
}

fn apply_relight(pos: vec2<i32>, input_rgb: vec3<f32>, params: MaskEffectParams) -> vec3<f32> {
    let amount = clamp(mask_effect_lane(params, RELIGHT_AMOUNT_LANE) / 100.0, 0.0, 1.0);
    let ambient = clamp(mask_effect_lane(params, RELIGHT_AMBIENT_LANE) / 100.0, 0.0, 1.0);
    let ambient_rgb = input_rgb * ambient;
    if amount <= 1e-6 { return ambient_rgb; }

    let light = relight_scene_light(params);
    let camera = light.camera;
    let size = relight_size(params);
    let uv = full_image_uv(pos);
    let point = uv * camera.image_size;

    // Without scene depth (still generating, or failed) the scene is a plane
    // facing the camera: the light keeps its falloff but casts no shadows.
    let has_depth = Common::scene_tone_uniforms.scene_depth_present != 0u;
    let shadows = clamp(mask_effect_lane(params, RELIGHT_SHADOWS_LANE) / 100.0, 0.0, 1.0);
    let shadow_channel = relight_shadow_channel(params);
    var depth = 0.0;
    var gradient = vec2<f32>(0.0);
    var mapped_visibility = 1.0;
    if has_depth {
        // The shadow map shares the depth's grid, so it takes the same taps.
        let taps = scene_depth_taps_at(pos);
        let texels = scene_depth_resolve(scene_depth_tex, taps);
        depth = texels.y;
        gradient = relight_smoothed_gradient(
            camera, uv, depth, texels.zw, mix(0.004, 0.05, size * size),
        );
        if shadow_channel != 0u && shadows > 1e-6 {
            mapped_visibility = scene_depth_resolve(relight_shadow_map, taps)[shadow_channel - 1u];
        }
    }
    let z = scene_depth_z(camera, depth);
    let surface = scene_camera_point(camera, point, z);
    let normal = relight_normal(camera, point, z, gradient);

    let to_light = light.position - surface;
    let distance = max(length(to_light), 1e-5);
    // Wrapped Lambert: a larger source reaches further around a form.
    let wrap = 0.5 * size;
    // The far end of normalized depth is where the depth model clips the sky
    // and distant scenery (typically 245–255 of 255, often with a gap below):
    // beyond any local light, it receives neither light nor shadows.
    let reachable = 1.0 - smoothstep(RELIGHT_REACH_FADE_START, RELIGHT_REACH_LIMIT, depth);
    let diffuse = clamp((dot(normal, to_light / distance) + wrap) / (1.0 + wrap), 0.0, 1.0)
        * reachable;
    if diffuse <= 1e-6 { return ambient_rgb; }
    // Inverse-square falloff with a finite core (scene_light_falloff).
    let falloff = 1.0 / (1.0 + distance * distance / (light.reach * light.reach));
    var visibility = 1.0;
    if has_depth && shadows > 1e-6 {
        var traced = mapped_visibility;
        if shadow_channel == 0u {
            // Beyond the shadow map's channels: trace this pixel.
            traced = relight_visibility(camera, surface, light.position, size, relight_jitter(uv));
        }
        visibility = mix(1.0, traced, shadows);
    }
    let irradiance = diffuse * falloff * visibility;

    // Divide out part of the existing shading relative to the scene's ambient
    // level (shared by export tiles), so the estimate approaches reflectance.
    let ambient_level = scene_ambient_level();
    let positive = Color::gamut_project_nonnegative_rec2020(input_rgb);
    let relative = clamp(Common::safe_luma(positive) / ambient_level, 0.02, 64.0);
    let reflectance = positive * pow(relative, -0.3);
    return ambient_rgb + reflectance * light.intensity * irradiance;
}
