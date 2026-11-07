#import calibraw::scene::camera as Camera

// The scene bake (pipeline/gpu/scene/): what depth effects read of the scene,
// built once per depth result and unedited scene, so that every pixel can be
// shaded as up to two surfaces instead of one blended depth.
//
// Frame and size. The bake is a grid over the full image (the whole photo
// before any crop) at the working size: the full image's aspect, its longer
// edge at most 1540 texels (scene/mod.rs `working_size`). Texel (x, y)
// covers full-image uv [x, x + 1) / width × [y, y + 1) / height, so crops and
// export tiles read the same bake.
//
// Colours are the unedited scene: scene-linear Rec.2020 before exposure,
// negative gamut mapped and clamped at 0. The guide that coverage follows is
// their per-channel square root (`scene_bake_guide`), which spreads shadows
// and highlights about as the eye does.
//
// The bake array (rgba16float, one layer per SCENE_LAYER_*):
//   GEOMETRY  linear depth d of the texel (x); the far surface's sky
//             probability (y), at no edge the texel's own; the depth d of
//             the near (z) and the far (w) surface. The near surface of an
//             edge is never sky: the sky lies behind everything. (Until the
//             layers are made, B2 to B7 hold the texel's own d, sky and its
//             band's least and greatest d here.)
//   COVERAGE  the near surface's coverage at a pixel is
//             clamp(dot(xyz, guide) + w, 0, 1) for the pixel's own guide
//   NORMALS   the near (xy) and far (zw) surface's normal, octahedral
//   NEAR      the near surface's colour (xyz); w is the texel's kind of edge
//             (SCENE_EDGE_*, 0 at no edge)
//   FAR       the far surface's colour (xyz); w as NEAR's
// The band is where a depth edge (SCENE_BAND_DEPTH) may lie within
// SCENE_BAND_RADIUS texels or the sky's outline (SCENE_BAND_SKY) within
// SCENE_SKY_BAND_RADIUS; B3 finds the edges in it. A texel at no edge holds
// one surface: both surfaces are its own and coverage is a constant 0 or 1.
// At the sky's outline the far surface is the sky.
//
// Normals use the camera frame of calibraw-core `SceneNormals`: x to the
// right, y down the image, z away from the camera; visible surfaces have
// z < 0.

// The bake's shared bindings. Numbers start at 84 so that the working-image
// accumulation, a pass of the processing pipeline, can import this module
// beside the pipeline's own bindings.
//
// The scratch buffer: one vec4 per texel of the working size, used in turn by
// the accumulation (summed colour and pixels, as f32 bits), the guided
// filter's inputs and the layers' push-pull pyramid (bake_layers.wgsl).
@group(0) @binding(84) var<storage, read_write> scene_bake_scratch: array<vec4<u32>>;
@group(0) @binding(85) var<uniform> scene_bake: SceneBakeUniforms;
// The depth result's inputs, each at its own size: disparity (r32float,
// high = near), sky probability (r8unorm) and normals (rg8unorm, x and y as
// SceneNormals stores them).
@group(0) @binding(86) var scene_disparity_tex: texture_2d<f32>;
@group(0) @binding(87) var scene_sky_tex: texture_2d<f32>;
@group(0) @binding(88) var scene_normals_tex: texture_2d<f32>;
// Linear filtering, clamped at the edges.
@group(0) @binding(89) var scene_bake_sampler: sampler;
// The working image: one mip level read, another written (mips: B6).
@group(0) @binding(90) var scene_working_tex: texture_2d<f32>;
@group(0) @binding(91) var scene_working_out: texture_storage_2d<rgba16float, write>;
// The bake array's layers, each read or written through its own view, so a
// pass may read some layers while it writes others.
@group(0) @binding(92) var scene_geometry_tex: texture_2d<f32>;
@group(0) @binding(93) var scene_coverage_tex: texture_2d<f32>;
@group(0) @binding(94) var scene_normals_layer_tex: texture_2d<f32>;
@group(0) @binding(95) var scene_near_tex: texture_2d<f32>;
@group(0) @binding(96) var scene_far_tex: texture_2d<f32>;
@group(0) @binding(97) var scene_geometry_out: texture_storage_2d<rgba16float, write>;
@group(0) @binding(98) var scene_coverage_out: texture_storage_2d<rgba16float, write>;
@group(0) @binding(99) var scene_normals_layer_out: texture_storage_2d<rgba16float, write>;
@group(0) @binding(100) var scene_near_out: texture_storage_2d<rgba16float, write>;
@group(0) @binding(101) var scene_far_out: texture_storage_2d<rgba16float, write>;

// The lens blur (B7, bake_focus.wgsl): its fit and the histogram it is
// fitted to.
@group(0) @binding(102) var<storage, read_write> scene_focus: SceneFocus;

