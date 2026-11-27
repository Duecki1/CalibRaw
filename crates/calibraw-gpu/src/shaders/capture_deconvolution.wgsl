// SPDX-License-Identifier: GPL-3.0-or-later
// Capture sharpening by deconvolution, adapted from darktable 5.6
// (src/iop/demosaicing/capture.c), which ports RawTherapee's
// (rtengine/capturesharpening.cc, Ingo Weyrich).
// Copyright (C) 2025-2026 darktable developers.
// Copyright (C) 2026 CalibRaw contributors (WGSL adaptation).
//
// The last passes of the RAW stage, after colour denoise: the scene's
// luminance is deconvolved with a Gaussian blur by Richardson-Lucy
// iterations where its local contrast clears a noise threshold, and every
// channel is scaled by the luminance's change (CaptureDeconvolution in
// calibraw-core sets the controls).
//
// 1. capture_prepare (scene -> tex1): luminance Y and the unblurred mask:
//    0 within the 21 pixels around a clipped photosite (or pixel of a
//    camera-linear raster) or a pixel darker than CAPTURE_YMIN, otherwise a sigmoid of the local contrast against
//    the contrast sensitivity (darktable's _prepare_blend and _modify_blend).
// 2. capture_mask_horizontal, capture_mask_vertical (tex1 -> tex2 -> tex1):
//    the mask blurred with a Gaussian of sigma 2 (darktable's
//    dt_gaussian_fast_blur, a recursive approximation of the same Gaussian),
//    then mixed back towards the unblurred mask where blurring lowered it
//    much, so fine edges keep their strength.
// 3. capture_divide, capture_multiply (tex1 -> tex2 -> tex1), once per
//    iteration: estimate *= blur(Y / blur(estimate)), the estimate starting
//    at Y. darktable leaves the ratio unwritten where the mask is 0 and its
//    buffer keeps older contents there; it is computed everywhere here, and
//    is 1 where blur(estimate) is below CAPTURE_YMIN.
// 4. capture_apply (tex1 and scene -> tex2): the estimate, its last
//    iteration counted by its share, mixed into Y by the mask and Masking's
//    edge restriction; each channel is scaled by the new luminance over Y,
//    except where Y is below CAPTURE_YMIN.
// 5. capture_store (tex2 -> scene).
//
// The work textures hold (Y, mask, estimate, the previous estimate or the
// ratio). Colour space: white-balanced camera RGB from the demosaic or
// raster finish, balanced by Common::scene_neutral_gains for a camera-linear
// raster, as darktable's demosaic output; Y takes BT.709 weights as
// darktable's. Values may be negative (read noise) or above 1 (highlight
// reconstruction); Y is floored at 0.
// Precision: the state lives in the work format, rgba32float for an export
// and rgba16float for a preview. In half precision a ratio near 1 resolves to
// about 0.001 and every pass rounds the estimate to about 0.05%, so over many
// iterations a 1:1 preview can differ slightly from the export of the same
// edit. The arithmetic is f32 in both.
// Coordinates: tile-local pixels, neighbours clamped to the tile, whose
// export halo covers CAPTURE_DECONVOLUTION_MAX_SUPPORT; the blur narrows over
// the 8 pixels at the full image's edge and the mask is 0 on its outer 2, as
// darktable's, by full-image position. The sigma is in processing pixels.

#import calibraw::common as Common
#import calibraw::raw_sampling as RawSampling

@group(0) @binding(11) var capture_read: texture_2d<f32>;
@group(0) @binding(12) var capture_scene: texture_2d<f32>;
@group(0) @binding(15) var capture_clipping_tex: texture_2d<f32>;
// A raster's source pixels: a camera-linear raster's normalized sensor RGB.
@group(0) @binding(16) var capture_source_tex: texture_2d<f32>;
@group(0) @binding(10) var capture_write: texture_storage_2d<rgba16float /* CALIBRAW_WORK_FORMAT */, write>;

