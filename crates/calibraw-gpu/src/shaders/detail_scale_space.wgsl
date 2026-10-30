#import calibraw::common as Common
#import calibraw::color as Color
#import calibraw::basic_adjustments as BasicAdjustments
#import calibraw::scene_adjustments as SceneAdjustments
#import calibraw::detail_utils as DetailUtils
#import calibraw::tone_common as ToneCommon


// Texture and Clarity bases: edge-avoiding à-trous cascades of log2
// luminance over the adjustment base. Level k filters level k - 1 with the 5×5
// binomial kernel at a step of 2^k pixels, so each level only skips over detail
// that the previous level has already removed and nothing finer than a step
// aliases into a base (the mask Blur stages filter each other the same way).
// The binomial kernel has a variance of one step², so level k has a Gaussian
// variance of sum_{j<=k} 4^j pixels². Each base blends the two levels around
// its target variance, so its radius follows the image size continuously.
//
// Each base has its own chain with its own edge stop. Clarity's is soft:
// detail that a strong edge stop keeps at a fine level would pass every
// coarser level unfiltered (a step of 2^k leaves a period of 2^k untouched),
// so a chain shared with Texture would bring fine contrast into the Clarity base.
//
// The levels ping-pong between two work textures; the last level holds every
// base. Channels, in log2 luminance relative to scene middle grey:
//   x: Clarity chain;
//   y: negative-Texture chain, then that base minus the pixel's own value;
//   z: Texture chain, then that base minus the pixel's own value;
//   w: Clarity base minus the pixel's own value.
// Storing bases relative to the pixel keeps Preview's rgba16float to about
// 0.001 EV in the bands.
// Taps clamp to the image edge (`detail_pyramid_tap_pos`). Export tiles
// reserve the cascade's footprint, KERNEL_RADIUS × (2^levels - 1) pixels
// (DETAIL_TEXTURE_SUPPORT and DETAIL_CLARITY_SUPPORT in tiles.rs), so a tile's
// own edge never clamps a tap that reaches its core.
const DETAIL_PYRAMID_LEVEL_COUNT: u32 = 7u;
// The levels Texture and negative Texture need; Clarity needs every level.
const DETAIL_PYRAMID_TEXTURE_LEVEL_COUNT: u32 = 5u;
// Taps on each side of a level's kernel; `Common::binomial5_weight` fixes it.
const DETAIL_PYRAMID_KERNEL_RADIUS: i32 = 2;

// Edge stops of the chains in channel order x, y, z: weight exp(-strength × ΔEV²).
const DETAIL_CHAIN_RANGE_STRENGTHS: vec3<f32> = vec3<f32>(1.05, 3.0, 10.5);

@group(0) @binding(50) var detail_pyramid_tex: texture_2d<f32>;
@group(0) @binding(51) var detail_pyramid_out: texture_storage_2d<rgba16float /* CALIBRAW_WORK_FORMAT */, write>;

// Gaussian variance in pixels² after levels 0..=level; level -1 is the
// unfiltered image.
fn detail_level_variance(level: i32) -> f32 {
    return (exp2(2.0 * f32(level + 1)) - 1.0) / 3.0;
}

// Target variances in pixels² of the Clarity (x), negative-Texture (y) and
// Texture (z) bases. Texture and negative Texture keep the Gaussian sigma of
// their former 5×5 and 7×7 bilateral filters (1.44 and 2.16 taps of 1.65 px per
// reference scale). Clarity's sigma of 16 px per reference scale is about 1.5 %
// of the short edge, a Lightroom-like local-contrast radius. A reduced preview
// may need a base below one pixel; it blends with the unfiltered image rather
// than widening, so it covers the same share of the photo as the export.
fn detail_target_variances() -> vec3<f32> {
    let scale = DetailUtils::presence_reference_scale();
    let texture_step = 1.65 * scale;
    let sigma = vec3<f32>(16.0 * scale, 2.16 * texture_step, 1.44 * texture_step);
    return sigma * sigma;
}

// Whether `level` completes a base of `variance` built from the first
// `level_count` levels: the first level at least that wide, which blends with
// the level before, or the last level for a wider base.
fn detail_base_completes_at(level: u32, level_count: u32, variance: f32) -> bool {
    let index = i32(level);
    return level < level_count
        && detail_level_variance(index - 1) < variance
        && (variance <= detail_level_variance(index) || level + 1u == level_count);
}

