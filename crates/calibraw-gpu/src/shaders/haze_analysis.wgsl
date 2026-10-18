#import calibraw::common as Common
#import calibraw::color as Color
#import calibraw::scene_source as SceneSource
#import calibraw::tone_common as ToneCommon
#import calibraw::haze as Haze

// Estimates the airlight and transmission that Dehaze applies (haze.wgsl)
// with the dark channel prior (He, Sun and Tang, "Single Image Haze Removal
// Using Dark Channel Prior", 2009): in haze-free outdoor scenes most patches
// hold a pixel that is dark in some channel, so a patch's darkest channel,
// relative to the airlight, measures the veil in front of it.
//
// Everything runs on the coarse haze grid over the full image
// (Haze::haze_grid), built with the tone statistics like the image-light grid:
//   accumulate_haze            per cell, each channel's darkest box mean and
//                              colour and luminance sums of the profiled
//                              scene. Export tiles add only their cores, so
//                              every tile later reads the whole image's map.
//   resolve_haze_airlight      the airlight: the mean colour of the haziest
//                              smooth region, and the veil negative amounts
//                              add, after the tone statistics are reduced.
//   resolve_haze_transmission  per cell, t from the dark channel after a
//                              morphological opening, the guide value, and
//                              how much of the veil removal may take.
//   haze_guide_coefficients,   a guided filter (He, Sun and Tang, "Guided
//   smooth_haze_coefficients   Image Filtering", 2010) on the grid. Pixels
//                              evaluate its coefficients against their own
//                              guide value (He and Sun, "Fast Guided Filter",
//                              2015), so the transmission follows edges inside
//                              a cell without reading neighbouring pixels and
//                              Dehaze needs no export-tile halo.

struct HazeCell {
    // Darkest box mean of each channel (xyz) and the pixels counted (w).
    darkest: vec4<f32>,
    // Summed colour of those pixels (xyz) and the summed squared luminance of
    // the box means, each weighted by its pixels (w).
    sums: vec4<f32>,
}

@group(0) @binding(16) var<storage, read> haze_tone_stats: ToneCommon::ToneStats;
@group(0) @binding(52) var<storage, read_write> haze_cells: array<HazeCell>;
@group(0) @binding(53) var<storage, read_write> haze_airlight_out: Haze::HazeAirlight;
// Per cell: transmission (x), guide value (y), whether the cell holds pixels
// (z) and the share of the veil removal may take (w).
@group(0) @binding(55) var haze_transmission_out: texture_storage_2d<rgba32float, write>;
@group(0) @binding(56) var haze_transmission_tex: texture_2d<f32>;
// Per cell: the guided filter's slope (x) and offset (y) in one window, and
// the cell's removal share (z).
@group(0) @binding(57) var haze_coefficients_out: texture_storage_2d<rgba32float, write>;
@group(0) @binding(58) var haze_coefficients_tex: texture_2d<f32>;
// The window-averaged coefficients and removal share haze.wgsl reads as
// `haze_map_tex`.
@group(0) @binding(59) var haze_map_out: texture_storage_2d<rgba32float, write>;

// Box means over this many native pixels a side set a cell's darkest
// channels, so that sensor noise and single hot or dead pixels do not; a
// reduced preview takes its share of the same box. A box reaches up to
// HAZE_BOX_MAX - 1 pixels past the tile core that counts it, which
// HAZE_BOX_SUPPORT in tiles.rs reserves.
const HAZE_BOX_SOURCE_PIXELS: f32 = 2.0;
const HAZE_BOX_MAX: i32 = 2;

// Invocations of resolve_haze_airlight, its @workgroup_size.
const HAZE_AIRLIGHT_THREADS: u32 = 256u;
// Cells scoring at least this share of the best score form the haziest region.
const HAZE_REGION_SHARE: f32 = 0.9;
// Largest saturation, 1 - darkest / brightest channel, the airlight keeps.
// Haze scatters nearly neutral light; a strongly coloured candidate is more
// likely a coloured surface or sunset light, whose colour removal would strip.
const HAZE_AIRLIGHT_MAX_SATURATION: f32 = 0.25;
// A clear photo's airlight is its brightest smooth region, often a sky stops
// above the subject, and a veil that bright would bury the photo. Negative
// amounts therefore add the airlight's colour at most this far above the
// scene's median luminance, as fog looks in a photo exposed for its subject.
const HAZE_ADDED_VEIL_ABOVE_MEDIAN_EV: f32 = 2.0;

