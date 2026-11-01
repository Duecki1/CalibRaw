#import calibraw::common as Common
#import calibraw::color as Color
#import calibraw::scene_source as SceneSource

// Builds the scene map that every effect reading the scene shares (read through
// mask_effects/scene_depth.wgsl and scene_environment.wgsl), on a coarse grid over the
// full image (Common::scene_map_grid), with the tone statistics like the image-light
// and haze grids:
//   accumulate_scene_map       per cell, the scene's colour, the sums of the guide
//                              and scene depth and stored depth's range. Export tiles
//                              add only their cores, so every tile later reads the
//                              whole image's map.
//   resolve_scene_map          the environment map: each cell's mean colour in
//                              unexposed scene-linear Rec.2020 and how much of a
//                              sharp edge the photo has there. Also a guided filter's
//                              coefficients in one window, how rough the photo's
//                              surface is there and its mean guide, and around the
//                              cell stored depth's range, how well the guide explains
//                              depth and how much of a depth edge there is.
//   smooth_scene_coefficients  the coefficients averaged over the windows covering
//                              each cell.
//   blur_scene_environment     a wide blur of the environment map, what a drop
//                              refracting about 165 degrees of its surroundings sees.
//   build_scene_ground         per cell, how much it is ground, how far it lies below
//                              what stands on it and how far it rises above the ground
//                              around it.
//   build_scene_focus          the lens blur at each depth.
//
// The guided filter (He, Sun and Tang, "Guided Image Filtering", 2010) is what keeps
// depth edges clean. Stored depth has blocky, misplaced edges at full size. Pixels
// instead evaluate the map's coefficients against the guide value of the photo itself
// (He and Sun, "Fast Guided Filter", 2015), a linear function that has no per-pixel
// decision to make: edges land where the photo's edges are, without staircases or a
// fringe of the texture beside them. The map depends only on the scene, scene depth
// and the direction of up, not on the rest of the edit.
//
// The ground is found in scene depth: surfaces whose depth rises toward the
// horizon, which walls (a sideways gradient) and surfaces facing the camera
// (hardly any) do not have. How fast it rises depends on the photo, so only its
// direction and that it rises at all count (ground_plane.rs finds the horizon by the same rule). Ground also runs on toward the viewer and has more ground beside it, which
// a patch of a wall or a pole's top with the same slope does not. Each cell's contact distance averages
// a few neighbouring rays, so a reflection changes gradually beside a pole rather
// than along a seam.
//
// Water lies in the ground's low places, so each cell also gets a terrain height.
// A surface on the broad slope of the ground around it is level. A kerb's face and
// top are not: their depth rises at another rate and stands out from the plane
// the ground around them predicts. The depth model blurs a kerb and misplaces it by
// about its own height, so depth only tells that the ground is uneven nearby; the
// photo's sharp edges there are where it steps. A shadow's edge on level ground,
// with no unevenness near it, stops no water.
//
// What is drawn on a surface must be as soft as the photo of that surface, so the
// map also holds how blurred the lens made each depth. The width of the photo's own
// strong edges tells: across a step blurred by a Gaussian of deviation s, half of
// the rise lies within 0.674 s of its centre on each side, and that holds up against
// noise where fine detail statistics do not. A cell's edge may be soft for another
// reason, and most surfaces have no edge at all, so cells pool by depth: the lens
// blurs by distance alone, and a featureless surface takes the blur of what lies at
// its depth.

struct SceneMapCell {
    // Summed colour (xyz) and pixels counted (w).
    color: vec4<f32>,
    // Summed guide g, g * g, scene depth d and g * d.
    guide: vec4<f32>,
    // A strong edge's blur in pixels (x) and contrast (y) measured at the cell's
    // centre, or nothing.
    edge: vec2<f32>,
    // One minus the nearest stored depth (x) and the farthest (y), so that a cell
    // cleared to zero holds no range yet and tiles merge theirs with `max`.
    depth_range: vec2<f32>,
    // Summed d * d.
    depth_square: f32,
}

@group(0) @binding(35) var scene_depth_tex: texture_2d<f32>;
@group(0) @binding(62) var<storage, read_write> scene_map_cells: array<SceneMapCell>;
@group(0) @binding(63) var scene_environment_out: texture_storage_2d<rgba16float, write>;
// Per cell: the guided filter's slope (x) and offset (y) in one window, the
// surface's roughness (z, scene_map_cell_roughness) and the window's mean guide (w).
@group(0) @binding(64) var scene_coefficients_out: texture_storage_2d<rgba32float, write>;
@group(0) @binding(65) var scene_coefficients_tex: texture_2d<f32>;
// The window-averaged slope, offset, roughness and mean guide that scene_depth.wgsl
// reads.
@group(0) @binding(66) var scene_depth_map_out: texture_storage_2d<rgba32float, write>;
@group(0) @binding(67) var scene_environment_tex: texture_2d<f32>;
@group(0) @binding(68) var scene_environment_blur_out: texture_storage_2d<rgba16float, write>;
@group(0) @binding(28) var scene_map_sampler: sampler;
// Per cell, around it: the nearest (x) and farthest (y) stored depth, how well the
// guide explains depth (z) and how much of a depth edge there is (w).
@group(0) @binding(76) var scene_depth_range_out: texture_storage_2d<rgba16float, write>;
// Per cell: how much it is ground (x), the distance from it along the ground to
// what stands on it (y), its smooth scene depth (z) and its terrain height (w,
// scene_terrain_height).
@group(0) @binding(72) var scene_ground_out: texture_storage_2d<rgba16float, write>;
// Per depth bin: the lens blur (x) in shorter edges.
@group(0) @binding(74) var scene_focus_out: texture_storage_2d<rgba16float, write>;

