#import calibraw::common as Common
#import calibraw::color as Color
#import calibraw::profile as Profile
#import calibraw::tone_common as ToneCommon
#import calibraw::scene_source as SceneSource


struct ToneHistogram {
    bins: array<atomic<u32>, 256>,
}

@group(0) @binding(15) var<storage, read_write> tone_histogram: ToneHistogram;
@group(0) @binding(16) var<storage, read_write> tone_stats_out: ToneCommon::ToneStats;
@group(0) @binding(17) var tone_guide_read: texture_2d<f32>;
@group(0) @binding(18) var tone_guide_write: texture_storage_2d<r32float, write>;

fn tone_unexposed_working_at(pos: vec2<i32>) -> vec3<f32> {
    let working = SceneSource::scene_working_at(pos);
    let characterized = Profile::apply_camera_characterization(working);
    let profile_exposure_ev = bitcast<f32>(Common::camera_uniforms.profile_flags.z);
    let exposed = characterized * exp2(profile_exposure_ev);
    return Color::map_negative_gamut(exposed);
}

fn tone_guide_cell_size() -> i32 {
    return max(i32(round(Common::camera_uniforms.tone_analysis_scale)), 1);
}

fn tone_guide_origin_cell() -> vec2<i32> {
    let scale = f32(tone_guide_cell_size());
    return vec2<i32>(floor(vec2<f32>(Common::tile_origin()) / scale));
}

@compute @workgroup_size(8, 8, 1)
fn tone_guide_prepare(@builtin(global_invocation_id) gid: vec3<u32>) {
    let guide_size = textureDimensions(tone_guide_write);
    if gid.x >= guide_size.x || gid.y >= guide_size.y { return; }

    let source_size = vec2<i32>(
        i32(Common::camera_uniforms.width),
        i32(Common::camera_uniforms.height),
    );
    let cell_size = tone_guide_cell_size();
    let tile_origin = Common::tile_origin();
    let global_cell = tone_guide_origin_cell() + vec2<i32>(gid.xy);
    let global_cell_min = global_cell * cell_size;
    let global_cell_max = global_cell_min + vec2<i32>(cell_size);
    let cell_min = clamp(global_cell_min - tile_origin, vec2<i32>(0), source_size);
    let cell_max = clamp(global_cell_max - tile_origin, vec2<i32>(0), source_size);

    var log_sum = 0.0;
    var count = 0.0;
    var brightest = vec4<f32>(ToneCommon::TONE_EV_MIN);
    var y = cell_min.y;
    loop {
        if y >= cell_max.y { break; }
        var x = cell_min.x;
        loop {
            if x >= cell_max.x { break; }
            let local_pos = vec2<i32>(x, y);
            let rgb = tone_unexposed_working_at(local_pos);
            let ev = clamp(
                log2(Common::safe_luma(rgb) / ToneCommon::SCENE_MIDDLE_GREY),
                ToneCommon::TONE_EV_MIN,
                ToneCommon::TONE_EV_MAX,
            );
            log_sum = log_sum + ev;
            if ev > brightest.x {
                brightest = vec4<f32>(ev, brightest.x, brightest.y, brightest.z);
            } else if ev > brightest.y {
                brightest = vec4<f32>(brightest.x, ev, brightest.y, brightest.z);
            } else if ev > brightest.z {
                brightest = vec4<f32>(brightest.x, brightest.y, ev, brightest.z);
            } else if ev > brightest.w {
                brightest = vec4<f32>(brightest.x, brightest.y, brightest.z, ev);
            }
            count = count + 1.0;

            let global_pos = local_pos + tile_origin;
            let histogram_min = vec2<i32>(Common::camera_uniforms.tone_histogram_bounds.xy);
            let histogram_max = vec2<i32>(Common::camera_uniforms.tone_histogram_bounds.zw);
            if all(global_pos >= histogram_min) && all(global_pos < histogram_max) {
                atomicAdd(&tone_histogram.bins[ToneCommon::tone_ev_to_bin(ev)], 1u);
            }
            x = x + 1;
        }
        y = y + 1;
    }

    let average_ev = log_sum / max(count, 1.0);

    let bright_count = min(count, 4.0);
    var bright_sum = brightest.x;
    if bright_count > 1.0 { bright_sum = bright_sum + brightest.y; }
    if bright_count > 2.0 { bright_sum = bright_sum + brightest.z; }
    if bright_count > 3.0 { bright_sum = bright_sum + brightest.w; }
    let robust_bright_ev = bright_sum / max(bright_count, 1.0);
    let guide_ev = max(average_ev, robust_bright_ev - 1.5);
    textureStore(tone_guide_write, vec2<i32>(gid.xy), vec4<f32>(guide_ev, 0.0, 0.0, 1.0));
}

