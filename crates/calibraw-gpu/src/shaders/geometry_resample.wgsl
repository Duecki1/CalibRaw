// Export geometry: resamples the developed photo through the edit's crop, quarter turns,
// flips, rotation, perspective and lens correction into the output raster
// (pipeline/gpu/geometry_resample.rs drives the passes).
//
// Colour: display-linear Rec.2020 RGB as the export tiles rendered it. Values pass through
// unclamped, so negative and HDR samples survive to the encoder.
// Coordinates: pixel centres at integers. The homography maps an output-raster pixel to the
// lens-corrected source pixel and the lens map then gives the source pixel shown there; the
// functions below mirror `GeometryInverseMap::source_position` and `LensGeometryMap` in
// calibraw-core, which map the same positions for thumbnails and the editor.
// Filter: an elliptical Mitchell-Netravali (B = C = 1/3) footprint shaped by the map's
// Jacobian and at least one source pixel across, so downscaling and stretching do not alias.
// Boundary: a position outside [-0.5, size - 0.5] shows black; taps are clamped to the image.
// Precision: f32 throughout. Positions differ from an f64 evaluation of the homography by
// about 1e-3 pixel.
//
// Passes: `geometry_footprint` finds the source rectangle each output cell reads, so the host
// uploads only that; `geometry_accumulate` adds a band's taps that lie in the uploaded chunk
// of source rows, in increasing row order, so a pixel's sum is the same however its rows are
// split into chunks; `geometry_resolve` turns the sums into the band's RGB rows.

struct GeometryResampleParams {
    // Rows of the homography from output-raster pixel (x, y, 1) to lens-corrected source
    // pixel; w unused.
    source_from_output: array<vec4<f32>, 3>,
    source_size: vec2<u32>,
    raster_size: vec2<u32>,
    // The part of the raster the export produces: left, top, width, height.
    window: vec4<u32>,
    // The lens map's image size and its grid size; unused without GEOMETRY_LENS.
    lens_map_size: vec2<u32>,
    lens_grid_size: vec2<u32>,
    // First window row of the band and its row count.
    band: vec2<u32>,
    // First window column of the accumulated region and its width.
    region: vec2<u32>,
    // Source pixels the chunk buffer holds: x, y, width, height.
    chunk: vec4<u32>,
    // Window pixels per footprint cell edge, and cells per window row.
    cell_edge: u32,
    cells_per_row: u32,
    // GEOMETRY_AFFINE | GEOMETRY_LENS.
    flags: u32,
    // `w` at or below which a point lies on or behind the vanishing line.
    horizon_epsilon: f32,
}

// The map has no lens or perspective part, so one Jacobian holds for every pixel.
const GEOMETRY_AFFINE: u32 = 1u;
const GEOMETRY_LENS: u32 = 2u;

// What an output pixel shows. Zero, the cleared accumulator, is OUTSIDE.
const PLAN_OUTSIDE: u32 = 0u;
// The source pixel nearest its position.
const PLAN_NEAREST: u32 = 1u;
// The weighted taps of its footprint.
const PLAN_FILTERED: u32 = 2u;

const MITCHELL_B: f32 = 1.0 / 3.0;
const MITCHELL_C: f32 = 1.0 / 3.0;

@group(0) @binding(0) var<uniform> params: GeometryResampleParams;
// Source position, in lens-map pixels, of every grid point in row order.
@group(0) @binding(1) var<storage, read> lens_grid: array<vec2<f32>>;
// Interleaved RGB of the source rectangle `params.chunk`, row by row.
@group(0) @binding(2) var<storage, read> chunk_rgb: array<f32>;
// Per footprint cell: max(!min_x), max(!min_y), max(max_x + 1), max(max_y + 1) of the source
// pixels it reads (inclusive bounds), so a zeroed cell reads nothing.
@group(0) @binding(3) var<storage, read_write> cell_bounds: array<atomic<u32>>;
// Per band pixel: (weighted RGB sum, weight sum), then (nearest source RGB, plan kind).
@group(0) @binding(4) var<storage, read_write> accumulator: array<vec4<f32>>;
// Interleaved RGB of the band, window width by band rows.
@group(0) @binding(5) var<storage, read_write> output_rgb: array<f32>;

struct Position {
    xy: vec2<f32>,
    // False on or behind the vanishing line, where the CPU map gives infinity.
    valid: bool,
}

