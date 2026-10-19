// SPDX-License-Identifier: GPL-3.0-or-later
// Adapted from darktable 5.6.1 Markesteijn X-Trans demosaicing.
// Copyright (C) 2010-2026 darktable developers.
// Markesteijn algorithm credit: Frank Markesteijn (via dcraw and darktable).
// Copyright (C) 2026 CalibRaw contributors (WGSL adaptation).
//
// The dispatches of Markesteijn 3-pass (xtrans/markesteijn.wgsl), in order:
// green; other; recalculate, other; recalculate, other; homogeneity;
// accumulate; smoothing. Each writes every pixel of its output.

#import calibraw::common as Common
#import calibraw::cfa_linear as CfaLinear
#import calibraw::dual_demosaic as Dual
#import calibraw::xtrans::markesteijn as Mark

// The state texture a pass builds: greens (green and recalculate passes) or
// other colours (other passes) of red and blue photosites, the accumulated
// result or the smoothed one.
@group(0) @binding(24) var mark_state_write: texture_storage_2d<rgba16float /* CALIBRAW_WORK_FORMAT */, write>;
// Homogeneity counts, 4 bits per candidate: buffers 0-3 in bits 0-15, 4-7 in
// bits 16-31.
@group(0) @binding(25) var mark_homogeneity_write: texture_storage_2d<r32uint, write>;
@group(0) @binding(31) var mark_homogeneity_read: texture_2d<u32>;
// The accumulated result, read by the smoothing pass.
@group(0) @binding(26) var mark_result_read: texture_2d<f32>;

// darktable fills this many pixels at the image border with the linear
// interpolation instead (`pad_tile` of the 3-pass variant).
const MARK_IMAGE_BORDER: i32 = 17;

fn mark_in_tile(gid: vec3<u32>) -> bool {
    return gid.x < Common::camera_uniforms.width && gid.y < Common::camera_uniforms.height;
}

// Greens of buffers 0-3; green photosites store their sample.
@compute @workgroup_size(8, 8, 1)
fn xtrans_markesteijn_green(@builtin(global_invocation_id) gid: vec3<u32>) {
    if !mark_in_tile(gid) { return; }
    let pos = vec2<i32>(gid.xy);
    var greens = vec4<f32>(Mark::mark_sample(pos));
    if !Mark::mark_is_green(pos) {
        greens = Mark::mark_initial_green(pos);
    }
    textureStore(mark_state_write, pos, greens);
}

// Other colours of the state whose greens are bound as lo; 0 at greens.
@compute @workgroup_size(8, 8, 1)
fn xtrans_markesteijn_other(@builtin(global_invocation_id) gid: vec3<u32>) {
    if !mark_in_tile(gid) { return; }
    let pos = vec2<i32>(gid.xy);
    var other = vec4<f32>(0.0);
    if !Mark::mark_is_green(pos) {
        other = Mark::mark_other_colour(Mark::MARK_LO, pos);
    }
    textureStore(mark_state_write, pos, other);
}

// Greens of the next state of buffers 4-7 from the state bound as lo.
@compute @workgroup_size(8, 8, 1)
fn xtrans_markesteijn_recalculate(@builtin(global_invocation_id) gid: vec3<u32>) {
    if !mark_in_tile(gid) { return; }
    let pos = vec2<i32>(gid.xy);
    var greens = vec4<f32>(Mark::mark_sample(pos));
    if !Mark::mark_is_green(pos) {
        greens = Mark::mark_refined_green(pos);
    }
    textureStore(mark_state_write, pos, greens);
}

fn mark_pack_counts(counts: vec4<u32>) -> u32 {
    return counts.x | (counts.y << 4u) | (counts.z << 8u) | (counts.w << 12u);
}

fn mark_unpack_counts(packed: u32) -> vec4<u32> {
    return (vec4<u32>(packed) >> vec4<u32>(0u, 4u, 8u, 12u)) & vec4<u32>(15u);
}

// darktable's homogeneity map: for each candidate, how many of the 3x3
// pixels around this one have a derivative at most 8 times the smallest
// derivative here.
@compute @workgroup_size(8, 8, 1)
fn xtrans_markesteijn_homogeneity(@builtin(global_invocation_id) gid: vec3<u32>) {
    if !mark_in_tile(gid) { return; }
    let pos = vec2<i32>(gid.xy);
    var lo = Mark::mark_derivatives(Mark::MARK_LO, pos);
    var hi = Mark::mark_derivatives(Mark::MARK_HI, pos);
    let smallest = min(lo[4], hi[4]);
    let threshold = vec4<f32>(8.0 * min(min(smallest.x, smallest.y), min(smallest.z, smallest.w)));
    var lo_count = vec4<u32>(0u);
    var hi_count = vec4<u32>(0u);
    for (var q = 0; q < 9; q = q + 1) {
        lo_count += select(vec4<u32>(0u), vec4<u32>(1u), lo[q] <= threshold);
        hi_count += select(vec4<u32>(0u), vec4<u32>(1u), hi[q] <= threshold);
    }
    let packed = mark_pack_counts(lo_count) | (mark_pack_counts(hi_count) << 16u);
    textureStore(mark_homogeneity_write, pos, vec4<u32>(packed, 0u, 0u, 0u));
}

