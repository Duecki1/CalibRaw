// SPDX-License-Identifier: GPL-3.0-or-later
// Adapted from darktable 5.6.0 hlreconstruct/opposed code.
// Copyright (C) 2010-2026 darktable developers.
// Copyright (C) 2026 CalibRaw contributors (WGSL adaptation).

#import calibraw::common as Common
#import calibraw::raw_sampling as RawSampling

@group(0) @binding(3) var reconstructed_raw_write: texture_storage_2d<r32float, write>;
// Lens vignetting gains (LensShadingMap in Rust): linear gains per CFA
// channel (R, G1, B, G2 in r, g, b, a) on a square grid whose nodes span the
// full image's pixel centres corner to corner. All 1 without lens shading.
@group(0) @binding(13) var lens_shading_tex: texture_2d<f32>;
// A binned source (BinnedSensor in Rust) instead of a mosaic: normalized
// sensor RGB, the AI-denoised RGB binned alike (a 1x1 placeholder without
// one), and where its reconstructed camera RGB goes.
@group(0) @binding(11) var binned_sensor_tex: texture_2d<f32>;
@group(0) @binding(12) var binned_ai_tex: texture_2d<f32>;
@group(0) @binding(10) var binned_camera_write: texture_storage_2d<rgba16float /* CALIBRAW_WORK_FORMAT */, write>;

const DARKTABLE_SQRT3: f32 = 1.7320508075688772;
const DARKTABLE_SQRT12: f32 = 3.4641016151377544;
const DARKTABLE_OPPOSED_CLIP_MAGIC: f32 = 0.987;

// The white-balanced level every channel of a clipped LCh block is limited to,
// as darktable's clip * processed_minimum. One level for all channels keeps a
// fully clipped block neutral; per-channel levels (clip * wb[channel]) would
// keep the white-balance ratio and leave blown areas tinted. Detection stays in
// sensor space (RawSampling::is_raw_clipped).
fn lch_chroma_clip() -> f32 {
    let wb = Common::camera_uniforms.wb;
    let lowest_wb = min(min(wb.r, wb.g), min(wb.b, wb.a));
    return RawSampling::shared_highlight_sensor_clip() * max(lowest_wb, 1e-6);
}

// darktable's LCh reconstruction of a clipped block of white-balanced camera
// RGB, `g_min` and `g_max` being its darkest and brightest green: lightness
// from the recorded values, chroma scaled down to what the values limited to
// `lch_chroma_clip` keep.
fn lch_recovered_rgb(r: f32, g_min: f32, g_max: f32, b: f32) -> vec3<f32> {
    let clip = lch_chroma_clip();
    let ro = min(r, clip);
    let go = min(g_min, clip);
    let bo = min(b, clip);
    let lightness = (r + g_max + b) / 3.0;
    var chroma = DARKTABLE_SQRT3 * (r - g_max);
    var hue_axis = 2.0 * b - g_max - r;
    let clipped_chroma = DARKTABLE_SQRT3 * (ro - go);
    let clipped_hue_axis = 2.0 * bo - go - ro;

    if r != g_max && g_max != b {
        let denominator = chroma * chroma + hue_axis * hue_axis;
        if denominator > 1e-12 {
            let numerator = max(
                clipped_chroma * clipped_chroma + clipped_hue_axis * clipped_hue_axis,
                0.0,
            );
            let ratio = sqrt(numerator / denominator);
            chroma = chroma * ratio;
            hue_axis = hue_axis * ratio;
        }
    }

    return vec3<f32>(
        lightness - hue_axis / 6.0 + chroma / DARKTABLE_SQRT12,
        lightness - hue_axis / 6.0 - chroma / DARKTABLE_SQRT12,
        lightness + hue_axis / 3.0,
    );
}

