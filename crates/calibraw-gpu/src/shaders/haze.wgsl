#import calibraw::common as Common
#import calibraw::color as Color

// Dehaze removes or adds atmospheric haze with the scattering model
//   I = J * t + A * (1 - t),
// where I is the photographed scene, J the scene without haze, A the airlight
// (the colour the veil tends to with distance) and t the transmission, the
// share of J's light that reaches the camera. haze_analysis.wgsl estimates A
// for the whole image and t on a coarse grid; this module applies them per
// pixel.
//
// Units: the profiled scene (SceneSource::profiled_scene_at), scene-linear
// Rec.2020 before the edit's exposure. Measurements use it without negative
// channels (Color::map_negative_gamut). Dehaze runs at the start of the scene
// preparation, so exposure and every tone and colour edit act on the dehazed
// scene, and the edit's exposure scales the result like any other light.
//
// An amount a in [-1, 1] scales the scattering coefficient by 1 - a. The
// transmission becomes t^(1 - a), so
//   I' = A + (I - A) * t^(-a):
// a = 1 recovers J, a = 0 leaves I, and a < 0 thickens the veil, which then
// tends to a dimmed airlight (`added_veil`). Global and mask amounts therefore
// add up in the exponent. Removal is bounded where the model's estimate is
// unreliable: see haze_removal_transmission.

struct HazeAirlight {
    // Airlight colour (xyz) and luminance (w). A luminance of zero means no
    // haze was found; Dehaze then leaves the photo unchanged.
    color: vec4<f32>,
    // The veil negative amounts thicken towards (xyz) and its luminance (w):
    // the airlight, dimmed to at most a few stops above the scene's median
    // (haze_analysis.wgsl).
    added_veil: vec4<f32>,
}

// Cells of the haze grid along the full image's longer edge. Matches
// HAZE_GRID_LONG in Rust, which sizes the grid's textures and buffer for a
// square grid.
const HAZE_GRID_LONG: u32 = 160u;

// Guide values are limited to this; removal has faded out there.
const HAZE_GUIDE_MAX: f32 = 2.0;

// Full removal divides the scene's departure from the airlight by at most
// 1 / HAZE_MIN_TRANSMISSION, which bounds the noise it amplifies in dense haze.
const HAZE_MIN_TRANSMISSION: f32 = 0.33;
// Veils below HAZE_CLEAR_VEIL are left alone and removal ramps in up to
// HAZE_HAZY_VEIL, so photos and regions without haze keep their tones.
const HAZE_CLEAR_VEIL: f32 = 0.05;
const HAZE_HAZY_VEIL: f32 = 0.2;
// Share of a pixel's darkest channel, relative to the airlight, that removal
// may take away. A pixel holds no more veil than that channel, so no channel
// turns negative; keeping a fifth of it stops grey cloud and dark detail
// inside haze from being crushed to black and saturation from running away.
const HAZE_PIXEL_VEIL_LIMIT: f32 = 0.8;
// Within this departure from the airlight, relative to its luminance, a
// pixel's difference from it is mostly sky, noise or a faint tint rather than
// attenuated detail, so removal is scaled back in proportion: amplifying it
// would only add noise and colour casts. Detail in dense haze departs further,
// by its transmission times its own difference from the airlight.
const HAZE_AIRLIGHT_TOLERANCE: f32 = 0.15;
// Removal fades out between these guide values. Light sources and the sun
// outshine any veil, and steepening them would only brighten them.
const HAZE_EMITTER_FADE_START: f32 = 1.0;
const HAZE_EMITTER_FADE_END: f32 = HAZE_GUIDE_MAX;
// Negative amounts thicken the veil to at least this transmission, so a photo
// without haze still gains a uniform one; hazier regions gain more, down to
// HAZE_MIN_TRANSMISSION.
const HAZE_ADDED_TRANSMISSION: f32 = 0.7;

@group(0) @binding(54) var<storage, read> haze_airlight: HazeAirlight;
// Per grid cell, the guided filter's slope (x) and offset (y), giving the
// transmission as slope * guide + offset, and the share of the veil removal
// may take (z) (haze_analysis.wgsl). Only the cells of `haze_grid` are valid.
@group(0) @binding(60) var haze_map_tex: texture_2d<f32>;

struct HazeEstimate {
    // The share of the scene's light that reaches the camera.
    transmission: f32,
    // The share of the veil removal may take: less where the haze hides no
    // detail.
    removal_share: f32,
}

