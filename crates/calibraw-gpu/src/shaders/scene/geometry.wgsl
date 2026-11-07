#import calibraw::common as Common
#import calibraw::color as Color
#import calibraw::scene_source as SceneSource
#import calibraw::scene_adjustments as SceneAdjustments
#import calibraw::scene::camera as Camera
#import calibraw::scene::bake_common as Bake
#import calibraw::scene::lights_common as Lights

// The scene's geometry at a pixel, as depth effects read it
// (docs/rework/DEPTH_EFFECTS_PLAN.md §4.2 and §4.5): up to two surfaces, each
// with its own depth, normal and colour, and how much of the pixel the near
// one covers. Effects shade each surface on its own (`shade(surface, …)`) and
// blend the two by coverage (`scene_blend`), so no pixel is ever shaded at a
// depth between two surfaces.
//
// It reads the scene bake (bake_common.wgsl) through one texture array and
// the light bake's visibility (visibility.wgsl) through another, both in the
// full-image frame at the working size, so every crop and export tile reads
// the same values at the same full-image position. A pixel reads the four
// working texels around its centre:
// - Coverage: their coverage coefficients, interpolated bilinearly and
//   evaluated with the pixel's own guide (He and Sun's fast guided filter),
//   so edges land on the photo's own edges.
// - Each surface: only from those texels that hold it (a texel at no edge
//   holds one surface, on the side of the edge its coverage says), with the
//   bilinear weights renormalized, so the near surface never takes the far
//   one's depth and the other way round.
//
// Positions are in the shading frame at the effect's relief scale E
// (camera.wgsl), normals in that frame facing the camera, colours the
// unedited scene (scene-linear Rec.2020 before exposure).

// The bake's layers (bake_common.wgsl SCENE_LAYER_*) and the visibility
// array, two layers per group of four lights: near, then far.
@group(0) @binding(113) var scene_bake_layers: texture_2d_array<f32>;
@group(0) @binding(112) var scene_visibility_layers: texture_2d_array<f32>;

struct SceneSurface {
    // In the shading frame at the effect's relief scale.
    position: vec3<f32>,
    normal: vec3<f32>,
    // The unedited scene's colour of this surface.
    color: vec3<f32>,
    // Linear depth d.
    d: f32,
    // How much of it is sky, which receives no light.
    sky: f32,
    // Which of the pixel's surfaces it is: 0 near, 1 far. Its visibility and
    // other per-surface bakes are read on that side.
    side: u32,
}

struct ScenePixel {
    // How much of the pixel the near surface covers.
    coverage: f32,
    near: SceneSurface,
    far: SceneSurface,
    // Both surfaces show: shade both and blend.
    in_band: bool,
    // The top-left of the four texels read, and per surface their weights.
    base: vec2<i32>,
    near_weights: vec4<f32>,
    far_weights: vec4<f32>,
}

// The unedited scene at tile pixel `pos`, as the bake's working image holds it
// (bake_accumulate.wgsl): what coverage is evaluated with and the surfaces'
// colours are split from.
fn scene_unedited_at(pos: vec2<i32>) -> vec3<f32> {
    return max(Color::map_negative_gamut(SceneSource::profiled_scene_at(pos)), vec3<f32>(0.0));
}

// Whether the scene bake holds a depth result's geometry: its uniforms hold the
// disparity's size.
fn scene_geometry_present() -> bool {
    return Bake::scene_bake.size.z > 0u;
}

// Whether a scene bake is bound, with or without a depth result: its uniforms
// hold a working size.
fn scene_image_present() -> bool {
    return Bake::scene_bake.size.x > 0u;
}

// The working image with its mip chain (bake_mips.wgsl): the unedited scene,
// scene-linear Rec.2020 before exposure, at the working size. The alpha of
// levels from 1 is the nearest visible depth, read only by `scene_trace`.
@group(0) @binding(117) var scene_working_image: texture_2d<f32>;

