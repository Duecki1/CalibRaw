// The effect API: what a mask effect reads of the processing pipeline.
//
// An effect that runs slot by slot in the creative pass (`EffectPass::Creative` in
// the effect registry) defines
//
//     fn apply_<name>(pos: vec2<i32>, rgb: vec3<f32>, params: MaskEffectParams) -> vec3<f32>
//
// and returns the effect at full strength at tile pixel `pos`, given `rgb`, the image
// so far in scene-linear Rec.2020, which may be negative or above 1. The pass blends
// the result by the slot's mask. The effect reads its controls from `params` by the
// lane names its settings declare (`<NAME>_<LANE>_LANE`), and the photo and the scene
// through the functions below and the participating media in medium.wgsl, so the
// pipeline behind them can change without touching the effect. Relight shades the
// scene's surfaces and shares its shadows with the scene's lights, so it reads
// scene_depth.wgsl and scene_lights.wgsl directly, as Rain and Wet Surfaces do from
// their own passes. Shader IDs, lanes and the creative pass's dispatch are generated
// from the registry (effect_wgsl.rs).
//
// Everything here depends on full-image coordinates, the effect uniforms, global tone
// statistics and full-image maps, never on the crop or the export tile, so export
// tiles match the preview. An effect that reads neighbouring pixels declares how far
// (`EffectReach`), and export tiles carry a halo that wide.

// --- Controls ---

// An effect slot's packed parameters. Lane n is component n % 4 of `primary`,
// `secondary` or `tertiary` for n / 4 = 0, 1, 2; a colour takes three lanes.
// `options` holds per-slot switches (`<NAME>_OPTION`). `slot` is the slot's index
// and `instances` its instance and its card's instance count
// (`mask_effect_instance`).
struct MaskEffectParams {
    primary: vec4<f32>,
    secondary: vec4<f32>,
    tertiary: vec4<f32>,
    options: vec4<f32>,
    slot: u32,
    instances: u32,
}

fn mask_effect_params(index: u32) -> MaskEffectParams {
    return MaskEffectParams(
        Common::mask_data[index].adjust_0_field,
        Common::mask_data[index].adjust_1_field,
        Common::mask_data[index].adjust_2_field,
        Common::mask_data[index].effect_options,
        index,
        Common::mask_data[index].metadata.z,
    );
}

// Instances. An effect with several instances in one card (Relight's lights,
// Light Rays' sources) fills one slot per instance, consecutively and with the
// same mask (instances.rs in calibraw-core). A shader that loops over slots
// sees each instance as one more slot of the effect; an effect that combines
// its instances reads them all in the slot of the first and leaves the others
// alone. `metadata.z` of an effect slot holds the instance in its low eight
// bits and the count minus one above them (`effect_mask_data` in
// mask_params.rs), so it is 0 for an effect with one instance.

// The instance a slot holds: 0 for the first, and for an effect with one.
fn mask_effect_instance(params: MaskEffectParams) -> u32 {
    return params.instances & 0xffu;
}

// The light bake's channel of the slot's light plus one, or 0 when the bake
// holds none for it (common.wgsl, MASK_SCENE_BAKE_LIGHT_BITS).
fn mask_effect_scene_bake_light(params: MaskEffectParams) -> u32 {
    return Common::mask_data[params.slot].metadata.w & Common::MASK_SCENE_BAKE_LIGHT_BITS;
}

// The instances of the slot's card.
fn mask_effect_instance_count(params: MaskEffectParams) -> u32 {
    return (params.instances >> 8u) + 1u;
}

// The parameters of instance `instance` of the slot's card.
fn mask_effect_instance_params(params: MaskEffectParams, instance: u32) -> MaskEffectParams {
    return mask_effect_params(params.slot - mask_effect_instance(params) + instance);
}

fn mask_effect_lane(params: MaskEffectParams, lane: u32) -> f32 {
    let row = select(
        select(params.tertiary, params.secondary, lane < 8u),
        params.primary,
        lane < 4u,
    );
    return row[lane % 4u];
}

// A control of 0 to 100 as 0 to 1.
fn mask_effect_percent(params: MaskEffectParams, lane: u32) -> f32 {
    return clamp(mask_effect_lane(params, lane) / 100.0, 0.0, 1.0);
}

// Two consecutive lanes, such as a position's x and y.
fn mask_effect_lane_pair(params: MaskEffectParams, lane: u32) -> vec2<f32> {
    return vec2<f32>(mask_effect_lane(params, lane), mask_effect_lane(params, lane + 1u));
}

