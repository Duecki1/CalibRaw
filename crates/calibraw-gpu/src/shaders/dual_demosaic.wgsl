// SPDX-License-Identifier: GPL-3.0-or-later
// Adapted from darktable 5.6.1 dual demosaicing (src/iop/demosaicing/dual.c)
// and the parts it calls: VNG's linear interpolation (vng.c), colour
// smoothing (basics.c), the detail mask (src/develop/masks/detail.c) and the
// 9x9 Gaussian (src/common/gaussian.c).
// Copyright (C) 2010-2026 darktable developers.
// Dual-demosaic credits: Ingo Weyrich and Hanno Schwalm.
// Copyright (C) 2026 CalibRaw contributors (WGSL adaptation).
//
// Dual demosaicing keeps the reference demosaic `high` (RCD or Markesteijn)
// where the image has detail and a smooth linear interpolation `low` in flat
// areas, where the reference leaves maze patterns and false colour in noise:
//     out = mask * high + (1 - mask) * low
// It runs in three passes at tile-local pixel positions:
//   1. the CFA's prepare entry point stores the linear interpolation and the
//      detail luminance of `high` (`dual_store_prepared`);
//   2. dual_demosaic_detail.wgsl applies the first colour-smoothing pass to
//      the interpolation and computes the unblurred detail mask;
//   3. the CFA's finish pass applies the second smoothing pass, blurs the
//      mask and mixes (`dual_blend`).
// Colours are white-balanced camera RGB with sensor white 1. The image border
// is handled as darktable handles it; tile edges inside an export halo take
// the same path and are discarded. A finished pixel reads `high` up to 5
// pixels away (Scharr 1 + Gaussian 4) and the CFA up to 3 (interpolation 1 +
// two smoothing passes).

#import calibraw::common as Common
#import calibraw::cfa_linear as CfaLinear

// Pass 1 output, read by pass 2: the linear interpolation (rgb) and the
// detail luminance of `high` (a).
@group(0) @binding(20) var dual_prepared_write: texture_storage_2d<rgba16float /* CALIBRAW_WORK_FORMAT */, write>;
@group(0) @binding(21) var dual_prepared_read: texture_2d<f32>;
// Pass 2 output, read by the finish pass: the interpolation after one
// smoothing pass (rgb) and the unblurred detail mask (a).
@group(0) @binding(23) var dual_low_read: texture_2d<f32>;

// 1 / the sum of exp(-r^2 / 8) over the 69 offsets with r <= 4.5: darktable's
// 9x9 Gaussian of sigma 2 (`_calc_9x9_gauss_coeffs`).
const DUAL_GAUSSIAN_NORM: f32 = 1.0 / 23.565945986579553;
// Largest squared offset of that Gaussian: r^2 <= 4.5^2 in whole pixels.
const DUAL_GAUSSIAN_MAX_R2: i32 = 20;

// darktable's detail-mask luminance (`dt_masks_calc_scharr_mask`): the square
// root of the mean channel before white balance, which makes noise about
// equally strong at every level.
fn dual_detail_luma(rgb: vec3<f32>) -> f32 {
    let wb = max(Common::camera_uniforms.wb.xyz, vec3<f32>(1e-6));
    let sensor = max(rgb / wb, vec3<f32>(0.0));
    return sqrt((sensor.r + sensor.g + sensor.b) / 3.0);
}

// Pass 1 for a CFA prepare entry point: `high` is the reference demosaic at
// `pos`; `split_greens` selects Bayer's four-colour interpolation (cfa_linear.wgsl).
fn dual_store_prepared(pos: vec2<i32>, high: vec3<f32>, split_greens: bool) {
    textureStore(
        dual_prepared_write,
        pos,
        vec4<f32>(CfaLinear::cfa_linear_rgb(pos, split_greens), dual_detail_luma(high)),
    );
}

// Orders `values[i] <= values[j]`.
fn dual_order(values: ptr<function, array<f32, 9>>, i: i32, j: i32) {
    let a = (*values)[i];
    let b = (*values)[j];
    (*values)[i] = min(a, b);
    (*values)[j] = max(a, b);
}

// Median of nine values: darktable's 19-exchange network (`color_smoothing`).
fn dual_median9(values: array<f32, 9>) -> f32 {
    var v = values;
    dual_order(&v, 1, 2); dual_order(&v, 4, 5); dual_order(&v, 7, 8);
    dual_order(&v, 0, 1); dual_order(&v, 3, 4); dual_order(&v, 6, 7);
    dual_order(&v, 1, 2); dual_order(&v, 4, 5); dual_order(&v, 7, 8);
    dual_order(&v, 0, 3); dual_order(&v, 5, 8); dual_order(&v, 4, 7);
    dual_order(&v, 3, 6); dual_order(&v, 1, 4); dual_order(&v, 2, 5);
    dual_order(&v, 4, 7); dual_order(&v, 4, 2); dual_order(&v, 6, 4);
    dual_order(&v, 4, 2);
    return v[4];
}