// Width of the box a mip level averages, in Gaussian deviations: a box of
// width w has deviation w / sqrt(12).
const SCENE_IMAGE_BOX_PER_DEVIATION: f32 = 3.46;
// Footprints (deviations, in working texels) from which `scene_image` reads a
// 3 × 3 pattern of taps instead of one: mip texels of a box that wide are too
// coarse for one trilinear tap to be local.
const SCENE_IMAGE_WIDE: f32 = 1.5;

// One trilinear tap of the mip chain whose box has deviation `footprint`
// shorter edges (0 reads the working texels).
fn scene_image_tap(uv: vec2<f32>, footprint: f32) -> vec3<f32> {
    let size = Bake::scene_bake_size();
    let short = f32(min(size.x, size.y));
    // Levels run down to one texel on the longer edge, as the bake's mips do.
    let last = f32(firstLeadingBit(u32(max(size.x, size.y))));
    let level = clamp(log2(max(SCENE_IMAGE_BOX_PER_DEVIATION * footprint * short, 1.0)), 0.0, last);
    return textureSampleLevel(scene_working_image, SceneAdjustments::local_mask_sampler, uv, level).rgb;
}

// The unedited scene at full-image `uv` averaged over a Gaussian of deviation
// `footprint` shorter edges, from the working image's mip chain. A wide
// footprint is a 3 × 3 Gauss–Hermite quadrature of Gaussian taps: nodes 0 and
// ±1.5σ with weights 2/3 and 1/6 per axis, each tap a half-σ blur, which
// together have deviation σ. Needs `scene_image_present`; edges clamp.
fn scene_image(uv: vec2<f32>, footprint: f32) -> vec3<f32> {
    let size = Bake::scene_bake_size();
    if footprint * f32(min(size.x, size.y)) < SCENE_IMAGE_WIDE {
        return scene_image_tap(uv, footprint);
    }
    let aspect = scene_aspect();
    let step = 1.5 * footprint / aspect;
    var sum = vec3<f32>(0.0);
    for (var y = -1; y <= 1; y = y + 1) {
        for (var x = -1; x <= 1; x = x + 1) {
            let weight = select(1.0 / 6.0, 2.0 / 3.0, x == 0) * select(1.0 / 6.0, 2.0 / 3.0, y == 0);
            sum += weight * scene_image_tap(uv + vec2<f32>(f32(x), f32(y)) * step, 0.5 * footprint);
        }
    }
    return sum;
}

// The image's width and height in shorter edges, from the working size,
// which has the full image's aspect.
fn scene_aspect() -> vec2<f32> {
    let working = vec2<f32>(Bake::scene_bake_size());
    return working / min(working.x, working.y);
}

fn scene_layer(texel: vec2<i32>, layer: u32) -> vec4<f32> {
    return textureLoad(scene_bake_layers, Bake::scene_bake_clamp(texel), layer, 0);
}

// The texel offset of corner `corner` of the four read.
fn scene_corner(corner: u32) -> vec2<i32> {
    return vec2<i32>(i32(corner % 2u), i32(corner / 2u));
}

