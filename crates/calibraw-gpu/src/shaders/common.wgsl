const MAX_RENDER_MASK_SLOTS: u32 = 428u;


// Naga reserves trailing numeric suffixes when rewriting composable modules.
// Exported struct member names must therefore not end in a digit.
struct MaskData {
    metadata: vec4<u32>,
    adjust_0_field: vec4<f32>,
    adjust_1_field: vec4<f32>,
    adjust_2_field: vec4<f32>,
    effect_options: vec4<f32>,
    curves: array<vec4<f32>, 9>,
    grade_shadows: vec4<f32>,
    grade_midtones: vec4<f32>,
    grade_highlights: vec4<f32>,
    grade_global: vec4<f32>,
    grade_options: vec4<f32>,
    curves_red: array<vec4<f32>, 9>,
    curves_green: array<vec4<f32>, 9>,
    curves_blue: array<vec4<f32>, 9>,
    hsl_hue_0_field: vec4<f32>,
    hsl_hue_1_field: vec4<f32>,
    hsl_saturation_0_field: vec4<f32>,
    hsl_saturation_1_field: vec4<f32>,
    hsl_luminance_0_field: vec4<f32>,
    hsl_luminance_1_field: vec4<f32>,
    point_colors: array<PointColor, 8>,
    point_color_meta: vec4<u32>,
}

// `CameraUniforms.view_transform_mode` (Rust `ViewTransformMode`).
// A developed raster at the reference tone curve keeps its rendered values.
const VIEW_TRANSFORM_PASSTHROUGH: u32 = 0u;
// Scene radiance takes the sigmoid view transform.
const VIEW_TRANSFORM_SCENE_SIGMOID: u32 = 1u;
// A developed raster is retoned from the reference sigmoid to the edit's sigmoid.
const VIEW_TRANSFORM_DISPLAY_RETONE: u32 = 2u;

// `metadata.w` packs the mask effect's shader ID above eight flag bits. Of
// these, an effect slot's lowest four hold its light's channel of the light
// bake plus one, or 0 (`assign_scene_bake_lights` in mask_params.rs).
const MASK_EFFECT_ID_SHIFT: u32 = 8u;
const MASK_SCENE_BAKE_LIGHT_BITS: u32 = 0xfu;

fn mask_effect_id(metadata: vec4<u32>) -> u32 {
    return metadata.w >> MASK_EFFECT_ID_SHIFT;
}

// Mask slots in use, the bound of every loop over `mask_data`.
fn mask_slot_count() -> u32 {
    return min(scene_tone_uniforms.mask_counts.x, MAX_RENDER_MASK_SLOTS);
}

// Whether the slot is enabled and changes the image: an adjustment that is not
// neutral, or an active effect. Rust: `MaskData::is_active`.
fn mask_slot_active(metadata: vec4<u32>) -> bool {
    return metadata.x != 0u && metadata.y != 0u;
}

// Whether the slot is active and holds `effect_id` (0 for adjustment masks).
// Rust: `MaskData::applies`.
fn mask_slot_applies(metadata: vec4<u32>, effect_id: u32) -> bool {
    return mask_slot_active(metadata) && mask_effect_id(metadata) == effect_id;
}