// Radius in cells of the guided filter's windows: 3 x 3 cells, about 1.2% of
// the longer edge. It must be wider than the stored depth's staircases.
const SCENE_GUIDE_RADIUS: i32 = 1;
// The filter's regularization relative to the window's squared mean guide.
// Where the guide varies by less than about its square root across a window,
// depth is smoothed instead of following the guide, so surface texture does
// not print into depth.
const SCENE_GUIDE_EPSILON: f32 = 1e-3;
// Largest change of depth across the window's mean guide value that the slope
// may make. A full depth step between two guide values of nearly the same
// brightness would otherwise amplify their noise.
const SCENE_GUIDE_MAX_RELATIVE_SLOPE: f32 = 3.0;
// Squared correlation of guide and depth in a window, from which the guide does not
// explain depth at all to where it does.
const SCENE_GUIDE_FIT: vec2<f32> = vec2<f32>(0.15, 0.5);
// Radius in cells over which the width of stored depth's range is averaged to find
// depth edges, and the mean width over which a surface becomes one. A surface's own
// relief and slope stay below it. The wide average makes the change gradual: depth
// is so close to the filter's at either end that only an abrupt change would show.
const SCENE_DEPTH_EDGE_RADIUS: i32 = 2;
const SCENE_DEPTH_EDGE_WIDTH: vec2<f32> = vec2<f32>(0.015, 0.05);
// How far a window's relative guide variance must exceed that of the smoothest cell
// in it for an edge to start and to be complete. Texture raises both alike.
const SCENE_EDGE_EXCESS: vec2<f32> = vec2<f32>(0.004, 0.02);
// The environment blur's standard deviation as a fraction of the longer edge,
// and its stride in environment cells.
const SCENE_BLUR_SIGMA: f32 = 0.08;
const SCENE_BLUR_STRIDE: i32 = 4;