// The geometry at full-image `uv` of a pixel whose unedited scene colour is
// `scene_rgb`, at relief scale `relief_scale`.
fn scene_pixel(uv: vec2<f32>, scene_rgb: vec3<f32>, relief_scale: f32) -> ScenePixel {
    let size = Bake::scene_bake_size();
    let position = uv * vec2<f32>(size) - vec2<f32>(0.5);
    let base = vec2<i32>(floor(position));
    let t = position - vec2<f32>(base);
    let bilinear = vec4<f32>((1.0 - t.x) * (1.0 - t.y), t.x * (1.0 - t.y), (1.0 - t.x) * t.y, t.x * t.y);

    var coefficients = vec4<f32>(0.0);
    var near_weights = vec4<f32>(0.0);
    var far_weights = vec4<f32>(0.0);
    var edge = false;
    var geometry: array<vec4<f32>, 4>;
    var normals: array<vec4<f32>, 4>;
    var near_sky = vec4<f32>(0.0);
    var far_sky = vec4<f32>(0.0);
    for (var corner = 0u; corner < 4u; corner = corner + 1u) {
        let texel = base + scene_corner(corner);
        let k = scene_layer(texel, Bake::SCENE_LAYER_COVERAGE);
        let g = scene_layer(texel, Bake::SCENE_LAYER_GEOMETRY);
        geometry[corner] = g;
        normals[corner] = scene_layer(texel, Bake::SCENE_LAYER_NORMALS);
        coefficients += bilinear[corner] * k;
        if Bake::scene_coverage_constant(k) {
            // One surface, the texel's own, on the side its coverage says.
            let own_sky = select(0.0, 1.0, g.y >= 0.5);
            if k.w > 0.5 {
                near_weights[corner] = bilinear[corner];
                near_sky[corner] = own_sky;
            } else {
                far_weights[corner] = bilinear[corner];
                far_sky[corner] = own_sky;
            }
        } else {
            edge = true;
            near_weights[corner] = bilinear[corner];
            far_weights[corner] = bilinear[corner];
            far_sky[corner] = g.y;
        }
    }
    let near_total = dot(near_weights, vec4<f32>(1.0));
    let far_total = dot(far_weights, vec4<f32>(1.0));
    var coverage = Bake::scene_coverage_of(coefficients, Bake::scene_bake_guide(scene_rgb));
    if near_total <= 0.0 {
        coverage = 0.0;
    } else if far_total <= 0.0 {
        coverage = 1.0;
    }
    near_weights = near_weights / max(near_total, 1e-6);
    far_weights = far_weights / max(far_total, 1e-6);
    let in_band = coverage > 0.0 && coverage < 1.0 && near_total > 0.0 && far_total > 0.0;

    var near = SceneSurface(vec3<f32>(0.0), vec3<f32>(0.0), scene_rgb, 0.0, dot(near_weights, near_sky), 0u);
    var far = SceneSurface(vec3<f32>(0.0), vec3<f32>(0.0), scene_rgb, 0.0, dot(far_weights, far_sky), 1u);
    var near_normal = vec3<f32>(0.0);
    var far_normal = vec3<f32>(0.0);
    var near_color = vec3<f32>(0.0);
    var far_color = vec3<f32>(0.0);
    for (var corner = 0u; corner < 4u; corner = corner + 1u) {
        near.d += near_weights[corner] * geometry[corner].z;
        far.d += far_weights[corner] * geometry[corner].w;
        near_normal += near_weights[corner] * Bake::scene_from_octahedral(normals[corner].xy);
        far_normal += far_weights[corner] * Bake::scene_from_octahedral(normals[corner].zw);
        if in_band && edge {
            let texel = base + scene_corner(corner);
            near_color += near_weights[corner] * scene_layer(texel, Bake::SCENE_LAYER_NEAR).rgb;
            far_color += far_weights[corner] * scene_layer(texel, Bake::SCENE_LAYER_FAR).rgb;
        }
    }
    if in_band && edge {
        near.color = near_color;
        far.color = far_color;
    }
    let camera = Bake::scene_bake.camera;
    let point = Camera::scene_image_point(uv, scene_aspect());
    near = scene_surface_at(near, near_normal, point, relief_scale);
    far = scene_surface_at(far, far_normal, point, relief_scale);
    if !in_band {
        // One surface: the one that covers the pixel, which keeps its side.
        if coverage < 0.5 {
            near = far;
        } else {
            far = near;
        }
    }
    return ScenePixel(coverage, near, far, in_band, base, near_weights, far_weights);
}

// A surface's position and frame normal from its depth and summed normal.
fn scene_surface_at(surface: SceneSurface, normal_sum: vec3<f32>, point: vec2<f32>, relief_scale: f32) -> SceneSurface {
    let camera = Bake::scene_bake.camera;
    var result = surface;
    let z = Camera::scene_frame_depth(camera, surface.d, relief_scale);
    result.position = Camera::scene_frame_position(camera, point, z);
    var normal = vec3<f32>(0.0, 0.0, -1.0);
    if dot(normal_sum, normal_sum) > 1e-8 {
        normal = normalize(normal_sum);
    }
    result.normal = Camera::scene_frame_normal(camera, normal, point, z, relief_scale);
    return result;
}