fn tone_bilateral_guide(pos: vec2<i32>, axis: vec2<i32>) -> f32 {
    let guide_max = vec2<i32>(textureDimensions(tone_guide_read)) - vec2<i32>(1);
    let center_pos = clamp(pos, vec2<i32>(0), guide_max);
    let center = textureLoad(tone_guide_read, center_pos, 0).x;
    let radius = clamp(i32(round(Common::camera_uniforms.tone_guide_radius)), 1, 6);
    let sigma = max(f32(radius) * 0.65, 1.0);

    var weighted_sum = 0.0;
    var weight_sum = 0.0;
    for (var offset = -6; offset <= 6; offset = offset + 1) {
        if abs(offset) > radius { continue; }
        let sample_pos = clamp(center_pos + axis * offset, vec2<i32>(0), guide_max);
        let sample_ev = textureLoad(tone_guide_read, sample_pos, 0).x;
        let distance = f32(offset);
        let spatial_weight = exp(-0.5 * distance * distance / (sigma * sigma));
        let range_weight = exp(-1.35 * abs(sample_ev - center));
        let weight = spatial_weight * range_weight;
        weighted_sum = weighted_sum + sample_ev * weight;
        weight_sum = weight_sum + weight;
    }

    return weighted_sum / max(weight_sum, 1e-6);
}

@compute @workgroup_size(8, 8, 1)
fn tone_guide_horizontal(@builtin(global_invocation_id) gid: vec3<u32>) {
    let guide_size = textureDimensions(tone_guide_write);
    if gid.x >= guide_size.x || gid.y >= guide_size.y { return; }
    let pos = vec2<i32>(gid.xy);
    let value = tone_bilateral_guide(pos, vec2<i32>(1, 0));
    textureStore(tone_guide_write, pos, vec4<f32>(value, 0.0, 0.0, 1.0));
}

@compute @workgroup_size(8, 8, 1)
fn tone_guide_vertical(@builtin(global_invocation_id) gid: vec3<u32>) {
    let guide_size = textureDimensions(tone_guide_write);
    if gid.x >= guide_size.x || gid.y >= guide_size.y { return; }
    let pos = vec2<i32>(gid.xy);
    let value = tone_bilateral_guide(pos, vec2<i32>(0, 1));
    textureStore(tone_guide_write, pos, vec4<f32>(value, 0.0, 0.0, 1.0));
}