struct Jacobian {
    // Source offset of one output pixel to the right and one down.
    x: vec2<f32>,
    y: vec2<f32>,
    valid: bool,
}

struct FilterAxes {
    major: vec2<f32>,
    minor: vec2<f32>,
    // Footprint scale along major and minor, at least one pixel.
    scales: vec2<f32>,
    // Half extent of the footprint's bounding box in x and y.
    radius: vec2<f32>,
}

struct Plan {
    kind: u32,
    position: vec2<f32>,
    // The pixel a NEAREST plan copies and a FILTERED plan falls back to when its weights
    // cancel.
    nearest: vec2<u32>,
    axes: FilterAxes,
    // Inclusive bounds of a FILTERED plan's taps.
    taps_min: vec2<u32>,
    taps_max: vec2<u32>,
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    // Not mix(): `a * (1 - t) + b * t` rounds differently from the CPU's form.
    return a + (b - a) * t;
}

// Rust's f32::round: halves away from zero (WGSL's round() takes halves to even).
fn round_half_away(value: f32) -> f32 {
    let whole = trunc(value);
    if (abs(value - whole) >= 0.5) {
        return whole + sign(value);
    }
    return whole;
}

// `Homography::apply` (mapping.rs).
fn corrected_position(output: vec2<f32>) -> Position {
    let rows = params.source_from_output;
    let w = rows[2].x * output.x + rows[2].y * output.y + rows[2].z;
    if (!(w > params.horizon_epsilon)) {
        return Position(vec2<f32>(0.0), false);
    }
    let x = (rows[0].x * output.x + rows[0].y * output.y + rows[0].z) / w;
    let y = (rows[1].x * output.x + rows[1].y * output.y + rows[1].z) / w;
    return Position(vec2<f32>(x, y), true);
}

// `scale_pixel_coordinate` (lens_map.rs).
fn scale_pixel_coordinate(value: f32, from_extent: u32, to_extent: u32) -> f32 {
    if (from_extent <= 1u || to_extent <= 1u) {
        return 0.0;
    }
    return value * f32(to_extent - 1u) / f32(from_extent - 1u);
}

// `LensGeometryMap::source_position` (lens_map.rs): bilinear in the grid, clamped to it.
fn lens_map_position(point: vec2<f32>) -> vec2<f32> {
    let size = params.lens_map_size;
    let grid = params.lens_grid_size;
    var gx = 0.0;
    if (size.x > 1u) {
        gx = point.x / f32(size.x - 1u) * f32(grid.x - 1u);
    }
    var gy = 0.0;
    if (size.y > 1u) {
        gy = point.y / f32(size.y - 1u) * f32(grid.y - 1u);
    }
    gx = clamp(gx, 0.0, f32(grid.x - 1u));
    gy = clamp(gy, 0.0, f32(grid.y - 1u));
    let x0 = u32(floor(gx));
    let y0 = u32(floor(gy));
    let x1 = min(x0 + 1u, grid.x - 1u);
    let y1 = min(y0 + 1u, grid.y - 1u);
    let tx = gx - f32(x0);
    let ty = gy - f32(y0);
    let a = lens_grid[y0 * grid.x + x0];
    let b = lens_grid[y0 * grid.x + x1];
    let c = lens_grid[y1 * grid.x + x0];
    let d = lens_grid[y1 * grid.x + x1];
    return vec2<f32>(
        lerp(lerp(a.x, b.x, tx), lerp(c.x, d.x, tx), ty),
        lerp(lerp(a.y, b.y, tx), lerp(c.y, d.y, tx), ty),
    );
}

// `LensGeometryMap::source_position_for_raster` (lens_map.rs).
fn lens_source_position(corrected: vec2<f32>) -> vec2<f32> {
    let raster = params.source_size;
    let map = params.lens_map_size;
    let mapped = lens_map_position(vec2<f32>(
        scale_pixel_coordinate(corrected.x, raster.x, map.x),
        scale_pixel_coordinate(corrected.y, raster.y, map.y),
    ));
    return vec2<f32>(
        scale_pixel_coordinate(mapped.x, map.x, raster.x),
        scale_pixel_coordinate(mapped.y, map.y, raster.y),
    );
}