// The pixel's colours of its two surfaces in the domain of `rgb`, the colour
// an effect receives, which may be edited: (near, far). They blend back to
// `rgb` exactly. The split is made on the unedited scene, where the bake's
// layer colours live: the near and far colours F and B are scaled per channel
// by the ratio of the pixel's colour to their blend, k = scene / (α F +
// (1 − α) B), so they keep the pixel's detail (§4.2) and stay non-negative.
// (Adding the residual scene − (α F + (1 − α) B) to both instead turns the
// darker surface negative wherever coverage is a little off at a high
// contrast, as a tree line against a bright sky.) Where the blend is nearly
// black the residual is added. Then both follow the edit by the ratio of
// luminance between `rgb` and the scene, with what that ratio does not
// explain added to both.
fn scene_split(pixel: ScenePixel, scene_rgb: vec3<f32>, rgb: vec3<f32>) -> array<vec3<f32>, 2> {
    if !pixel.in_band {
        return array<vec3<f32>, 2>(rgb, rgb);
    }
    let alpha = pixel.coverage;
    let blend = alpha * pixel.near.color + (1.0 - alpha) * pixel.far.color;
    let ratio = scene_rgb / max(blend, vec3<f32>(SCENE_SPLIT_LEAST_BLEND));
    let scaled = blend > vec3<f32>(SCENE_SPLIT_LEAST_BLEND);
    let residual = scene_rgb - blend;
    let near = select(pixel.near.color + residual, pixel.near.color * ratio, scaled);
    let far = select(pixel.far.color + residual, pixel.far.color * ratio, scaled);
    let gain = clamp(
        Common::safe_luma(max(rgb, vec3<f32>(0.0))) / max(Common::safe_luma(max(scene_rgb, vec3<f32>(0.0))), 1e-6),
        0.0,
        SCENE_SPLIT_MAX_GAIN,
    );
    let unexplained = rgb - gain * scene_rgb;
    return array<vec3<f32>, 2>(gain * near + unexplained, gain * far + unexplained);
}

// The largest edit gain the split follows: beyond it the edited colour is
// split as is.
const SCENE_SPLIT_MAX_GAIN: f32 = 256.0;
// A blend of the layer colours below this (scene-linear, about −16 EV from
// mid grey) is split by adding the residual: a ratio there is noise.
const SCENE_SPLIT_LEAST_BLEND: f32 = 3e-6;

// The two surfaces' results blended by coverage.
fn scene_blend(pixel: ScenePixel, near_rgb: vec3<f32>, far_rgb: vec3<f32>) -> vec3<f32> {
    if !pixel.in_band {
        return near_rgb;
    }
    return mix(far_rgb, near_rgb, pixel.coverage);
}

// How much of the light in light bake channel `channel` reaches `surface`, one
// of the pixel's, from the four texels the pixel read.
fn scene_visibility(pixel: ScenePixel, surface: SceneSurface, channel: u32) -> f32 {
    let layer = 2u * (channel / 4u) + surface.side;
    let weights = select(pixel.near_weights, pixel.far_weights, surface.side == 1u);
    var visibility = 0.0;
    for (var corner = 0u; corner < 4u; corner = corner + 1u) {
        if weights[corner] <= 0.0 {
            continue;
        }
        let texel = Bake::scene_bake_clamp(pixel.base + scene_corner(corner));
        visibility += weights[corner] * textureLoad(scene_visibility_layers, texel, layer, 0)[channel % 4u];
    }
    return visibility;
}