@compute @workgroup_size(1, 1, 1)
fn tone_reduce_histogram(@builtin(global_invocation_id) gid: vec3<u32>) {
    if any(gid != vec3<u32>(0u)) { return; }

    var total = 0u;
    for (var index = 0u; index < ToneCommon::TONE_HISTOGRAM_BIN_COUNT; index = index + 1u) {
        total = total + atomicLoad(&tone_histogram.bins[index]);
    }

    if total == 0u {
        tone_stats_out.percentiles_0_field = vec4<f32>(-8.0, -5.0, 0.0, 2.5);
        tone_stats_out.percentiles_1_field = vec4<f32>(4.0, 12.0, 0.0, 0.0);
        return;
    }

    let target_005 = max(1u, u32(ceil(f32(total) * 0.005)));
    let target_05 = max(1u, u32(ceil(f32(total) * 0.05)));
    let target_50 = max(1u, u32(ceil(f32(total) * 0.50)));
    let target_95 = max(1u, u32(ceil(f32(total) * 0.95)));
    let target_995 = max(1u, u32(ceil(f32(total) * 0.995)));

    var cumulative = 0u;
    var p005_field = ToneCommon::TONE_EV_MIN;
    var p05_field = ToneCommon::TONE_EV_MIN;
    var p50_field = 0.0;
    var p95_field = ToneCommon::TONE_EV_MAX;
    var p995_field = ToneCommon::TONE_EV_MAX;
    var found_005 = false;
    var found_05 = false;
    var found_50 = false;
    var found_95 = false;
    var found_995 = false;

    for (var index = 0u; index < ToneCommon::TONE_HISTOGRAM_BIN_COUNT; index = index + 1u) {
        cumulative = cumulative + atomicLoad(&tone_histogram.bins[index]);
        let ev = ToneCommon::tone_bin_to_ev(index);
        if !found_005 && cumulative >= target_005 { p005_field = ev; found_005 = true; }
        if !found_05 && cumulative >= target_05 { p05_field = ev; found_05 = true; }
        if !found_50 && cumulative >= target_50 { p50_field = ev; found_50 = true; }
        if !found_95 && cumulative >= target_95 { p95_field = ev; found_95 = true; }
        if !found_995 && cumulative >= target_995 { p995_field = ev; found_995 = true; }
    }

    tone_stats_out.percentiles_0_field = vec4<f32>(p005_field, p05_field, p50_field, p95_field);
    tone_stats_out.percentiles_1_field = vec4<f32>(p995_field, max(p995_field - p005_field, 1.0), f32(total), 0.0);
}

// Image lights: light sources in the photograph (lamps, lit windows, signs)
// for effects that scatter light, such as Fog's Image lights.
//
// `accumulate_image_lights` sums each cell of a coarse full-image grid
// (Common::image_light_grid) in one-stop brightness bands of unexposed
// scene-linear Rec.2020, measured by the strongest channel so saturated neon
// counts as fully as white light. Export tiles add their cores to the same
// grid during the tone prepass, so every tile later sees the lights of the
// whole image. Once the tone statistics are reduced, `resolve_image_lights`
// keeps the light of bands well above both the scene's median and the median
// of the cell's surroundings: a lamp outshines its neighbourhood, while a
// bright sky, fog or white wall does not. What counts as a light therefore
// depends on the scene, not on exposure, which the receiving effect applies.
// Two separable Gaussians then spread it into a halo with a tight core and a
// wide tail.

const IMAGE_LIGHT_BANDS: u32 = 16u;
// Band b holds EVs in [IMAGE_LIGHT_BAND_MIN_EV + b, ... + b + 1); brighter and
// darker pixels join the top and bottom bands.
const IMAGE_LIGHT_BAND_MIN_EV: f32 = -6.0;
// A light is at least this far above the scene's median ...
const IMAGE_LIGHT_ABOVE_MEDIAN_EV: f32 = 2.5;
// ... and this far above the median of the surrounding cells.
const IMAGE_LIGHT_ABOVE_SURROUNDINGS_EV: f32 = 3.0;
// Surrounding cells on each side of a cell.
const IMAGE_LIGHT_SURROUNDINGS: i32 = 3;
// Width of the ramp from no light to full light, in stops.
const IMAGE_LIGHT_RAMP_EV: f32 = 2.0;
// Halo core and tail: standard deviations as fractions of the grid's longer
// edge, and the tail's share of the light.
const IMAGE_LIGHT_CORE_SIGMA: f32 = 0.012;
const IMAGE_LIGHT_TAIL_SIGMA: f32 = 0.06;
const IMAGE_LIGHT_TAIL_SHARE: f32 = 0.45;

struct ImageLightCell {
    // Summed colour (xyz) and pixel count (w) of each band.
    bands: array<vec4<f32>, IMAGE_LIGHT_BANDS>,
}

@group(0) @binding(38) var<storage, read_write> image_light_cells: array<ImageLightCell>;
@group(0) @binding(39) var image_light_out: texture_storage_2d<rgba16float, write>;
@group(0) @binding(40) var image_light_in: texture_2d<f32>;
@group(0) @binding(41) var image_light_core_out: texture_storage_2d<rgba16float, write>;
@group(0) @binding(42) var image_light_tail_out: texture_storage_2d<rgba16float, write>;
@group(0) @binding(43) var image_light_core_in: texture_2d<f32>;
@group(0) @binding(44) var image_light_tail_in: texture_2d<f32>;