// The surface mip level ground cells read, 256 x 256 texels, and the coarser one
// reflection rays and the walk toward the viewer read: a thin rail does not end a
// reflection, and interpolating there keeps contact distances continuous.
const SCENE_GROUND_LEVEL: f32 = 2.0;
const SCENE_GROUND_RAY_LEVEL: f32 = 3.0;
// Depth per shorter edge over which a surface's slope comes to count as ground,
// and the cosines of the angle from up over which its direction does.
// ground_plane.rs tests the middle of each.
const SCENE_GROUND_SLOPE: vec2<f32> = vec2<f32>(0.06, 0.18);
const SCENE_GROUND_ALIGNMENT: vec2<f32> = vec2<f32>(0.82, 0.96);
// Depths beyond which nothing is ground: the sky and far scenery. The generated map's
// few last levels span most of the distance, so a higher limit would take their noise
// for ground and let reflection rays run on into the sky.
const SCENE_GROUND_FAR: vec2<f32> = vec2<f32>(0.48, 0.77);
// Below the horizon in shorter edges over which ground fades in.
const SCENE_HORIZON_FADE: f32 = 0.03;
// Points checked toward the viewer for ground running on from a cell, how far
// they reach in shorter edges, and their mean score from no to full support.
const SCENE_SUPPORT_STEPS: u32 = 5u;
const SCENE_SUPPORT_REACH: f32 = 0.22;
const SCENE_SUPPORT_RANGE: vec2<f32> = vec2<f32>(0.3, 0.65);
// A ray's score below this ends the ground, unless the ground goes on within
// SCENE_THIN_OBJECT shorter edges: a chain or rail reflects as nothing visible
// and would only break the reflection of what lies behind it.
const SCENE_CONTACT_SCORE: f32 = 0.35;
const SCENE_THIN_OBJECT: f32 = 0.05;
const SCENE_CONTACT_STEPS: u32 = 14u;
// Halvings of the step that found the contact. Long steps would otherwise show
// as bands in the reflection, where the contact jumps from one step to the next.
const SCENE_CONTACT_REFINEMENTS: u32 = 5u;
// Offsets of the averaged rays across the view, in cells.
const SCENE_CONTACT_RAY_OFFSET: f32 = 0.75;
// The surface mip level that gives the broad slope of the ground around a cell,
// 32 x 32 texels.
const SCENE_TERRAIN_BROAD_LEVEL: f32 = 5.0;
// Radius of the ring of points predicting a cell's depth, in shorter edges: about
// the width of a kerb, and how many points it has.
const SCENE_TERRAIN_RING: f32 = 0.035;
const SCENE_TERRAIN_RING_POINTS: u32 = 12u;
// How far beside a point ground is looked for, per shorter edge below the horizon
// (a fixed width on the ground), and its score from none to enough.
const SCENE_GROUND_BESIDE_REACH: f32 = 0.12;
const SCENE_GROUND_BESIDE_RANGE: vec2<f32> = vec2<f32>(0.2, 0.6);
// Share of the slope of the ground it runs on from over which a surface stops being
// ground. Ground's slope changes gradually in depth; a wall with a little of the
// same slope stands on far steeper ground.
const SCENE_GROUND_SLOPE_SHARE: vec2<f32> = vec2<f32>(0.14, 0.24);
// Share of the ring that must be ground for its prediction to count.
const SCENE_TERRAIN_MIN_SUPPORT: f32 = 0.25;
// Height in camera heights from which relief counts no further, either way.
const SCENE_TERRAIN_RELIEF_LIMIT: vec2<f32> = vec2<f32>(-0.06, 0.2);
// Departure of a surface's slope from the broad slope, relative to the broad slope,
// over which it stops being level, and the height that adds. Past it no water
// stands there at any Puddles setting.
const SCENE_TERRAIN_DEPARTURE: vec2<f32> = vec2<f32>(0.14, 0.34);
const SCENE_TERRAIN_DEPARTURE_RISE: f32 = 0.5;
// The same for a photo edge in the uneven ground near a cell, and how far toward the
// viewer from the cell an edge counts, in cells. A kerb's face and top lie beyond its
// foot, and the road before the foot keeps its water right up to it.
const SCENE_TERRAIN_EDGE_RISE: f32 = 0.5;
const SCENE_TERRAIN_EDGE_REACH: i32 = 3;
// Smallest broad slope to divide by, in depth per shorter edge.
const SCENE_TERRAIN_MIN_SLOPE: f32 = 0.05;
// Farthest an edge profile reads from a cell's centre, in pixels
// (SCENE_EDGE_SUPPORT in tiles.rs), and how many samples it has at most on each
// side. Samples lie further apart in larger images, so the profile covers the same
// share of a photo at every size until the reach limits it.
const SCENE_EDGE_REACH: i32 = 64;
const SCENE_EDGE_SAMPLES: i32 = 32;
const SCENE_EDGE_LONG_PER_STEP: f32 = 3500.0;
// Fewest samples on each side of an edge's centre to measure it.
const SCENE_EDGE_MIN_SAMPLES: i32 = 4;
// Pixels beside a profile averaged into it, against noise.
const SCENE_EDGE_LINES: i32 = 1;
// Rise in the guide from which an edge counts: far above noise.
const SCENE_EDGE_MIN_CONTRAST: f32 = 0.06;
// Share of its largest rise an edge must keep at the profile's end. Otherwise the
// edge goes on past it or is a thin line, and its width says nothing.
const SCENE_EDGE_LEVELS_OFF: f32 = 0.8;
// A step blurred by a Gaussian rises by half within this many deviations of its
// centre on each side.
const SCENE_EDGE_HALF_RISE: f32 = 0.674;
// Bins that smooth the table together on each side.
const SCENE_FOCUS_SMOOTHING: i32 = 1;

fn scene_map_cell_index(cell: vec2<i32>) -> u32 {
    return u32(cell.y) * Common::SCENE_MAP_GRID_LONG + u32(cell.x);
}

@compute @workgroup_size(8, 8, 1)
fn accumulate_scene_map(@builtin(global_invocation_id) gid: vec3<u32>) {
    let grid = Common::scene_map_grid();
    if gid.x >= grid.x || gid.y >= grid.y { return; }
    let span_x = Common::full_image_grid_pixel_span(gid.x, grid.x, Common::camera_uniforms.full_width);
    let span_y = Common::full_image_grid_pixel_span(gid.y, grid.y, Common::camera_uniforms.full_height);
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

    let full = vec2<f32>(
        f32(max(Common::camera_uniforms.full_width, 1u)),
        f32(max(Common::camera_uniforms.full_height, 1u)),
    );
    let depth_size = vec2<f32>(textureDimensions(scene_depth_tex));
    var color = vec4<f32>(0.0);
    var guide = vec4<f32>(0.0);
    var depth_range = vec2<f32>(0.0);
    var depth_square = 0.0;
    for (var y = lo.y; y < hi.y; y = y + 1) {
        for (var x = lo.x; x < hi.x; x = x + 1) {
            let pos = vec2<i32>(x, y) - origin;
            let rgb = max(Color::map_negative_gamut(SceneSource::profiled_scene_at(pos)), vec3<f32>(0.0));
            color += vec4<f32>(rgb, 1.0);
            // The nearest depth texel: window means smooth it far more than
            // interpolating would.
            let uv = (vec2<f32>(f32(x), f32(y)) + vec2<f32>(0.5)) / full;
            let depth = textureLoad(scene_depth_tex, vec2<i32>(uv * depth_size), 0).x;
            let g = SceneSource::scene_guide_at(pos);
            guide += vec4<f32>(g, g * g, depth, g * depth);
            depth_range = max(depth_range, vec2<f32>(1.0 - depth, depth));
            depth_square += depth * depth;
        }
    }
    // Tiles are dispatched one after another, so each cell has one writer.
    let index = scene_map_cell_index(vec2<i32>(gid.xy));
    scene_map_cells[index].color += color;
    scene_map_cells[index].guide += guide;
    scene_map_cells[index].depth_range = max(scene_map_cells[index].depth_range, depth_range);
    scene_map_cells[index].depth_square += depth_square;
    // The tile that holds the cell's centre measures its edge, reading no farther
    // than the tile's halo.
    let center = vec2<i32>(span_x.x + span_x.y, span_y.x + span_y.y) / 2;
    if all(center >= lo) && all(center < hi) {
        scene_map_cells[index].edge = scene_edge_at(center, origin);
    }
}