struct CameraUniforms {
    // Scalar fields keep the following vec4 on the Rust/WGSL 16-byte boundary.
    black_point: f32,
    highlight_clip: f32,
    chroma_denoise: f32,
    highlight_reconstruction: f32,
    tone_analysis_scale: f32,
    tone_guide_radius: f32,
    demosaic_mode: f32,
    dual_threshold: f32,
    frequency_chroma: f32,
    // One of the VIEW_TRANSFORM_* modes.
    view_transform_mode: u32,
    padding_0_field: u32,
    padding_1_field: u32,
    highlight_options: vec4<f32>,
    noise_shot: vec4<f32>,
    noise_read: vec4<f32>,
    noise_options: vec4<f32>,
    wb: vec4<f32>,
    cam_to_srgb_0_field: vec4<f32>,
    cam_to_srgb_1_field: vec4<f32>,
    cam_to_srgb_2_field: vec4<f32>,
    black_levels: vec4<f32>,
    white_levels: vec4<f32>,
    width: u32,
    height: u32,
    tile_origin_x: i32,
    tile_origin_y: i32,
    full_width: u32,
    full_height: u32,
    abi_version: u32,
    abi_size_bytes: u32,
    tone_histogram_bounds: vec4<u32>,
    profile_hue_sat: vec4<u32>,
    profile_look: vec4<u32>,
    profile_tone: vec4<u32>,
    profile_flags: vec4<u32>,
    ai_denoise_enabled: u32,
    user_exposure_bits: u32,
    ai_cfa_weight: f32,
    // Shorter edge in pixels of the whole photo at its native resolution;
    // full_width and full_height give the processing resolution.
    source_short_edge: u32,
    // Chromatic aberration (chromatic_aberration.wgsl): (amount in [0, 1],
    // then the gains that white-balance the scene's red, green and blue), then
    // the photo's lateral model of red and of blue, [k1, k2, tx, ty] scaled by
    // the amount.
    chromatic_aberration: vec4<f32>,
    lateral_ca_red: vec4<f32>,
    lateral_ca_blue: vec4<f32>,
    // Film negative (film_positive in scene_source.wgsl): the base in scene
    // texture units, then the mode (FILM_MODE_*); each channel's exponent,
    // then the print exposure in EV; and the gains of the positive.
    film_base: vec4<f32>,
    film_exponents: vec4<f32>,
    film_gains: vec4<f32>,
}

struct SceneToneUniforms {
    exposure: f32,
    saturation: f32,
    vibrance: f32,
    // Keeps the next vector aligned.
    _padding: u32,
    basic_tone: vec4<f32>,
    // The edit's sigmoid: (white target, black target, log2 paper exposure, film fog)
    // and (film power, paper power, hue preservation, colour processing).
    sigmoid_curve: vec4<f32>,
    sigmoid_power: vec4<f32>,
    // The sigmoid of default settings, packed the same way. A developed raster is
    // taken to be rendered with it, so VIEW_TRANSFORM_DISPLAY_RETONE inverts it.
    reference_sigmoid_curve: vec4<f32>,
    reference_sigmoid_power: vec4<f32>,
    // Master, red, green and blue point curves. Blocks 0-7 hold point pairs
    // (x0, y0, x1, y1) in curve-encoded [0, 1]; block 8 is (count, identity, 0, 0).
    tone_curves: array<array<vec4<f32>, 9>, 4>,
    hsl_hue_0_field: vec4<f32>,
    hsl_hue_1_field: vec4<f32>,
    hsl_saturation_0_field: vec4<f32>,
    hsl_saturation_1_field: vec4<f32>,
    hsl_luminance_0_field: vec4<f32>,
    hsl_luminance_1_field: vec4<f32>,
    mask_counts: vec4<u32>,
    grade_shadows: vec4<f32>,
    grade_midtones: vec4<f32>,
    grade_highlights: vec4<f32>,
    grade_global: vec4<f32>,
    grade_options: vec4<f32>,
    // WGSL uses a 16-byte vec3 matrix-column stride; Rust mirrors padded columns.
    rec2020_to_xyz: mat3x3<f32>,
    xyz_to_rec2020_field: mat3x3<f32>,
    xyz_to_bradford: mat3x3<f32>,
    bradford_to_xyz: mat3x3<f32>,
    point_colors: array<PointColor, 8>,
    point_color_meta: vec4<u32>,
    // The edit's LUT (look_lut.wgsl): amount (0 without a LUT), lattice points per
    // axis, 0, 0; the domain minimum; the reciprocal of the domain extent.
    look_lut: vec4<f32>,
    look_lut_domain_min: vec4<f32>,
    look_lut_domain_scale: vec4<f32>,
    // The Temperature of a rendered photo, -100 (cool) to +100 (warm), 0 on any
    // other photo; then 0, 0, 0.
    raster_temperature: vec4<f32>,
}