// Share of the veil the transmission accounts for (omega in He et al.): a
// trace of haze is kept, so distant objects still read as distant.
const HAZE_VEIL_SHARE: f32 = 0.95;
// Where a cell's luminance spread, relative to its mean, is below
// HAZE_FEATURELESS_SPREAD it holds no detail the haze could hide: a sky or
// featureless fog, which the dark channel cannot tell from dense haze in front
// of a dark object. Removal there could only darken it and amplify its tint,
// so it takes HAZE_FEATURELESS_REMOVAL of the veil, rising to all of it at
// HAZE_DETAIL_SPREAD. Box means keep sensor noise out of the spread, and the
// share is averaged over 3 x 3 cells so that it changes gradually. Added haze
// ignores it.
const HAZE_FEATURELESS_SPREAD: f32 = 0.02;
const HAZE_DETAIL_SPREAD: f32 = 0.08;
const HAZE_FEATURELESS_REMOVAL: f32 = 0.3;

// Radius in cells of the guided filter's windows, 3 x 3 cells or about 2% of
// the longer edge. Wider windows smear the transmission of buildings and
// mountains over the sky beside them where their guides are alike.
const HAZE_GUIDE_RADIUS: i32 = 1;
// The guided filter's regularization (epsilon in He et al.), in squared guide
// units. Where the guide varies by less than about its square root across a
// window, the transmission is smoothed instead of following the guide.
const HAZE_GUIDE_EPSILON: f32 = 1e-3;

var<workgroup> haze_best_score_bits: atomic<u32>;
var<workgroup> haze_region_sums: array<vec4<f32>, HAZE_AIRLIGHT_THREADS>;

fn haze_cell_index(cell: vec2<i32>) -> u32 {
    return u32(cell.y) * Haze::HAZE_GRID_LONG + u32(cell.x);
}

fn haze_cell(cell: vec2<i32>) -> HazeCell {
    return haze_cells[haze_cell_index(cell)];
}

fn haze_cell_pixels(cell: HazeCell) -> f32 {
    return cell.darkest.w;
}

fn haze_cell_mean(cell: HazeCell) -> vec3<f32> {
    return cell.sums.xyz / max(haze_cell_pixels(cell), 1.0);
}

// Standard deviation of the luminance of the cell's box means.
fn haze_cell_spread(cell: HazeCell) -> f32 {
    let pixels = max(haze_cell_pixels(cell), 1.0);
    let mean = dot(cell.sums.xyz, Common::LUMA) / pixels;
    return sqrt(max(cell.sums.w / pixels - mean * mean, 0.0));
}

// Box edge in processing pixels: at most the narrowest cell, so every cell
// holds the start of a box.
fn haze_box_edge(grid: vec2<u32>) -> i32 {
    let full = vec2<f32>(
        f32(Common::camera_uniforms.full_width),
        f32(Common::camera_uniforms.full_height),
    );
    let cell_extent = floor(full / vec2<f32>(grid));
    let largest = clamp(i32(min(cell_extent.x, cell_extent.y)), 1, HAZE_BOX_MAX);
    return clamp(
        i32(round(HAZE_BOX_SOURCE_PIXELS * Common::processing_per_source_pixel())),
        1,
        largest,
    );
}