// Cells of the haze grid across and down the full image, at least one pixel
// each so that tiny images leave no cell empty.
fn haze_grid() -> vec2<u32> {
    let full = vec2<u32>(
        max(Common::camera_uniforms.full_width, 1u),
        max(Common::camera_uniforms.full_height, 1u),
    );
    return min(Common::full_image_grid(HAZE_GRID_LONG), full);
}

// What the transmission follows across edges: luminance relative to the
// airlight's, for colours without negative channels.
fn haze_guide(rgb: vec3<f32>, airlight: vec4<f32>) -> f32 {
    return clamp(dot(rgb, Common::LUMA) / airlight.w, 0.0, HAZE_GUIDE_MAX);
}

// Grid coordinates of a tile pixel's centre, with cell centres at integers.
fn haze_grid_position(pos: vec2<i32>) -> vec2<f32> {
    let full = vec2<f32>(
        f32(max(Common::camera_uniforms.full_width, 1u)),
        f32(max(Common::camera_uniforms.full_height, 1u)),
    );
    let global_center = vec2<f32>(pos + Common::tile_origin()) + vec2<f32>(0.5);
    return global_center * vec2<f32>(haze_grid()) / full - vec2<f32>(0.5);
}

// The haze at a tile pixel with guide value `guide`: the map interpolated
// between the four nearest cell centres, its guided filter evaluated against
// the pixel's own guide.
fn haze_estimate_at(pos: vec2<i32>, guide: f32) -> HazeEstimate {
    let last = vec2<i32>(haze_grid()) - vec2<i32>(1);
    let position = clamp(haze_grid_position(pos), vec2<f32>(0.0), vec2<f32>(last));
    let base = vec2<i32>(floor(position));
    let next = min(base + vec2<i32>(1), last);
    let f = position - vec2<f32>(base);
    let top = mix(
        textureLoad(haze_map_tex, base, 0).xyz,
        textureLoad(haze_map_tex, vec2<i32>(next.x, base.y), 0).xyz,
        f.x,
    );
    let bottom = mix(
        textureLoad(haze_map_tex, vec2<i32>(base.x, next.y), 0).xyz,
        textureLoad(haze_map_tex, next, 0).xyz,
        f.x,
    );
    let map = mix(top, bottom, f.y);
    return HazeEstimate(clamp(map.x * guide + map.y, 0.0, 1.0), map.z);
}

// The transmission that removal divides by: its share of the mapped veil, at
// most what the pixel's own darkest channel allows, kept only where that is
// clearly haze, faded out for light sources, floored to bound the gain and
// raised close to the airlight.
fn haze_removal_transmission(
    measured: vec3<f32>,
    airlight: vec4<f32>,
    guide: f32,
    estimate: HazeEstimate,
) -> f32 {
    let relative = measured / airlight.xyz;
    let pixel_limit = HAZE_PIXEL_VEIL_LIMIT * min(relative.r, min(relative.g, relative.b));
    var veil = min((1.0 - estimate.transmission) * estimate.removal_share, pixel_limit);
    veil = veil * smoothstep(HAZE_CLEAR_VEIL, HAZE_HAZY_VEIL, veil);
    veil = veil * (1.0 - smoothstep(HAZE_EMITTER_FADE_START, HAZE_EMITTER_FADE_END, guide));
    let difference = abs(measured - airlight.xyz) / airlight.w;
    let departure = max(difference.r, max(difference.g, difference.b));
    let tolerance = max(1.0, HAZE_AIRLIGHT_TOLERANCE / max(departure, 1e-6));
    return min(max(1.0 - veil, HAZE_MIN_TRANSMISSION) * tolerance, 1.0);
}

// Removes haze for `amount` > 0 and adds it for `amount` < 0. `amount` is the
// combined perceptual amount of global and mask Dehaze, in [-1, 1].
fn apply_dehaze(pos: vec2<i32>, rgb: vec3<f32>, amount: f32) -> vec3<f32> {
    let airlight = haze_airlight.color;
    if abs(amount) < 1e-6 || airlight.w <= 0.0 {
        return rgb;
    }
    let measured = Color::map_negative_gamut(rgb);
    let guide = haze_guide(measured, airlight);
    let estimate = haze_estimate_at(pos, guide);
    if amount > 0.0 {
        let removal = haze_removal_transmission(measured, airlight, guide, estimate);
        return airlight.xyz + (rgb - airlight.xyz) * pow(removal, -amount);
    }
    let veil = haze_airlight.added_veil.xyz;
    let added = clamp(estimate.transmission, HAZE_MIN_TRANSMISSION, HAZE_ADDED_TRANSMISSION);
    return veil + (rgb - veil) * pow(added, -amount);
}
