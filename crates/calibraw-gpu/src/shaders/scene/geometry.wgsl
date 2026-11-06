#import calibraw::common as Common
#import calibraw::color as Color
#import calibraw::scene_source as SceneSource
#import calibraw::scene::camera as Camera
#import calibraw::scene::bake_common as Bake

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

// Whether a scene bake is bound: its uniforms hold a working size.
fn scene_geometry_present() -> bool {
    return Bake::scene_bake.size.x > 0u;
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
// layer colours live: the near and far colours F and B take the pixel's own
// residual r = scene − (α F + (1 − α) B), so they keep its detail (§4.2), and
// then follow the edit by the ratio of luminance between `rgb` and the scene,
// with what that ratio does not explain added to both.
fn scene_split(pixel: ScenePixel, scene_rgb: vec3<f32>, rgb: vec3<f32>) -> array<vec3<f32>, 2> {
    if !pixel.in_band {
        return array<vec3<f32>, 2>(rgb, rgb);
    }
    let alpha = pixel.coverage;
    let residual = scene_rgb - (alpha * pixel.near.color + (1.0 - alpha) * pixel.far.color);
    let near = pixel.near.color + residual;
    let far = pixel.far.color + residual;
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
