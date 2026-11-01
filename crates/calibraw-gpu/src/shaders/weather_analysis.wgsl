#import calibraw::common as Common
#import calibraw::color as Color
#import calibraw::scene_source as SceneSource

// Builds the weather map that Rain and Wet Surfaces read (weather.wgsl), on a
// coarse grid over the full image (Common::weather_grid), with the tone
// statistics like the image-light and haze grids:
//   accumulate_weather           per cell, the scene's colour and the sums of
//                                the guide and scene depth. Export tiles add
//                                only their cores, so every tile later reads
//                                the whole image's map.
//   resolve_weather              the environment map: each cell's mean colour
//                                in unexposed scene-linear Rec.2020. Also a
//                                guided filter's coefficients in one window, and
//                                how rough the photo's surface is there.
//   smooth_weather_coefficients  those averaged over the windows covering each
//                                cell.
//   blur_weather_environment     a wide blur of the environment map, what a
//                                drop refracting about 165 degrees of its
//                                surroundings sees.
//   build_weather_ground         per cell, how much it is ground and how far it
//                                lies below what stands on it.
//
// The guided filter (He, Sun and Tang, "Guided Image Filtering", 2010) is what
// keeps these effects' depth edges clean. Stored depth has blocky, misplaced
// edges at full size. Pixels instead evaluate the map's coefficients against
// the guide value of the photo itself (He and Sun, "Fast Guided Filter", 2015),
// a linear function that has no per-pixel decision to make: edges land where
// the photo's edges are, without staircases or a fringe of the texture beside
// them. The map depends only on the scene, scene depth and the direction of up,
// not on the rest of the edit.
//
// The ground is found in scene depth: surfaces whose depth rises toward the
// horizon, which walls (a sideways gradient) and surfaces facing the camera
// (hardly any) do not have. How fast it rises depends on the photo, as the depth
// model's output is not linear in distance or its inverse, so only its direction
// and that it rises at all count (ground_plane.rs finds the horizon by the same
// rule). Ground also runs on toward the viewer, which a patch of a wall or a
// pole's top with the same slope does not. Each cell's contact distance averages
// a few neighbouring rays, so a reflection changes gradually beside a pole rather
// than along a seam.

struct WeatherCell {
    // Summed colour (xyz) and pixels counted (w).
    color: vec4<f32>,
    // Summed guide g, g * g, scene depth d and g * d.
    guide: vec4<f32>,
}

@group(0) @binding(35) var scene_depth_tex: texture_2d<f32>;
@group(0) @binding(62) var<storage, read_write> weather_cells: array<WeatherCell>;
@group(0) @binding(63) var weather_environment_out: texture_storage_2d<rgba16float, write>;
// Per cell: the guided filter's slope (x) and offset (y) in one window, and the
// surface's roughness (z, weather_cell_roughness).
@group(0) @binding(64) var weather_coefficients_out: texture_storage_2d<rgba32float, write>;
@group(0) @binding(65) var weather_coefficients_tex: texture_2d<f32>;
// The window-averaged slope, offset and roughness that weather.wgsl reads.
@group(0) @binding(66) var weather_depth_map_out: texture_storage_2d<rgba32float, write>;
@group(0) @binding(67) var weather_environment_tex: texture_2d<f32>;
@group(0) @binding(68) var weather_environment_blur_out: texture_storage_2d<rgba16float, write>;
@group(0) @binding(28) var weather_sampler: sampler;
// Per cell: how much it is ground (x), the distance from it along the ground to
// what stands on it (y) and its smooth scene depth (z).
@group(0) @binding(72) var weather_ground_out: texture_storage_2d<rgba16float, write>;

// Radius in cells of the guided filter's windows: 3 x 3 cells, about 1.2% of
// the longer edge. It must be wider than the stored depth's staircases.
const WEATHER_GUIDE_RADIUS: i32 = 1;
// The filter's regularization relative to the window's squared mean guide.
// Where the guide varies by less than about its square root across a window,
// depth is smoothed instead of following the guide, so surface texture does
// not print into depth.
const WEATHER_GUIDE_EPSILON: f32 = 1e-3;
// Largest change of depth across the window's mean guide value that the slope
// may make. A full depth step between two guide values of nearly the same
// brightness would otherwise amplify their noise.
const WEATHER_GUIDE_MAX_RELATIVE_SLOPE: f32 = 3.0;
// The environment blur's standard deviation as a fraction of the longer edge,
// and its stride in environment cells.
const WEATHER_BLUR_SIGMA: f32 = 0.08;
const WEATHER_BLUR_STRIDE: i32 = 4;