// `GeometryInverseMap::source_position` (inverse_map.rs): positions outside the corrected
// image skip the lens map.
fn source_position(output: vec2<f32>) -> Position {
    let corrected = corrected_position(output);
    let size = vec2<f32>(params.source_size);
    if (!corrected.valid
        || corrected.xy.x < -0.5
        || corrected.xy.y < -0.5
        || corrected.xy.x > size.x - 0.5
        || corrected.xy.y > size.y - 0.5
        || (params.flags & GEOMETRY_LENS) == 0u) {
        return corrected;
    }
    return Position(lens_source_position(corrected.xy), true);
}

// The source offsets of the next output pixel right and down (the previous one at the
// raster's last column or row), at `output` clamped to the raster.
fn jacobian_at(output: vec2<f32>) -> Jacobian {
    let raster = params.raster_size;
    let point = clamp(output, vec2<f32>(0.0), vec2<f32>(raster - vec2<u32>(1u)));
    let origin = source_position(point);
    var valid = origin.valid;
    var x: vec2<f32>;
    if (raster.x == 1u) {
        let left = source_position(vec2<f32>(-0.5, point.y));
        let right = source_position(vec2<f32>(0.5, point.y));
        x = right.xy - left.xy;
        valid = valid && left.valid && right.valid;
    } else {
        var sample_x = point.x + 1.0;
        var direction = 1.0;
        if (!(point.x + 1.0 < f32(raster.x))) {
            sample_x = point.x - 1.0;
            direction = -1.0;
        }
        let step = source_position(vec2<f32>(sample_x, point.y));
        x = (step.xy - origin.xy) * direction;
        valid = valid && step.valid;
    }
    var y: vec2<f32>;
    if (raster.y == 1u) {
        let top = source_position(vec2<f32>(point.x, -0.5));
        let bottom = source_position(vec2<f32>(point.x, 0.5));
        y = bottom.xy - top.xy;
        valid = valid && top.valid && bottom.valid;
    } else {
        var sample_y = point.y + 1.0;
        var direction = 1.0;
        if (!(point.y + 1.0 < f32(raster.y))) {
            sample_y = point.y - 1.0;
            direction = -1.0;
        }
        let step = source_position(vec2<f32>(point.x, sample_y));
        y = (step.xy - origin.xy) * direction;
        valid = valid && step.valid;
    }
    return Jacobian(x, y, valid);
}

// The ellipse an output pixel covers in the source: the eigenvectors of J Jᵀ, with each
// axis at least one pixel long.
fn filter_axes(jacobian: Jacobian) -> FilterAxes {
    let jx = jacobian.x;
    let jy = jacobian.y;
    let c00 = jx.x * jx.x + jy.x * jy.x;
    let c01 = jx.x * jx.y + jy.x * jy.y;
    let c11 = jx.y * jx.y + jy.y * jy.y;
    let trace = c00 + c11;
    let discriminant = sqrt((c00 - c11) * (c00 - c11) + 4.0 * c01 * c01);
    let lambda_major = max((trace + discriminant) * 0.5, 0.0);
    let lambda_minor = max((trace - discriminant) * 0.5, 0.0);

    var major = vec2<f32>(1.0, 0.0);
    if (discriminant > 1e-8) {
        let candidate_a = vec2<f32>(c01, lambda_major - c00);
        let candidate_b = vec2<f32>(lambda_major - c11, c01);
        let norm_a = candidate_a.x * candidate_a.x + candidate_a.y * candidate_a.y;
        let norm_b = candidate_b.x * candidate_b.x + candidate_b.y * candidate_b.y;
        var candidate = candidate_b;
        if (norm_a >= norm_b) {
            candidate = candidate_a;
        }
        let length = sqrt(candidate.x * candidate.x + candidate.y * candidate.y);
        if (length > 1e-8) {
            major = vec2<f32>(candidate.x / length, candidate.y / length);
        }
    }
    let minor = vec2<f32>(-major.y, major.x);
    let major_scale = max(sqrt(lambda_major), 1.0);
    let minor_scale = max(sqrt(lambda_minor), 1.0);
    let radius = vec2<f32>(
        2.0 * (abs(major.x) * major_scale + abs(minor.x) * minor_scale),
        2.0 * (abs(major.y) * major_scale + abs(minor.y) * minor_scale),
    );
    return FilterAxes(major, minor, vec2<f32>(major_scale, minor_scale), radius);
}