// The three lanes of a colour.
fn mask_effect_color(params: MaskEffectParams, lane: u32) -> vec3<f32> {
    return vec3<f32>(
        mask_effect_lane(params, lane),
        mask_effect_lane(params, lane + 1u),
        mask_effect_lane(params, lane + 2u),
    );
}

fn mask_effect_srgb_component_to_linear(value: f32) -> f32 {
    let encoded = clamp(value, 0.0, 1.0);
    if encoded <= 0.04045 {
        return encoded / 12.92;
    }
    return pow((encoded + 0.055) / 1.055, 2.4);
}

// A colour picker's sRGB colour in the working space.
fn mask_effect_picker_color_to_working(color: vec3<f32>) -> vec3<f32> {
    let linear_srgb = vec3<f32>(
        mask_effect_srgb_component_to_linear(color.r),
        mask_effect_srgb_component_to_linear(color.g),
        mask_effect_srgb_component_to_linear(color.b),
    );
    return max(Common::SRGB_TO_REC2020 * linear_srgb, vec3<f32>(0.0));
}

// --- Coordinates ---

// A tile pixel's position in the full image, clamped to it.
fn effect_image_pixel(pos: vec2<i32>) -> vec2<i32> {
    return clamp(pos + Common::tile_origin(), vec2<i32>(0), Common::full_image_max());
}

// A pixel's position over the full image, whatever the crop or export tile.
fn full_image_uv(pos: vec2<i32>) -> vec2<f32> {
    let dimensions = max(
        vec2<f32>(f32(Common::camera_uniforms.full_width), f32(Common::camera_uniforms.full_height)),
        vec2<f32>(1.0),
    );
    return (vec2<f32>(effect_image_pixel(pos)) + vec2<f32>(0.5)) / dimensions;
}

// A pixel relative to the image's centre, in units of its height.
fn effect_image_point(pos: vec2<i32>) -> vec2<f32> {
    let full_size = max(
        vec2<f32>(
            f32(Common::camera_uniforms.full_width),
            f32(Common::camera_uniforms.full_height),
        ),
        vec2<f32>(1.0),
    );
    var point = full_image_uv(pos) - vec2<f32>(0.5);
    point.x = point.x * full_size.x / full_size.y;
    return point;
}

// The full image's shorter edge in pixels.
fn effect_short_edge() -> f32 {
    return max(
        f32(min(Common::camera_uniforms.full_width, Common::camera_uniforms.full_height)),
        1.0,
    );
}

// Shorter-edge units per full-image pixel.
fn scene_pixel_extent() -> f32 {
    return 1.0 / effect_short_edge();
}

// The developed output's size in pixels: the frame after crop, rotation and
// perspective correction.
fn effect_output_size() -> vec2<f32> {
    return max(Common::effects_uniforms.vignette_frame.xy, vec2<f32>(1.0));
}

// A pixel's position in the developed output, 0 to 1 across it. The divisor is
// positive across the photo, whose perspective correction keeps its vanishing line
// outside the frame.
fn effect_output_uv(pos: vec2<i32>) -> vec2<f32> {
    let source = vec3<f32>(full_image_uv(pos), 1.0);
    let projection = Common::effects_uniforms.vignette_projection;
    return vec2<f32>(dot(projection[0].xyz, source), dot(projection[1].xyz, source))
        / dot(projection[2].xyz, source);
}

// --- The photo ---

fn effect_luma(rgb: vec3<f32>) -> f32 {
    return Common::safe_luma(rgb);
}

// The image at `pos` in tile pixels, bilinearly filtered.
fn mask_effect_source_linear_at(pos: vec2<f32>) -> vec3<f32> {
    // Manual bilinear filtering also works with the non-filterable 32-bit
    // working texture used for high-quality processing.
    let base = vec2<i32>(floor(pos));
    let fraction = pos - vec2<f32>(base);
    let top = mix(
        SceneAdjustments::local_effects_at(base),
        SceneAdjustments::local_effects_at(base + vec2<i32>(1, 0)),
        fraction.x,
    );
    let bottom = mix(
        SceneAdjustments::local_effects_at(base + vec2<i32>(0, 1)),
        SceneAdjustments::local_effects_at(base + vec2<i32>(1, 1)),
        fraction.x,
    );
    return mix(top, bottom, fraction.y);
}

// --- The scene ---
// An effect that reads the scene's geometry, its working image or its lights names it
// in its registry row (`scene: [...]`) and reads it through scene/geometry.wgsl
// (`Geometry::`, medium.wgsl and scene_lights.wgsl for what scenes lights do).

// The distance to assume until scene depth is generated.
const EFFECT_SCENE_DEPTH_UNKNOWN: f32 = SCENE_DEPTH_UNKNOWN;