// A surface's normal averaged over a disc of `radius` shorter edges around
// full-image `uv`, as a light of that size integrates the surface. Taps whose
// surface leaves the tangent plane of `surface` by more than about the disc's
// size, such as a background beside a silhouette, are rejected, so a nearby
// surface does not tilt this one. Each tap reads one working texel and takes
// whichever of its surfaces lies closer to the plane.
fn scene_soft_normal(surface: SceneSurface, uv: vec2<f32>, radius: f32, relief_scale: f32) -> vec3<f32> {
    let size = vec2<f32>(Bake::scene_bake_size());
    let aspect = scene_aspect();
    let short = min(size.x, size.y);
    if radius * short < 1.0 {
        return surface.normal;
    }
    let camera = Bake::scene_bake.camera;
    // Plane distance relative to depth that still counts as the surface.
    let tolerance = 0.01 + 2.0 * Camera::scene_frame_position(camera, vec2<f32>(radius), 1.0).x;
    var sum = surface.normal;
    for (var tap = 0u; tap < SCENE_SOFT_NORMAL_TAPS; tap = tap + 1u) {
        let distance = radius * sqrt((f32(tap) + 0.5) / f32(SCENE_SOFT_NORMAL_TAPS));
        let angle = f32(tap) * 2.39996323;
        let tap_uv = uv + distance * vec2<f32>(cos(angle), sin(angle)) / aspect;
        let texel = vec2<i32>(floor(tap_uv * size));
        let g = scene_layer(texel, Bake::SCENE_LAYER_GEOMETRY);
        let n = scene_layer(texel, Bake::SCENE_LAYER_NORMALS);
        let point = Camera::scene_image_point(tap_uv, aspect);
        var best = 1e9;
        var best_normal = vec3<f32>(0.0);
        for (var side = 0u; side < 2u; side = side + 1u) {
            let d = select(g.z, g.w, side == 1u);
            let z = Camera::scene_frame_depth(camera, d, relief_scale);
            let offset = dot(surface.normal, Camera::scene_frame_position(camera, point, z) - surface.position);
            let distance_to_plane = abs(offset) / surface.position.z;
            if distance_to_plane < best {
                best = distance_to_plane;
                let encoded = select(n.xy, n.zw, side == 1u);
                best_normal = Camera::scene_frame_normal(
                    camera, Bake::scene_from_octahedral(encoded), point, z, relief_scale,
                );
            }
        }
        let residual = best / tolerance;
        sum += best_normal * exp(-residual * residual);
    }
    return normalize(sum);
}

const SCENE_SOFT_NORMAL_TAPS: u32 = 8u;

// The air light (L3, air_light.wgsl): its texture, with the grid and region
// it holds in Lights::scene_air.
@group(0) @binding(114) var scene_air_tex: texture_2d_array<f32>;

// Depth difference, as a share of frame depth, over which a grid texel's
// share stops counting for a surface: the rays of the texels around a pixel
// that met another surface do not shade it.
const SCENE_AIR_DEPTH_TOLERANCE: f32 = 0.05;