// The weight of `level` against `level - 1` that gives a base of `variance`
// (a mixture's variance is the weighted mean of its parts').
fn detail_level_blend(level: u32, variance: f32) -> f32 {
    let lower = detail_level_variance(i32(level) - 1);
    let upper = detail_level_variance(i32(level));
    return clamp((variance - lower) / (upper - lower), 0.0, 1.0);
}

// The next value of a Texture chain channel: the new level while its base is
// wider, then the base relative to `center`, which later levels keep.
fn detail_texture_channel(
    level: u32,
    variance: f32,
    previous: f32,
    filtered: f32,
    center: f32,
) -> f32 {
    if level >= DETAIL_PYRAMID_TEXTURE_LEVEL_COUNT
        || variance <= detail_level_variance(i32(level) - 1) {
        return previous;
    }
    if detail_base_completes_at(level, DETAIL_PYRAMID_TEXTURE_LEVEL_COUNT, variance) {
        return mix(previous, filtered, detail_level_blend(level, variance)) - center;
    }
    return filtered;
}

// Clamps a tap to the full image before the tile. An export tile's halo
// beyond the image mirrors it, but a level filtered there differs from the
// edge pixel's own; clamping in image coordinates reads the edge pixel's
// level, as the full frame does.
fn detail_pyramid_tap_pos(pos: vec2<i32>) -> vec2<i32> {
    let origin = Common::tile_origin();
    let image_pos = clamp(pos + origin, vec2<i32>(0), Common::full_image_max()) - origin;
    return Common::clamp_pos(image_pos);
}

fn detail_pyramid_input_at(pos: vec2<i32>, level: u32, pivot_ev: f32) -> vec4<f32> {
    let tap = detail_pyramid_tap_pos(pos);
    if level == 0u {
        let ev = Common::log_luminance(SceneAdjustments::adjustment_base_at(tap)) - pivot_ev;
        return vec4<f32>(ev, ev, ev, 0.0);
    }
    return textureLoad(detail_pyramid_tex, tap, 0);
}

// Builds `level` of the cascade from the previous level (the adjustment base
// for level 0) and completes the bases that reach their radius here.
fn store_detail_pyramid_level(gid: vec3<u32>, level: u32) {
    if gid.x >= Common::camera_uniforms.width || gid.y >= Common::camera_uniforms.height { return; }
    let pos = vec2<i32>(i32(gid.x), i32(gid.y));
    let step = 1 << level;
    let pivot_ev = log2(ToneCommon::SCENE_MIDDLE_GREY);

    // Channels that already hold a base filter meaningless values; the
    // results are discarded below.
    let previous = detail_pyramid_input_at(pos, level, pivot_ev);
    var sums = vec3<f32>(0.0);
    var weights = vec3<f32>(0.0);
    for (var ky = -DETAIL_PYRAMID_KERNEL_RADIUS; ky <= DETAIL_PYRAMID_KERNEL_RADIUS; ky = ky + 1) {
        for (var kx = -DETAIL_PYRAMID_KERNEL_RADIUS; kx <= DETAIL_PYRAMID_KERNEL_RADIUS; kx = kx + 1) {
            let tap = detail_pyramid_input_at(pos + vec2<i32>(kx, ky) * step, level, pivot_ev).xyz;
            let spatial = Common::binomial5_weight(kx) * Common::binomial5_weight(ky);
            let delta = tap - previous.xyz;
            let weight = spatial * exp(-DETAIL_CHAIN_RANGE_STRENGTHS * delta * delta);
            sums = sums + tap * weight;
            weights = weights + weight;
        }
    }
    let filtered = sums / max(weights, vec3<f32>(1e-6));

    let targets = detail_target_variances();
    let center = Common::log_luminance(SceneAdjustments::adjustment_base_at(pos)) - pivot_ev;
    var stored = vec4<f32>(
        filtered.x,
        detail_texture_channel(level, targets.y, previous.y, filtered.y, center),
        detail_texture_channel(level, targets.z, previous.z, filtered.z, center),
        previous.w,
    );
    if detail_base_completes_at(level, DETAIL_PYRAMID_LEVEL_COUNT, targets.x) {
        stored.w = mix(previous.x, filtered.x, detail_level_blend(level, targets.x)) - center;
    }
    textureStore(detail_pyramid_out, pos, stored);
}