// Scene-linear middle grey at the photo's ambient light and current exposure.
fn effect_ambient() -> f32 {
    return scene_ambient_level();
}

// Whether any effect places a light in the scene (Relight, Light Rays).
fn effect_scene_lights_present() -> bool {
    return scene_lights_present();
}

// --- Noise ---

// Integer hash finalizer shared by the procedural effects: mixes `seed` and
// maps its low 24 bits to [0, 1].
fn mask_effect_hash_unit(seed: u32) -> f32 {
    var h = (seed ^ (seed >> 16u)) * 2246822519u;
    h = (h ^ (h >> 13u)) * 3266489917u;
    h = h ^ (h >> 16u);
    return f32(h & 0x00ffffffu) / 16777215.0;
}

// Four uniform values for a cell of a procedural pattern (pcg2d).
fn effect_cell_random(cell: vec2<i32>, salt: u32) -> vec4<f32> {
    var v = bitcast<vec2<u32>>(cell) * 1664525u + vec2<u32>(1013904223u, salt);
    v.x = v.x + v.y * 1664525u;
    v.y = v.y + v.x * 1664525u;
    v = v ^ (v >> vec2<u32>(16u));
    v.x = v.x + v.y * 1664525u;
    v.y = v.y + v.x * 1664525u;
    v = v ^ (v >> vec2<u32>(16u));
    return vec4<f32>(
        f32(v.x & 0xffffu),
        f32(v.x >> 16u),
        f32(v.y & 0xffffu),
        f32(v.y >> 16u),
    ) / 65535.0;
}

fn effect_hash(position: vec2<f32>) -> f32 {
    let signed_cell = vec2<i32>(position);
    let cell = bitcast<vec2<u32>>(signed_cell);
    return mask_effect_hash_unit(cell.x * 1597334677u ^ cell.y * 3812015801u);
}

// Value noise in [0, 1].
fn effect_noise(position: vec2<f32>) -> f32 {
    let cell = floor(position);
    let fraction = position - cell;
    let smooth_fraction = fraction * fraction * (vec2<f32>(3.0) - 2.0 * fraction);
    let bottom = mix(
        effect_hash(cell),
        effect_hash(cell + vec2<f32>(1.0, 0.0)),
        smooth_fraction.x,
    );
    let top = mix(
        effect_hash(cell + vec2<f32>(0.0, 1.0)),
        effect_hash(cell + vec2<f32>(1.0, 1.0)),
        smooth_fraction.x,
    );
    return mix(bottom, top, smooth_fraction.y);
}

// Octaves of `effect_noise`, each rotated and finer, weighted by powers of `gain`.
fn effect_fbm_octaves(position: vec2<f32>, octaves: u32, gain: f32) -> f32 {
    var point = position;
    var amplitude = 0.5;
    var value = 0.0;
    var normalization = 0.0;
    for (var octave = 0u; octave < octaves; octave = octave + 1u) {
        value = value + effect_noise(point) * amplitude;
        normalization = normalization + amplitude;
        point = vec2<f32>(
            point.x * 1.62 + point.y * 1.17,
            point.x * -1.17 + point.y * 1.62,
        ) + vec2<f32>(13.7, 9.2);
        amplitude = amplitude * gain;
    }
    return value / max(normalization, 1e-6);
}

fn effect_fbm(position: vec2<f32>) -> f32 {
    return effect_fbm_octaves(position, 5u, 0.52);
}

fn effect_hash3(cell: vec3<i32>) -> f32 {
    let p = bitcast<vec3<u32>>(cell);
    return mask_effect_hash_unit(p.x * 1597334677u ^ p.y * 3812015801u ^ p.z * 2798796415u);
}

// Value noise in [0, 1] over a volume.
fn effect_noise3(point: vec3<f32>) -> f32 {
    let cell = vec3<i32>(floor(point));
    let f = point - vec3<f32>(cell);
    let u = f * f * f * (f * (f * 6.0 - vec3<f32>(15.0)) + vec3<f32>(10.0));
    let a = mix(
        mix(effect_hash3(cell), effect_hash3(cell + vec3<i32>(1, 0, 0)), u.x),
        mix(effect_hash3(cell + vec3<i32>(0, 1, 0)), effect_hash3(cell + vec3<i32>(1, 1, 0)), u.x), u.y,
    );
    let b = mix(
        mix(effect_hash3(cell + vec3<i32>(0, 0, 1)), effect_hash3(cell + vec3<i32>(1, 0, 1)), u.x),
        mix(effect_hash3(cell + vec3<i32>(0, 1, 1)), effect_hash3(cell + vec3<i32>(1, 1, 1)), u.x), u.y,
    );
    return mix(a, b, u.z);
}