// The share of the light in light bake channel `channel` that a view ray at
// full-image `uv` receives in the air on its way to `surface`, which the
// scene does not shadow (1 when no air light is built). A joint bilateral
// upsample of the air light's grid (Kopf et al., "Joint Bilateral
// Upsampling", 2007): the four grid texels around the pixel, each read on the
// side (near or far surface) whose depth lies closer to the surface's, by
// their bilinear weight times how alike that depth is. So shafts end on the
// photo's own outlines, at the pixel's resolution.
fn scene_air_light(uv: vec2<f32>, surface: SceneSurface, channel: u32) -> f32 {
    let air = Lights::scene_air;
    if air.march.x == 0u || channel >= 4u * air.march.w {
        return 1.0;
    }
    let camera = Bake::scene_bake.camera;
    let position = uv * vec2<f32>(air.image.xy) / f32(air.march.x) - vec2<f32>(0.5) - vec2<f32>(air.region.xy);
    let base = vec2<i32>(floor(position));
    let t = position - vec2<f32>(base);
    let last = air.region.zw - vec2<i32>(1);
    let own = log(Camera::scene_frame_depth(camera, surface.d, 1.0));
    let layer = 1u + 2u * (channel / 4u);
    var total = 0.0;
    var share = 0.0;
    var closest = 1e9;
    var closest_share = 1.0;
    for (var corner = 0u; corner < 4u; corner = corner + 1u) {
        let offset = scene_corner(corner);
        let texel = clamp(base + offset, vec2<i32>(0), last);
        let depths = textureLoad(scene_air_tex, texel, 0, 0).xy;
        let differences = abs(vec2<f32>(
            log(Camera::scene_frame_depth(camera, depths.x, 1.0)),
            log(Camera::scene_frame_depth(camera, depths.y, 1.0)),
        ) - vec2<f32>(own));
        let side = select(0u, 1u, differences.y < differences.x);
        let difference = min(differences.x, differences.y) / SCENE_AIR_DEPTH_TOLERANCE;
        let value = textureLoad(scene_air_tex, texel, layer + side, 0)[channel % 4u];
        let bilinear = select(1.0 - t.x, t.x, offset.x == 1) * select(1.0 - t.y, t.y, offset.y == 1);
        let weight = bilinear * exp(-difference * difference);
        total += weight;
        share += weight * value;
        if difference < closest {
            closest = difference;
            closest_share = value;
        }
    }
    if total < 1e-4 {
        return closest_share;
    }
    return share / total;
}

// --- The ground plane (B8) ---

// Whether the photo shows a ground: the bake's plane is fitted
// (scene/ground.rs) and valid.
fn scene_has_ground() -> bool {
    return Bake::scene_bake.ground_horizon.w > 0.5;
}

// The ground plane in the shading frame of relief scale 1: its unit normal
// facing up and the camera (xyz) and offset w < 0, n · X = w for its points
// X; the camera lies |w| above it.
fn scene_ground_plane() -> vec4<f32> {
    return Bake::scene_bake.ground_plane;
}

// The horizon as image points p (shorter edges from the image centre) satisfy
// h · (p, 1) = 0, with h = (x, y, z) = (2 · focal_tan · n.xy, n.z). Points
// below it have h · (p, 1) < 0.
fn scene_horizon_line(point: vec2<f32>) -> f32 {
    let horizon = Bake::scene_bake.ground_horizon;
    return dot(horizon.xy, point) + horizon.z;
}

// Unit vector in image points toward the horizon, up the image.
fn scene_horizon_up() -> vec2<f32> {
    let horizon = Bake::scene_bake.ground_horizon;
    return horizon.xy / max(length(horizon.xy), 1e-6);
}

// The distance of image point `point` below the horizon, in shorter edges
// (negative above it).
fn scene_below_horizon(point: vec2<f32>) -> f32 {
    return -scene_horizon_line(point) / max(length(Bake::scene_bake.ground_horizon.xy), 1e-6);
}

// The ground plane's axes in the frame: across the view (right) and along it
// (forward, the camera's forward direction projected onto the plane).
fn scene_ground_axes() -> array<vec3<f32>, 2> {
    let normal = Bake::scene_bake.ground_plane.xyz;
    var forward = vec3<f32>(0.0, 0.0, 1.0) - normal * normal.z;
    if dot(forward, forward) < 1e-4 {
        forward = vec3<f32>(0.0, -1.0, 0.0) - normal * normal.y;
    }
    forward = normalize(forward);
    var right = normalize(cross(forward, normal));
    if right.x < 0.0 {
        right = -right;
    }
    return array<vec3<f32>, 2>(right, forward);
}

// The point of the ground plane that image point `point` sees, in camera
// heights: x across the view and y along it on the plane, and in z the ground
// distance one step of `step` shorter edges up the image covers there. Above
// the horizon the plane lies behind the camera; the horizon's limit is
// clamped.
fn scene_ground_position(point: vec2<f32>, step: f32) -> vec3<f32> {
    let plane = Bake::scene_bake.ground_plane;
    let lens = Bake::scene_bake.camera.lens.x;
    let height = max(-plane.w, 1e-4);
    let axes = scene_ground_axes();
    let first = scene_ground_ray(point, plane, lens);
    let second = scene_ground_ray(point + scene_horizon_up() * step, plane, lens);
    return vec3<f32>(dot(first, axes[0]) / height, dot(first, axes[1]) / height, length(second - first) / height);
}