// The surface mip level ground cells read, 256 x 256 texels, and the coarser one
// reflection rays and the walk toward the viewer read: a thin rail does not end a
// reflection, and interpolating there keeps contact distances continuous.
const WEATHER_GROUND_LEVEL: f32 = 2.0;
const WEATHER_GROUND_RAY_LEVEL: f32 = 3.0;
// Depth per shorter edge over which a surface's slope comes to count as ground,
// and the cosines of the angle from up over which its direction does.
// ground_plane.rs tests the middle of each.
const WEATHER_GROUND_SLOPE: vec2<f32> = vec2<f32>(0.06, 0.18);
const WEATHER_GROUND_ALIGNMENT: vec2<f32> = vec2<f32>(0.82, 0.96);
// Depths beyond which nothing is ground: the sky and far scenery.
const WEATHER_GROUND_FAR: vec2<f32> = vec2<f32>(0.9, 0.97);
// Below the horizon in shorter edges over which ground fades in.
const WEATHER_HORIZON_FADE: f32 = 0.03;
// Points checked toward the viewer for ground running on from a cell, how far
// they reach in shorter edges, and their mean score from no to full support.
const WEATHER_SUPPORT_STEPS: u32 = 5u;
const WEATHER_SUPPORT_REACH: f32 = 0.22;
const WEATHER_SUPPORT_RANGE: vec2<f32> = vec2<f32>(0.3, 0.65);
// A ray's score below this ends the ground, unless the ground goes on within
// WEATHER_THIN_OBJECT shorter edges: a chain or rail reflects as nothing visible
// and would only break the reflection of what lies behind it.
const WEATHER_CONTACT_SCORE: f32 = 0.35;
const WEATHER_THIN_OBJECT: f32 = 0.05;
const WEATHER_CONTACT_STEPS: u32 = 14u;
// Halvings of the step that found the contact. Long steps would otherwise show
// as bands in the reflection, where the contact jumps from one step to the next.
const WEATHER_CONTACT_REFINEMENTS: u32 = 5u;
// Offsets of the averaged rays across the view, in cells.
const WEATHER_CONTACT_RAY_OFFSET: f32 = 0.75;

fn weather_cell_index(cell: vec2<i32>) -> u32 {
    return u32(cell.y) * Common::WEATHER_GRID_LONG + u32(cell.x);
}

@compute @workgroup_size(8, 8, 1)
fn accumulate_weather(@builtin(global_invocation_id) gid: vec3<u32>) {
    let grid = Common::weather_grid();
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
    for (var y = lo.y; y < hi.y; y = y + 1) {
        for (var x = lo.x; x < hi.x; x = x + 1) {
            let pos = vec2<i32>(x, y) - origin;
            let rgb = max(Color::map_negative_gamut(SceneSource::profiled_scene_at(pos)), vec3<f32>(0.0));
            color += vec4<f32>(rgb, 1.0);
            // The nearest depth texel: window means smooth it far more than
            // interpolating would.
            let uv = (vec2<f32>(f32(x), f32(y)) + vec2<f32>(0.5)) / full;
            let depth = textureLoad(scene_depth_tex, vec2<i32>(uv * depth_size), 0).x;
            let g = SceneSource::weather_guide_at(pos);
            guide += vec4<f32>(g, g * g, depth, g * depth);
        }
    }
    // Tiles are dispatched one after another, so each cell has one writer.
    let index = weather_cell_index(vec2<i32>(gid.xy));
    weather_cells[index].color += color;
    weather_cells[index].guide += guide;
}

// How rough the photo's surface is in a cell: its guide's variance relative to
// its squared mean, high on gravel and grass and low on asphalt or paving.
fn weather_cell_roughness(data: WeatherCell) -> f32 {
    let count = max(data.color.w, 1.0);
    let mean = data.guide.x / count;
    return max(data.guide.y / count - mean * mean, 0.0) / max(mean * mean, 1e-6);
}