@compute @workgroup_size(8, 8, 1)
fn accumulate_haze(@builtin(global_invocation_id) gid: vec3<u32>) {
    let grid = Haze::haze_grid();
    if gid.x >= grid.x || gid.y >= grid.y { return; }
    let span_x = Common::full_image_grid_pixel_span(
        gid.x,
        grid.x,
        Common::camera_uniforms.full_width,
    );
    let span_y = Common::full_image_grid_pixel_span(
        gid.y,
        grid.y,
        Common::camera_uniforms.full_height,
    );
    // Only boxes starting in this tile's core count, so overlapping tile
    // halos add nothing twice.
    let bounds = vec4<i32>(Common::camera_uniforms.tone_histogram_bounds);
    let origin = Common::tile_origin();
    let tile_end = origin + vec2<i32>(
        i32(Common::camera_uniforms.width),
        i32(Common::camera_uniforms.height),
    );
    let lo = max(max(vec2<i32>(span_x.x, span_y.x), bounds.xy), origin);
    let hi = min(min(vec2<i32>(span_x.y, span_y.y), bounds.zw), tile_end);
    if any(hi <= lo) { return; }

    // Boxes lie on a global grid, so every tiling forms the same boxes. Each
    // belongs to the cell and core holding its first pixel and is cut off at
    // the image edge rather than repeating edge pixels.
    let edge = haze_box_edge(grid);
    let full = vec2<i32>(
        i32(Common::camera_uniforms.full_width),
        i32(Common::camera_uniforms.full_height),
    );
    let first = (lo + vec2<i32>(edge - 1)) / edge * edge;
    var darkest = vec3<f32>(3.0e38);
    var sums = vec4<f32>(0.0);
    var pixels = 0.0;
    for (var box_y = first.y; box_y < hi.y; box_y = box_y + edge) {
        for (var box_x = first.x; box_x < hi.x; box_x = box_x + edge) {
            let box_end = min(vec2<i32>(box_x, box_y) + vec2<i32>(edge), full);
            var box_sum = vec3<f32>(0.0);
            var box_pixels = 0.0;
            for (var y = box_y; y < box_end.y; y = y + 1) {
                for (var x = box_x; x < box_end.x; x = x + 1) {
                    box_sum = box_sum + Color::map_negative_gamut(
                        SceneSource::profiled_scene_at(vec2<i32>(x, y) - origin),
                    );
                    box_pixels = box_pixels + 1.0;
                }
            }
            let box_mean = box_sum / box_pixels;
            let box_luma = dot(box_mean, Common::LUMA);
            darkest = min(darkest, box_mean);
            sums = sums + vec4<f32>(box_sum, box_pixels * box_luma * box_luma);
            pixels = pixels + box_pixels;
        }
    }
    if pixels <= 0.0 { return; }

    // Tiles are dispatched one after another, so each cell has one writer.
    let index = haze_cell_index(vec2<i32>(gid.xy));
    var cell = haze_cells[index];
    if haze_cell_pixels(cell) > 0.0 {
        darkest = min(darkest, cell.darkest.xyz);
    }
    cell.darkest = vec4<f32>(darkest, haze_cell_pixels(cell) + pixels);
    cell.sums = cell.sums + sums;
    haze_cells[index] = cell;
}

// Each channel's darkest box mean over the 3 x 3 cells around `cell`, about
// 2% of the longer edge. Zero when no cell there holds pixels.
fn haze_patch_darkest(cell: vec2<i32>, grid: vec2<i32>) -> vec3<f32> {
    var darkest = vec3<f32>(3.0e38);
    var found = false;
    for (var dy = -1; dy <= 1; dy = dy + 1) {
        for (var dx = -1; dx <= 1; dx = dx + 1) {
            let neighbour = cell + vec2<i32>(dx, dy);
            if any(neighbour < vec2<i32>(0)) || any(neighbour >= grid) { continue; }
            let data = haze_cell(neighbour);
            if haze_cell_pixels(data) <= 0.0 { continue; }
            darkest = min(darkest, data.darkest.xyz);
            found = true;
        }
    }
    return select(vec3<f32>(0.0), darkest, found);
}

// How much a cell looks like the airlight itself: a bright dark channel over
// the 3 x 3 cells around it, as He et al. pick the airlight among the
// brightest dark-channel pixels, less the cell's luminance spread, so that
// small bright objects and white surfaces with detail do not pass for haze.
fn haze_airlight_score(cell: vec2<i32>, grid: vec2<i32>) -> f32 {
    let data = haze_cell(cell);
    if haze_cell_pixels(data) <= 0.0 { return 0.0; }
    let darkest = haze_patch_darkest(cell, grid);
    return min(darkest.r, min(darkest.g, darkest.b)) - haze_cell_spread(data);
}

// The darkest channel of a cell relative to the airlight, or 2 when the cell
// holds no pixels, which no minimum below takes.
fn haze_relative_dark(cell: vec2<i32>, airlight: vec4<f32>) -> f32 {
    let data = haze_cell(cell);
    if haze_cell_pixels(data) <= 0.0 { return 2.0; }
    let relative = data.darkest.xyz / airlight.xyz;
    return min(relative.r, min(relative.g, relative.b));
}

// The relative dark channel at `cell` after a morphological opening with 3 x 3
// cells: the largest of the 3 x 3 minima that contain the cell. A bright
// object narrower than three cells cannot pass for haze, as with a 3 x 3
// patch, yet a dark object does not lower the veil of the sky beside it, a
// halo the guided filter cannot undo where that sky's guide matches.
fn haze_opened_dark(cell: vec2<i32>, grid: vec2<i32>, airlight: vec4<f32>) -> f32 {
    var opened = 0.0;
    for (var ny = -1; ny <= 1; ny = ny + 1) {
        for (var nx = -1; nx <= 1; nx = nx + 1) {
            let center = cell + vec2<i32>(nx, ny);
            if any(center < vec2<i32>(0)) || any(center >= grid) { continue; }
            var eroded = 2.0;
            for (var my = -1; my <= 1; my = my + 1) {
                for (var mx = -1; mx <= 1; mx = mx + 1) {
                    let neighbour = center + vec2<i32>(mx, my);
                    if any(neighbour < vec2<i32>(0)) || any(neighbour >= grid) { continue; }
                    eroded = min(eroded, haze_relative_dark(neighbour, airlight));
                }
            }
            if eroded < 2.0 {
                opened = max(opened, eroded);
            }
        }
    }
    return opened;
}