struct SceneFocus {
    // The lens blur's Gaussian deviation in shorter edges at frame depth z,
    // σ(z) = x · |1/z − y| + z: x scales the defocus, y is 1 / z of the plane
    // in focus and z the blur everything has. w is 1 when the photo had
    // edges to measure, else 0 and the photo counts as sharp.
    fit: vec4<f32>,
    // Strong edges by depth (SCENE_FOCUS_DEPTH_BINS, even in 1/z from the
    // farthest surface to the nearest) and blur (SCENE_FOCUS_BLUR_BINS): their
    // summed contrast in fixed point (SCENE_FOCUS_WEIGHT_SCALE). Integer sums
    // do not depend on the order threads add them.
    // SCENE_FOCUS_DEPTH_BINS × SCENE_FOCUS_BLUR_BINS entries.
    histogram: array<atomic<u32>, 768>,
}

const SCENE_FOCUS_DEPTH_BINS: u32 = 32u;
const SCENE_FOCUS_BLUR_BINS: u32 = 24u;
// Blur bin k holds deviations from SCENE_FOCUS_LEAST_BLUR · 2^(k / 4) working
// texels up.
const SCENE_FOCUS_LEAST_BLUR: f32 = 0.25;
const SCENE_FOCUS_BINS_PER_OCTAVE: f32 = 4.0;
const SCENE_FOCUS_WEIGHT_SCALE: f32 = 256.0;

// The lens blur's deviation at frame depth `z`, in working texels; 0 before
// the photo was measured.
fn scene_focus_blur(z: f32) -> f32 {
    let fit = scene_focus.fit;
    if fit.w == 0.0 {
        return 0.0;
    }
    let short = f32(min(scene_bake.size.x, scene_bake.size.y));
    return (fit.x * abs(1.0 / z - fit.y) + fit.z) * short;
}

// The lens blur's deviation at frame depth `z`, in working texels, as
// `scene_focus_blur` gives it, for passes that read the uniform's copy of the
// fit.
fn scene_lens_blur(z: f32) -> f32 {
    let fit = scene_bake.focus_fit;
    if fit.w == 0.0 {
        return 0.0;
    }
    let short = f32(min(scene_bake.size.x, scene_bake.size.y));
    return (fit.x * abs(1.0 / z - fit.y) + fit.z) * short;
}

// 1 / z of the farthest surface: depth bins span [this, 1].
fn scene_focus_farthest_inverse() -> f32 {
    return 1.0 / scene_bake_frame_depth(1.0);
}

struct SceneBakeUniforms {
    // Working size (xy) and the disparity's size (zw), in texels.
    size: vec4<u32>,
    // The sky probability's size (xy) and the normals' size (zw).
    input_size: vec4<u32>,
    camera: Camera::SceneCamera,
    // The ground plane (B8, scene/ground.rs) in the camera frame at relief
    // scale 1: its unit normal facing up (xyz) and offset w, with n · X = w
    // for its points X.
    ground_plane: vec4<f32>,
    // The horizon of that plane: image points p, in shorter edges from the
    // image centre, with x · p.x + y · p.y + z = 0; w is 1 when the photo
    // shows a ground, else 0 and the rest unset.
    ground_horizon: vec4<f32>,
    // The lens blur's fit (`SceneFocus.fit`), copied here after B7 so that
    // effects, which bind no lens blur buffer, read it (`scene_lens_blur`).
    focus_fit: vec4<f32>,
}

// Layers of the bake array.
const SCENE_LAYER_GEOMETRY: u32 = 0u;
const SCENE_LAYER_COVERAGE: u32 = 1u;
const SCENE_LAYER_NORMALS: u32 = 2u;
const SCENE_LAYER_NEAR: u32 = 3u;
const SCENE_LAYER_FAR: u32 = 4u;

// Band codes, added: a depth edge and the sky's outline nearby.
const SCENE_BAND_DEPTH: u32 = 1u;
const SCENE_BAND_SKY: u32 = 2u;
// How far from an edge the band reaches, in working texels: the width of the
// depth model's own ramp across a silhouette (6–10 texels at 1540).
const SCENE_BAND_RADIUS: i32 = 8;
// How far from the sky's outline the band reaches. Coverage there follows the
// photo's own ramp (bake_coverage.wgsl), which a defocused tree line spreads
// over 20–30 texels at 1540, and the depth model's outline may lie 20 texels
// off its middle.
const SCENE_SKY_BAND_RADIUS: i32 = 32;
// Kinds of edge, added: a depth edge and the sky's outline (NEAR.w, FAR.w).
const SCENE_EDGE_DEPTH: u32 = 1u;
const SCENE_EDGE_SKY: u32 = 2u;
// A depth edge: the farthest surface within SCENE_BAND_RADIUS lies more than
// this fraction farther than the nearest, in the shading frame. A surface
// seen at a grazing angle may pass it too; its two halves then shade alike.
const SCENE_BAND_RELIEF: f32 = 0.1;
// The sky's outline: sky probability within SCENE_SKY_BAND_RADIUS spans past
// both.
const SCENE_SKY_EDGE: vec2<f32> = vec2<f32>(0.02, 0.98);
// A texel whose coverage lies beyond these shows one surface only: a sample
// of that surface for the layers (bake_layers.wgsl).
const SCENE_PURE_COVERAGE: vec2<f32> = vec2<f32>(0.05, 0.95);