@compute @workgroup_size(8, 8, 1)
fn resolve_weather(@builtin(global_invocation_id) gid: vec3<u32>) {
    let grid = vec2<i32>(Common::weather_grid());
    let cell = vec2<i32>(gid.xy);
    if any(cell >= grid) { return; }
    let own = weather_cells[weather_cell_index(cell)].color;
    textureStore(
        weather_environment_out,
        cell,
        vec4<f32>(own.xyz / max(own.w, 1.0), 1.0),
    );

    var sums = vec4<f32>(0.0);
    var count = 0.0;
    // The smoothest cell nearby: a cell an edge crosses is no rougher than the
    // surfaces beside it, so an object standing on smooth ground leaves no rough
    // ring around it.
    var roughness = 1e6;
    for (var dy = -WEATHER_GUIDE_RADIUS; dy <= WEATHER_GUIDE_RADIUS; dy = dy + 1) {
        for (var dx = -WEATHER_GUIDE_RADIUS; dx <= WEATHER_GUIDE_RADIUS; dx = dx + 1) {
            let neighbour = cell + vec2<i32>(dx, dy);
            if any(neighbour < vec2<i32>(0)) || any(neighbour >= grid) { continue; }
            let data = weather_cells[weather_cell_index(neighbour)];
            sums += data.guide;
            count += data.color.w;
            if data.color.w > 0.0 {
                roughness = min(roughness, weather_cell_roughness(data));
            }
        }
    }
    var coefficients = vec2<f32>(0.0);
    if count > 0.0 {
        let means = sums / count;
        let squared_mean = max(means.x * means.x, 1e-6);
        let variance = max(means.y - means.x * means.x, 0.0);
        let covariance = means.w - means.x * means.z;
        let limit = WEATHER_GUIDE_MAX_RELATIVE_SLOPE / max(means.x, 1e-3);
        let slope = clamp(
            covariance / (variance + WEATHER_GUIDE_EPSILON * squared_mean),
            -limit,
            limit,
        );
        // The offset keeps the window's mean depth at its mean guide.
        coefficients = vec2<f32>(slope, means.z - slope * means.x);
    }
    textureStore(weather_coefficients_out, cell, vec4<f32>(coefficients, roughness, 1.0));
}

@compute @workgroup_size(8, 8, 1)
fn smooth_weather_coefficients(@builtin(global_invocation_id) gid: vec3<u32>) {
    let grid = vec2<i32>(Common::weather_grid());
    let cell = vec2<i32>(gid.xy);
    if any(cell >= grid) { return; }
    var sum = vec3<f32>(0.0);
    var count = 0.0;
    for (var dy = -WEATHER_GUIDE_RADIUS; dy <= WEATHER_GUIDE_RADIUS; dy = dy + 1) {
        for (var dx = -WEATHER_GUIDE_RADIUS; dx <= WEATHER_GUIDE_RADIUS; dx = dx + 1) {
            let neighbour = cell + vec2<i32>(dx, dy);
            if any(neighbour < vec2<i32>(0)) || any(neighbour >= grid) { continue; }
            sum += textureLoad(weather_coefficients_tex, neighbour, 0).xyz;
            count += 1.0;
        }
    }
    textureStore(weather_depth_map_out, cell, vec4<f32>(sum / count, 1.0));
}

@compute @workgroup_size(8, 8, 1)
fn blur_weather_environment(@builtin(global_invocation_id) gid: vec3<u32>) {
    let blur_grid = Common::weather_blur_grid();
    if gid.x >= blur_grid.x || gid.y >= blur_grid.y { return; }
    let grid = vec2<i32>(Common::weather_grid());
    let scale = vec2<f32>(grid) / vec2<f32>(blur_grid);
    let center = (vec2<f32>(gid.xy) + vec2<f32>(0.5)) * scale;
    let sigma = WEATHER_BLUR_SIGMA * f32(Common::WEATHER_GRID_LONG);
    let reach = i32(ceil(3.0 * sigma)) / WEATHER_BLUR_STRIDE;
    var sum = vec3<f32>(0.0);
    var weights = 0.0;
    for (var dy = -reach; dy <= reach; dy = dy + 1) {
        for (var dx = -reach; dx <= reach; dx = dx + 1) {
            let offset = vec2<f32>(f32(dx), f32(dy)) * f32(WEATHER_BLUR_STRIDE);
            let cell = vec2<i32>(floor(center + offset));
            if any(cell < vec2<i32>(0)) || any(cell >= grid) { continue; }
            let weight = exp(-0.5 * dot(offset, offset) / (sigma * sigma));
            sum += textureLoad(weather_environment_tex, cell, 0).xyz * weight;
            weights += weight;
        }
    }
    textureStore(
        weather_environment_blur_out,
        vec2<i32>(gid.xy),
        vec4<f32>(sum / max(weights, 1e-6), 1.0),
    );
}

