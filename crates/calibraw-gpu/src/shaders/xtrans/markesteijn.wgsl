// SPDX-License-Identifier: GPL-3.0-or-later
// Adapted from darktable 5.6.1 Markesteijn X-Trans demosaicing
// (src/iop/demosaicing/xtrans.c, `xtrans_markesteijn_interpolate`), itself
// adapted from dcraw 9.20.
// Copyright (C) 2010-2026 darktable developers.
// Markesteijn algorithm credit: Frank Markesteijn (via dcraw and darktable).
// Copyright (C) 2026 CalibRaw contributors (WGSL adaptation).
//
// Markesteijn 3-pass interpolates eight candidate images: four directional
// interpolations, which passes 2 and 3 refine into four more. The result
// averages, per pixel, the candidates that are most homogeneous in YPbPr.
// darktable keeps the candidates as RGB buffers that each step updates in
// place. No step reads a value that the same step writes, so one dispatch
// per step computes the same values.
//
// What a dispatch hands on, a "state", is the green and the other colour of
// every red or blue photosite in four buffers: x, y, z and w of two textures.
// Red and blue of green photosites follow from it and are recomputed where
// they are read:
//   - a solitary green (four red or blue neighbours) from the greens of the
//     photosites 1 and 2 pixels away along each axis;
//   - a green in a 2x2 block from two red or blue photosites of its hexagon.
// Positions are tile-local pixels. Channels are white-balanced camera RGB
// with sensor white 1; samples below black count as 0, as in darktable.

#import calibraw::common as Common
#import calibraw::raw_sampling as RawSampling

// The state of buffers 0-3 (lo) and 4-7 (hi). A pass that builds the next
// state reads the previous one through lo and binds it to hi as well.
@group(0) @binding(27) var mark_lo_green: texture_2d<f32>;
@group(0) @binding(28) var mark_lo_other: texture_2d<f32>;
@group(0) @binding(29) var mark_hi_green: texture_2d<f32>;
@group(0) @binding(30) var mark_hi_other: texture_2d<f32>;

const MARK_LO: u32 = 0u;
const MARK_HI: u32 = 1u;

// dcraw's hexagon tables (`orth`, `patt`). orth holds (row, column) rotations;
// patt holds the offsets of the six greens around a red or blue photosite
// (first row) and of the red or blue photosites a 2x2-block green pairs up
// (second row).
const MARK_ORTH = array<i32, 12>(1, 0, 0, 1, -1, 0, 0, -1, 1, 0, 0, 1);
const MARK_PATT = array<array<i32, 16>, 2>(
    array<i32, 16>(0, 1, 0, -1, 2, 0, -1, 0, 1, 1, 1, -1, 0, 0, 0, 0),
    array<i32, 16>(0, 1, 0, -2, 1, 0, -2, 0, 1, 1, -2, -2, 1, -1, -1, 1),
);

// The directions darktable differentiates candidate d along (`dir[d & 3]`), as
// (column, row).
const MARK_DIRECTIONS = array<vec2<i32>, 4>(
    vec2<i32>(1, 0),
    vec2<i32>(0, 1),
    vec2<i32>(1, 1),
    vec2<i32>(-1, 1),
);

struct MarkRgb {
    red: vec4<f32>,
    green: vec4<f32>,
    blue: vec4<f32>,
}

struct MarkRedBlue {
    red: vec4<f32>,
    blue: vec4<f32>,
}

// Red and blue of a solitary green from one axis, for four buffers.
// `steepness` is darktable's `diff`, which chooses the smoother axis.
struct MarkAxis {
    red: vec4<f32>,
    blue: vec4<f32>,
    steepness: vec4<f32>,
}

struct MarkYpbpr {
    y: vec4<f32>,
    pb: vec4<f32>,
    pr: vec4<f32>,
}

fn mark_color(pos: vec2<i32>) -> u32 {
    return RawSampling::color_at(pos);
}

fn mark_is_green(pos: vec2<i32>) -> bool {
    return mark_color(pos) == 1u;
}

// The photosite's sample. darktable starts every buffer from max(0, sample).
fn mark_sample(pos: vec2<i32>) -> f32 {
    return max(RawSampling::raw_cfa_at(pos), 0.0);
}