// The guide `offset` pixels along a line through `center` in the direction `axis`,
// averaged over the pixels beside it.
fn scene_edge_sample(center: vec2<i32>, axis: vec2<i32>, offset: i32) -> f32 {
    let beside = vec2<i32>(axis.y, axis.x);
    var sum = 0.0;
    for (var line = -SCENE_EDGE_LINES; line <= SCENE_EDGE_LINES; line = line + 1) {
        sum += SceneSource::scene_guide_at(center + axis * offset + beside * line);
    }
    return sum / f32(2 * SCENE_EDGE_LINES + 1);
}

// The strong edge nearest a full-image point, if any: its blur in pixels (x) and
// contrast (y).
fn scene_edge_at(point: vec2<i32>, origin: vec2<i32>) -> vec2<f32> {
    let full = vec2<i32>(
        i32(Common::camera_uniforms.full_width),
        i32(Common::camera_uniforms.full_height),
    );
    let long = f32(max(full.x, full.y));
    let step = clamp(i32(round(long / SCENE_EDGE_LONG_PER_STEP)), 1, SCENE_EDGE_REACH / SCENE_EDGE_MIN_SAMPLES);
    let samples = min(SCENE_EDGE_SAMPLES, SCENE_EDGE_REACH / step);
    let reach = samples * step;
    if any(point < vec2<i32>(reach + SCENE_EDGE_LINES)) || any(point >= full - vec2<i32>(reach + SCENE_EDGE_LINES)) {
        return vec2<f32>(0.0);
    }
    let center = point - origin;

    // The direction the guide changes most in, at three distances: most cells have
    // no strong edge and cost only this.
    var axis = vec2<i32>(1, 0);
    var strongest = 0.0;
    for (var index = 0; index < 2; index = index + 1) {
        let candidate = vec2<i32>(1 - index, index);
        for (var distance = reach; distance >= reach / 4; distance = distance / 2) {
            let rise = abs(
                SceneSource::scene_guide_at(center + candidate * distance)
                    - SceneSource::scene_guide_at(center - candidate * distance),
            );
            if rise > strongest {
                strongest = rise;
                axis = candidate;
            }
        }
    }
    if strongest < SCENE_EDGE_MIN_CONTRAST { return vec2<f32>(0.0); }

    var profile: array<f32, 2 * SCENE_EDGE_SAMPLES + 1>;
    for (var index = 0; index <= 2 * samples; index = index + 1) {
        profile[index] = scene_edge_sample(center, axis, (index - samples) * step);
    }
    // The edge's centre: where the guide rises most over two steps.
    var middle = samples;
    var steepest = 0.0;
    for (var index = 1; index < 2 * samples; index = index + 1) {
        let rise = abs(profile[index + 1] - profile[index - 1]);
        if rise > steepest {
            steepest = rise;
            middle = index;
        }
    }
    let span = min(middle, 2 * samples - middle);
    if span < SCENE_EDGE_MIN_SAMPLES { return vec2<f32>(0.0); }
    let sign_of_rise = sign(profile[middle + 1] - profile[middle - 1]);
    var contrast = 0.0;
    for (var distance = 1; distance <= span; distance = distance + 1) {
        contrast = max(contrast, sign_of_rise * (profile[middle + distance] - profile[middle - distance]));
    }
    let end_rise = sign_of_rise * (profile[middle + span] - profile[middle - span]);
    if contrast < SCENE_EDGE_MIN_CONTRAST || end_rise < SCENE_EDGE_LEVELS_OFF * contrast {
        return vec2<f32>(0.0);
    }
    var previous = 0.0;
    var half_rise = f32(span);
    for (var distance = 1; distance <= span; distance = distance + 1) {
        let rise = sign_of_rise * (profile[middle + distance] - profile[middle - distance]);
        if rise >= 0.5 * contrast {
            half_rise = f32(distance - 1) + (0.5 * contrast - previous) / max(rise - previous, 1e-6);
            break;
        }
        previous = rise;
    }
    // The samples' spacing itself reads as about one step of blur.
    let deviation = half_rise * f32(step) / SCENE_EDGE_HALF_RISE;
    let blur = sqrt(max(deviation * deviation - f32(step * step), 0.25));
    return vec2<f32>(blur, contrast);
}

