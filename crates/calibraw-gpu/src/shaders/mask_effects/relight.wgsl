// Relight: a virtual point light shading the scene that scene depth describes.
//
// Geometry. `scene_depth_tex` (atmosphere.wgsl) covers the full source image.
// Channel y is dequantized smooth depth and z/w its edge-aware gradient per
// shorter-edge length; mip levels average them (scene_surface.rs). Positions
// use a pinhole camera in relative units: x right and y down as in the image,
// z forward, the nearest surface at z = 1 and the farthest at z = 1 + relief.
// Image points are in shorter-edge units with (0, 0) at the top-left corner.
// Everything depends only on full-image coordinates, the shared depth texture
// and global tone statistics, so export tiles match the preview.
//
// Shading is scene-linear Rec.2020 before tone mapping. The added light is
// proportional to an estimate of surface reflectance (the pixel with part of
// its existing shading divided out), so lit shadows recover their texture and
// colour instead of receiving a flat wash.

// tan(half the field of view) across the shorter edge, about a 30 mm lens on
// full frame. Monocular depth carries no focal length.
const RELIGHT_FOCAL_TAN: f32 = 0.4;
const RELIGHT_NORMAL_TAPS: u32 = 12u;
const RELIGHT_SHADOW_STEPS: u32 = 40u;
// Narrowest shadow-sample footprint in level-0 texels: about one pixel of the
// depth model (roughly 700 across the image), whose silhouettes are blocky.
const RELIGHT_MIN_SHADOW_FOOTPRINT: f32 = 4.0;
const RELIGHT_GOLDEN_ANGLE: f32 = 2.399963229728653;
// Scene-linear gain of Amount 100 on a surface facing a nearby light.
const RELIGHT_GAIN: f32 = 2.5;

struct RelightCamera {
    // Full image in shorter-edge units.
    image_size: vec2<f32>,
    // Depth range in units of the nearest surface's distance.
    relief: f32,
}

fn relight_lateral(camera: RelightCamera, point: vec2<f32>) -> vec2<f32> {
    return 2.0 * RELIGHT_FOCAL_TAN * (point - 0.5 * camera.image_size);
}

fn relight_camera_point(camera: RelightCamera, point: vec2<f32>, z: f32) -> vec3<f32> {
    return vec3<f32>(relight_lateral(camera, point) * z, z);
}

fn relight_image_point(camera: RelightCamera, position: vec3<f32>) -> vec2<f32> {
    return 0.5 * camera.image_size + position.xy / (2.0 * RELIGHT_FOCAL_TAN * position.z);
}

fn relight_depth_z(camera: RelightCamera, depth: f32) -> f32 {
    return 1.0 + camera.relief * depth;
}

fn relight_surface(uv: vec2<f32>, level: f32) -> vec4<f32> {
    return textureSampleLevel(scene_depth_tex, SceneAdjustments::local_mask_sampler, uv, level);
}

// Mean extent of one level-0 texel in shorter-edge units.
fn relight_texel_extent(camera: RelightCamera) -> f32 {
    let texels = vec2<f32>(textureDimensions(scene_depth_tex));
    let extent = camera.image_size / texels;
    return sqrt(extent.x * extent.y);
}