fn image_light_cell_index(cell: vec2<u32>) -> u32 {
    return cell.y * Common::IMAGE_LIGHT_GRID_LONG + cell.x;
}

// Global pixels whose centres fall in grid cells [cell, cell + 1) along one axis.
fn image_light_pixel_span(cell: u32, cells: u32, full: u32) -> vec2<i32> {
    let scale = f32(full) / f32(cells);
    return vec2<i32>(
        i32(ceil(f32(cell) * scale - 0.5)),
        i32(ceil(f32(cell + 1u) * scale - 0.5)),
    );
}

@compute @workgroup_size(8, 8, 1)
fn accumulate_image_lights(@builtin(global_invocation_id) gid: vec3<u32>) {
    let grid = Common::image_light_grid();
    if gid.x >= grid.x || gid.y >= grid.y { return; }
    let span_x = image_light_pixel_span(gid.x, grid.x, Common::camera_uniforms.full_width);
    let span_y = image_light_pixel_span(gid.y, grid.y, Common::camera_uniforms.full_height);
    // Only this tile's core counts, so overlapping tile halos add nothing twice.
    let bounds = vec4<i32>(Common::camera_uniforms.tone_histogram_bounds);
    let origin = Common::tile_origin();
    let tile_end = origin + vec2<i32>(
        i32(Common::camera_uniforms.width),
        i32(Common::camera_uniforms.height),
    );
    let lo = max(max(vec2<i32>(span_x.x, span_y.x), bounds.xy), origin);
    let hi = min(min(vec2<i32>(span_x.y, span_y.y), bounds.zw), tile_end);
    if any(hi <= lo) { return; }

    var bands: array<vec4<f32>, IMAGE_LIGHT_BANDS>;
    for (var y = lo.y; y < hi.y; y = y + 1) {
        for (var x = lo.x; x < hi.x; x = x + 1) {
            let rgb = max(tone_unexposed_working_at(vec2<i32>(x, y) - origin), vec3<f32>(0.0));
            let strongest = max(rgb.r, max(rgb.g, rgb.b));
            let ev = log2(max(strongest, 1e-9) / ToneCommon::SCENE_MIDDLE_GREY);
            let band = u32(clamp(ev - IMAGE_LIGHT_BAND_MIN_EV, 0.0, f32(IMAGE_LIGHT_BANDS - 1u)));
            bands[band] = bands[band] + vec4<f32>(rgb, 1.0);
        }
    }
    // Tiles are dispatched one after another, so each cell has one writer.
    let index = image_light_cell_index(gid.xy);
    for (var band = 0u; band < IMAGE_LIGHT_BANDS; band = band + 1u) {
        image_light_cells[index].bands[band] += bands[band];
    }
}

// Median EV of the cells around `cell`, from their band counts.
fn image_light_surroundings_ev(cell: vec2<i32>, grid: vec2<i32>) -> f32 {
    var counts: array<f32, IMAGE_LIGHT_BANDS>;
    var total = 0.0;
    for (var dy = -IMAGE_LIGHT_SURROUNDINGS; dy <= IMAGE_LIGHT_SURROUNDINGS; dy = dy + 1) {
        for (var dx = -IMAGE_LIGHT_SURROUNDINGS; dx <= IMAGE_LIGHT_SURROUNDINGS; dx = dx + 1) {
            let neighbour = cell + vec2<i32>(dx, dy);
            if any(neighbour < vec2<i32>(0)) || any(neighbour >= grid) { continue; }
            let index = image_light_cell_index(vec2<u32>(neighbour));
            for (var band = 0u; band < IMAGE_LIGHT_BANDS; band = band + 1u) {
                let count = image_light_cells[index].bands[band].w;
                counts[band] = counts[band] + count;
                total = total + count;
            }
        }
    }
    var cumulative = 0.0;
    for (var band = 0u; band < IMAGE_LIGHT_BANDS; band = band + 1u) {
        cumulative = cumulative + counts[band];
        if cumulative >= 0.5 * total {
            return IMAGE_LIGHT_BAND_MIN_EV + f32(band) + 0.5;
        }
    }
    return IMAGE_LIGHT_BAND_MIN_EV + f32(IMAGE_LIGHT_BANDS) - 0.5;
}