fn lch_reconstructed_cfa_at(pos: vec2<i32>) -> f32 {
    let center = Common::clamp_pos(pos);
    let center_color = RawSampling::color_at(center);
    let original = RawSampling::raw_camera_at(center);
    let clip = lch_chroma_clip();
    let strength = clamp(Common::camera_uniforms.highlight_reconstruction, 0.0, 1.0);

    if center.x >= i32(Common::camera_uniforms.width) - 1 || center.y >= i32(Common::camera_uniforms.height) - 1 {
        let limited = select(original, min(original, clip), RawSampling::is_raw_clipped(center));
        return mix(original, limited, strength);
    }

    var r = 0.0;
    var b = 0.0;
    var g_min = 1e20;
    var g_max = -1e20;
    var have_r = false;
    var have_b = false;
    var greens = 0u;
    var clipped = false;

    for (var dy = 0; dy <= 1; dy = dy + 1) {
        for (var dx = 0; dx <= 1; dx = dx + 1) {
            let p = center + vec2<i32>(dx, dy);
            let channel = RawSampling::color_at(p);
            let value = RawSampling::raw_camera_at(p);
            clipped = clipped || RawSampling::is_raw_clipped(p);
            if channel == 0u {
                r = value;
                have_r = true;
            } else if channel == 1u {
                g_min = min(g_min, value);
                g_max = max(g_max, value);
                greens = greens + 1u;
            } else {
                b = value;
                have_b = true;
            }
        }
    }

    if !have_r || !have_b || greens < 2u || !clipped {
        return original;
    }

    let rgb = lch_recovered_rgb(r, g_min, g_max, b);
    let recovered = select(select(rgb.r, rgb.g, center_color == 1u), rgb.b, center_color == 2u);
    return mix(original, max(recovered, 0.0), strength);
}

fn inpaint_opposed_refavg(pos: vec2<i32>) -> f32 {
    let center = Common::clamp_pos(pos);
    let color = RawSampling::color_at(center);
    var mean = vec3<f32>(0.0);
    var count = vec3<f32>(0.0);
    let max_row = max(i32(Common::camera_uniforms.height) - 1, 0);
    let max_col = max(i32(Common::camera_uniforms.width) - 1, 0);
    let row_end = min(max_row, center.y + 2);
    let col_end = min(max_col, center.x + 2);

    for (var row = max(0, center.y - 1); row < row_end; row = row + 1) {
        for (var col = max(0, center.x - 1); col < col_end; col = col + 1) {
            let sample_pos = vec2<i32>(col, row);
            let sample_color = RawSampling::color_at(sample_pos);
            mean[sample_color] = mean[sample_color] + max(RawSampling::raw_camera_at(sample_pos), 0.0);
            count[sample_color] = count[sample_color] + 1.0;
        }
    }
    for (var channel = 0u; channel < 3u; channel = channel + 1u) {
        mean[channel] = select(0.0, pow(mean[channel] / count[channel], 1.0 / 3.0), count[channel] > 0.0);
    }
    return opposed_reference(mean, color);
}

// The level opposed reconstruction expects of colour `color` (0 R, 1 G,
// 2 B) from the cube roots `roots` of each colour's mean around it: the cube
// of the mean root of the other two colours.
fn opposed_reference(roots: vec3<f32>, color: u32) -> f32 {
    let root_reference = select(
        select(0.5 * (roots.r + roots.b), 0.5 * (roots.g + roots.b), color == 0u),
        0.5 * (roots.r + roots.g),
        color == 2u,
    );
    return root_reference * root_reference * root_reference;
}

fn inpaint_opposed_cfa_at(pos: vec2<i32>) -> f32 {
    let center = Common::clamp_pos(pos);
    let physical_channel = RawSampling::cfa_channel_at(center);
    let color = RawSampling::color_at(center);
    let original = RawSampling::raw_camera_at(center);
    let clip = DARKTABLE_OPPOSED_CLIP_MAGIC
        * max(Common::camera_uniforms.highlight_clip, 0.01)
        * RawSampling::wb_for_cfa_channel(physical_channel);
    if original < clip {
        return original;
    }
    let reference = inpaint_opposed_refavg(center);
    // y/z/w are estimated once from the full active RAW source with the same
    // adjusted WB carried in camera_uniforms.wb, then shared by proxies/tiles.
    let full_source_chrominance = Common::camera_uniforms.highlight_options[color + 1u];
    return max(original, reference + full_source_chrominance);
}

