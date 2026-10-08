// Scene depth: the full-image depth texture and the joint upsampling its
// readers share. Fog and Smoke read stored depth through a wide tent
// (`fog_depth_at` in atmosphere.wgsl); Relight reads the relighting surface
// and its shadow map through the four nearest texels (`scene_depth_taps_at`).
//
// Depth is normalized relative distance (near 0, far 1) over the full image,
// whatever the crop or export tile. Level 0 channel x holds the stored depth;
// the other channels and the mip levels hold the relighting surface
// (scene_surface.rs). The texture is square, so its texels stretch to the
// image's aspect.
@group(0) @binding(35) var scene_depth_tex: texture_2d<f32>;

// The image colour that guides joint upsampling at a pixel.
fn scene_depth_guide_center(pos: vec2<i32>) -> vec3<f32> {
    return sqrt(max(SceneAdjustments::local_effects_at(pos), vec3<f32>(0.0)));
}

// How much the level-0 texel `cell` of a grid of `size` texels may contribute
// at a pixel whose guide colour is `center`: almost nothing where the image's
// colour at the texel centre differs, so upsampled depth follows the image's
// edges rather than the depth model's blocky ones.
fn scene_depth_guide_weight(cell: vec2<i32>, size: vec2<i32>, center: vec3<f32>) -> f32 {
    let full_size = vec2<f32>(
        f32(Common::camera_uniforms.full_width),
        f32(Common::camera_uniforms.full_height),
    );
    let uv = (vec2<f32>(cell) + vec2<f32>(0.5)) / vec2<f32>(size);
    let guide_pos = uv * full_size - vec2<f32>(0.5) - vec2<f32>(Common::tile_origin());
    let guide = sqrt(max(mask_effect_source_linear_at(guide_pos), vec3<f32>(0.0)));
    let delta = (guide - center) / max(length(center), 0.15);
    return max(exp(-dot(delta, delta) * 64.0), 0.0001);
}

// The four level-0 texels around an image pixel and their joint upsampling
// weights: bilinear weights scaled by `scene_depth_guide_weight`, which keeps
// background depth out of foreground silhouettes. No depth-range mask curve
// applies: a selection is not a measurement of distance. Tap `i` is the texel
// at `base + (i % 2, i / 2)`, clamped to the grid.
struct SceneDepthTaps {
    base: vec2<i32>,
    weights: vec4<f32>,
}

fn scene_depth_tap_cell(taps: SceneDepthTaps, tap: u32) -> vec2<i32> {
    let size = vec2<i32>(textureDimensions(scene_depth_tex));
    let offset = vec2<i32>(i32(tap % 2u), i32(tap / 2u));
    return clamp(taps.base + offset, vec2<i32>(0), size - vec2<i32>(1));
}

fn scene_depth_taps_at(pos: vec2<i32>) -> SceneDepthTaps {
    let size = vec2<i32>(textureDimensions(scene_depth_tex));
    let p = full_image_uv(pos) * vec2<f32>(size) - vec2<f32>(0.5);
    let f = fract(p);
    let center = scene_depth_guide_center(pos);
    var taps = SceneDepthTaps(vec2<i32>(floor(p)), vec4<f32>(0.0));
    for (var tap = 0u; tap < 4u; tap = tap + 1u) {
        let spatial = select(1.0 - f.x, f.x, tap % 2u == 1u)
            * select(1.0 - f.y, f.y, tap / 2u == 1u);
        taps.weights[tap] = spatial
            * scene_depth_guide_weight(scene_depth_tap_cell(taps, tap), size, center);
    }
    return taps;
}

// Level 0 of `grid`, a texture on the scene-depth grid (the depth itself or a
// map derived from it), upsampled with `taps`.
fn scene_depth_resolve(grid: texture_2d<f32>, taps: SceneDepthTaps) -> vec4<f32> {
    var total = vec4<f32>(0.0);
    var weights = 0.0;
    for (var tap = 0u; tap < 4u; tap = tap + 1u) {
        total += textureLoad(grid, scene_depth_tap_cell(taps, tap), 0) * taps.weights[tap];
        weights += taps.weights[tap];
    }
    return total / max(weights, 1e-6);
}