// The share of the veil removal takes at `cell`, from the detail of the 3 x 3
// cells around it.
fn haze_removal_share(cell: vec2<i32>, grid: vec2<i32>) -> f32 {
    var detail = 0.0;
    var count = 0.0;
    for (var dy = -1; dy <= 1; dy = dy + 1) {
        for (var dx = -1; dx <= 1; dx = dx + 1) {
            let neighbour = cell + vec2<i32>(dx, dy);
            if any(neighbour < vec2<i32>(0)) || any(neighbour >= grid) { continue; }
            let data = haze_cell(neighbour);
            if haze_cell_pixels(data) <= 0.0 { continue; }
            let luma = dot(haze_cell_mean(data), Common::LUMA);
            let spread = haze_cell_spread(data) / max(luma, 1e-6);
            detail = detail + smoothstep(HAZE_FEATURELESS_SPREAD, HAZE_DETAIL_SPREAD, spread);
            count = count + 1.0;
        }
    }
    return mix(HAZE_FEATURELESS_REMOVAL, 1.0, detail / max(count, 1.0));
}

fn haze_grid_cell(index: u32, grid: vec2<i32>) -> vec2<i32> {
    return vec2<i32>(i32(index) % grid.x, i32(index) / grid.x);
}

// The airlight from the summed mean colours (xyz) and count (w) of the
// haziest region's cells, desaturated to HAZE_AIRLIGHT_MAX_SATURATION at the
// same luminance. Zero without a region.
fn resolved_airlight(region: vec4<f32>) -> vec4<f32> {
    if region.w < 1.0 {
        return vec4<f32>(0.0);
    }
    let mean = region.xyz / region.w;
    let luma = dot(mean, Common::LUMA);
    if luma <= 1e-6 {
        return vec4<f32>(0.0);
    }
    // With colour c' = Y + k (c - Y), 1 - min(c') / max(c') <= s holds for
    // k [(max - min) - s (max - Y)] <= s Y.
    let brightest = max(mean.r, max(mean.g, mean.b));
    let darkest = min(mean.r, min(mean.g, mean.b));
    let excess = (brightest - darkest) - HAZE_AIRLIGHT_MAX_SATURATION * (brightest - luma);
    var share = 1.0;
    if excess > 0.0 {
        share = min(1.0, HAZE_AIRLIGHT_MAX_SATURATION * luma / excess);
    }
    return vec4<f32>(vec3<f32>(luma) + share * (mean - vec3<f32>(luma)), luma);
}

// The veil negative amounts thicken towards: `airlight` dimmed to at most
// HAZE_ADDED_VEIL_ABOVE_MEDIAN_EV above the scene's median.
fn added_veil(airlight: vec4<f32>) -> vec4<f32> {
    let median_ev = haze_tone_stats.percentiles_0_field.z;
    let limit = ToneCommon::SCENE_MIDDLE_GREY * exp2(median_ev + HAZE_ADDED_VEIL_ABOVE_MEDIAN_EV);
    return airlight * min(1.0, limit / max(airlight.w, 1e-6));
}

@compute @workgroup_size(256, 1, 1)
fn resolve_haze_airlight(@builtin(local_invocation_index) thread: u32) {
    let grid = vec2<i32>(Haze::haze_grid());
    let cells = u32(grid.x * grid.y);
    for (var index = thread; index < cells; index = index + HAZE_AIRLIGHT_THREADS) {
        let score = haze_airlight_score(haze_grid_cell(index, grid), grid);
        if score > 0.0 {
            // The bits of positive floats order like the floats.
            atomicMax(&haze_best_score_bits, bitcast<u32>(score));
        }
    }
    workgroupBarrier();

    let best = bitcast<f32>(atomicLoad(&haze_best_score_bits));
    var region = vec4<f32>(0.0);
    if best > 0.0 {
        for (var index = thread; index < cells; index = index + HAZE_AIRLIGHT_THREADS) {
            let cell = haze_grid_cell(index, grid);
            if haze_airlight_score(cell, grid) >= best * HAZE_REGION_SHARE {
                region = region + vec4<f32>(haze_cell_mean(haze_cell(cell)), 1.0);
            }
        }
    }
    haze_region_sums[thread] = region;
    workgroupBarrier();
    for (var stride = HAZE_AIRLIGHT_THREADS / 2u; stride > 0u; stride = stride / 2u) {
        if thread < stride {
            haze_region_sums[thread] = haze_region_sums[thread] + haze_region_sums[thread + stride];
        }
        workgroupBarrier();
    }
    if thread == 0u {
        let airlight = resolved_airlight(haze_region_sums[0]);
        haze_airlight_out.color = airlight;
        haze_airlight_out.added_veil = added_veil(airlight);
    }
}

