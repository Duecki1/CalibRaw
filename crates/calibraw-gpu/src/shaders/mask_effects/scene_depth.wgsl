// Scene depth: the full-image depth texture and the joint upsampling its
// readers share. Fog and Smoke read stored depth through a wide tent
// (`fog_depth_at` in atmosphere.wgsl); Relight reads the relighting surface
// and its shadow map through the nearest 4×4 texels (`scene_depth_taps_at`).
//
// Depth is normalized relative distance (near 0, far 1) over the full image,
// whatever the crop or export tile. Level 0 channel x holds the stored depth;
// once Relight allocated its surface, the other channels and the mip levels
// hold the relighting surface (scene_surface.rs). The texture is square, so
// its texels stretch to the image's aspect.
@group(0) @binding(35) var scene_depth_tex: texture_2d<f32>;

// Joint-upsampling guide on the level-0 depth grid: per texel, the square
// root of the image colour at the texel's centre (rgb) and the stored depth
// (a). `build_scene_depth_guide` fills it from the creative pass's input right
// before that pass, so readers take each texel's guide with one load instead
// of filtering the image around every tap of every pixel.
@group(0) @binding(48) var scene_depth_guide_tex: texture_2d<f32>;
@group(0) @binding(49) var scene_depth_guide_out: texture_storage_2d<rgba16float, write>;

// Guide texels this far beyond the tile are never read: readers look at most
// 3.5 texels away (`fog_depth_at`).
const SCENE_DEPTH_GUIDE_MARGIN_TEXELS: f32 = 4.0;

@compute @workgroup_size(8, 8, 1)
fn build_scene_depth_guide(@builtin(global_invocation_id) gid: vec3<u32>) {
    let size = textureDimensions(scene_depth_tex);
    if gid.x >= size.x || gid.y >= size.y { return; }
    let cell = vec2<i32>(gid.xy);
    let full_size = vec2<f32>(
        f32(Common::camera_uniforms.full_width),
        f32(Common::camera_uniforms.full_height),
    );
    let uv = (vec2<f32>(cell) + vec2<f32>(0.5)) / vec2<f32>(size);
    // The texel centre in this tile's pixels.
    let guide_pos = uv * full_size - vec2<f32>(0.5) - vec2<f32>(Common::tile_origin());
    let margin = SCENE_DEPTH_GUIDE_MARGIN_TEXELS * full_size / vec2<f32>(size);
    let tile = vec2<f32>(
        f32(Common::camera_uniforms.width),
        f32(Common::camera_uniforms.height),
    );
    if any(guide_pos < -margin) || any(guide_pos > tile + margin) { return; }
    let guide = sqrt(max(mask_effect_source_linear_at(guide_pos), vec3<f32>(0.0)));
    textureStore(
        scene_depth_guide_out,
        cell,
        vec4<f32>(guide, textureLoad(scene_depth_tex, cell, 0).x),
    );
}

// The image colour that guides joint upsampling at a pixel.
fn scene_depth_guide_center(pos: vec2<i32>) -> vec3<f32> {
    return sqrt(max(SceneAdjustments::local_effects_at(pos), vec3<f32>(0.0)));
}

// Selectivity of Fog and Smoke's joint upsampling (`scene_depth_guide_weight`).
const SCENE_DEPTH_FOG_GUIDE_SHARPNESS: f32 = 64.0;
// Selectivity of Relight's joint upsampling. Relight shades each texel's own
// surface, so a pixel that switched abruptly between the texels of a
// foreground edge and those of the background, as its colour crosses the
// photo's (often defocused) edge and each texel's single-pixel guide colour,
// would show the texel grid as a sawtooth along the silhouette.
const SCENE_DEPTH_RELIGHT_GUIDE_SHARPNESS: f32 = 16.0;