struct PointColor {
    sample_range: vec4<f32>,
    hue_range: vec4<f32>,
    saturation_range: vec4<f32>,
    luminance_range: vec4<f32>,
    shifts: vec4<f32>,
}

struct EffectsUniforms {
    // Texture, clarity, dehaze, reserved.
    presence: vec4<f32>,
    // Sharpening amount, radius, detail, masking.
    sharpen: vec4<f32>,
    // Radius of the shared Glow diffusion, reserved, reserved, reserved.
    glow_diffusion: vec4<f32>,
    // The frame and calibration of Vignette effects: the output width and height, two reserved,
    // then the rows of the homography from normalized source to output position (0–1 across
    // the output), each with one reserved.
    vignette_frame: vec4<f32>,
    vignette_projection: array<vec4<f32>, 3>,
    vignette_dark_half_fit: vec4<f32>,
    vignette_dark_full_fit: vec4<f32>,
    vignette_light_half_fit: vec4<f32>,
    vignette_light_full_fit: vec4<f32>,
    capture_scale_sigma: vec4<f32>,
    capture_thresholds: vec4<f32>,
    capture_mask_coherence: vec4<f32>,
}

@group(0) @binding(0) var<uniform> camera_uniforms: CameraUniforms;
@group(1) @binding(0) var<uniform> scene_tone_uniforms: SceneToneUniforms;
@group(2) @binding(0) var<uniform> effects_uniforms: EffectsUniforms;

@group(0) @binding(33) var<storage, read> mask_data: array<MaskData>;


@group(0) @binding(1) var raw_tex: texture_2d<u32>;
@group(0) @binding(2) var color_tex: texture_2d<u32>;
@group(0) @binding(19) var black_tex: texture_2d<f32>;

const LUMA: vec3<f32> = vec3<f32>(0.2627002, 0.6779981, 0.0593017);
const SRGB_LUMA: vec3<f32> = vec3<f32>(0.2126729, 0.7151522, 0.0721750);

const REC2020_TO_SRGB: mat3x3<f32> = mat3x3<f32>(
    vec3<f32>( 1.6604910, -0.1245505, -0.0181508),
    vec3<f32>(-0.5876411,  1.1328999, -0.1005789),
    vec3<f32>(-0.0728499, -0.0083494,  1.1187297),
);

const SRGB_TO_REC2020: mat3x3<f32> = mat3x3<f32>(
    vec3<f32>(0.6274039, 0.0690973, 0.0163914),
    vec3<f32>(0.3292830, 0.9195404, 0.0880133),
    vec3<f32>(0.0433131, 0.0113623, 0.8955953),
);

fn image_max() -> vec2<i32> {
    return vec2<i32>(i32(camera_uniforms.width) - 1, i32(camera_uniforms.height) - 1);
}

fn tile_origin() -> vec2<i32> {
    return vec2<i32>(camera_uniforms.tile_origin_x, camera_uniforms.tile_origin_y);
}

fn full_image_max() -> vec2<i32> {
    return vec2<i32>(i32(camera_uniforms.full_width) - 1, i32(camera_uniforms.full_height) - 1);
}

fn processing_short_edge() -> f32 {
    return f32(max(min(camera_uniforms.full_width, camera_uniforms.full_height), 1u));
}

fn source_short_edge() -> f32 {
    return select(
        processing_short_edge(),
        f32(camera_uniforms.source_short_edge),
        camera_uniforms.source_short_edge > 0u,
    );
}

// Processing pixels per pixel of the photo at its native resolution: 1 in an
// export, less in a reduced preview. A limit in native pixels times this is
// the same limit at the processing resolution.
fn processing_per_source_pixel() -> f32 {
    return processing_short_edge() / source_short_edge();
}