fn mitchell_netravali(distance: f32) -> f32 {
    let value = abs(distance);
    if (value >= 2.0) {
        return 0.0;
    }
    let value2 = value * value;
    let value3 = value2 * value;
    if (value < 1.0) {
        return ((12.0 - 9.0 * MITCHELL_B - 6.0 * MITCHELL_C) * value3
            + (-18.0 + 12.0 * MITCHELL_B + 6.0 * MITCHELL_C) * value2
            + (6.0 - 2.0 * MITCHELL_B)) / 6.0;
    }
    return ((-MITCHELL_B - 6.0 * MITCHELL_C) * value3
        + (6.0 * MITCHELL_B + 30.0 * MITCHELL_C) * value2
        + (-12.0 * MITCHELL_B - 48.0 * MITCHELL_C) * value
        + (8.0 * MITCHELL_B + 24.0 * MITCHELL_C)) / 6.0;
}

fn raster_point(window_x: u32, window_y: u32) -> vec2<f32> {
    return vec2<f32>(f32(params.window.x + window_x), f32(params.window.y + window_y));
}

// What the output pixel at raster position `output` shows, and which source pixels it reads.
fn plan_for(output: vec2<f32>) -> Plan {
    var plan: Plan;
    let position = source_position(output);
    let p = position.xy;
    let size = vec2<f32>(params.source_size);
    if (!position.valid || p.x < -0.5 || p.y < -0.5 || p.x > size.x - 0.5 || p.y > size.y - 0.5) {
        plan.kind = PLAN_OUTSIDE;
        return plan;
    }
    plan.position = p;
    let rounded = vec2<f32>(round_half_away(p.x), round_half_away(p.y));
    let last = size - vec2<f32>(1.0);
    plan.nearest = vec2<u32>(clamp(rounded, vec2<f32>(0.0), last));

    var jacobian_point = output;
    if ((params.flags & GEOMETRY_AFFINE) != 0u) {
        jacobian_point = vec2<f32>(0.0);
    }
    let jacobian = jacobian_at(jacobian_point);
    if (!jacobian.valid) {
        // A neighbour beyond the vanishing line: the CPU's Jacobian was not finite and it
        // fell back to the nearest pixel.
        plan.kind = PLAN_NEAREST;
        return plan;
    }
    let axes = filter_axes(jacobian);
    if (axes.scales.x <= 1.0 + 1e-6 && axes.scales.y <= 1.0 + 1e-6
        && abs(p.x - rounded.x) <= 1e-6 && abs(p.y - rounded.y) <= 1e-6) {
        plan.kind = PLAN_NEAREST;
        return plan;
    }
    plan.kind = PLAN_FILTERED;
    plan.axes = axes;
    plan.taps_min = vec2<u32>(max(floor(p - axes.radius), vec2<f32>(0.0)));
    plan.taps_max = vec2<u32>(min(ceil(p + axes.radius), last));
    return plan;
}

fn chunk_index(source_x: u32, source_y: u32) -> u32 {
    let chunk = params.chunk;
    return ((source_y - chunk.y) * chunk.z + (source_x - chunk.x)) * 3u;
}

// `sum` plus the weighted taps of `plan` in source rows `first_row..=last_row`.
fn add_taps(plan: Plan, sum: vec4<f32>, first_row: u32, last_row: u32) -> vec4<f32> {
    var total = sum;
    for (var source_y = first_row; source_y <= last_row; source_y += 1u) {
        for (var source_x = plan.taps_min.x; source_x <= plan.taps_max.x; source_x += 1u) {
            let dx = f32(source_x) - plan.position.x;
            let dy = f32(source_y) - plan.position.y;
            let major_distance =
                (dx * plan.axes.major.x + dy * plan.axes.major.y) / plan.axes.scales.x;
            let minor_distance =
                (dx * plan.axes.minor.x + dy * plan.axes.minor.y) / plan.axes.scales.y;
            let radius_squared = major_distance * major_distance + minor_distance * minor_distance;
            if (radius_squared >= 4.0) {
                continue;
            }
            let weight = mitchell_netravali(sqrt(radius_squared));
            if (weight == 0.0) {
                continue;
            }
            let index = chunk_index(source_x, source_y);
            total = vec4<f32>(
                total.x + chunk_rgb[index] * weight,
                total.y + chunk_rgb[index + 1u] * weight,
                total.z + chunk_rgb[index + 2u] * weight,
                total.w + weight,
            );
        }
    }
    return total;
}