// How rough the photo's surface is in a cell: its guide's variance relative to
// its squared mean, high on gravel and grass and low on asphalt or paving.
fn scene_map_cell_roughness(data: SceneMapCell) -> f32 {
    let count = max(data.color.w, 1.0);
    let mean = data.guide.x / count;
    return max(data.guide.y / count - mean * mean, 0.0) / max(mean * mean, 1e-6);
}

@compute @workgroup_size(8, 8, 1)
fn resolve_scene_map(@builtin(global_invocation_id) gid: vec3<u32>) {
    let grid = vec2<i32>(Common::scene_map_grid());
    let cell = vec2<i32>(gid.xy);
    if any(cell >= grid) { return; }

    var sums = vec4<f32>(0.0);
    var depth_square = 0.0;
    var count = 0.0;
    var depth_range = vec2<f32>(0.0);
    // The smoothest cell nearby: a cell an edge crosses is no rougher than the
    // surfaces beside it, so an object standing on smooth ground leaves no rough
    // ring around it.
    var roughness = 1e6;
    for (var dy = -SCENE_GUIDE_RADIUS; dy <= SCENE_GUIDE_RADIUS; dy = dy + 1) {
        for (var dx = -SCENE_GUIDE_RADIUS; dx <= SCENE_GUIDE_RADIUS; dx = dx + 1) {
            let neighbour = cell + vec2<i32>(dx, dy);
            if any(neighbour < vec2<i32>(0)) || any(neighbour >= grid) { continue; }
            let data = scene_map_cells[scene_map_cell_index(neighbour)];
            sums += data.guide;
            depth_square += data.depth_square;
            count += data.color.w;
            depth_range = max(depth_range, data.depth_range);
            if data.color.w > 0.0 {
                roughness = min(roughness, scene_map_cell_roughness(data));
            }
        }
    }
    var coefficients = vec2<f32>(0.0);
    var mean_guide = 0.0;
    var edge = 0.0;
    var fit = 0.0;
    if count > 0.0 {
        let means = sums / count;
        let squared_mean = max(means.x * means.x, 1e-6);
        let variance = max(means.y - means.x * means.x, 0.0);
        let covariance = means.w - means.x * means.z;
        let limit = SCENE_GUIDE_MAX_RELATIVE_SLOPE / max(means.x, 1e-3);
        let slope = clamp(
            covariance / (variance + SCENE_GUIDE_EPSILON * squared_mean),
            -limit,
            limit,
        );
        // The offset keeps the window's mean depth at its mean guide.
        coefficients = vec2<f32>(slope, means.z - slope * means.x);
        let depth_variance = max(depth_square / count - means.z * means.z, 0.0);
        fit = smoothstep(
            SCENE_GUIDE_FIT.x,
            SCENE_GUIDE_FIT.y,
            covariance * covariance / max(variance * depth_variance, 1e-12),
        );
        mean_guide = means.x;
        edge = smoothstep(
            SCENE_EDGE_EXCESS.x,
            SCENE_EDGE_EXCESS.y,
            variance / squared_mean - roughness,
        );
    }
    let own = scene_map_cells[scene_map_cell_index(cell)].color;
    textureStore(scene_environment_out, cell, vec4<f32>(own.xyz / max(own.w, 1.0), edge));
    textureStore(scene_coefficients_out, cell, vec4<f32>(coefficients, roughness, mean_guide));
    // A window without pixels limits nothing.
    let nearest = select(0.0, 1.0 - depth_range.x, count > 0.0);
    let farthest = select(1.0, depth_range.y, count > 0.0);
    textureStore(
        scene_depth_range_out,
        cell,
        vec4<f32>(nearest, farthest, fit, scene_depth_edge(cell, grid)),
    );
}

// How much of a depth edge there is around a cell.
fn scene_depth_edge(cell: vec2<i32>, grid: vec2<i32>) -> f32 {
    var width = 0.0;
    var count = 0.0;
    for (var dy = -SCENE_DEPTH_EDGE_RADIUS; dy <= SCENE_DEPTH_EDGE_RADIUS; dy = dy + 1) {
        for (var dx = -SCENE_DEPTH_EDGE_RADIUS; dx <= SCENE_DEPTH_EDGE_RADIUS; dx = dx + 1) {
            let neighbour = cell + vec2<i32>(dx, dy);
            if any(neighbour < vec2<i32>(0)) || any(neighbour >= grid) { continue; }
            let data = scene_map_cells[scene_map_cell_index(neighbour)];
            if data.color.w <= 0.0 { continue; }
            width += max(data.depth_range.y + data.depth_range.x - 1.0, 0.0);
            count += 1.0;
        }
    }
    return smoothstep(SCENE_DEPTH_EDGE_WIDTH.x, SCENE_DEPTH_EDGE_WIDTH.y, width / max(count, 1.0));
}

