// SPDX-License-Identifier: GPL-3.0-or-later
// The LCh reconstruction is adapted from darktable 5.6.0.
// Copyright (C) 2010-2026 darktable developers.
// Copyright (C) 2026 CalibRaw contributors (WGSL adaptation, guided reconstruction).

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

// The highlight model (HighlightModel in Rust) on a grid of
// camera_uniforms.highlight_options.yz cells over the full image, cell (i, j)
// centred at ((i + 0.5) / cells.x, (j + 0.5) / cells.y) of it. Texel (i, j)
// holds the cell's slopes in rgb; texel (i + cells.x, j) its entry colour
// multiplied by its support in rgb and the support in a. Colours are
// white-balanced camera RGB, the units of RawSampling::raw_camera_at. The
// texture's content is undefined while the cell counts are 0.
@group(0) @binding(14) var highlight_model_tex: texture_2d<f32>;

const DARKTABLE_SQRT3: f32 = 1.7320508075688772;
const DARKTABLE_SQRT12: f32 = 3.4641016151377544;

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

// Guided reconstruction (highlight_model.rs describes the model). A clipped
// colour is rebuilt from the colours beside it that did not clip, the "known"
// ones:
// - two known colours a and b: (a + b) / 2 + slope * (a - b);
// - one known colour: a mix of the entry colour and blown white that reaches
//   white as the known colour nears its own clip level;
// - none: blown white, every colour at the highest clip level.
// A recorded clipped value is a lower bound of its colour, so the two-colour
// relation on the recorded values is a floor of every case.

struct HighlightCell {
    // Slope of red against green - blue, of green against red - blue and of
    // blue against red - green.
    slopes: vec3<f32>,
    // Entry colour multiplied by `support`.
    entry: vec3<f32>,
    // 0 where no reconstructed or bright unclipped colour is near, up to 1.
    support: f32,
}

// The model at tile-local `pos`, interpolated bilinearly between cells.
// Positions outside the full image, such as tile halos, take its edge cells.
// Without a model every slope and the support are 0, which rebuilds clipped
// colours as neutral.
fn highlight_cell_at(pos: vec2<i32>) -> HighlightCell {
    let cells = vec2<i32>(Common::camera_uniforms.highlight_options.yz);
    if cells.x <= 0 || cells.y <= 0 {
        return HighlightCell(vec3<f32>(0.0), vec3<f32>(0.0), 0.0);
    }
    let full = vec2<f32>(Common::full_image_max() + vec2<i32>(1));
    let uv = (vec2<f32>(pos + Common::tile_origin()) + vec2<f32>(0.5)) / max(full, vec2<f32>(1.0));
    let last = cells - vec2<i32>(1);
    let grid = clamp(uv * vec2<f32>(cells) - vec2<f32>(0.5), vec2<f32>(0.0), vec2<f32>(last));
    let p0 = min(vec2<i32>(floor(grid)), last);
    let p1 = min(p0 + vec2<i32>(1), last);
    let f = grid - vec2<f32>(p0);
    let entry_column = vec2<i32>(cells.x, 0);
    let slopes = mix(
        mix(textureLoad(highlight_model_tex, p0, 0), textureLoad(highlight_model_tex, vec2<i32>(p1.x, p0.y), 0), f.x),
        mix(textureLoad(highlight_model_tex, vec2<i32>(p0.x, p1.y), 0), textureLoad(highlight_model_tex, p1, 0), f.x),
        f.y,
    );
    let entry = mix(
        mix(
            textureLoad(highlight_model_tex, p0 + entry_column, 0),
            textureLoad(highlight_model_tex, vec2<i32>(p1.x, p0.y) + entry_column, 0),
            f.x,
        ),
        mix(
            textureLoad(highlight_model_tex, vec2<i32>(p0.x, p1.y) + entry_column, 0),
            textureLoad(highlight_model_tex, p1 + entry_column, 0),
            f.x,
        ),
        f.y,
    );
    return HighlightCell(slopes.rgb, entry.rgb, clamp(entry.a, 0.0, 1.0));
}

// The white-balanced level each colour (R, G, B) clips at. Green takes the
// mean multiplier of both greens when the sensor has a second one.
fn highlight_clip_levels() -> vec3<f32> {
    let wb = Common::camera_uniforms.wb;
    let green = select(wb.g, 0.5 * (wb.g + wb.a), wb.a > 0.0);
    return max(Common::camera_uniforms.highlight_clip, 0.01) * vec3<f32>(wb.r, green, wb.b);
}

// The two colours a clipped `color` is rebuilt from, in the order the slope's
// difference takes them (other_channels in highlight_model.rs).
fn highlight_other_colors(color: u32) -> vec2<u32> {
    if color == 0u { return vec2<u32>(1u, 2u); }
    if color == 1u { return vec2<u32>(0u, 2u); }
    return vec2<u32>(0u, 1u);
}

fn highlight_two_known(a: f32, b: f32, slope: f32) -> f32 {
    return 0.5 * (a + b) + slope * (a - b);
}