// Pixels per reference pixel for effects sized against a 1080-pixel short edge.
// The scale is set by the photo at its native resolution, limited to
// [minimum, maximum] there so export tiles keep bounded halos, and then
// carried to the processing resolution. A reduced preview therefore uses the
// export's radii in proportion instead of limits of its own.
fn resolution_scale(minimum: f32, maximum: f32) -> f32 {
    return clamp(source_short_edge() / 1080.0, minimum, maximum)
        * processing_per_source_pixel();
}

// Cells across and down a coarse grid over the full image with `long_cells`
// cells along its longer edge. Cells cover the same share of the photo at
// every processing resolution and tile origin.
fn full_image_grid(long_cells: u32) -> vec2<u32> {
    let full = vec2<f32>(
        f32(max(camera_uniforms.full_width, 1u)),
        f32(max(camera_uniforms.full_height, 1u)),
    );
    let cells = round(f32(long_cells) * full / max(full.x, full.y));
    return clamp(vec2<u32>(cells), vec2<u32>(1u), vec2<u32>(long_cells));
}

// Global pixels whose centres fall in grid cells [cell, cell + 1) along one
// axis of `cells` cells over `full` pixels.
fn full_image_grid_pixel_span(cell: u32, cells: u32, full: u32) -> vec2<i32> {
    let scale = f32(full) / f32(cells);
    return vec2<i32>(
        i32(ceil(f32(cell) * scale - 0.5)),
        i32(ceil(f32(cell + 1u) * scale - 0.5)),
    );
}

// Image lights (tone_analysis.wgsl): a coarse grid over the full image with
// IMAGE_LIGHT_GRID_LONG cells along its longer edge, stored in textures and
// a buffer sized for a square grid. Matches IMAGE_LIGHT_GRID_LONG in Rust.
const IMAGE_LIGHT_GRID_LONG: u32 = 160u;

// Cells of the image-light grid across and down the full image.
fn image_light_grid() -> vec2<u32> {
    return full_image_grid(IMAGE_LIGHT_GRID_LONG);
}

// Contours (tone_analysis.wgsl `accumulate_contours`): how much of each cell of a
// coarse full-image grid lies on the photo's contours, for effects that emit light
// along them (Neon, scene_emission.wgsl). A cell counts its sampled pixels by the
// contrast across them and the brightness of their brighter side, so the reader
// applies its own threshold, line width and exposure. CONTOUR_GRID_LONG cells along
// the longer edge, in a buffer sized for a square grid. Match the constants of the
// same names in Rust.
const CONTOUR_GRID_LONG: u32 = 64u;
const CONTOUR_CONTRAST_BINS: u32 = 12u;
const CONTOUR_BRIGHTNESS_BINS: u32 = 8u;
// CONTOUR_CONTRAST_BINS × CONTOUR_BRIGHTNESS_BINS.
const CONTOUR_BINS: u32 = 96u;
// The bin counts, four per vector, then the cell's sample count in the last
// vector's x and the tap spacing in reference pixels in its y.
const CONTOUR_CELL_VECTORS: u32 = 25u;

struct ContourCell {
    // Bin `contrast * CONTOUR_BRIGHTNESS_BINS + brightness` is component
    // `bin % 4` of vector `bin / 4`.
    bins: array<vec4<f32>, CONTOUR_CELL_VECTORS>,
}