// darktable's initial buffer: the sample in its own channel, 0 in the others.
fn mark_initial(pos: vec2<i32>, channel: u32) -> f32 {
    return select(0.0, mark_sample(pos), mark_color(pos) == channel);
}

fn mark_is_solitary(pos: vec2<i32>) -> bool {
    return mark_is_green(pos)
        && !mark_is_green(pos + vec2<i32>(1, 0))
        && !mark_is_green(pos + vec2<i32>(-1, 0))
        && !mark_is_green(pos + vec2<i32>(0, 1))
        && !mark_is_green(pos + vec2<i32>(0, -1));
}

// Whether a red or blue photosite pairs with a horizontal neighbour. Such
// pairs fill the rows of solitary greens, which darktable finds as
// `(row - sgrow) % 3 == 0`; the other rows hold vertical pairs.
fn mark_in_horizontal_pair(pos: vec2<i32>) -> bool {
    return !mark_is_green(pos + vec2<i32>(1, 0)) || !mark_is_green(pos + vec2<i32>(-1, 0));
}

// darktable's `allhex` entry for `pos` as (column, row) offsets, built from
// the photosite's own neighbours (the green layout repeats every 3 pixels).
fn mark_hexagon(pos: vec2<i32>) -> array<vec2<i32>, 8> {
    var orth = MARK_ORTH;
    var patt = MARK_PATT;
    let g = select(0, 1, mark_is_green(pos));
    var hexagon: array<vec2<i32>, 8>;
    var run = 0;
    for (var d = 0; d < 10; d = d + 2) {
        if mark_is_green(pos + vec2<i32>(orth[d + 2], orth[d])) {
            run = 0;
        } else {
            run = run + 1;
        }
        if run == g + 1 {
            for (var c = 0; c < 8; c = c + 1) {
                let a = patt[g][c * 2];
                let b = patt[g][c * 2 + 1];
                let row = orth[d] * a + orth[d + 1] * b;
                let column = orth[d + 2] * a + orth[d + 3] * b;
                hexagon[c ^ ((g * 2) & d)] = vec2<i32>(column, row);
            }
        }
    }
    return hexagon;
}

// The range darktable clamps the greens of a red or blue photosite to
// (`gmin`, `gmax`): the six greens around the first photosite of its pair,
// the left or the upper one.
fn mark_green_bounds(pos: vec2<i32>) -> vec2<f32> {
    var first = pos;
    if !mark_is_green(pos + vec2<i32>(-1, 0)) {
        first = pos + vec2<i32>(-1, 0);
    }
    if !mark_is_green(pos + vec2<i32>(0, -1)) {
        first = pos + vec2<i32>(0, -1);
    }
    var hexagon = mark_hexagon(first);
    var lo = 3.0e38;
    var hi = 0.0;
    for (var k = 0; k < 6; k = k + 1) {
        let green = mark_initial(first + hexagon[k], 1u);
        lo = min(lo, green);
        hi = max(hi, green);
    }
    return vec2<f32>(lo, hi);
}

// Step 1: the greens of a red or blue photosite in buffers 0-3, interpolated
// horizontally, vertically and along both diagonals with dcraw's weights.
fn mark_initial_green(pos: vec2<i32>) -> vec4<f32> {
    let f = mark_color(pos);
    var hx = mark_hexagon(pos);
    let own = mark_sample(pos);
    let estimates = vec4<f32>(
        0.6796875 * (mark_initial(pos + hx[1], 1u) + mark_initial(pos + hx[0], 1u))
            - 0.1796875 * (mark_initial(pos + 2 * hx[1], 1u) + mark_initial(pos + 2 * hx[0], 1u)),
        0.87109375 * mark_initial(pos + hx[3], 1u) + mark_initial(pos + hx[2], 1u) * 0.13
            + 0.359375 * (own - mark_initial(pos - hx[2], f)),
        0.640625 * mark_initial(pos + hx[4], 1u) + 0.359375 * mark_initial(pos - 2 * hx[4], 1u)
            + 0.12890625 * (2.0 * own - mark_initial(pos + 3 * hx[4], f)
                - mark_initial(pos - 3 * hx[4], f)),
        0.640625 * mark_initial(pos + hx[5], 1u) + 0.359375 * mark_initial(pos - 2 * hx[5], 1u)
            + 0.12890625 * (2.0 * own - mark_initial(pos + 3 * hx[5], f)
                - mark_initial(pos - 3 * hx[5], f)),
    );
    let bounds = mark_green_bounds(pos);
    let greens = clamp(estimates, vec4<f32>(bounds.x), vec4<f32>(bounds.y));
    // In the rows of horizontal pairs buffer c ^ 1 takes direction c.
    return select(greens, greens.yxwz, mark_in_horizontal_pair(pos));
}