// One colour-smoothing pass of darktable (`color_smoothing`) for the centre of
// a 3x3 window in row order: red and blue become the centre's green plus the
// median of their difference to green. Green keeps its value. darktable runs
// two passes over the interpolation and skips the image border.
fn dual_smoothing_step(window: array<vec3<f32>, 9>) -> vec3<f32> {
    var samples = window;
    var red: array<f32, 9>;
    var blue: array<f32, 9>;
    for (var i = 0; i < 9; i = i + 1) {
        red[i] = samples[i].r - samples[i].g;
        blue[i] = samples[i].b - samples[i].g;
    }
    let green = samples[4].g;
    return vec3<f32>(
        max(green + dual_median9(red), 0.0),
        green,
        max(green + dual_median9(blue), 0.0),
    );
}

fn dual_window_offset(index: i32) -> vec2<i32> {
    return vec2<i32>(index % 3 - 1, index / 3 - 1);
}

// Pass 2: the first smoothing pass over the prepared interpolation.
fn dual_smoothed_prepared(pos: vec2<i32>) -> vec3<f32> {
    if CfaLinear::cfa_on_border(pos) {
        return textureLoad(dual_prepared_read, pos, 0).rgb;
    }
    var window: array<vec3<f32>, 9>;
    for (var i = 0; i < 9; i = i + 1) {
        window[i] = textureLoad(dual_prepared_read, pos + dual_window_offset(i), 0).rgb;
    }
    return dual_smoothing_step(window);
}

// The Scharr mask value where the detail blend crosses one half. The Dual
// Threshold setting spans 0..100 where darktable's spans 0..1.
fn dual_contrast_threshold() -> f32 {
    let threshold = clamp(Common::camera_uniforms.dual_threshold, 0.0, 100.0) / 100.0;
    return 0.005 * pow(threshold, 1.1);
}

fn dual_prepared_luma(pos: vec2<i32>) -> f32 {
    return textureLoad(dual_prepared_read, pos, 0).a;
}

// Pass 2: the unblurred detail blend at `pos` (`dt_masks_calc_scharr_mask`
// and `dt_masks_calc_detail_blend`), a sigmoid of the Scharr gradient of the
// detail luminance that is 1 on detail and 0 in flat areas. Border pixels
// take the gradient of their nearest interior pixel.
fn dual_detail_blend_at(pos: vec2<i32>) -> f32 {
    let last = Common::image_max();
    let c = clamp(pos, vec2<i32>(1), max(last - vec2<i32>(1), vec2<i32>(1)));
    let nw = dual_prepared_luma(c + vec2<i32>(-1, -1));
    let n  = dual_prepared_luma(c + vec2<i32>( 0, -1));
    let ne = dual_prepared_luma(c + vec2<i32>( 1, -1));
    let w  = dual_prepared_luma(c + vec2<i32>(-1,  0));
    let e  = dual_prepared_luma(c + vec2<i32>( 1,  0));
    let sw = dual_prepared_luma(c + vec2<i32>(-1,  1));
    let s  = dual_prepared_luma(c + vec2<i32>( 0,  1));
    let se = dual_prepared_luma(c + vec2<i32>( 1,  1));
    let gx = 47.0 / 255.0 * (nw - ne + sw - se) + 162.0 / 255.0 * (w - e);
    let gy = 47.0 / 255.0 * (nw - sw + ne - se) + 162.0 / 255.0 * (n - s);
    let detail = clamp(length(vec2<f32>(gx, gy)) / 16.0, 0.0, 1.0);
    // darktable evaluates exp with dt_fast_expf, which is within a few
    // percent near the threshold; far below it both give a blend of about 0.
    let steepness = 16.0 / max(dual_contrast_threshold(), 1e-7);
    return clamp(1.0 / (1.0 + exp(16.0 - steepness * detail)), 0.0, 1.0);
}

// The detail blend after darktable's 9x9 Gaussian of sigma 2. Taps outside
// the image are left out without renormalising, as darktable does.
fn dual_blurred_mask(pos: vec2<i32>) -> f32 {
    var sum = 0.0;
    for (var dy = -4; dy <= 4; dy = dy + 1) {
        for (var dx = -4; dx <= 4; dx = dx + 1) {
            let r2 = dx * dx + dy * dy;
            let q = pos + vec2<i32>(dx, dy);
            if r2 > DUAL_GAUSSIAN_MAX_R2 || !CfaLinear::cfa_in_bounds(q) { continue; }
            sum += exp(-f32(r2) / 8.0) * textureLoad(dual_low_read, q, 0).a;
        }
    }
    return clamp(sum * DUAL_GAUSSIAN_NORM, 0.0, 1.0);
}

// The second smoothing pass, over pass 2's output.
fn dual_smoothed_low(pos: vec2<i32>) -> vec3<f32> {
    if CfaLinear::cfa_on_border(pos) {
        return textureLoad(dual_low_read, pos, 0).rgb;
    }
    var window: array<vec3<f32>, 9>;
    for (var i = 0; i < 9; i = i + 1) {
        window[i] = textureLoad(dual_low_read, pos + dual_window_offset(i), 0).rgb;
    }
    return dual_smoothing_step(window);
}

// The finish pass: the dual demosaic of reference `high` at `pos`. A zero
// threshold keeps `high`, and the dual passes do not run then.
fn dual_blend(pos: vec2<i32>, high: vec3<f32>) -> vec3<f32> {
    if dual_contrast_threshold() <= 0.0 {
        return high;
    }
    return mix(dual_smoothed_low(pos), high, dual_blurred_mask(pos));
}