var<workgroup> workgroup_bounds: array<atomic<u32>, 4>;

// Over the whole window: the source rectangle every cell reads. A cell's edge is a multiple
// of the workgroup's, so a workgroup merges its pixels and writes its cell once.
@compute @workgroup_size(8, 8)
fn geometry_footprint(
    @builtin(global_invocation_id) id: vec3<u32>,
    @builtin(local_invocation_index) local_index: u32,
) {
    if (id.x < params.window.z && id.y < params.window.w) {
        let plan = plan_for(raster_point(id.x, id.y));
        if (plan.kind != PLAN_OUTSIDE) {
            var low = plan.nearest;
            var high = plan.nearest;
            if (plan.kind == PLAN_FILTERED) {
                low = plan.taps_min;
                high = plan.taps_max;
            }
            atomicMax(&workgroup_bounds[0], ~low.x);
            atomicMax(&workgroup_bounds[1], ~low.y);
            atomicMax(&workgroup_bounds[2], high.x + 1u);
            atomicMax(&workgroup_bounds[3], high.y + 1u);
        }
    }
    workgroupBarrier();
    if (local_index == 0u) {
        let reads = atomicLoad(&workgroup_bounds[2]);
        if (reads != 0u) {
            let cell = id.xy / params.cell_edge;
            let slot = (cell.y * params.cells_per_row + cell.x) * 4u;
            atomicMax(&cell_bounds[slot], atomicLoad(&workgroup_bounds[0]));
            atomicMax(&cell_bounds[slot + 1u], atomicLoad(&workgroup_bounds[1]));
            atomicMax(&cell_bounds[slot + 2u], reads);
            atomicMax(&cell_bounds[slot + 3u], atomicLoad(&workgroup_bounds[3]));
        }
    }
}

// Over the region's columns of the band: adds the taps in the chunk's rows. The chunk holds
// every column the region reads.
@compute @workgroup_size(8, 8)
fn geometry_accumulate(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= params.region.y || id.y >= params.band.y) {
        return;
    }
    let window_x = params.region.x + id.x;
    let plan = plan_for(raster_point(window_x, params.band.x + id.y));
    if (plan.kind == PLAN_OUTSIDE) {
        return;
    }
    let slot = (id.y * params.window.z + window_x) * 2u;
    let first_row = params.chunk.y;
    let last_row = params.chunk.y + params.chunk.w - 1u;
    if (plan.nearest.y >= first_row && plan.nearest.y <= last_row) {
        let index = chunk_index(plan.nearest.x, plan.nearest.y);
        accumulator[slot + 1u] = vec4<f32>(
            chunk_rgb[index],
            chunk_rgb[index + 1u],
            chunk_rgb[index + 2u],
            f32(plan.kind),
        );
    }
    if (plan.kind == PLAN_FILTERED) {
        let from_row = max(plan.taps_min.y, first_row);
        let to_row = min(plan.taps_max.y, last_row);
        if (from_row <= to_row) {
            accumulator[slot] = add_taps(plan, accumulator[slot], from_row, to_row);
        }
    }
}

// Over the band: normalizes the sums into RGB.
@compute @workgroup_size(8, 8)
fn geometry_resolve(@builtin(global_invocation_id) id: vec3<u32>) {
    let width = params.window.z;
    if (id.x >= width || id.y >= params.band.y) {
        return;
    }
    let pixel = id.y * width + id.x;
    let sum = accumulator[pixel * 2u];
    let nearest = accumulator[pixel * 2u + 1u];
    let kind = u32(nearest.w);
    var rgb = vec3<f32>(0.0);
    if (kind == PLAN_NEAREST) {
        rgb = nearest.xyz;
    } else if (kind == PLAN_FILTERED) {
        if (abs(sum.w) > 1e-6) {
            rgb = sum.xyz / sum.w;
        } else {
            rgb = nearest.xyz;
        }
    }
    output_rgb[pixel * 3u] = rgb.x;
    output_rgb[pixel * 3u + 1u] = rgb.y;
    output_rgb[pixel * 3u + 2u] = rgb.z;
}