@compute @workgroup_size(8, 8, 1)
fn resolve_haze_transmission(@builtin(global_invocation_id) gid: vec3<u32>) {
    let grid = vec2<i32>(Haze::haze_grid());
    let cell = vec2<i32>(gid.xy);
    if any(cell >= grid) { return; }
    let airlight = Haze::haze_airlight.color;
    let data = haze_cell(cell);
    // Photos without haze, and cells without pixels, stay clear.
    var transmission = 1.0;
    var guide = 0.0;
    var removal_share = 1.0;
    let counted = haze_cell_pixels(data) > 0.0;
    if counted && airlight.w > 0.0 {
        let veil = clamp(haze_opened_dark(cell, grid, airlight), 0.0, 1.0);
        transmission = 1.0 - HAZE_VEIL_SHARE * veil;
        guide = Haze::haze_guide(haze_cell_mean(data), airlight);
        removal_share = haze_removal_share(cell, grid);
    }
    textureStore(
        haze_transmission_out,
        cell,
        vec4<f32>(transmission, guide, select(0.0, 1.0, counted), removal_share),
    );
}

@compute @workgroup_size(8, 8, 1)
fn haze_guide_coefficients(@builtin(global_invocation_id) gid: vec3<u32>) {
    let grid = vec2<i32>(Haze::haze_grid());
    let cell = vec2<i32>(gid.xy);
    if any(cell >= grid) { return; }
    // Window means of the transmission t, the guide g, g * g and g * t.
    var sums = vec4<f32>(0.0);
    var count = 0.0;
    for (var dy = -HAZE_GUIDE_RADIUS; dy <= HAZE_GUIDE_RADIUS; dy = dy + 1) {
        for (var dx = -HAZE_GUIDE_RADIUS; dx <= HAZE_GUIDE_RADIUS; dx = dx + 1) {
            let neighbour = cell + vec2<i32>(dx, dy);
            if any(neighbour < vec2<i32>(0)) || any(neighbour >= grid) { continue; }
            let sample = textureLoad(haze_transmission_tex, neighbour, 0);
            let weight = sample.z;
            let t = sample.x;
            let g = sample.y;
            sums = sums + weight * vec4<f32>(t, g, g * g, g * t);
            count = count + weight;
        }
    }
    var coefficients = vec2<f32>(0.0, 1.0);
    if count > 0.0 {
        let means = sums / count;
        let variance = max(means.z - means.y * means.y, 0.0);
        let covariance = means.w - means.y * means.x;
        let slope = covariance / (variance + HAZE_GUIDE_EPSILON);
        coefficients = vec2<f32>(slope, means.x - slope * means.y);
    }
    let removal_share = textureLoad(haze_transmission_tex, cell, 0).w;
    textureStore(haze_coefficients_out, cell, vec4<f32>(coefficients, removal_share, 1.0));
}

@compute @workgroup_size(8, 8, 1)
fn smooth_haze_coefficients(@builtin(global_invocation_id) gid: vec3<u32>) {
    let grid = vec2<i32>(Haze::haze_grid());
    let cell = vec2<i32>(gid.xy);
    if any(cell >= grid) { return; }
    var sum = vec3<f32>(0.0);
    var count = 0.0;
    for (var dy = -HAZE_GUIDE_RADIUS; dy <= HAZE_GUIDE_RADIUS; dy = dy + 1) {
        for (var dx = -HAZE_GUIDE_RADIUS; dx <= HAZE_GUIDE_RADIUS; dx = dx + 1) {
            let neighbour = cell + vec2<i32>(dx, dy);
            if any(neighbour < vec2<i32>(0)) || any(neighbour >= grid) { continue; }
            sum = sum + textureLoad(haze_coefficients_tex, neighbour, 0).xyz;
            count = count + 1.0;
        }
    }
    textureStore(haze_map_out, cell, vec4<f32>(sum / count, 1.0));
}