// Luminance below which a pixel is neither sharpened nor divided by
// (darktable's CAPTURE_YMIN).
const CAPTURE_YMIN: f32 = 0.001;
// darktable's photometric BT.709 weights.
const CAPTURE_LUMINANCE: vec3<f32> = vec3<f32>(0.212671, 0.715160, 0.072169);
// Sigmas below this blur with the 5x5 kernel, others with the 9x9 one.
const CAPTURE_SMALL_SIGMA: f32 = 0.66;
// The mask's Gaussian blur and the taps a side it takes (3 sigma).
const CAPTURE_MASK_SIGMA: f32 = 2.0;
const CAPTURE_MASK_TAPS: i32 = 6;
// The smallest positive normal f32, darktable's NORM_MIN.
const CAPTURE_NORM_MIN: f32 = 1.17549435e-38;
// CameraUniforms::capture_clip.y; CAPTURE_CLIP_* in params.rs.
const CAPTURE_CLIP_MOSAIC: f32 = 1.0;
const CAPTURE_CLIP_TEXTURE: f32 = 2.0;
const CAPTURE_CLIP_RASTER: f32 = 3.0;
// Masking's edge restriction: the thresholds of the legacy capture
// sharpening's (capture_mask_coherence.xy in detail_capture.wgsl), on the
// gradient of log2 luminance.
const CAPTURE_EDGE_LOW: f32 = 0.035;
const CAPTURE_EDGE_HIGH: f32 = 0.62;

fn capture_luminance(rgb: vec3<f32>) -> f32 {
    return max(dot(rgb * Common::scene_neutral_gains(), CAPTURE_LUMINANCE), 0.0);
}

fn capture_global(pos: vec2<i32>) -> vec2<i32> {
    return pos + Common::tile_origin();
}

// Pixels from `pos` to the nearest edge of the full image.
fn capture_edge_distance(pos: vec2<i32>) -> i32 {
    let global = capture_global(pos);
    let far = Common::full_image_max() - global;
    return min(min(global.x, global.y), min(far.x, far.y));
}

fn capture_clipped(pos: vec2<i32>) -> bool {
    let source = Common::camera_uniforms.capture_clip.y;
    if source == CAPTURE_CLIP_MOSAIC {
        return RawSampling::raw_sensor_at(pos) >= Common::camera_uniforms.capture_clip.x;
    }
    if source == CAPTURE_CLIP_TEXTURE {
        let share = textureLoad(capture_clipping_tex, Common::clamp_pos(pos), 0).rgb;
        return max(share.r, max(share.g, share.b)) > 0.0;
    }
    if source == CAPTURE_CLIP_RASTER {
        let sensor = textureLoad(capture_source_tex, Common::clamp_pos(pos), 0).rgb;
        return max(sensor.r, max(sensor.g, sensor.b)) >= Common::camera_uniforms.capture_clip.x;
    }
    return false;
}

fn capture_scene_luminance_at(pos: vec2<i32>) -> f32 {
    return capture_luminance(textureLoad(capture_read, Common::clamp_pos(pos), 0).xyz);
}

fn capture_read_at(pos: vec2<i32>) -> vec4<f32> {
    return textureLoad(capture_read, Common::clamp_pos(pos), 0);
}

// Whether `offset` is one of the 21 pixels darktable's mask reads: the 5x5
// square without its corners.
fn capture_in_mask_window(offset: vec2<i32>) -> bool {
    return !(abs(offset.x) == 2 && abs(offset.y) == 2);
}

@compute @workgroup_size(8, 8, 1)
fn capture_prepare(@builtin(global_invocation_id) dispatch_gid: vec3<u32>) {
    let gid = Common::dispatch_id(dispatch_gid);
    if gid.x >= Common::camera_uniforms.width || gid.y >= Common::camera_uniforms.height { return; }
    let pos = vec2<i32>(i32(gid.x), i32(gid.y));

    let center = capture_scene_luminance_at(pos);
    var valid = capture_edge_distance(pos) >= 2;
    var sum = 0.0;
    var sum_sq = 0.0;
    for (var dy = -2; dy <= 2; dy = dy + 1) {
        for (var dx = -2; dx <= 2; dx = dx + 1) {
            let offset = vec2<i32>(dx, dy);
            if !capture_in_mask_window(offset) { continue; }
            let y = capture_scene_luminance_at(pos + offset);
            if y < CAPTURE_YMIN || capture_clipped(pos + offset) {
                valid = false;
            }
            // Relative to the centre, as darktable computes it since 5.6.
            let delta = y - center;
            sum = sum + delta;
            sum_sq = sum_sq + delta * delta;
        }
    }
    var mask = 0.0;
    if valid {
        let mean_delta = sum / 21.0;
        let mean = max(CAPTURE_NORM_MIN, center + mean_delta);
        let variance = max(0.0, sum_sq / 21.0 - mean_delta * mean_delta);
        // The standard deviation over the square root of the mean, which
        // shot noise alone keeps constant.
        let t = log(1.0 + sqrt(variance / mean));
        let sensitivity = Common::camera_uniforms.capture_sharpening.y;
        let threshold = 0.6 * sensitivity * sensitivity;
        let offset = -2.5 + 100.0 * threshold;
        let weight = 1.0 / (1.0 + exp(offset - 200.0 * t));
        mask = clamp(1.01011 * (weight - 0.01), 0.0, 1.0);
    }
    textureStore(capture_write, pos, vec4<f32>(center, mask, 0.0, 0.0));
}