fn mark_state_green(state: u32, pos: vec2<i32>) -> vec4<f32> {
    let p = Common::clamp_pos(pos);
    if state == MARK_HI {
        return textureLoad(mark_hi_green, p, 0);
    }
    return textureLoad(mark_lo_green, p, 0);
}

fn mark_state_other(state: u32, pos: vec2<i32>) -> vec4<f32> {
    let p = Common::clamp_pos(pos);
    if state == MARK_HI {
        return textureLoad(mark_hi_other, p, 0);
    }
    return textureLoad(mark_lo_other, p, 0);
}

// Green of the four buffers at any photosite.
fn mark_green4(state: u32, pos: vec2<i32>) -> vec4<f32> {
    if mark_is_green(pos) {
        return vec4<f32>(mark_sample(pos));
    }
    return mark_state_green(state, pos);
}

// Red and blue of the solitary green at `pos` along `axis`: the photosites
// 1 pixel away carry one colour and those 2 pixels away the other, each
// corrected by the green change towards the centre.
fn mark_solitary_axis(state: u32, pos: vec2<i32>, axis: vec2<i32>) -> MarkAxis {
    let center = vec4<f32>(mark_sample(pos));
    let near_plus = mark_green4(state, pos + axis);
    let near_minus = mark_green4(state, pos - axis);
    let far_plus = mark_green4(state, pos + 2 * axis);
    let far_minus = mark_green4(state, pos - 2 * axis);
    let near_samples = vec2<f32>(mark_sample(pos + axis), mark_sample(pos - axis));
    let far_samples = vec2<f32>(mark_sample(pos + 2 * axis), mark_sample(pos - 2 * axis));
    let near_curve = 2.0 * center - near_plus - near_minus;
    let far_curve = 2.0 * center - far_plus - far_minus;
    let near = near_curve + vec4<f32>(near_samples.x + near_samples.y);
    let far = far_curve + vec4<f32>(far_samples.x + far_samples.y);
    let near_slope = near_plus - near_minus - vec4<f32>(near_samples.x - near_samples.y);
    let far_slope = far_plus - far_minus - vec4<f32>(far_samples.x - far_samples.y);
    let steepness = near_slope * near_slope + near_curve * near_curve
        + far_slope * far_slope + far_curve * far_curve;
    let near_is_red = mark_color(pos + axis) == 0u;
    return MarkAxis(
        select(far, near, near_is_red),
        select(near, far, near_is_red),
        steepness,
    );
}

// Step 2: red and blue of a solitary green. Buffer 0 takes the horizontal
// estimate, buffer 1 the vertical one, and buffers 2 and 3 whichever axis is
// smoother, the vertical one on ties.
fn mark_solitary_red_blue(state: u32, pos: vec2<i32>) -> MarkRedBlue {
    let horizontal = mark_solitary_axis(state, pos, vec2<i32>(1, 0));
    let vertical = mark_solitary_axis(state, pos, vec2<i32>(0, 1));
    let use_horizontal = vec4<bool>(
        true,
        false,
        horizontal.steepness.z < vertical.steepness.z,
        horizontal.steepness.w < vertical.steepness.w,
    );
    return MarkRedBlue(
        0.5 * select(vertical.red, horizontal.red, use_horizontal),
        0.5 * select(vertical.blue, horizontal.blue, use_horizontal),
    );
}

// Channel `channel` of the four buffers at a photosite of that colour or at a
// solitary green, the only photosites step 3 reads red or blue from.
fn mark_channel4(state: u32, pos: vec2<i32>, channel: u32) -> vec4<f32> {
    if mark_color(pos) == channel {
        return vec4<f32>(mark_sample(pos));
    }
    let red_blue = mark_solitary_red_blue(state, pos);
    return select(red_blue.blue, red_blue.red, channel == 0u);
}