// The lens shading gains of every CFA channel (R, G1, B, G2) at tile-local
// `pos`, interpolated bilinearly between grid nodes. Positions outside the
// full image, such as tile halos, take the gains at its edge.
fn lens_shading_gains(pos: vec2<i32>) -> vec4<f32> {
    let last_node = vec2<i32>(textureDimensions(lens_shading_tex)) - vec2<i32>(1);
    let full_extent = max(vec2<f32>(Common::full_image_max()), vec2<f32>(1.0));
    let global = clamp(vec2<f32>(pos + Common::tile_origin()), vec2<f32>(0.0), full_extent);
    let grid = global / full_extent * vec2<f32>(last_node);
    let p0 = clamp(vec2<i32>(floor(grid)), vec2<i32>(0), max(last_node - vec2<i32>(1), vec2<i32>(0)));
    let p1 = min(p0 + vec2<i32>(1), last_node);
    let f = clamp(grid - vec2<f32>(p0), vec2<f32>(0.0), vec2<f32>(1.0));
    let a = textureLoad(lens_shading_tex, p0, 0);
    let b = textureLoad(lens_shading_tex, vec2<i32>(p1.x, p0.y), 0);
    let c = textureLoad(lens_shading_tex, vec2<i32>(p0.x, p1.y), 0);
    let d = textureLoad(lens_shading_tex, p1, 0);
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

// Reconstructs clipped highlights on the recorded mosaic, then applies the
// lens shading gain. Clip detection and opposed-highlight chroma thus see the
// sensor values: a corner brightened by vignetting correction is not clipped.
@compute @workgroup_size(8, 8, 1)
fn highlight_reconstruct(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= Common::camera_uniforms.width || gid.y >= Common::camera_uniforms.height { return; }
    let pos = vec2<i32>(i32(gid.x), i32(gid.y));
    let method = Common::camera_uniforms.highlight_options.x;
    var output = RawSampling::raw_camera_at(pos);
    if method >= 0.5 && method < 1.5 {
        output = lch_reconstructed_cfa_at(pos);
    } else if method >= 1.5 {
        output = inpaint_opposed_cfa_at(pos);
    }
    let shading = lens_shading_gains(pos)[RawSampling::cfa_channel_at(pos)];
    textureStore(
        reconstructed_raw_write,
        pos,
        vec4<f32>(output * shading, 0.0, 0.0, 1.0),
    );
}

// The sensor RGB of a binned source at tile-local `pos`, normalized as
// RawSampling::raw_sensor_at normalizes a photosite: the AI-denoised samples
// blended in and the black point applied. The source is normalized at black
// point 0, where raising the black by a fraction `o` of the range takes a
// value v to (v - o) / (1 - o); raw_sensor_at keeps that black between 0 and
// one below white, which bounds `o` per channel. Green uses the first green's
// levels.
fn binned_sensor_at(pos: vec2<i32>) -> vec3<f32> {
    let p = Common::clamp_pos(pos);
    var samples = textureLoad(binned_sensor_tex, p, 0).rgb;
    let ai_weight = Common::camera_uniforms.ai_cfa_weight;
    if ai_weight >= 1.0 {
        samples = textureLoad(binned_ai_tex, p, 0).rgb;
    } else if ai_weight > 0.0 {
        samples = mix(samples, textureLoad(binned_ai_tex, p, 0).rgb, ai_weight);
    }
    let black = Common::camera_uniforms.black_levels.rgb;
    let white = max(Common::camera_uniforms.white_levels.rgb, black + 1.0);
    let range = white - black;
    let offset = clamp(
        vec3<f32>(clamp(Common::camera_uniforms.black_point, -0.25, 0.25)),
        -black / range,
        (range - 1.0) / range,
    );
    return clamp((samples - offset) / (1.0 - offset), vec3<f32>(-1.0), vec3<f32>(4.0));
}

// White-balanced camera RGB of a binned source, the units of
// RawSampling::raw_camera_at. The Rust side gives wb.g the mean multiplier of
// the greens a binned green averages.
fn binned_camera_at(pos: vec2<i32>) -> vec3<f32> {
    return binned_sensor_at(pos) * Common::camera_uniforms.wb.rgb;
}

// LCh reconstruction of a binned pixel: lch_reconstructed_cfa_at with the
// pixel's own colours as the 2x2 block, which one clipped colour marks.
fn binned_lch_rgb(sensor: vec3<f32>, camera: vec3<f32>) -> vec3<f32> {
    if !any(sensor >= vec3<f32>(RawSampling::shared_highlight_sensor_clip())) {
        return camera;
    }
    let recovered = lch_recovered_rgb(camera.r, camera.g, camera.g, camera.b);
    let strength = clamp(Common::camera_uniforms.highlight_reconstruction, 0.0, 1.0);
    return mix(camera, max(recovered, vec3<f32>(0.0)), strength);
}

// Opposed reconstruction of a binned pixel: inpaint_opposed_cfa_at for each
// clipped colour, the colour means taken over the window
// inpaint_opposed_refavg reads.
fn binned_opposed_rgb(pos: vec2<i32>, camera: vec3<f32>) -> vec3<f32> {
    let clip = DARKTABLE_OPPOSED_CLIP_MAGIC
        * max(Common::camera_uniforms.highlight_clip, 0.01)
        * Common::camera_uniforms.wb.rgb;
    let clipped = camera >= clip;
    if !any(clipped) {
        return camera;
    }
    let center = Common::clamp_pos(pos);
    var mean = vec3<f32>(0.0);
    var count = 0.0;
    let max_row = max(i32(Common::camera_uniforms.height) - 1, 0);
    let max_col = max(i32(Common::camera_uniforms.width) - 1, 0);
    let row_end = min(max_row, center.y + 2);
    let col_end = min(max_col, center.x + 2);
    for (var row = max(0, center.y - 1); row < row_end; row = row + 1) {
        for (var col = max(0, center.x - 1); col < col_end; col = col + 1) {
            mean += max(binned_camera_at(vec2<i32>(col, row)), vec3<f32>(0.0));
            count += 1.0;
        }
    }
    let roots = select(vec3<f32>(0.0), pow(mean / count, vec3<f32>(1.0 / 3.0)), count > 0.0);
    let reference = vec3<f32>(
        opposed_reference(roots, 0u),
        opposed_reference(roots, 1u),
        opposed_reference(roots, 2u),
    ) + Common::camera_uniforms.highlight_options.yzw;
    return select(camera, max(camera, reference), clipped);
}

// highlight_reconstruct for a binned source, which holds every colour at
// every pixel: reconstructs the clipped colours, applies the lens shading
// gains and writes white-balanced camera RGB with sensor white 1, what the
// demosaic finish passes write. The raster finish program then adds sensor
// denoise on the way into the scene.
@compute @workgroup_size(8, 8, 1)
fn binned_highlight_reconstruct(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= Common::camera_uniforms.width || gid.y >= Common::camera_uniforms.height { return; }
    let pos = vec2<i32>(i32(gid.x), i32(gid.y));
    let method = Common::camera_uniforms.highlight_options.x;
    let sensor = binned_sensor_at(pos);
    var output = sensor * Common::camera_uniforms.wb.rgb;
    if method >= 0.5 && method < 1.5 {
        output = binned_lch_rgb(sensor, output);
    } else if method >= 1.5 {
        output = binned_opposed_rgb(pos, output);
    }
    let shading = lens_shading_gains(pos);
    output *= vec3<f32>(shading.r, 0.5 * (shading.g + shading.a), shading.b);
    textureStore(binned_camera_write, pos, vec4<f32>(output, 1.0));
}
