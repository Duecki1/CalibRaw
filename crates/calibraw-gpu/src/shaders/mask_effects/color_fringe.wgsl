// Color Fringe moves the photo's red channel one way and its blue channel the
// other, as lateral chromatic aberration (Lens) or an RGB split (Shift) would
// (color_fringe.rs in calibraw-core documents the model).
//
// Colour: scene-linear Rec.2020, negative and above-1 values kept; the channels
// move as they are, with no clamp.
//
// Coordinates: the shift is a vector D in processing pixels, from the full image's
// pixels with y down, so preview tiles, export tiles and reduced previews agree. Red
// at a pixel is read from p - D, blue from p + D, green at p. Lens points D away from
// the full image's centre and scales it with the distance to it, reaching SHIFT
// shorter edges at the corners; Shift points it along DIRECTION everywhere.
//
// Prism: each channel integrates its source over a segment of the line through
// p - D, p and p + D, with the weights of the dispersion in Blender's Lens Distortion
// node (`accumulate` in its compositor): red ramps down from p - D toward p, green is a
// tent around p, blue ramps down from p + D toward p. PRISM is the share of the way to
// the neighbouring channel the ramps reach, so 0 reads three sharp copies and 1
// Blender's smear. Each channel's weights are normalized, so an even area keeps its
// colour. Taps are about one pixel apart along a segment, at most
// COLOR_FRINGE_MAX_TAPS per channel side, and filtered bilinearly.
//
// Boundaries: reads clamp to the processed texture, as Motion Blur's do. The shift is
// capped at COLOR_FRINGE_SUPPORT native pixels (minus the bilinear tap), the reach the
// effect registry reserves in export halos (`EffectReach::Creative`).
//
// The result keeps what earlier cards of the creative pass did to `rgb` and adds the
// fringe of the pass's input: lines an earlier card drew (Edge Glow, Pixelate's
// blocks) are not split, while those of the scene effects pass (Neon) are.

const COLOR_FRINGE_SUPPORT: u32 = 144u;
const COLOR_FRINGE_MAX_TAPS: u32 = 64u;
const COLOR_FRINGE_LENS: f32 = 0.0;

// The shift vector D at tile pixel `pos`, in processing pixels.
fn color_fringe_shift(pos: vec2<i32>, params: MaskEffectParams) -> vec2<f32> {
    let full_size = vec2<f32>(
        f32(Common::camera_uniforms.full_width),
        f32(Common::camera_uniforms.full_height),
    );
    let limit = max(
        f32(COLOR_FRINGE_SUPPORT) * Common::processing_per_source_pixel() - 1.0,
        0.0,
    );
    let distance = min(
        mask_effect_lane(params, COLOR_FRINGE_SHIFT_LANE) * effect_short_edge(),
        limit,
    );
    if mask_effect_lane(params, COLOR_FRINGE_STYLE_LANE) == COLOR_FRINGE_LENS {
        // Pixel centres, so the image's centre is still at any parity.
        let from_center = vec2<f32>(pos + Common::tile_origin()) + vec2<f32>(0.5) - 0.5 * full_size;
        let half_diagonal = max(0.5 * length(full_size), 1.0);
        return from_center * (distance / half_diagonal);
    }
    let angle = mask_effect_lane(params, COLOR_FRINGE_DIRECTION_LANE);
    return vec2<f32>(cos(angle), sin(angle)) * distance;
}

// One channel's ramp from `start` (weight 1) toward `start + span` (weight 0), with
// `span` a share of the way to the neighbouring channel. Returns the weighted
// source colour and the weight.
fn color_fringe_ramp(pos: vec2<f32>, start: vec2<f32>, span: vec2<f32>) -> vec4<f32> {
    let taps = u32(clamp(ceil(length(span)), 1.0, f32(COLOR_FRINGE_MAX_TAPS)));
    var sum = vec4<f32>(0.0);
    for (var index = 0u; index < COLOR_FRINGE_MAX_TAPS; index += 1u) {
        if index >= taps { break; }
        let along = (f32(index) + 0.5) / f32(taps);
        let weight = 1.0 - along;
        sum += vec4<f32>(mask_effect_source_linear_at(pos + start + span * along) * weight, weight);
    }
    return sum;
}

fn apply_color_fringe(pos: vec2<i32>, rgb: vec3<f32>, params: MaskEffectParams) -> vec3<f32> {
    let shift = color_fringe_shift(pos, params);
    if dot(shift, shift) <= 1e-8 { return rgb; }
    let prism = clamp(mask_effect_lane(params, COLOR_FRINGE_PRISM_LANE), 0.0, 1.0);
    let center = vec2<f32>(pos);
    let red = color_fringe_ramp(center, -shift, shift * prism);
    let blue = color_fringe_ramp(center, shift, -shift * prism);
    // Green's tent is two ramps from p, one toward each neighbour, each with half the
    // taps' share of p itself.
    let green = color_fringe_ramp(center, vec2<f32>(0.0), -shift * prism)
        + color_fringe_ramp(center, vec2<f32>(0.0), shift * prism);
    let fringe = vec3<f32>(red.r / red.w, green.g / green.w, blue.b / blue.w);
    return rgb + fringe - SceneAdjustments::local_effects_at(pos);
}