// Averages the depth gradient over a disc, as a light of that size integrates
// the surface. Taps that leave the pixel's own surface (their depth departs
// from the tangent plane) are rejected, so a background slope does not tilt
// a foreground edge and the light leaves no halo around silhouettes.
fn relight_smoothed_gradient(
    camera: RelightCamera,
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
fn relight_normal(camera: RelightCamera, point: vec2<f32>, z: f32, gradient: vec2<f32>) -> vec3<f32> {
    let focal = 2.0 * RELIGHT_FOCAL_TAN;
    let lateral = relight_lateral(camera, point);
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
    camera: RelightCamera,
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
            let occluder_z = relight_depth_z(camera, textureLoad(scene_depth_tex, cell, level).y);
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

// Screen-space shadow: marches from the surface toward the light through the
// depth map. Depth maps hold only front surfaces, so an occluder is assumed to
// be solid for a thickness proportional to its distance; a ray passing farther
// behind it is lit.
//
// Each step is a cone section toward the light: its footprint widens with the
// distance from the receiver and with light size, as a real penumbra does, and
// is never narrower than the step spacing or a depth-model pixel. The step reads
// the mip level whose texels are half that footprint, filters the occlusion
// test over it and averages three taps across the ray, so silhouettes in the
// low-resolution depth map give soft shadow edges instead of texel staircases,
// and thin structures fade instead of striping.
fn relight_visibility(
    camera: RelightCamera,
    surface: vec3<f32>,
    light: vec3<f32>,
    size: f32,
) -> f32 {
    let path = light - surface;
    let path_length = length(path);
    let start = relight_image_point(camera, surface);
    let end = relight_image_point(camera, light);
    let texel = relight_texel_extent(camera);
    let path_texels = length(end - start) / texel;
    if path_texels < 1.0 { return 1.0; }
    let direction = (end - start) / (path_texels * texel);
    let across = vec2<f32>(-direction.y, direction.x);
    // Leave the receiver's own texels; the tangent plane already shades them.
    let first = min(1.5 / path_texels, 0.5);
    let penumbra_rate = mix(0.01, 0.25, size);
    let top_level = f32(textureNumLevels(scene_depth_tex)) - 1.0;
    var visibility = 1.0;
    for (var index = 0u; index < RELIGHT_SHADOW_STEPS; index = index + 1u) {
        let fraction = (f32(index) + 1.0) / f32(RELIGHT_SHADOW_STEPS);
        // Denser near the receiver, where contact shadows are narrow.
        let t = mix(first, 1.0, fraction * fraction);
        let position = surface + path * t;
        if position.z <= 0.02 { break; }
        let point = relight_image_point(camera, position);
        let uv = point / camera.image_size;
        // Nothing is known beyond the frame: it casts no shadow.
        if any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) { break; }
        let travel = path_length * t;
        // Penumbra radius in the image at this step's distance.
        let cone = travel * penumbra_rate / (2.0 * RELIGHT_FOCAL_TAN * position.z);
        let spacing = path_texels * (1.0 - first) * 2.0 * fraction / f32(RELIGHT_SHADOW_STEPS);
        let footprint = max(max(cone / texel, spacing), RELIGHT_MIN_SHADOW_FOOTPRINT);
        // The filtered test ramps over one texel of the level, half the footprint.
        let level = i32(clamp(round(log2(footprint) - 1.0), 0.0, top_level));
        let width = 0.01 * position.z + travel * penumbra_rate;
        let side = across * (0.5 * footprint * texel) / camera.image_size;
        let blocked = 0.5 * relight_blocked(camera, position, uv, level, width)
            + 0.25 * relight_blocked(camera, position, uv - side, level, width)
            + 0.25 * relight_blocked(camera, position, uv + side, level, width);
        visibility = min(visibility, 1.0 - blocked);
        if visibility <= 0.0 { break; }
    }
    return visibility;
}

fn apply_relight(
    pos: vec2<i32>,
    input_rgb: vec3<f32>,
    primary: vec4<f32>,
    secondary: vec4<f32>,
    tertiary: vec4<f32>,
) -> vec3<f32> {
    let amount = clamp(primary.x / 100.0, 0.0, 1.0);
    let ambient = clamp(tertiary.w / 100.0, 0.0, 1.0);
    let ambient_rgb = input_rgb * ambient;
    if amount <= 1e-6 { return ambient_rgb; }

    let full_size = max(vec2<f32>(
        f32(Common::camera_uniforms.full_width),
        f32(Common::camera_uniforms.full_height),
    ), vec2<f32>(1.0));
    let relief_control = clamp(tertiary.z / 100.0, 0.0, 1.0);
    let camera = RelightCamera(
        full_size / min(full_size.x, full_size.y),
        mix(0.25, 6.0, pow(relief_control, 1.5)),
    );
    let size = clamp(tertiary.x / 100.0, 0.0, 1.0);
    let uv = full_image_uv(pos);
    let point = uv * camera.image_size;

    // Without scene depth (still generating, or failed) the scene is a plane
    // facing the camera: the light keeps its falloff but casts no shadows.
    let has_depth = Common::scene_tone_uniforms.scene_depth_present != 0u;
    var depth = 0.0;
    var gradient = vec2<f32>(0.0);
    if has_depth {
        let texels = scene_depth_texels_at(pos);
        depth = texels.y;
        gradient = relight_smoothed_gradient(
            camera, uv, depth, texels.zw, mix(0.004, 0.05, size * size),
        );
    }
    let z = relight_depth_z(camera, depth);
    let surface = relight_camera_point(camera, point, z);
    let normal = relight_normal(camera, point, z, gradient);

    // Light depth: -1 at the camera, 0 at the nearest surface, 1 at the farthest.
    let light_depth = clamp(secondary.w / 100.0, -1.0, 1.0);
    let light_z = select(
        relight_depth_z(camera, light_depth),
        1.0 + 0.9 * light_depth,
        light_depth < 0.0,
    );
    let light = relight_camera_point(camera, primary.zw / 100.0 * camera.image_size, light_z);
    let to_light = light - surface;
    let distance = max(length(to_light), 1e-5);
    // Wrapped Lambert: a larger source reaches further around a form.
    let wrap = 0.5 * size;
    // The far end of normalized depth is where the depth model clips the sky
    // and distant scenery (typically 245–255 of 255, often with a gap below):
    // beyond any local light, it receives neither light nor shadows.
    let reachable = 1.0 - smoothstep(0.92, 0.96, depth);
    let diffuse = clamp((dot(normal, to_light / distance) + wrap) / (1.0 + wrap), 0.0, 1.0)
        * reachable;
    if diffuse <= 1e-6 { return ambient_rgb; }
    // Inverse-square falloff with a finite core; Reach is the distance at
    // which the light has halved, in shorter-edge widths at the nearest surface.
    let reach = clamp(primary.y / 100.0, 0.1, 4.0) * 2.0 * RELIGHT_FOCAL_TAN;
    let falloff = 1.0 / (1.0 + distance * distance / (reach * reach));
    var visibility = 1.0;
    let shadows = clamp(tertiary.y / 100.0, 0.0, 1.0);
    if has_depth && shadows > 1e-6 {
        visibility = mix(1.0, relight_visibility(camera, surface, light, size), shadows);
    }
    let irradiance = diffuse * falloff * visibility;

    // Divide out part of the existing shading relative to the scene's ambient
    // level (shared by export tiles), so the estimate approaches reflectance.
    let ambient_ev = Tonemap::tone_stats.percentiles_0_field.w + Common::scene_tone_uniforms.exposure;
    let ambient_level = ToneCommon::SCENE_MIDDLE_GREY * exp2(clamp(ambient_ev, -12.0, 6.0));
    let positive = Color::gamut_project_nonnegative_rec2020(input_rgb);
    let relative = clamp(Common::safe_luma(positive) / ambient_level, 0.02, 64.0);
    let reflectance = positive * pow(relative, -0.3);
    let color = mask_effect_picker_color_to_working(secondary.xyz);
    return ambient_rgb + reflectance * color * (irradiance * amount * RELIGHT_GAIN);
}