@compute @workgroup_size(8, 8, 1)
fn smooth_scene_coefficients(@builtin(global_invocation_id) gid: vec3<u32>) {
    let grid = vec2<i32>(Common::scene_map_grid());
    let cell = vec2<i32>(gid.xy);
    if any(cell >= grid) { return; }
    var sum = vec4<f32>(0.0);
    var count = 0.0;
    for (var dy = -SCENE_GUIDE_RADIUS; dy <= SCENE_GUIDE_RADIUS; dy = dy + 1) {
        for (var dx = -SCENE_GUIDE_RADIUS; dx <= SCENE_GUIDE_RADIUS; dx = dx + 1) {
            let neighbour = cell + vec2<i32>(dx, dy);
            if any(neighbour < vec2<i32>(0)) || any(neighbour >= grid) { continue; }
            sum += textureLoad(scene_coefficients_tex, neighbour, 0);
            count += 1.0;
        }
    }
    textureStore(scene_depth_map_out, cell, sum / count);
}

@compute @workgroup_size(8, 8, 1)
fn blur_scene_environment(@builtin(global_invocation_id) gid: vec3<u32>) {
    let blur_grid = Common::scene_map_blur_grid();
    if gid.x >= blur_grid.x || gid.y >= blur_grid.y { return; }
    let grid = vec2<i32>(Common::scene_map_grid());
    let scale = vec2<f32>(grid) / vec2<f32>(blur_grid);
    let center = (vec2<f32>(gid.xy) + vec2<f32>(0.5)) * scale;
    let sigma = SCENE_BLUR_SIGMA * f32(Common::SCENE_MAP_GRID_LONG);
    let reach = i32(ceil(3.0 * sigma)) / SCENE_BLUR_STRIDE;
    var sum = vec3<f32>(0.0);
    var weights = 0.0;
    for (var dy = -reach; dy <= reach; dy = dy + 1) {
        for (var dx = -reach; dx <= reach; dx = dx + 1) {
            let offset = vec2<f32>(f32(dx), f32(dy)) * f32(SCENE_BLUR_STRIDE);
            let cell = vec2<i32>(floor(center + offset));
            if any(cell < vec2<i32>(0)) || any(cell >= grid) { continue; }
            let weight = exp(-0.5 * dot(offset, offset) / (sigma * sigma));
            sum += textureLoad(scene_environment_tex, cell, 0).xyz * weight;
            weights += weight;
        }
    }
    textureStore(
        scene_environment_blur_out,
        vec2<i32>(gid.xy),
        vec4<f32>(sum / max(weights, 1e-6), 1.0),
    );
}

// How much a surface texel (smooth depth in y, gradient in zw) is ground.
fn scene_texel_ground(texel: vec4<f32>, up: vec2<f32>) -> f32 {
    let slope = length(texel.zw);
    if slope <= 1e-5 { return 0.0; }
    return smoothstep(
            SCENE_GROUND_ALIGNMENT.x,
            SCENE_GROUND_ALIGNMENT.y,
            dot(texel.zw, up) / slope,
        )
        * smoothstep(SCENE_GROUND_SLOPE.x, SCENE_GROUND_SLOPE.y, slope)
        * (1.0 - smoothstep(SCENE_GROUND_FAR.x, SCENE_GROUND_FAR.y, texel.y));
}

// A coarse surface texel, where reflection rays and the walk toward the viewer read.
fn scene_ray_texel(point: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(
        scene_depth_tex,
        scene_map_sampler,
        point / Common::scene_image_size(),
        SCENE_GROUND_RAY_LEVEL,
    );
}

// How much a surface of some slope along up can continue ground of slope `reference`.
fn scene_slope_share(slope: f32, reference: f32) -> f32 {
    let limit = max(reference, 1e-3);
    return smoothstep(SCENE_GROUND_SLOPE_SHARE.x * limit, SCENE_GROUND_SLOPE_SHARE.y * limit, slope);
}

// Whether there is ground beside a point `below` under the horizon. Past the frame
// does not count, so a strip of wall along the frame's edge is not ground.
fn scene_ground_beside(point: vec2<f32>, below: f32, up: vec2<f32>) -> f32 {
    let image_size = Common::scene_image_size();
    let across = vec2<f32>(-up.y, up.x) * SCENE_GROUND_BESIDE_REACH * below;
    var beside = 0.0;
    for (var side = -1.0; side <= 1.0; side = side + 2.0) {
        let at = point + across * side;
        if any(at < vec2<f32>(0.0)) || any(at > image_size) { continue; }
        beside = max(beside, scene_texel_ground(scene_ray_texel(at), up));
    }
    return smoothstep(SCENE_GROUND_BESIDE_RANGE.x, SCENE_GROUND_BESIDE_RANGE.y, beside);
}