// The ground point seen at image point `point`, in the frame.
fn scene_ground_ray(point: vec2<f32>, plane: vec4<f32>, lens: f32) -> vec3<f32> {
    let ray = vec3<f32>(2.0 * lens * point, 1.0);
    return ray * (plane.w / min(dot(plane.xyz, ray), -1e-3));
}

// --- Tracing rays through the scene (Wet Surfaces' reflections) ---

// Steps of a trace, the image distance of the first one (working texels) and
// how fast steps grow. 48 steps from 1.5 texels at ×1.12 reach 2900 texels.
const SCENE_TRACE_STEPS: u32 = 48u;
const SCENE_TRACE_FIRST_STRIDE: f32 = 1.5;
const SCENE_TRACE_GROWTH: f32 = 1.12;
// A ray lies behind the surface it meets by at most this share of the
// surface's depth to count as hitting it, and must be at least
// SCENE_TRACE_SELF_BIAS of the depth behind it: single-photo depth is noisy
// to about 1%, which would make a grazing ray hit the ground it left.
const SCENE_TRACE_THICKNESS: f32 = 0.25;
const SCENE_TRACE_SELF_BIAS: f32 = 0.01;
// Closest the ray may come to the camera plane, in frame depth.
const SCENE_TRACE_NEAREST: f32 = 0.05;
const SCENE_TRACE_REFINEMENTS: u32 = 6u;

struct SceneTraceHit {
    // The ray met a surface, at `uv`; else it left the image or ran out of
    // steps, and `uv` is the last point inside the image.
    found: bool,
    uv: vec2<f32>,
    // The image distance from the origin to `uv`, in shorter edges.
    travel: f32,
}

// The least visible linear depth in the working-image block of mip `level`
// at full-image `uv` (bake_mips.wgsl; levels from 1).
fn scene_hiz(uv: vec2<f32>, level: u32) -> f32 {
    let size = Bake::scene_bake_size();
    let level_size = max(size >> vec2<u32>(level), vec2<i32>(1));
    let texel = clamp(vec2<i32>(floor(uv * vec2<f32>(level_size))), vec2<i32>(0), level_size - vec2<i32>(1));
    return textureLoad(scene_working_image, texel, i32(level)).a;
}

// Full-image uv of a position in the frame of relief scale 1.
fn scene_frame_uv(position: vec3<f32>) -> vec2<f32> {
    let lens = Bake::scene_bake.camera.lens.x;
    return position.xy / (2.0 * lens * position.z) / scene_aspect() + vec2<f32>(0.5);
}