// Clipped `color` from the one known colour `known_color` at level `known`.
// Below the level the entry colour has there it keeps that colour's ratio;
// above, it moves to `white` as `known` nears its clip level. Without support
// the entry colour is neutral at `known`.
fn highlight_one_known(
    known: f32,
    known_color: u32,
    color: u32,
    cell: HighlightCell,
    clip_levels: vec3<f32>,
    white: f32,
) -> f32 {
    let entry = cell.entry + vec3<f32>((1.0 - cell.support) * known);
    let entry_known = entry[known_color];
    let entry_color = entry[color];
    if known <= entry_known {
        return entry_color * known / max(entry_known, 1e-6);
    }
    let clip = clip_levels[known_color];
    let entry_share = clamp((clip - known) / max(clip - entry_known, 1e-6), 0.0, 1.0);
    return mix(white, entry_color, entry_share);
}

// Guided reconstruction of a photosite. The known colours are the means of
// the unclipped samples of the other two colours in the 3x3 window around
// it; a colour with clipped samples there counts as known by its share of
// unclipped ones, which blends the cases.
fn guided_cfa_at(pos: vec2<i32>) -> f32 {
    let center = Common::clamp_pos(pos);
    let original = RawSampling::raw_camera_at(center);
    if !RawSampling::is_raw_clipped(center) {
        return original;
    }
    let color = RawSampling::color_at(center);
    var known_sum = vec3<f32>(0.0);
    var known_count = vec3<f32>(0.0);
    var recorded_sum = vec3<f32>(0.0);
    var recorded_count = vec3<f32>(0.0);
    let image_max = vec2<i32>(
        i32(Common::camera_uniforms.width) - 1,
        i32(Common::camera_uniforms.height) - 1,
    );
    for (var dy = -1; dy <= 1; dy = dy + 1) {
        for (var dx = -1; dx <= 1; dx = dx + 1) {
            let sample_pos = center + vec2<i32>(dx, dy);
            if any(sample_pos < vec2<i32>(0)) || any(sample_pos > image_max) {
                continue;
            }
            let sample_color = RawSampling::color_at(sample_pos);
            let value = max(RawSampling::raw_camera_at(sample_pos), 0.0);
            recorded_sum[sample_color] = recorded_sum[sample_color] + value;
            recorded_count[sample_color] = recorded_count[sample_color] + 1.0;
            if !RawSampling::is_raw_clipped(sample_pos) {
                known_sum[sample_color] = known_sum[sample_color] + value;
                known_count[sample_color] = known_count[sample_color] + 1.0;
            }
        }
    }
    let recorded = recorded_sum / max(recorded_count, vec3<f32>(1.0));
    let known = select(recorded, known_sum / max(known_count, vec3<f32>(1.0)), known_count > vec3<f32>(0.0));
    let known_share = known_count / max(recorded_count, vec3<f32>(1.0));

    let others = highlight_other_colors(color);
    let cell = highlight_cell_at(center);
    let slope = cell.slopes[color];
    let clip_levels = highlight_clip_levels();
    let white = max(clip_levels.r, max(clip_levels.g, clip_levels.b));
    let share_a = known_share[others.x];
    let share_b = known_share[others.y];
    let estimate = share_a * share_b * highlight_two_known(known[others.x], known[others.y], slope)
        + share_a * (1.0 - share_b) * highlight_one_known(known[others.x], others.x, color, cell, clip_levels, white)
        + (1.0 - share_a) * share_b * highlight_one_known(known[others.y], others.y, color, cell, clip_levels, white)
        + (1.0 - share_a) * (1.0 - share_b) * white;
    let lower_bound = highlight_two_known(recorded[others.x], recorded[others.y], slope);
    return max(original, max(estimate, lower_bound));
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
// lens shading gain. Clip detection and the highlight model thus see the
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
        output = guided_cfa_at(pos);
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

// Guided reconstruction of a binned pixel: guided_cfa_at with the pixel's own
// colours as the known ones.
fn binned_guided_rgb(pos: vec2<i32>, sensor: vec3<f32>, camera: vec3<f32>) -> vec3<f32> {
    let clipped = sensor >= vec3<f32>(0.987 * max(Common::camera_uniforms.highlight_clip, 0.01));
    if !any(clipped) {
        return camera;
    }
    let cell = highlight_cell_at(Common::clamp_pos(pos));
    let clip_levels = highlight_clip_levels();
    let white = max(clip_levels.r, max(clip_levels.g, clip_levels.b));
    let recorded = max(camera, vec3<f32>(0.0));
    var output = camera;
    for (var color = 0u; color < 3u; color = color + 1u) {
        if !clipped[color] {
            continue;
        }
        let others = highlight_other_colors(color);
        let a = recorded[others.x];
        let b = recorded[others.y];
        let lower_bound = highlight_two_known(a, b, cell.slopes[color]);
        var estimate = white;
        if !clipped[others.x] && !clipped[others.y] {
            estimate = lower_bound;
        } else if !clipped[others.x] {
            estimate = highlight_one_known(a, others.x, color, cell, clip_levels, white);
        } else if !clipped[others.y] {
            estimate = highlight_one_known(b, others.y, color, cell, clip_levels, white);
        }
        output[color] = max(camera[color], max(estimate, lower_bound));
    }
    return output;
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
        output = binned_guided_rgb(pos, sensor, output);
    }
    let shading = lens_shading_gains(pos);
    output *= vec3<f32>(shading.r, 0.5 * (shading.g + shading.a), shading.b);
    textureStore(binned_camera_write, pos, vec4<f32>(output, 1.0));
}