fn scene_ray_ground(point: vec2<f32>, below: f32, up: vec2<f32>, start_slope: f32) -> f32 {
    let texel = scene_ray_texel(point);
    return scene_texel_ground(texel, up)
        * scene_slope_share(dot(texel.zw, up), start_slope)
        * scene_ground_beside(point, below, up);
}

// How much of the ground runs on from a point toward the viewer, at a slope like
// the point's own. Past the frame counts as ground.
fn scene_ground_support(point: vec2<f32>, up: vec2<f32>, slope: f32) -> f32 {
    let image_size = Common::scene_image_size();
    var sum = 0.0;
    var steepest = 0.0;
    for (var step = 1u; step <= SCENE_SUPPORT_STEPS; step = step + 1u) {
        let travel = SCENE_SUPPORT_REACH * f32(step) / f32(SCENE_SUPPORT_STEPS);
        let at = point - up * travel;
        if any(at < vec2<f32>(0.0)) || any(at > image_size) {
            sum += 1.0;
            continue;
        }
        let texel = scene_ray_texel(at);
        let ground = scene_texel_ground(texel, up);
        sum += ground;
        steepest = max(steepest, dot(texel.zw, up) * ground);
    }
    return smoothstep(
            SCENE_SUPPORT_RANGE.x,
            SCENE_SUPPORT_RANGE.y,
            sum / f32(SCENE_SUPPORT_STEPS),
        )
        * scene_slope_share(slope, steepest);
}

// Distance along the ground from a point `below` under the horizon to what
// stands on it, or to the horizon.
fn scene_contact_distance(point: vec2<f32>, below: f32, up: vec2<f32>) -> f32 {
    let start_slope = dot(scene_ray_texel(point).zw, up);
    var near = 0.0;
    var far = below;
    for (var step = 1u; step <= SCENE_CONTACT_STEPS; step = step + 1u) {
        // Denser near the point, where a contact moves its reflection most.
        let fraction = f32(step) / f32(SCENE_CONTACT_STEPS);
        let travel = below * fraction * fraction;
        if scene_ray_ground(point + up * travel, below - travel, up, start_slope) < SCENE_CONTACT_SCORE
            && scene_ray_ground(point + up * (travel + SCENE_THIN_OBJECT), below - travel - SCENE_THIN_OBJECT, up, start_slope) < SCENE_CONTACT_SCORE {
            far = travel;
            break;
        }
        near = travel;
    }
    if far >= below { return below; }
    for (var step = 0u; step < SCENE_CONTACT_REFINEMENTS; step = step + 1u) {
        let middle = 0.5 * (near + far);
        if scene_ray_ground(point + up * middle, below - middle, up, start_slope) < SCENE_CONTACT_SCORE {
            far = middle;
        } else {
            near = middle;
        }
    }
    return 0.5 * (near + far);
}

// How far a ground cell rises above the ground around it, in camera heights, plus
// what its departure from level adds. A point on flat ground `below` under the
// horizon moves up the image by `below` times a height in camera heights (Delta b
// = f h / z, b = f H / z), and depth converts that image distance through the broad
// slope. Each point of the ring predicts the cell's depth along that slope, so a
// ring cut short by a wall or the frame leaves no bias.
fn scene_terrain_height(point: vec2<f32>, below: f32, texel: vec4<f32>, up: vec2<f32>) -> f32 {
    let image_size = Common::scene_image_size();
    let broad = textureSampleLevel(
        scene_depth_tex,
        scene_map_sampler,
        point / image_size,
        SCENE_TERRAIN_BROAD_LEVEL,
    ).zw;
    let slope = max(dot(broad, up), SCENE_TERRAIN_MIN_SLOPE);
    var predicted = 0.0;
    var support = 0.0;
    var departure = length(texel.zw - broad);
    var nearby = departure;
    for (var index = 0u; index < SCENE_TERRAIN_RING_POINTS; index = index + 1u) {
        let angle = 6.2831853 * f32(index) / f32(SCENE_TERRAIN_RING_POINTS);
        let offset = vec2<f32>(cos(angle), sin(angle)) * SCENE_TERRAIN_RING;
        let at = point + offset;
        if any(at < vec2<f32>(0.0)) || any(at > image_size) { continue; }
        let around = scene_ray_texel(at);
        let weight = scene_texel_ground(around, up);
        predicted += (around.y - dot(broad, offset)) * weight;
        support += weight;
        nearby = max(nearby, length(around.zw - broad));
    }
    var relief = 0.0;
    if support >= SCENE_TERRAIN_MIN_SUPPORT * f32(SCENE_TERRAIN_RING_POINTS) {
        relief = (predicted / support - texel.y) / slope / max(below, SCENE_HORIZON_FADE);
    }
    let grid = vec2<f32>(Common::scene_map_grid());
    let cell_size = image_size / grid;
    var edge = 0.0;
    for (var step = 0; step <= SCENE_TERRAIN_EDGE_REACH; step = step + 1) {
        let at = (point - up * f32(step) * cell_size) / cell_size;
        let cell = clamp(vec2<i32>(at), vec2<i32>(0), vec2<i32>(grid) - vec2<i32>(1));
        edge = max(edge, textureLoad(scene_environment_tex, cell, 0).w);
    }
    return clamp(relief, SCENE_TERRAIN_RELIEF_LIMIT.x, SCENE_TERRAIN_RELIEF_LIMIT.y)
        + smoothstep(SCENE_TERRAIN_DEPARTURE.x, SCENE_TERRAIN_DEPARTURE.y, departure / slope)
        * SCENE_TERRAIN_DEPARTURE_RISE
        + edge * smoothstep(SCENE_TERRAIN_DEPARTURE.x, SCENE_TERRAIN_DEPARTURE.y, nearby / slope)
        * SCENE_TERRAIN_EDGE_RISE;
}