// Step 3: red at a blue photosite or blue at a red one, buffers 0-3. Each
// buffer interpolates the colour difference to green along the pair's axis,
// or 3 pixels across it when buffer 0 (vertical pairs) or 1 (horizontal
// pairs) finds the greens across it at least twice as smooth.
fn mark_other_colour(state: u32, pos: vec2<i32>) -> vec4<f32> {
    let f = 2u - mark_color(pos);
    let horizontal = mark_in_horizontal_pair(pos);
    let along = select(vec2<i32>(0, 1), vec2<i32>(1, 0), horizontal);
    let across = select(vec2<i32>(3, 0), vec2<i32>(0, 3), horizontal);
    let green = mark_state_green(state, pos);
    let along_plus = mark_green4(state, pos + along);
    let along_minus = mark_green4(state, pos - along);
    let across_plus = mark_green4(state, pos + across);
    let across_minus = mark_green4(state, pos - across);
    let along_change = abs(green - along_plus) + abs(green - along_minus);
    let across_change = abs(green - across_plus) + abs(green - across_minus);
    let always_along = select(
        vec4<bool>(false, true, true, true),
        vec4<bool>(true, false, true, true),
        horizontal,
    );
    let use_along = always_along | (along_change < 2.0 * across_change);
    let along_value = 0.5 * (mark_channel4(state, pos + along, f)
        + mark_channel4(state, pos - along, f) + 2.0 * green - along_plus - along_minus);
    let across_value = 0.5 * (mark_channel4(state, pos + across, f)
        + mark_channel4(state, pos - across, f) + 2.0 * green - across_plus - across_minus);
    return select(across_value, along_value, use_along);
}

fn mark_red_blue_rgb(state: u32, pos: vec2<i32>) -> MarkRgb {
    let own = vec4<f32>(mark_sample(pos));
    let green = mark_state_green(state, pos);
    let other = mark_state_other(state, pos);
    if mark_color(pos) == 0u {
        return MarkRgb(own, green, other);
    }
    return MarkRgb(other, green, own);
}

// RGB of the photosites a 2x2-block green reads: red or blue photosites and
// solitary greens.
fn mark_hexagon_rgb(state: u32, pos: vec2<i32>) -> MarkRgb {
    if mark_is_green(pos) {
        let red_blue = mark_solitary_red_blue(state, pos);
        return MarkRgb(red_blue.red, vec4<f32>(mark_sample(pos)), red_blue.blue);
    }
    return mark_red_blue_rgb(state, pos);
}

// Step 4: red and blue of a 2x2-block green in buffer `n`, from the photosites
// at hexagon entries 2n and 2n + 1: weighted 2:1 unless they are opposite,
// each carrying its colour difference to green.
fn mark_block_red_blue_of(state: u32, pos: vec2<i32>, n: i32, a: vec2<i32>, b: vec2<i32>) -> vec2<f32> {
    let center = mark_sample(pos);
    let pa = mark_hexagon_rgb(state, pos + a);
    let pb = mark_hexagon_rgb(state, pos + b);
    let red_blue_a = vec2<f32>(pa.red[n], pa.blue[n]);
    let red_blue_b = vec2<f32>(pb.red[n], pb.blue[n]);
    if any(a + b != vec2<i32>(0)) {
        let g = 3.0 * center - 2.0 * pa.green[n] - pb.green[n];
        return (vec2<f32>(g) + 2.0 * red_blue_a + red_blue_b) / 3.0;
    }
    let g = 2.0 * center - pa.green[n] - pb.green[n];
    return (vec2<f32>(g) + red_blue_a + red_blue_b) / 2.0;
}

fn mark_block_red_blue(state: u32, pos: vec2<i32>) -> MarkRedBlue {
    var hexagon = mark_hexagon(pos);
    var red = vec4<f32>(0.0);
    var blue = vec4<f32>(0.0);
    for (var n = 0; n < 4; n = n + 1) {
        let red_blue = mark_block_red_blue_of(state, pos, n, hexagon[2 * n], hexagon[2 * n + 1]);
        red[n] = red_blue.x;
        blue[n] = red_blue.y;
    }
    return MarkRedBlue(red, blue);
}