// Traces the ray from `origin` along unit vector `direction`, both in the
// frame of relief scale 1, through the scene's visible surfaces (hierarchical
// depth in the working image's alpha, bake_mips.wgsl; Uludag, "Hi-Z Screen-
// Space Cone-Traced Reflections", 2014). Steps cover an even, growing image
// distance; a step whose end lies behind the nearest surface of its block is
// searched by bisection for the surface it crosses, which counts as hit if
// the ray lies behind it by less than SCENE_TRACE_THICKNESS of its depth,
// else the ray passes behind it. Hits resolve to the second mip level, 2
// working texels.
fn scene_trace(origin: vec3<f32>, direction: vec3<f32>) -> SceneTraceHit {
    let camera = Bake::scene_bake.camera;
    let size = Bake::scene_bake_size();
    let short = f32(min(size.x, size.y));
    let last_level = u32(firstLeadingBit(u32(max(size.x, size.y))));
    let aspect = scene_aspect();
    let tan2 = 2.0 * camera.lens.x;
    let start_uv = scene_frame_uv(origin);
    var result = SceneTraceHit(false, start_uv, 0.0);
    var stride = SCENE_TRACE_FIRST_STRIDE / short;
    var t = 0.0;
    var in_front = true;
    for (var step = 0u; step < SCENE_TRACE_STEPS; step = step + 1u) {
        let here = origin + direction * t;
        // Image speed per unit of t: the ray's lateral change relative to its depth.
        let velocity = (direction.xy - here.xy * (direction.z / here.z)) / (tan2 * here.z);
        var dt = stride / max(length(velocity), 1e-6);
        if direction.z < 0.0 {
            dt = min(dt, (here.z - SCENE_TRACE_NEAREST) / -direction.z);
        }
        if dt <= 1e-6 {
            break;
        }
        let next_t = t + dt;
        let next = origin + direction * next_t;
        if next.z < SCENE_TRACE_NEAREST {
            break;
        }
        let next_uv = scene_frame_uv(next);
        if any(next_uv < vec2<f32>(0.0)) || any(next_uv > vec2<f32>(1.0)) {
            // Left the image: its edge along the ray is the last point.
            result.uv = clamp(next_uv, vec2<f32>(0.0), vec2<f32>(1.0));
            result.travel = length((result.uv - start_uv) * aspect);
            return result;
        }
        let level = clamp(u32(ceil(log2(max(stride * short, 1.0)))), 1u, last_level);
        let middle = origin + direction * (t + 0.5 * dt);
        let near_end = Bake::scene_bake_frame_depth(scene_hiz(next_uv, level));
        let near_middle = Bake::scene_bake_frame_depth(scene_hiz(scene_frame_uv(middle), level));
        let behind = next.z > near_end * (1.0 + SCENE_TRACE_SELF_BIAS)
            || middle.z > near_middle * (1.0 + SCENE_TRACE_SELF_BIAS);
        if behind && in_front {
            var low = t;
            var high = next_t;
            for (var i = 0u; i < SCENE_TRACE_REFINEMENTS; i = i + 1u) {
                let mid = 0.5 * (low + high);
                let point = origin + direction * mid;
                let surface = Bake::scene_bake_frame_depth(scene_hiz(scene_frame_uv(point), 1u));
                if point.z > surface * (1.0 + SCENE_TRACE_SELF_BIAS) {
                    high = mid;
                } else {
                    low = mid;
                }
            }
            let point = origin + direction * high;
            let uv = scene_frame_uv(point);
            let surface = Bake::scene_bake_frame_depth(scene_hiz(uv, 1u));
            let excess = point.z - surface * (1.0 + SCENE_TRACE_SELF_BIAS);
            if excess > 0.0 && excess < SCENE_TRACE_THICKNESS * surface {
                return SceneTraceHit(true, uv, length((uv - start_uv) * aspect));
            }
        }
        // In front of the surface at the step's end, or past one it did not hit.
        let end_surface = Bake::scene_bake_frame_depth(scene_hiz(next_uv, 1u));
        in_front = next.z <= end_surface * (1.0 + SCENE_TRACE_SELF_BIAS);
        result.uv = next_uv;
        result.travel = length((next_uv - start_uv) * aspect);
        t = next_t;
        stride = min(stride * SCENE_TRACE_GROWTH, 0.3);
    }
    return result;
}

// The share of light bake channel `channel` that reaches `position`, a point in
// the air or on a surface, in the light's own frame (its relief scale), from the
// light's shadow cube: four filter taps, no blocker search. 1 without a light
// bake or where the channel holds no light.
fn scene_point_visibility(position: vec3<f32>, channel: u32) -> f32 {
    let light = Lights::scene_shadow_light(channel);
    if !Lights::scene_shadow_light_active(light) {
        return 1.0;
    }
    return Lights::scene_cube_visibility(light, position, vec3<f32>(0.0), 0u, SCENE_POINT_TAPS);
}

// Filter taps of `scene_point_visibility`, as the Final air light's.
const SCENE_POINT_TAPS: u32 = 4u;