fn scene_bake_size() -> vec2<i32> {
    return vec2<i32>(scene_bake.size.xy);
}

fn scene_bake_index(texel: vec2<i32>) -> u32 {
    return u32(texel.y) * scene_bake.size.x + u32(texel.x);
}

fn scene_bake_clamp(texel: vec2<i32>) -> vec2<i32> {
    return clamp(texel, vec2<i32>(0), scene_bake_size() - vec2<i32>(1));
}

// Full-image uv of a working texel's centre.
fn scene_bake_uv(texel: vec2<i32>) -> vec2<f32> {
    return (vec2<f32>(texel) + vec2<f32>(0.5)) / vec2<f32>(scene_bake_size());
}

// The guide coverage follows, of an unedited scene colour.
fn scene_bake_guide(rgb: vec3<f32>) -> vec3<f32> {
    return sqrt(max(rgb, vec3<f32>(0.0)));
}

// Whether coverage coefficients are constant 0 or 1: the texel shows one
// surface (bake_coverage.wgsl). Half floats hold both exactly.
fn scene_coverage_constant(coefficients: vec4<f32>) -> bool {
    return all(coefficients.xyz == vec3<f32>(0.0)) && (coefficients.w == 0.0 || coefficients.w == 1.0);
}

// The near surface's coverage at a point with guide `guide`, from coverage
// coefficients (the COVERAGE layer).
fn scene_coverage_of(coefficients: vec4<f32>, guide: vec3<f32>) -> f32 {
    return clamp(dot(coefficients.xyz, guide) + coefficients.w, 0.0, 1.0);
}

// Disparity at full-image `uv`, interpolated by hand: r32float is not
// filterable everywhere. Edges clamp.
fn scene_disparity_at(uv: vec2<f32>) -> f32 {
    let size = vec2<i32>(scene_bake.size.zw);
    let position = uv * vec2<f32>(size) - vec2<f32>(0.5);
    let base = vec2<i32>(floor(position));
    let t = position - vec2<f32>(base);
    let last = size - vec2<i32>(1);
    let p00 = clamp(base, vec2<i32>(0), last);
    let p11 = clamp(base + vec2<i32>(1), vec2<i32>(0), last);
    let top = mix(
        textureLoad(scene_disparity_tex, p00, 0).x,
        textureLoad(scene_disparity_tex, vec2<i32>(p11.x, p00.y), 0).x,
        t.x,
    );
    let bottom = mix(
        textureLoad(scene_disparity_tex, vec2<i32>(p00.x, p11.y), 0).x,
        textureLoad(scene_disparity_tex, p11, 0).x,
        t.x,
    );
    return mix(top, bottom, t.y);
}

// Linear depth d (x) and sky probability (y) at a working texel's centre,
// from the depth result's inputs (B1).
fn scene_texel_depth(texel: vec2<i32>) -> vec2<f32> {
    let uv = scene_bake_uv(texel);
    let d = Camera::scene_linear_depth(scene_bake.camera, scene_disparity_at(uv));
    let sky = textureSampleLevel(scene_sky_tex, scene_bake_sampler, uv, 0.0).x;
    return vec2<f32>(d, sky);
}

// The depth model's normal at full-image `uv`, interpolated, as a unit
// vector facing the camera.
fn scene_input_normal_at(uv: vec2<f32>) -> vec3<f32> {
    let stored = textureSampleLevel(scene_normals_tex, scene_bake_sampler, uv, 0.0).xy;
    // SceneNormals: round(127 n) + 128 per component, in 8 bits.
    let xy = clamp((stored * 255.0 - vec2<f32>(128.0)) / 127.0, vec2<f32>(-1.0), vec2<f32>(1.0));
    let z = -sqrt(max(1.0 - dot(xy, xy), 0.0));
    return normalize(vec3<f32>(xy, z));
}

// Frame depth (relief scale 1) of linear depth `d`.
fn scene_bake_frame_depth(d: f32) -> f32 {
    return Camera::scene_frame_depth(scene_bake.camera, d, 1.0);
}

// Octahedral encoding of a unit vector into [-1, 1]² (Cigolle et al., "A
// Survey of Efficient Representations for Independent Unit Vectors", 2014),
// with the hemisphere visible surfaces face (z < 0) unfolded.
fn scene_octahedral(n: vec3<f32>) -> vec2<f32> {
    let p = n.xy / (abs(n.x) + abs(n.y) + abs(n.z));
    if n.z > 0.0 {
        return (vec2<f32>(1.0) - abs(p.yx)) * scene_sign_not_zero(p);
    }
    return p;
}

fn scene_from_octahedral(e: vec2<f32>) -> vec3<f32> {
    let z = abs(e.x) + abs(e.y) - 1.0;
    var xy = e;
    if z > 0.0 {
        xy = (vec2<f32>(1.0) - abs(e.yx)) * scene_sign_not_zero(e);
    }
    return normalize(vec3<f32>(xy, z));
}

fn scene_sign_not_zero(v: vec2<f32>) -> vec2<f32> {
    return select(vec2<f32>(-1.0), vec2<f32>(1.0), v >= vec2<f32>(0.0));
}