fn capture_mask_weight(offset: i32) -> f32 {
    let distance = f32(offset);
    return exp(-0.5 * distance * distance / (CAPTURE_MASK_SIGMA * CAPTURE_MASK_SIGMA));
}

@compute @workgroup_size(8, 8, 1)
fn capture_mask_horizontal(@builtin(global_invocation_id) dispatch_gid: vec3<u32>) {
    let gid = Common::dispatch_id(dispatch_gid);
    if gid.x >= Common::camera_uniforms.width || gid.y >= Common::camera_uniforms.height { return; }
    let pos = vec2<i32>(i32(gid.x), i32(gid.y));

    var sum = 0.0;
    var weight_sum = 0.0;
    for (var dx = -CAPTURE_MASK_TAPS; dx <= CAPTURE_MASK_TAPS; dx = dx + 1) {
        let weight = capture_mask_weight(dx);
        sum = sum + weight * capture_read_at(pos + vec2<i32>(dx, 0)).y;
        weight_sum = weight_sum + weight;
    }
    let center = capture_read_at(pos);
    textureStore(capture_write, pos, vec4<f32>(center.x, center.y, sum / weight_sum, 0.0));
}

@compute @workgroup_size(8, 8, 1)
fn capture_mask_vertical(@builtin(global_invocation_id) dispatch_gid: vec3<u32>) {
    let gid = Common::dispatch_id(dispatch_gid);
    if gid.x >= Common::camera_uniforms.width || gid.y >= Common::camera_uniforms.height { return; }
    let pos = vec2<i32>(i32(gid.x), i32(gid.y));

    var sum = 0.0;
    var weight_sum = 0.0;
    for (var dy = -CAPTURE_MASK_TAPS; dy <= CAPTURE_MASK_TAPS; dy = dy + 1) {
        let weight = capture_mask_weight(dy);
        sum = sum + weight * capture_read_at(pos + vec2<i32>(0, dy)).z;
        weight_sum = weight_sum + weight;
    }
    let center = capture_read_at(pos);
    let unblurred = center.y;
    let blurred = sum / weight_sum;
    // Where the blur lowered the mask much, the unblurred value counts.
    let keep = 1.0 / (1.0 + exp(5.0 - 10.0 * (unblurred - blurred)));
    let mask = clamp(keep * unblurred + (1.0 - keep) * blurred, 0.0, 1.0);
    let y = center.x;
    textureStore(capture_write, pos, vec4<f32>(y, mask, y, y));
}

// The blur's sigma at `pos`: the uniform's, narrowed to 0 over the 8 pixels
// at the full image's edge.
fn capture_sigma_at(pos: vec2<i32>) -> f32 {
    let edge = f32(min(8, capture_edge_distance(pos)));
    return Common::camera_uniforms.capture_sharpening.x * 0.125 * max(edge, 0.0);
}

// The read texture's `channel` blurred around `pos` by darktable's kernel of
// `sigma`: a Gaussian over the pixels within 2.5 (sigma below
// CAPTURE_SMALL_SIGMA) or 4.5 pixels, normalized over them.
fn capture_blur(pos: vec2<i32>, sigma: f32, channel: u32) -> f32 {
    let s = max(sigma, 1e-7);
    let small = s < CAPTURE_SMALL_SIGMA;
    let reach = select(4, 2, small);
    let range_sq = select(20.25, 6.25, small);
    var gauss: array<f32, 5>;
    for (var d = 0; d < 5; d = d + 1) {
        gauss[d] = exp(-f32(d * d) / (2.0 * s * s));
    }
    var sum = 0.0;
    var weight_sum = 0.0;
    for (var dy = -reach; dy <= reach; dy = dy + 1) {
        for (var dx = -reach; dx <= reach; dx = dx + 1) {
            if f32(dx * dx + dy * dy) > range_sq { continue; }
            let weight = gauss[abs(dx)] * gauss[abs(dy)];
            sum = sum + weight * capture_read_at(pos + vec2<i32>(dx, dy))[channel];
            weight_sum = weight_sum + weight;
        }
    }
    return sum / weight_sum;
}