// Contrast is Neon's measure (neon.wgsl): the Sobel response of log2 luminance,
// half a stop's difference per stop across a step. Bins start at
// CONTOUR_CONTRAST_MIN and widen by 2^(2/3) each; lower contrast is not binned.
const CONTOUR_CONTRAST_MIN: f32 = 0.01;
const CONTOUR_CONTRAST_STOPS_PER_BIN: f32 = 0.6666667;
// Brightness of the brighter side in unexposed scene-linear luminance: two stops
// per bin, from 12 stops below middle grey.
const CONTOUR_BRIGHTNESS_LOWEST_EV: f32 = -12.0;
const CONTOUR_BRIGHTNESS_STOPS_PER_BIN: f32 = 2.0;
const CONTOUR_MIDDLE_GREY: f32 = 0.18;
// Samples per cell edge at most; larger cells sample a lattice.
const CONTOUR_SAMPLES_PER_CELL_EDGE: i32 = 16;

// Cells of the contour grid across and down the full image.
fn contour_grid() -> vec2<u32> {
    return full_image_grid(CONTOUR_GRID_LONG);
}

// The bin of `contrast`, or -1 below the lowest.
fn contour_contrast_bin(contrast: f32) -> i32 {
    if contrast < CONTOUR_CONTRAST_MIN { return -1; }
    let bin = floor(log2(contrast / CONTOUR_CONTRAST_MIN) / CONTOUR_CONTRAST_STOPS_PER_BIN);
    return i32(min(bin, f32(CONTOUR_CONTRAST_BINS - 1u)));
}

// The geometric centre of contrast bin `bin`.
fn contour_contrast_at(bin: u32) -> f32 {
    return CONTOUR_CONTRAST_MIN * exp2((f32(bin) + 0.5) * CONTOUR_CONTRAST_STOPS_PER_BIN);
}

fn contour_brightness_bin(luminance: f32) -> u32 {
    let ev = log2(max(luminance, 1e-12) / CONTOUR_MIDDLE_GREY);
    let bin = floor((ev - CONTOUR_BRIGHTNESS_LOWEST_EV) / CONTOUR_BRIGHTNESS_STOPS_PER_BIN);
    return u32(clamp(bin, 0.0, f32(CONTOUR_BRIGHTNESS_BINS - 1u)));
}

// The luminance at the centre of brightness bin `bin`, two stops wide.
fn contour_brightness_at(bin: u32) -> f32 {
    let ev = CONTOUR_BRIGHTNESS_LOWEST_EV + (f32(bin) + 0.5) * CONTOUR_BRIGHTNESS_STOPS_PER_BIN;
    return CONTOUR_MIDDLE_GREY * exp2(ev);
}

// The Sobel spacing contrast is measured at, in whole pixels: Neon's at an
// Edge width of 1 (`mask_focus_blur_radius`). Readers scale the share of a
// cell on a line by their own spacing over this one.
fn contour_radius() -> i32 {
    return max(i32(round(resolution_scale(0.55, 3.0))), 1);
}

// Pixels between samples along a cell edge `span` pixels long.
fn contour_sample_step(span: i32) -> i32 {
    return max(span / CONTOUR_SAMPLES_PER_CELL_EDGE, 1);
}

// The full image in shorter-edge units, the unit the scene's image points are in.
fn scene_image_size() -> vec2<f32> {
    let full = max(
        vec2<f32>(f32(camera_uniforms.full_width), f32(camera_uniforms.full_height)),
        vec2<f32>(1.0),
    );
    return full / min(full.x, full.y);
}

fn clamp_pos(pos: vec2<i32>) -> vec2<i32> {
    return clamp(pos, vec2<i32>(0, 0), image_max());
}

/// Unnormalised 1-4-6-4-1 binomial weight for a tap at `offset` in [-2, 2].
fn binomial5_weight(offset: i32) -> f32 {
    let a = abs(offset);
    if a == 0 { return 6.0; }
    if a == 1 { return 4.0; }
    return 1.0;
}

fn safe_luma(rgb: vec3<f32>) -> f32 {
    return max(dot(rgb, LUMA), 1e-6);
}

/// Base-2 log of the Rec.2020 luminance, floored at 1e-6 to stay finite.
fn log_luminance(rgb: vec3<f32>) -> f32 {
    return log2(safe_luma(rgb));
}