@compute @workgroup_size(8, 8, 1)
fn resolve_image_lights(@builtin(global_invocation_id) gid: vec3<u32>) {
    let grid = Common::image_light_grid();
    if gid.x >= grid.x || gid.y >= grid.y { return; }
    let median = tone_stats_out.percentiles_0_field.z;
    let surroundings = image_light_surroundings_ev(vec2<i32>(gid.xy), vec2<i32>(grid));
    let threshold = max(
        median + IMAGE_LIGHT_ABOVE_MEDIAN_EV,
        surroundings + IMAGE_LIGHT_ABOVE_SURROUNDINGS_EV,
    );
    let index = image_light_cell_index(gid.xy);
    var light = vec3<f32>(0.0);
    var count = 0.0;
    for (var band = 0u; band < IMAGE_LIGHT_BANDS; band = band + 1u) {
        let ev = IMAGE_LIGHT_BAND_MIN_EV + f32(band) + 0.5;
        let share = smoothstep(threshold, threshold + IMAGE_LIGHT_RAMP_EV, ev);
        let sums = image_light_cells[index].bands[band];
        light += sums.xyz * share;
        count += sums.w;
    }
    // Mean emitted light over the cell.
    textureStore(image_light_out, vec2<i32>(gid.xy), vec4<f32>(light / max(count, 1.0), 1.0));
}

fn image_light_gaussian(offset: f32, sigma: f32) -> f32 {
    return exp(-0.5 * offset * offset / (sigma * sigma)) / (sigma * 2.5066282746);
}

// One axis of both Gaussians. No light comes from beyond the frame, so taps
// outside the grid add nothing and the kernels keep their full weight.
fn image_light_blur(
    source: texture_2d<f32>,
    pos: vec2<i32>,
    axis: vec2<i32>,
    cells: i32,
    sigma: f32,
) -> vec3<f32> {
    let reach = i32(ceil(3.0 * sigma));
    var sum = vec3<f32>(0.0);
    for (var offset = -reach; offset <= reach; offset = offset + 1) {
        let tap = pos + axis * offset;
        let along = dot(tap, axis);
        if along < 0 || along >= cells { continue; }
        sum += textureLoad(source, tap, 0).xyz * image_light_gaussian(f32(offset), sigma);
    }
    return sum;
}

fn image_light_sigmas() -> vec2<f32> {
    let long = f32(Common::IMAGE_LIGHT_GRID_LONG);
    return vec2<f32>(IMAGE_LIGHT_CORE_SIGMA, IMAGE_LIGHT_TAIL_SIGMA) * long;
}

@compute @workgroup_size(8, 8, 1)
fn blur_image_lights_horizontal(@builtin(global_invocation_id) gid: vec3<u32>) {
    let grid = Common::image_light_grid();
    if gid.x >= grid.x || gid.y >= grid.y { return; }
    let pos = vec2<i32>(gid.xy);
    let sigmas = image_light_sigmas();
    let axis = vec2<i32>(1, 0);
    let cells = i32(grid.x);
    textureStore(image_light_core_out, pos,
        vec4<f32>(image_light_blur(image_light_in, pos, axis, cells, sigmas.x), 1.0));
    textureStore(image_light_tail_out, pos,
        vec4<f32>(image_light_blur(image_light_in, pos, axis, cells, sigmas.y), 1.0));
}

@compute @workgroup_size(8, 8, 1)
fn blur_image_lights_vertical(@builtin(global_invocation_id) gid: vec3<u32>) {
    let grid = Common::image_light_grid();
    if gid.x >= grid.x || gid.y >= grid.y { return; }
    let pos = vec2<i32>(gid.xy);
    let sigmas = image_light_sigmas();
    let axis = vec2<i32>(0, 1);
    let cells = i32(grid.y);
    let core = image_light_blur(image_light_core_in, pos, axis, cells, sigmas.x);
    let tail = image_light_blur(image_light_tail_in, pos, axis, cells, sigmas.y);
    textureStore(image_light_out, pos,
        vec4<f32>(mix(core, tail, IMAGE_LIGHT_TAIL_SHARE), 1.0));
}