// How much a surface texel (smooth depth in y, gradient in zw) is ground.
fn weather_texel_ground(texel: vec4<f32>, up: vec2<f32>) -> f32 {
    let slope = length(texel.zw);
    if slope <= 1e-5 { return 0.0; }
    return smoothstep(
            WEATHER_GROUND_ALIGNMENT.x,
            WEATHER_GROUND_ALIGNMENT.y,
            dot(texel.zw, up) / slope,
        )
        * smoothstep(WEATHER_GROUND_SLOPE.x, WEATHER_GROUND_SLOPE.y, slope)
        * (1.0 - smoothstep(WEATHER_GROUND_FAR.x, WEATHER_GROUND_FAR.y, texel.y));
}

fn weather_ray_ground(point: vec2<f32>, up: vec2<f32>) -> f32 {
    let texel = textureSampleLevel(
        scene_depth_tex,
        weather_sampler,
        point / Common::weather_image_size(),
        WEATHER_GROUND_RAY_LEVEL,
    );
    return weather_texel_ground(texel, up);
}

// How much of the ground runs on from a point toward the viewer. Past the frame
// counts as ground.
fn weather_ground_support(point: vec2<f32>, up: vec2<f32>) -> f32 {
    let image_size = Common::weather_image_size();
    var sum = 0.0;
    for (var step = 1u; step <= WEATHER_SUPPORT_STEPS; step = step + 1u) {
        let travel = WEATHER_SUPPORT_REACH * f32(step) / f32(WEATHER_SUPPORT_STEPS);
        let at = point - up * travel;
        if any(at < vec2<f32>(0.0)) || any(at > image_size) {
            sum += 1.0;
        } else {
            sum += weather_ray_ground(at, up);
        }
    }
    return smoothstep(
        WEATHER_SUPPORT_RANGE.x,
        WEATHER_SUPPORT_RANGE.y,
        sum / f32(WEATHER_SUPPORT_STEPS),
    );
}

// Distance along the ground from a point `below` under the horizon to what
// stands on it, or to the horizon.
fn weather_contact_distance(point: vec2<f32>, below: f32, up: vec2<f32>) -> f32 {
    var near = 0.0;
    var far = below;
    for (var step = 1u; step <= WEATHER_CONTACT_STEPS; step = step + 1u) {
        // Denser near the point, where a contact moves its reflection most.
        let fraction = f32(step) / f32(WEATHER_CONTACT_STEPS);
        let travel = below * fraction * fraction;
        if weather_ray_ground(point + up * travel, up) < WEATHER_CONTACT_SCORE
            && weather_ray_ground(point + up * (travel + WEATHER_THIN_OBJECT), up) < WEATHER_CONTACT_SCORE {
            far = travel;
            break;
        }
        near = travel;
    }
    if far >= below { return below; }
    for (var step = 0u; step < WEATHER_CONTACT_REFINEMENTS; step = step + 1u) {
        let middle = 0.5 * (near + far);
        if weather_ray_ground(point + up * middle, up) < WEATHER_CONTACT_SCORE {
            far = middle;
        } else {
            near = middle;
        }
    }
    return 0.5 * (near + far);
}

@compute @workgroup_size(8, 8, 1)
fn build_weather_ground(@builtin(global_invocation_id) gid: vec3<u32>) {
    let grid = Common::weather_grid();
    if gid.x >= grid.x || gid.y >= grid.y { return; }
    let cell = vec2<i32>(gid.xy);
    let ground = Common::effects_uniforms.weather_ground;
    let uv = (vec2<f32>(gid.xy) + vec2<f32>(0.5)) / vec2<f32>(grid);
    let texel = textureSampleLevel(scene_depth_tex, weather_sampler, uv, WEATHER_GROUND_LEVEL);
    let point = uv * Common::weather_image_size();
    let below = ground.z - dot(point, ground.xy);
    if ground.w < 0.5 || below <= 0.0 {
        textureStore(weather_ground_out, cell, vec4<f32>(0.0, 0.0, texel.y, 1.0));
        return;
    }
    let score = weather_texel_ground(texel, ground.xy)
        * smoothstep(0.0, WEATHER_HORIZON_FADE, below)
        * weather_ground_support(point, ground.xy);
    let across = vec2<f32>(-ground.y, ground.x) * WEATHER_CONTACT_RAY_OFFSET
        * Common::weather_image_size().x / f32(grid.x);
    let contact = (weather_contact_distance(point - across, below, ground.xy)
        + weather_contact_distance(point, below, ground.xy)
        + weather_contact_distance(point + across, below, ground.xy)) / 3.0;
    textureStore(weather_ground_out, cell, vec4<f32>(score, contact, texel.y, 1.0));
}