fn creative_edge_guard(pos: vec2<i32>) -> f32 {
    let step = SceneAdjustments::presence_step(1.0, 3);
    let left = Common::log_luminance(SceneAdjustments::adjustment_base_at(pos + vec2<i32>(-step, 0)));
    let right = Common::log_luminance(SceneAdjustments::adjustment_base_at(pos + vec2<i32>(step, 0)));
    let up = Common::log_luminance(SceneAdjustments::adjustment_base_at(pos + vec2<i32>(0, -step)));
    let down = Common::log_luminance(SceneAdjustments::adjustment_base_at(pos + vec2<i32>(0, step)));
    let gradient = length(vec2<f32>(right - left, down - up));
    return 1.0 - 0.78 * smoothstep(0.48, 1.25, gradient);
}

// Measures Texture and Clarity on the adjustment base and applies the resulting
// EV change to `rgb`. The bases are neighbourhoods of the adjustment base, so the
// centre is measured there as well: when `rgb` already carries earlier effects
// (local masks run after the global ones), their brightness change is not
// mistaken for detail. For the global pass `rgb` is the adjustment base itself.
// Reads the detail pyramid's last level, which the encoder builds whenever a
// Texture or Clarity value is set (`GpuParams::detail_pyramid_levels`).
fn apply_texture_and_clarity_values(
    pos: vec2<i32>,
    rgb: vec3<f32>,
    texture_value: f32,
    clarity_value: f32,
) -> vec3<f32> {
    let texture = BasicAdjustments::perceptual_control(texture_value);
    let clarity = BasicAdjustments::perceptual_control(clarity_value);
    if abs(texture) < 1e-6 && abs(clarity) < 1e-6 {
        return rgb;
    }

    let center_ev = Common::log_luminance(SceneAdjustments::adjustment_base_at(pos));
    let bases = textureLoad(detail_pyramid_tex, pos, 0);
    let fine_base_ev = center_ev + bases.z;
    var broad_base_ev = fine_base_ev;
    if abs(clarity) >= 1e-6 {
        broad_base_ev = center_ev + bases.w;
    }

    let texture_band_ev = center_ev - fine_base_ev;
    let clarity_band_ev = fine_base_ev - broad_base_ev;

    let signal_gate = smoothstep(-7.4, -2.35, center_ev);
    let shadow_noise = 1.0 - signal_gate;
    let texture_threshold = mix(0.028, 0.006, signal_gate) * mix(1.0, 1.65, shadow_noise);
    let positive_texture = DetailUtils::soft_detail_threshold(texture_band_ev, texture_threshold);
    var negative_texture_base_ev = fine_base_ev;
    if texture < 0.0 {
        negative_texture_base_ev = center_ev + bases.y;
    }
    let negative_texture = clamp(center_ev - negative_texture_base_ev, -0.32, 0.32);

    let midtone_gate = smoothstep(-7.0, -2.25, center_ev)
        * (1.0 - 0.74 * smoothstep(0.9, 3.6, center_ev));
    let selected_clarity = DetailUtils::soft_detail_threshold(clarity_band_ev, 0.0065);
    let halo_guard = creative_edge_guard(pos);

    // Texture and Clarity strengths are set so that ±50 changes local contrast
    // (RMS log2 luminance) on the Demo photos as much as the single-level bases
    // that the cascade replaced did. Both only move a pixel relative to its
    // bases, so neither shifts the image's overall tone.
    let positive_texture_strength = 3.10 * mix(0.88, 1.12, texture);
    var texture_ev = texture
        * positive_texture
        * positive_texture_strength
        * mix(0.72, 1.0, signal_gate);
    if texture < 0.0 {
        let smoothing = min(
            (-texture) * 0.49 * mix(0.72, 1.0, signal_gate),
            0.63,
        );
        texture_ev = -negative_texture * smoothing;
    }

    let clarity_strength = select(0.73, 2.76, clarity >= 0.0)
        * mix(0.90, 1.10, abs(clarity));
    let clarity_ev = clarity * selected_clarity * clarity_strength * midtone_gate * halo_guard;
    let delta_ev = clamp(texture_ev + clarity_ev, -1.90, 1.20);
    return Color::gamut_project_nonnegative_rec2020(rgb * exp2(delta_ev));
}