@compute @workgroup_size(8, 8, 1)
fn capture_divide(@builtin(global_invocation_id) dispatch_gid: vec3<u32>) {
    let gid = Common::dispatch_id(dispatch_gid);
    if gid.x >= Common::camera_uniforms.width || gid.y >= Common::camera_uniforms.height { return; }
    let pos = vec2<i32>(i32(gid.x), i32(gid.y));

    let center = capture_read_at(pos);
    let blurred = capture_blur(pos, capture_sigma_at(pos), 2u);
    // Too dark to divide by: a neutral ratio, so that the blur of the ratios
    // does not darken the estimates of the brighter pixels around it.
    let ratio = select(1.0, center.x / blurred, blurred >= CAPTURE_YMIN);
    textureStore(capture_write, pos, vec4<f32>(center.x, center.y, center.z, ratio));
}

@compute @workgroup_size(8, 8, 1)
fn capture_multiply(@builtin(global_invocation_id) dispatch_gid: vec3<u32>) {
    let gid = Common::dispatch_id(dispatch_gid);
    if gid.x >= Common::camera_uniforms.width || gid.y >= Common::camera_uniforms.height { return; }
    let pos = vec2<i32>(i32(gid.x), i32(gid.y));

    let center = capture_read_at(pos);
    var estimate = center.z;
    if center.y > 0.0 {
        estimate = estimate * capture_blur(pos, capture_sigma_at(pos), 3u);
    }
    textureStore(capture_write, pos, vec4<f32>(center.x, center.y, estimate, center.z));
}

fn capture_log_luminance_at(pos: vec2<i32>) -> f32 {
    return log2(max(capture_read_at(pos).x, 1e-6));
}

// Masking's restriction to edges at `pos`, 1 without Masking: the legacy
// capture sharpening's edge mask on the central gradient of log2 Y.
fn capture_edge_mask(pos: vec2<i32>) -> f32 {
    let masking = Common::camera_uniforms.capture_sharpening.z;
    if masking <= 1e-6 {
        return 1.0;
    }
    let gradient = vec2<f32>(
        capture_log_luminance_at(pos + vec2<i32>(1, 0)) - capture_log_luminance_at(pos - vec2<i32>(1, 0)),
        capture_log_luminance_at(pos + vec2<i32>(0, 1)) - capture_log_luminance_at(pos - vec2<i32>(0, 1)),
    );
    let threshold = mix(CAPTURE_EDGE_LOW, CAPTURE_EDGE_HIGH, pow(masking, 1.35));
    return smoothstep(threshold * 0.72, threshold + 0.16, length(gradient));
}

@compute @workgroup_size(8, 8, 1)
fn capture_apply(@builtin(global_invocation_id) dispatch_gid: vec3<u32>) {
    let gid = Common::dispatch_id(dispatch_gid);
    if gid.x >= Common::camera_uniforms.width || gid.y >= Common::camera_uniforms.height { return; }
    let pos = vec2<i32>(i32(gid.x), i32(gid.y));

    let scene = textureLoad(capture_scene, pos, 0);
    let state = capture_read_at(pos);
    var rgb = scene.xyz;
    let mask = clamp(state.y * capture_edge_mask(pos), 0.0, 1.0);
    let y = state.x;
    // The mask's blur can leave a pixel darker than CAPTURE_YMIN a share of
    // its neighbours' mask; it is not sharpened, as `capture_prepare` intends,
    // rather than scaled by a luminance it does not have.
    if mask > 0.0 && y >= CAPTURE_YMIN {
        let estimate = mix(state.w, state.z, Common::camera_uniforms.capture_sharpening.w);
        let sharpened = mix(y, estimate, mask);
        rgb = rgb * (sharpened / y);
    }
    textureStore(capture_write, pos, vec4<f32>(rgb, scene.w));
}

@compute @workgroup_size(8, 8, 1)
fn capture_store(@builtin(global_invocation_id) dispatch_gid: vec3<u32>) {
    let gid = Common::dispatch_id(dispatch_gid);
    if gid.x >= Common::camera_uniforms.width || gid.y >= Common::camera_uniforms.height { return; }
    let pos = vec2<i32>(i32(gid.x), i32(gid.y));
    textureStore(capture_write, pos, textureLoad(capture_read, pos, 0));
}
