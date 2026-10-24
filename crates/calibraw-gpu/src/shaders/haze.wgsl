#import calibraw::common as Common
#import calibraw::color as Color
#import calibraw::scene_source as SceneSource

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
// unreliable: see haze_removal_transmission. Positive amounts also add depth
// (haze_deepened), which a clear photo shows most and dense haze, already
// steepened by removal, not at all.
//
// Every decision about t reads the guide scene, the profiled scene blurred
// over a few pixels short of edges (haze_guide_scene_at), never the pixel
// alone. The gain t^(-a) is therefore smooth across the image and the pixel
// enters the model linearly, so its noise is amplified by the gain of the haze
// around it. A gain that followed the pixel would turn noise into blotches
// wherever a limit takes over. Colour detail finer than the guide, mostly
// demosaicing fringes and colour noise, takes a reduced gain (haze_removed).

struct HazeAirlight {
    // Airlight colour (xyz) and luminance (w). A luminance of zero means no
    // haze was found; Dehaze then neither removes nor adds haze, and positive
    // amounts only add depth.
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

// The guide scene blurs over this many native pixels on each side, enough to
// average out sensor noise and colour fringes yet follow fine branches; a
// reduced preview blurs its share of them, at least one pixel. At most
// HAZE_GUIDE_MAX_RADIUS processing pixels, which HAZE_GUIDE_SUPPORT in
// tiles.rs reserves.
const HAZE_GUIDE_SOURCE_RADIUS: f32 = 2.0;
const HAZE_GUIDE_MAX_RADIUS: i32 = 2;
// Neighbours whose luminance differs from the pixel's by much more than this
// many stops, the far side of an edge, barely enter its guide scene (the
// standard deviation of the blur's range weights). Otherwise the bright sky
// beside a dark roofline would lend the roofline its veil. Noise and fine
// texture differ by far less.
const HAZE_GUIDE_RANGE_STOPS: f32 = 0.35;
// Luminances are compared with this share of a reference luminance added, so
// that noise in the darkest shadows, large in stops, does not read as edges:
// the airlight's, or in a photo without haze the depth base's.
const HAZE_GUIDE_RANGE_FLOOR: f32 = 0.01;

// Full removal divides the scene's departure from the airlight by at most
// 1 / HAZE_MIN_TRANSMISSION, which bounds the noise it amplifies in dense haze.
const HAZE_MIN_TRANSMISSION: f32 = 0.33;
// Veils below HAZE_CLEAR_VEIL are left alone and removal ramps in up to
// HAZE_HAZY_VEIL, so photos and regions without haze keep their tones.
const HAZE_CLEAR_VEIL: f32 = 0.05;
const HAZE_HAZY_VEIL: f32 = 0.2;
// Share of the guide scene's darkest channel, relative to the airlight, that
// removal may take away. A colour holds no more veil than that channel;
// keeping a fifth of it stops grey cloud and dark detail inside haze from
// being crushed to black and saturation from running away.
const HAZE_DARK_CHANNEL_VEIL_LIMIT: f32 = 0.8;
// Removal keeps at least this share of each channel of a pixel. A pixel
// darker than its guide scene can hold less veil than the guide allows, and
// removal would otherwise turn its channels negative.
const HAZE_DARK_DETAIL_KEEP: f32 = 0.2;
// Within HAZE_AIRLIGHT_NEAR of the airlight's luminance, relative to it, a
// colour's difference from the airlight is mostly sky, a faint tint or a
// colour fringe rather than attenuated detail, so removal is left out and
// ramps in up to HAZE_AIRLIGHT_TOLERANCE: amplifying it would only deepen
// colour casts. Detail in dense haze departs further, by its transmission
// times its own difference from the airlight. Colour does not count, so a
// tinted sky is not taken for detail.
const HAZE_AIRLIGHT_NEAR: f32 = 0.075;
const HAZE_AIRLIGHT_TOLERANCE: f32 = 0.15;
// Removal fades out between these guide values. Light sources and the sun
// outshine any veil, and steepening them would only brighten them.
const HAZE_EMITTER_FADE_START: f32 = 1.0;
const HAZE_EMITTER_FADE_END: f32 = HAZE_GUIDE_MAX;
// Width, in units of veil and transmission, over which the limits above take
// over from the mapped veil. A sharp takeover creases the gain, which shows
// as contours across smooth skies.
const HAZE_LIMIT_KNEE: f32 = 0.1;
// Colour detail finer than the guide scene takes the gain to this power: the
// haze veils it like any detail, but at this scale it is mostly demosaicing
// fringes, chromatic aberration and colour noise, which full gain would
// multiply. Luminance detail takes the full gain.
const HAZE_DETAIL_COLOR_GAIN_EXPONENT: f32 = 0.5;
// Below this share of the airlight's luminance the guide scene's colour is
// mostly noise, and detail there takes a neutral tint.
const HAZE_TINT_FLOOR: f32 = 1e-3;
// Negative amounts thicken the veil to at least this transmission, so a photo
// without haze still gains a uniform one; hazier regions gain more, down to
// HAZE_MIN_TRANSMISSION.
const HAZE_ADDED_TRANSMISSION: f32 = 0.7;

// Positive amounts also steepen log luminance around the depth base, a wide
// edge-preserving average of the scene (haze_analysis.wgsl), and lift the
// chroma of moderately coloured regions: what a clear photo, which holds
// little veil to remove, gains from Dehaze as contrast and depth. Full Dehaze
// steepens by HAZE_DEPTH_CONTRAST. The guide scene's departure from the base
// is soft-limited to HAZE_DEPTH_DARKEN_STOPS below it and
// HAZE_DEPTH_BRIGHTEN_STOPS above, so deep shadows lose at most
// HAZE_DEPTH_CONTRAST * HAZE_DEPTH_DARKEN_STOPS and are not crushed.
const HAZE_DEPTH_CONTRAST: f32 = 0.5;
const HAZE_DEPTH_DARKEN_STOPS: f32 = 0.75;
const HAZE_DEPTH_BRIGHTEN_STOPS: f32 = 2.0;
// Departures within about this many stops of the base barely steepen: the
// noise left in the guide scene departs by less, and following it would
// spread the gain into grain over flat skies and fog. Beyond it the slope is
// HAZE_DEPTH_CONTRAST.
const HAZE_DEPTH_THRESHOLD_STOPS: f32 = 0.15;
// Standard deviation, in stops, of the range weights that pick the depth base
// among the four cells around a pixel, so a pixel takes the base of its side
// of an edge (joint bilateral upsampling).
const HAZE_DEPTH_UPSAMPLE_RANGE_STOPS: f32 = 0.7;
// Full Dehaze adds this share of the guide scene's chroma to regions whose
// saturation, 1 - darkest / brightest channel, measured both from neutral and
// from the airlight's tint (haze_relative_saturation), passes the rising
// ramp. Near-neutral colours keep their chroma, and so do colours near the
// airlight's tint, which is the cast of a hazy photo: tinted fog and clouds.
// The lift fades out as OKLab chroma relative to lightness crosses the
// falling ramp, like Vibrance, so colours that already look vivid, such as
// sunlit leaves, do not turn garish.
const HAZE_DEPTH_CHROMA: f32 = 0.3;
const HAZE_DEPTH_CHROMA_RISE: vec2<f32> = vec2<f32>(0.12, 0.4);
const HAZE_DEPTH_CHROMA_FALL: vec2<f32> = vec2<f32>(0.08, 0.18);
// The chroma lift keeps at least this share of each positive channel.
const HAZE_DEPTH_CHANNEL_KEEP: f32 = 0.5;

@group(0) @binding(54) var<storage, read> haze_airlight: HazeAirlight;
// Per grid cell, the guided filter's slope (x) and offset (y), giving the
// transmission as slope * guide + offset, the share of the veil removal may
// take (z) and the depth base in log2 luminance (w) (haze_analysis.wgsl). Only
// the cells of `haze_grid` are valid.
@group(0) @binding(60) var haze_map_tex: texture_2d<f32>;

struct HazeEstimate {
    // The share of the scene's light that reaches the camera.
    transmission: f32,
    // The share of the veil removal may take: less where the haze hides no
    // detail.
    removal_share: f32,
}

// The four haze map cells around a tile pixel and the pixel's position
// between their centres.
struct HazeMapCells {
    top_left: vec4<f32>,
    top_right: vec4<f32>,
    bottom_left: vec4<f32>,
    bottom_right: vec4<f32>,
    fraction: vec2<f32>,
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

// The smaller of `a` and `b`, blended over a parabola within `knee` of each
// other so that neither takes over with a crease (Quilez's polynomial smooth
// minimum). It lies at most knee / 4 below min(a, b); `knee` must be positive.
fn haze_smooth_min(a: f32, b: f32, knee: f32) -> f32 {
    let blend = max(knee - abs(a - b), 0.0) / knee;
    return min(a, b) - blend * blend * knee * 0.25;
}

fn haze_smooth_max(a: f32, b: f32, knee: f32) -> f32 {
    return -haze_smooth_min(-a, -b, knee);
}

// Radius of the guide scene's blur in processing pixels.
fn haze_guide_radius() -> i32 {
    let radius = round(HAZE_GUIDE_SOURCE_RADIUS * Common::processing_per_source_pixel());
    return clamp(i32(radius), 1, HAZE_GUIDE_MAX_RADIUS);
}

// Binomial weight of the tap `offset` pixels from the centre of a blur of
// radius 1 (1 2 1) or 2 (1 4 6 4 1). Both cancel a pattern that alternates
// from pixel to pixel, the finest noise, entirely.
fn haze_guide_weight(offset: i32, radius: i32) -> f32 {
    let distance = abs(offset);
    if radius == 1 {
        return select(1.0, 2.0, distance == 0);
    }
    return select(select(1.0, 4.0, distance == 1), 6.0, distance == 0);
}

// Log luminance of camera RGB that the guide scene's range weights compare,
// with `floor` added. Working RGB before the profile is close enough to the
// profiled scene for telling the sides of an edge apart.
fn haze_guide_log_luminance(camera_rgb: vec3<f32>, floor: f32) -> f32 {
    return log2(max(dot(Color::cam_to_working(camera_rgb), Common::LUMA), 0.0) + floor);
}

// The guide scene at a tile pixel: the profiled scene blurred over
// haze_guide_radius() with weights that leave out the far side of edges (a
// cross bilateral filter). Camera RGB is blurred and then characterized once,
// rather than per tap: the conversion is linear apart from gamut compression
// and the profile's hue and saturation map, which barely bend across the
// colours of a few neighbouring pixels. Taps beyond the tile repeat its edge,
// which HAZE_GUIDE_SUPPORT keeps out of export tile cores.
fn haze_guide_scene_at(pos: vec2<i32>, reference_luminance: f32) -> vec3<f32> {
    let radius = haze_guide_radius();
    let floor = HAZE_GUIDE_RANGE_FLOOR * reference_luminance;
    let centre = haze_guide_log_luminance(SceneSource::source_scene_at(pos), floor);
    var sum = vec3<f32>(0.0);
    var total = 0.0;
    for (var dy = -radius; dy <= radius; dy = dy + 1) {
        let row_weight = haze_guide_weight(dy, radius);
        for (var dx = -radius; dx <= radius; dx = dx + 1) {
            let tap = SceneSource::source_scene_at(pos + vec2<i32>(dx, dy));
            let stops = (haze_guide_log_luminance(tap, floor) - centre) / HAZE_GUIDE_RANGE_STOPS;
            let weight = row_weight * haze_guide_weight(dx, radius) * exp(-0.5 * stops * stops);
            sum = sum + weight * tap;
            total = total + weight;
        }
    }
    return SceneSource::profiled_from_camera(sum / total);
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

// The four haze map cells whose centres surround a tile pixel.
fn haze_map_cells_at(pos: vec2<i32>) -> HazeMapCells {
    let last = vec2<i32>(haze_grid()) - vec2<i32>(1);
    let position = clamp(haze_grid_position(pos), vec2<f32>(0.0), vec2<f32>(last));
    let base = vec2<i32>(floor(position));
    let next = min(base + vec2<i32>(1), last);
    return HazeMapCells(
        textureLoad(haze_map_tex, base, 0),
        textureLoad(haze_map_tex, vec2<i32>(next.x, base.y), 0),
        textureLoad(haze_map_tex, vec2<i32>(base.x, next.y), 0),
        textureLoad(haze_map_tex, next, 0),
        position - vec2<f32>(base),
    );
}

// Bilinear weights of the cells in HazeMapCells order.
fn haze_bilinear_weights(cells: HazeMapCells) -> vec4<f32> {
    let f = cells.fraction;
    return vec4<f32>((1.0 - f.x) * (1.0 - f.y), f.x * (1.0 - f.y), (1.0 - f.x) * f.y, f.x * f.y);
}

// The haze at a pixel whose guide scene has guide value `guide`: the map
// interpolated between the four nearest cell centres, its guided filter
// evaluated against that guide.
fn haze_estimate(cells: HazeMapCells, guide: f32) -> HazeEstimate {
    let f = cells.fraction;
    let top = mix(cells.top_left.xyz, cells.top_right.xyz, f.x);
    let bottom = mix(cells.bottom_left.xyz, cells.bottom_right.xyz, f.x);
    let map = mix(top, bottom, f.y);
    return HazeEstimate(clamp(map.x * guide + map.y, 0.0, 1.0), map.z);
}

// The cells' depth bases in HazeMapCells order.
fn haze_depth_bases(cells: HazeMapCells) -> vec4<f32> {
    return vec4<f32>(cells.top_left.w, cells.top_right.w, cells.bottom_left.w, cells.bottom_right.w);
}

// The depth base at a pixel, interpolated without regard to edges.
fn haze_plain_depth_base(cells: HazeMapCells) -> f32 {
    return dot(haze_bilinear_weights(cells), haze_depth_bases(cells));
}

// The depth base at a pixel whose guide scene has log2 luminance
// `log_luminance`: the four cells' bases weighted by their distance and by how
// close they lie to that luminance. Where none lies close, the small constant
// leaves the bilinear weights.
fn haze_depth_base(cells: HazeMapCells, log_luminance: f32) -> f32 {
    let bases = haze_depth_bases(cells);
    let stops = (bases - vec4<f32>(log_luminance)) / HAZE_DEPTH_UPSAMPLE_RANGE_STOPS;
    let weights = haze_bilinear_weights(cells) * (exp(-0.5 * stops * stops) + vec4<f32>(1e-3));
    return dot(weights, bases) / dot(weights, vec4<f32>(1.0));
}

// The transmission that removal divides by, from the guide scene `measured`
// (without negative channels) and its guide value: its share of the mapped
// veil, at most what the darkest channel allows, kept only where that is
// clearly haze, faded out for light sources and near the airlight, and
// floored to bound the gain.
fn haze_removal_transmission(
    measured: vec3<f32>,
    airlight: vec4<f32>,
    guide: f32,
    estimate: HazeEstimate,
) -> f32 {
    let relative = measured / airlight.xyz;
    let dark_limit = HAZE_DARK_CHANNEL_VEIL_LIMIT * min(relative.r, min(relative.g, relative.b));
    var veil = haze_smooth_min(
        (1.0 - estimate.transmission) * estimate.removal_share,
        dark_limit,
        HAZE_LIMIT_KNEE,
    );
    veil = veil * smoothstep(HAZE_CLEAR_VEIL, HAZE_HAZY_VEIL, veil);
    veil = veil * (1.0 - smoothstep(HAZE_EMITTER_FADE_START, HAZE_EMITTER_FADE_END, guide));
    let departure = abs(dot(measured, Common::LUMA) / airlight.w - 1.0);
    veil = veil * smoothstep(HAZE_AIRLIGHT_NEAR, HAZE_AIRLIGHT_TOLERANCE, departure);
    return haze_smooth_max(1.0 - veil, HAZE_MIN_TRANSMISSION, HAZE_LIMIT_KNEE);
}

// `dehazed` with each positive channel of `rgb` keeping at least
// HAZE_DARK_DETAIL_KEEP of itself, joined over a knee half as wide as that
// floor. Channels at least 1.5 times the floor are untouched, so is everything
// without removal. Channels that are not positive are left as they are.
fn haze_keep_dark_share(dehazed: vec3<f32>, rgb: vec3<f32>) -> vec3<f32> {
    var kept = dehazed;
    for (var channel = 0; channel < 3; channel = channel + 1) {
        let floor = HAZE_DARK_DETAIL_KEEP * rgb[channel];
        if floor > 0.0 {
            kept[channel] = haze_smooth_max(dehazed[channel], floor, 0.5 * floor);
        }
    }
    return kept;
}

// The pixel `rgb` with haze removed at gain `gain` = t^(-a), given its guide
// scene, raw and without negative channels (`measured`). The scattering model
// A + (I - A) * gain applies to the guide scene and to the pixel's luminance
// detail around it. The pixel's colour detail, its departure from the guide
// scene's tint at the same luminance, takes the gain only to the power
// HAZE_DETAIL_COLOR_GAIN_EXPONENT. At an exponent of 1 this is exactly the
// model.
fn haze_removed(
    rgb: vec3<f32>,
    guide_scene: vec3<f32>,
    measured: vec3<f32>,
    airlight: vec4<f32>,
    gain: f32,
) -> vec3<f32> {
    let base = airlight.xyz + (guide_scene - airlight.xyz) * gain;
    let detail = rgb - guide_scene;
    let floor = HAZE_TINT_FLOOR * airlight.w;
    // The guide scene's colour at luminance 1, as Common::LUMA sums to 1, so
    // colour detail holds no luminance.
    let tint = (measured + vec3<f32>(floor)) / (dot(measured, Common::LUMA) + floor);
    let luminance_detail = dot(detail, Common::LUMA) * tint;
    let color_detail = detail - luminance_detail;
    let dehazed = base
        + luminance_detail * gain
        + color_detail * pow(gain, HAZE_DETAIL_COLOR_GAIN_EXPONENT);
    return haze_keep_dark_share(dehazed, rgb);
}

// 1 - darkest / brightest channel of `rgb` relative to `reference`, both
// without negative channels.
fn haze_relative_saturation(rgb: vec3<f32>, reference: vec3<f32>) -> f32 {
    let relative = rgb / max(reference, vec3<f32>(1e-6));
    let brightest = max(relative.r, max(relative.g, relative.b));
    return 1.0 - min(relative.r, min(relative.g, relative.b)) / max(brightest, 1e-20);
}

// `rgb` steepened around the depth base and its chroma lifted, for an
// `amount` in [0, 1], given its guide scene without negative channels
// (`measured`) and the airlight's tint at luminance 1 (`airlight_tint`),
// neutral without haze. Both follow the guide scene, as removal does: the
// luminance gain is smooth across the image and the pixel enters linearly,
// and the lift adds the guide scene's chroma at the pixel's luminance, so
// neither amplifies the pixel's noise or colour fringes. Positive channels
// keep at least HAZE_DEPTH_CHANNEL_KEEP of themselves.
fn haze_deepened(
    rgb: vec3<f32>,
    measured: vec3<f32>,
    airlight_tint: vec3<f32>,
    cells: HazeMapCells,
    amount: f32,
) -> vec3<f32> {
    let luminance = dot(measured, Common::LUMA);
    if amount <= 1e-6 || luminance <= 0.0 {
        return rgb;
    }
    let log_luminance = log2(luminance);
    let measured_departure = log_luminance - haze_depth_base(cells, log_luminance);
    // Shrunk smoothly towards zero by the threshold: no slope at zero, full
    // slope well beyond it. tanh rounds to +/-1 in f32 beyond +/-10, so
    // bounding its argument preserves the result and avoids overflow in
    // backend intrinsics that evaluate tanh through exponentials.
    let threshold_departure = clamp(measured_departure / HAZE_DEPTH_THRESHOLD_STOPS, -10.0, 10.0);
    let departure = measured_departure
        - HAZE_DEPTH_THRESHOLD_STOPS * tanh(threshold_departure);
    let limit = select(HAZE_DEPTH_DARKEN_STOPS, HAZE_DEPTH_BRIGHTEN_STOPS, departure > 0.0);
    let limited = departure * inverseSqrt(1.0 + departure * departure / (limit * limit));
    let gain = exp2(amount * HAZE_DEPTH_CONTRAST * limited);

    let saturation = haze_relative_saturation(measured, vec3<f32>(1.0));
    let departure_from_airlight = haze_relative_saturation(measured, airlight_tint);
    let lab = Color::rec2020_to_oklab(measured);
    let colourfulness = length(lab.yz) / max(lab.x, 1e-4);
    var lift = amount * HAZE_DEPTH_CHROMA
        * smoothstep(HAZE_DEPTH_CHROMA_RISE.x, HAZE_DEPTH_CHROMA_RISE.y, saturation)
        * smoothstep(HAZE_DEPTH_CHROMA_RISE.x, HAZE_DEPTH_CHROMA_RISE.y, departure_from_airlight)
        * (1.0 - smoothstep(HAZE_DEPTH_CHROMA_FALL.x, HAZE_DEPTH_CHROMA_FALL.y, colourfulness));
    // The guide scene's chroma at the pixel's luminance, as Common::LUMA sums
    // to 1.
    let chroma = max(dot(rgb, Common::LUMA), 0.0) * (measured / luminance - vec3<f32>(1.0));
    for (var channel = 0; channel < 3; channel = channel + 1) {
        if rgb[channel] > 0.0 && chroma[channel] < 0.0 {
            lift = min(lift, (1.0 - HAZE_DEPTH_CHANNEL_KEEP) * rgb[channel] / -chroma[channel]);
        }
    }
    return (rgb + lift * chroma) * gain;
}

// The share of depth (haze_deepened) a pixel takes where removal divides by
// `removal`: none at the strongest removal, which already steepens the scene
// and lifts its chroma, so that the two do not compound in dense haze.
fn haze_depth_share(removal: f32) -> f32 {
    return smoothstep(HAZE_MIN_TRANSMISSION, 1.0, removal);
}

// Removes haze for `amount` > 0 and adds it for `amount` < 0. `amount` is the
// combined perceptual amount of global and mask Dehaze, in [-1, 1].
fn apply_dehaze(pos: vec2<i32>, rgb: vec3<f32>, amount: f32) -> vec3<f32> {
    let airlight = haze_airlight.color;
    let hazy = airlight.w > 0.0;
    if abs(amount) < 1e-6 || (amount < 0.0 && !hazy) {
        return rgb;
    }
    let cells = haze_map_cells_at(pos);
    // Without haze the guide scene's range floor scales with the depth base.
    let reference = select(exp2(haze_plain_depth_base(cells)), airlight.w, hazy);
    let guide_scene = haze_guide_scene_at(pos, reference);
    let measured = Color::map_negative_gamut(guide_scene);
    if amount < 0.0 {
        let estimate = haze_estimate(cells, haze_guide(measured, airlight));
        let veil = haze_airlight.added_veil.xyz;
        let added = clamp(estimate.transmission, HAZE_MIN_TRANSMISSION, HAZE_ADDED_TRANSMISSION);
        return veil + (rgb - veil) * pow(added, -amount);
    }
    var dehazed = rgb;
    var removal = 1.0;
    var airlight_tint = vec3<f32>(1.0);
    if hazy {
        let guide = haze_guide(measured, airlight);
        removal = haze_removal_transmission(measured, airlight, guide, haze_estimate(cells, guide));
        dehazed = haze_removed(rgb, guide_scene, measured, airlight, pow(removal, -amount));
        airlight_tint = airlight.xyz / airlight.w;
    }
    return haze_deepened(
        dehazed,
        measured,
        airlight_tint,
        cells,
        amount * haze_depth_share(removal),
    );
}