fn mark_near_image_border(pos: vec2<i32>) -> bool {
    let image = pos + Common::tile_origin();
    let extent = vec2<i32>(
        i32(Common::camera_uniforms.full_width),
        i32(Common::camera_uniforms.full_height),
    );
    return any(image < vec2<i32>(MARK_IMAGE_BORDER))
        || any(image >= extent - vec2<i32>(MARK_IMAGE_BORDER));
}

// The result: the mean of the candidates whose 5x5 homogeneity is within an
// eighth of the best, after dropping the less homogeneous of each pair of
// buffers d and d + 4.
@compute @workgroup_size(8, 8, 1)
fn xtrans_markesteijn_accumulate(@builtin(global_invocation_id) gid: vec3<u32>) {
    if !mark_in_tile(gid) { return; }
    let pos = vec2<i32>(gid.xy);
    if mark_near_image_border(pos) {
        textureStore(mark_state_write, pos, vec4<f32>(CfaLinear::cfa_linear_rgb(pos, false), 1.0));
        return;
    }
    var lo = vec4<u32>(0u);
    var hi = vec4<u32>(0u);
    for (var dy = -2; dy <= 2; dy = dy + 1) {
        for (var dx = -2; dx <= 2; dx = dx + 1) {
            let packed = textureLoad(
                mark_homogeneity_read,
                Common::clamp_pos(pos + vec2<i32>(dx, dy)),
                0,
            ).x;
            lo += mark_unpack_counts(packed);
            hi += mark_unpack_counts(packed >> 16u);
        }
    }
    var best = max(max(max(lo.x, lo.y), max(lo.z, lo.w)), max(max(hi.x, hi.y), max(hi.z, hi.w)));
    best -= best >> 3u;
    let keep_lo = select(lo, vec4<u32>(0u), lo < hi);
    let keep_hi = select(hi, vec4<u32>(0u), hi < lo);
    let use_lo = select(vec4<f32>(0.0), vec4<f32>(1.0), keep_lo >= vec4<u32>(best));
    let use_hi = select(vec4<f32>(0.0), vec4<f32>(1.0), keep_hi >= vec4<u32>(best));
    let lo_rgb = Mark::mark_rgb(Mark::MARK_LO, pos);
    let hi_rgb = Mark::mark_rgb(Mark::MARK_HI, pos);
    let sum = vec3<f32>(
        dot(lo_rgb.red, use_lo) + dot(hi_rgb.red, use_hi),
        dot(lo_rgb.green, use_lo) + dot(hi_rgb.green, use_hi),
        dot(lo_rgb.blue, use_lo) + dot(hi_rgb.blue, use_hi),
    );
    let count = dot(use_lo, vec4<f32>(1.0)) + dot(use_hi, vec4<f32>(1.0));
    textureStore(mark_state_write, pos, vec4<f32>(max(sum / count, vec3<f32>(0.0)), 1.0));
}

// One pass of darktable's colour smoothing (`color_smoothing`, the "color
// smoothing: once" option of its demosaic module) over the result: red and
// blue become green plus the 3x3 median of their difference to green, the
// image border keeps its value. Markesteijn gives a bright point one or two
// photosites wide the colour of the photosites it covers, which leaves
// rainbow specks around small highlights; the median removes them. darktable
// smooths after dual demosaicing; here every finish mode reads the smoothed
// result.
@compute @workgroup_size(8, 8, 1)
fn xtrans_markesteijn_smooth(@builtin(global_invocation_id) gid: vec3<u32>) {
    if !mark_in_tile(gid) { return; }
    let pos = vec2<i32>(gid.xy);
    if CfaLinear::cfa_on_border(pos) {
        textureStore(mark_state_write, pos, textureLoad(mark_result_read, pos, 0));
        return;
    }
    var window: array<vec3<f32>, 9>;
    for (var i = 0; i < 9; i = i + 1) {
        window[i] = textureLoad(mark_result_read, pos + Dual::dual_window_offset(i), 0).rgb;
    }
    textureStore(mark_state_write, pos, vec4<f32>(Dual::dual_smoothing_step(window), 1.0));
}