@compute @workgroup_size(8, 8, 1)
fn build_scene_ground(@builtin(global_invocation_id) gid: vec3<u32>) {
    let grid = Common::scene_map_grid();
    if gid.x >= grid.x || gid.y >= grid.y { return; }
    let cell = vec2<i32>(gid.xy);
    let ground = Common::effects_uniforms.scene_ground;
    let uv = (vec2<f32>(gid.xy) + vec2<f32>(0.5)) / vec2<f32>(grid);
    let texel = textureSampleLevel(scene_depth_tex, scene_map_sampler, uv, SCENE_GROUND_LEVEL);
    let point = uv * Common::scene_image_size();
    let below = ground.z - dot(point, ground.xy);
    if ground.w < 0.5 || below <= 0.0 {
        textureStore(scene_ground_out, cell, vec4<f32>(0.0, 0.0, texel.y, 1.0));
        return;
    }
    let score = scene_texel_ground(texel, ground.xy)
        * smoothstep(0.0, SCENE_HORIZON_FADE, below)
        * scene_ground_support(point, ground.xy, dot(texel.zw, ground.xy))
        * scene_ground_beside(point, below, ground.xy);
    let across = vec2<f32>(-ground.y, ground.x) * SCENE_CONTACT_RAY_OFFSET
        * Common::scene_image_size().x / f32(grid.x);
    let contact = (scene_contact_distance(point - across, below, ground.xy)
        + scene_contact_distance(point, below, ground.xy)
        + scene_contact_distance(point + across, below, ground.xy)) / 3.0;
    let terrain = scene_terrain_height(point, below, texel, ground.xy);
    textureStore(scene_ground_out, cell, vec4<f32>(score, contact, texel.y, terrain));
}

// The lens blur at each depth: the mean of the logarithm of the blur of the edges
// there, by contrast. Depths without edges take the nearest that has them.
@compute @workgroup_size(1, 1, 1)
fn build_scene_focus() {
    let grid = vec2<i32>(Common::scene_map_grid());
    let bins = i32(Common::SCENE_FOCUS_BINS);
    var sums: array<f32, Common::SCENE_FOCUS_BINS>;
    var weights: array<f32, Common::SCENE_FOCUS_BINS>;
    for (var y = 0; y < grid.y; y = y + 1) {
        for (var x = 0; x < grid.x; x = x + 1) {
            let data = scene_map_cells[scene_map_cell_index(vec2<i32>(x, y))];
            if data.edge.y <= 0.0 || data.color.w <= 0.0 { continue; }
            let bin = clamp(i32(data.guide.z / data.color.w * f32(bins)), 0, bins - 1);
            sums[bin] += log2(data.edge.x) * data.edge.y;
            weights[bin] += data.edge.y;
        }
    }
    var logs: array<f32, Common::SCENE_FOCUS_BINS>;
    var measured = false;
    for (var bin = 0; bin < bins; bin = bin + 1) {
        for (var distance = 0; distance < bins; distance = distance + 1) {
            let below = max(bin - distance, 0);
            let above = min(bin + distance, bins - 1);
            let nearest = select(above, below, weights[below] > 0.0);
            if weights[nearest] > 0.0 {
                logs[bin] = sums[nearest] / weights[nearest];
                measured = true;
                break;
            }
        }
    }
    let short_edge = f32(max(min(Common::camera_uniforms.full_width, Common::camera_uniforms.full_height), 1u));
    for (var bin = 0; bin < bins; bin = bin + 1) {
        var sum = 0.0;
        var count = 0.0;
        for (var other = max(bin - SCENE_FOCUS_SMOOTHING, 0); other <= min(bin + SCENE_FOCUS_SMOOTHING, bins - 1); other = other + 1) {
            sum += logs[other];
            count += 1.0;
        }
        let blur = select(0.0, exp2(sum / count) / short_edge, measured);
        textureStore(scene_focus_out, vec2<i32>(bin, 0), vec4<f32>(blur, 0.0, 0.0, 1.0));
    }
}