// How much a level-0 texel whose guide colour is `guide` may contribute at a
// pixel whose guide colour is `center`: little where the image's colour at
// the texel centre differs, the less the higher `sharpness`, so upsampled
// depth follows the image's edges rather than the depth model's blocky ones.
fn scene_depth_guide_weight(guide: vec3<f32>, center: vec3<f32>, sharpness: f32) -> f32 {
    let delta = (guide - center) / max(length(center), 0.15);
    return max(exp(-dot(delta, delta) * sharpness), 0.0001);
}

// The 4×4 level-0 texels around an image pixel and their joint upsampling
// weights: a tent two texels in radius, scaled by `scene_depth_guide_weight`,
// which keeps background depth out of foreground silhouettes, and normalized
// to sum to one. The tent's overlap between neighbouring pixels makes
// guided transitions ramp over several pixels instead of snapping at texel
// boundaries. No depth-range mask curve applies: a selection is not a
// measurement of distance. Tap `i` is the texel at
// `base + (i % 4, i / 4)`, clamped to the grid.
const SCENE_DEPTH_TAPS: u32 = 16u;

struct SceneDepthTaps {
    base: vec2<i32>,
    // Row y holds the weights of the texels at `base + (0..3, y)`.
    weights: array<vec4<f32>, 4>,
}

fn scene_depth_tap_cell(taps: SceneDepthTaps, tap: u32) -> vec2<i32> {
    let size = vec2<i32>(textureDimensions(scene_depth_tex));
    let offset = vec2<i32>(i32(tap % 4u), i32(tap / 4u));
    return clamp(taps.base + offset, vec2<i32>(0), size - vec2<i32>(1));
}

fn scene_depth_tap_weight(taps: SceneDepthTaps, tap: u32) -> f32 {
    var weights = taps.weights;
    return weights[tap / 4u][tap % 4u];
}

fn scene_depth_taps_at(pos: vec2<i32>) -> SceneDepthTaps {
    let size = vec2<i32>(textureDimensions(scene_depth_tex));
    let p = full_image_uv(pos) * vec2<f32>(size) - vec2<f32>(0.5);
    let center = scene_depth_guide_center(pos);
    var taps = SceneDepthTaps(vec2<i32>(floor(p)) - vec2<i32>(1), array<vec4<f32>, 4>());
    var weights = array<vec4<f32>, 4>();
    var total = 0.0;
    for (var tap = 0u; tap < SCENE_DEPTH_TAPS; tap = tap + 1u) {
        let texel = taps.base + vec2<i32>(i32(tap % 4u), i32(tap / 4u));
        let offset = abs(vec2<f32>(texel) - p);
        let tent = max(1.0 - offset.x * 0.5, 0.0) * max(1.0 - offset.y * 0.5, 0.0);
        let guide = textureLoad(scene_depth_guide_tex, scene_depth_tap_cell(taps, tap), 0).xyz;
        let weight = tent
            * scene_depth_guide_weight(guide, center, SCENE_DEPTH_RELIGHT_GUIDE_SHARPNESS);
        weights[tap / 4u][tap % 4u] = weight;
        total += weight;
    }
    // The tent is positive on at least the four nearest texels, and every
    // guide weight is at least 0.0001, so `total` is positive.
    for (var row = 0u; row < 4u; row = row + 1u) {
        weights[row] /= total;
    }
    taps.weights = weights;
    return taps;
}

// Level 0 of `scene_depth_tex` upsampled with `taps`. Readers of other maps
// on the scene-depth grid repeat this loop over their own texture: a texture
// passed to a function crashes the Mali-G710 shader compiler
// (`no_handle_arguments` test).
fn scene_depth_resolve_depth(taps: SceneDepthTaps) -> vec4<f32> {
    var total = vec4<f32>(0.0);
    for (var tap = 0u; tap < SCENE_DEPTH_TAPS; tap = tap + 1u) {
        total += textureLoad(scene_depth_tex, scene_depth_tap_cell(taps, tap), 0)
            * scene_depth_tap_weight(taps, tap);
    }
    return total;
}