// The four candidates of `state` at any photosite.
fn mark_rgb(state: u32, pos: vec2<i32>) -> MarkRgb {
    if !mark_is_green(pos) {
        return mark_red_blue_rgb(state, pos);
    }
    let green = vec4<f32>(mark_sample(pos));
    var red_blue: MarkRedBlue;
    if mark_is_solitary(pos) {
        red_blue = mark_solitary_red_blue(state, pos);
    } else {
        red_blue = mark_block_red_blue(state, pos);
    }
    return MarkRgb(red_blue.red, green, red_blue.blue);
}

// Candidate `n` of `state` at any photosite.
fn mark_rgb_of(state: u32, pos: vec2<i32>, n: i32) -> vec3<f32> {
    if !mark_is_green(pos) || mark_is_solitary(pos) {
        let rgb = mark_hexagon_rgb(state, pos);
        return vec3<f32>(rgb.red[n], rgb.green[n], rgb.blue[n]);
    }
    var hexagon = mark_hexagon(pos);
    let red_blue = mark_block_red_blue_of(state, pos, n, hexagon[2 * n], hexagon[2 * n + 1]);
    return vec3<f32>(red_blue.x, mark_sample(pos), red_blue.y);
}

// Passes 2 and 3 refine buffers 4-7, starting from the previous state, before
// steps 2 to 4 run again: three of the four greens of a red or blue photosite
// become the colour difference interpolated from the hexagon photosites
// 1 and 2 pixels away (entries 3-5), clamped to the pair's range. The fourth
// keeps its green. Reads the previous state through lo.
fn mark_refined_green(pos: vec2<i32>) -> vec4<f32> {
    let f = mark_color(pos);
    var hexagon = mark_hexagon(pos);
    let swap = select(0, 1, mark_in_horizontal_pair(pos));
    let own = mark_sample(pos);
    let bounds = mark_green_bounds(pos);
    var greens = mark_state_green(MARK_LO, pos);
    for (var d = 3; d < 6; d = d + 1) {
        let n = (d - 2) ^ swap;
        let h = hexagon[d];
        let far = mark_rgb_of(MARK_LO, pos - 2 * h, n);
        let near = mark_rgb_of(MARK_LO, pos + h, n);
        let value = far.g + 2.0 * near.g - far[f] - 2.0 * near[f] + 3.0 * own;
        greens[n] = clamp(value / 3.0, bounds.x, bounds.y);
    }
    return greens;
}

// darktable's perceptual space for the homogeneity test: BT.2020 YPbPr.
fn mark_ypbpr(state: u32, pos: vec2<i32>) -> MarkYpbpr {
    let rgb = mark_rgb(state, pos);
    let y = 0.2627 * rgb.red + 0.6780 * rgb.green + 0.0593 * rgb.blue;
    return MarkYpbpr(y, (rgb.blue - y) * 0.56433, (rgb.red - y) * 0.67815);
}

fn mark_window_index(offset: vec2<i32>) -> i32 {
    return (offset.y + 2) * 5 + offset.x + 2;
}

// The squared YPbPr second derivative of the four candidates of `state` along
// their directions, at the 3x3 pixels around `pos` in row order.
fn mark_derivatives(state: u32, pos: vec2<i32>) -> array<vec4<f32>, 9> {
    var window: array<MarkYpbpr, 25>;
    for (var i = 0; i < 25; i = i + 1) {
        window[i] = mark_ypbpr(state, pos + vec2<i32>(i % 5 - 2, i / 5 - 2));
    }
    var directions = MARK_DIRECTIONS;
    var derivatives: array<vec4<f32>, 9>;
    for (var q = 0; q < 9; q = q + 1) {
        let offset = vec2<i32>(q % 3 - 1, q / 3 - 1);
        let c = mark_window_index(offset);
        for (var n = 0; n < 4; n = n + 1) {
            let plus = mark_window_index(offset + directions[n]);
            let minus = mark_window_index(offset - directions[n]);
            let y = 2.0 * window[c].y[n] - window[plus].y[n] - window[minus].y[n];
            let pb = 2.0 * window[c].pb[n] - window[plus].pb[n] - window[minus].pb[n];
            let pr = 2.0 * window[c].pr[n] - window[plus].pr[n] - window[minus].pr[n];
            derivatives[q][n] = y * y + pb * pb + pr * pr;
        }
    }
    return derivatives;
}
