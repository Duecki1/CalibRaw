#import calibraw::scene::bake_common as Bake

// B3: how much of a pixel the near surface covers (bake_common.wgsl, COVERAGE).
//
// Edges first. A band texel lies at a depth edge when the depths around it
// fall into two groups: two-means on log frame depth over SCENE_BAND_RADIUS
// splits them, and the split counts when the groups lie far enough apart
// (SCENE_BAND_RELIEF) and explain most of the depths' variance
// (SCENE_EDGE_BIMODALITY). A surface that merely recedes, such as the ground
// behind a subject, spreads its depths evenly and does not pass, however deep
// it runs: it stays one surface. A texel lies at the sky's outline when the
// sky probability around it crosses 1/2.
//
// At an edge, coverage is a colour guided filter (He, Sun and Tang, "Guided
// Image Filtering", 2010, §3.5) of the near surface's mask against the
// working image's guide: within a window around the texel, the mask is
// modelled as mask ≈ a · guide + b. Pixels evaluate that with their own
// full-resolution guide (He and Sun, "Fast Guided Filter", 2015), so an edge
// lands on the photo's own edge with sub-pixel precision, and where the photo
// shows no edge the regularization leaves a smooth ramp across the depth
// model's. The mask is 1 nearer than the split, and for the sky's outline the
// near surface's share of the photo's colour (below); with both, the smaller.
// The coefficients are
// not averaged over overlapping windows: the pixels' bilinear reading of them
// smooths them instead. The window follows the lens blur at the near
// surface's depth (B7), so a defocused edge feathers over its real width and
// a sharp one stays sharp; wide windows are sampled on a sparser lattice.
//
// Elsewhere coverage is constant: 0 or 1, the side of the nearest edge's split
// the texel lies on (1 away from edges). So coverage read between an edge
// texel and its neighbour never blends in the other surface, and the layers
// (bake_layers.wgsl) treat only edge texels as holding two surfaces.
//
// The sky's outline. A defocused tree line spreads over far more than the
// depth model's own outline, and the model draws that outline in shapes the
// photo does not show, so its sky probability cannot say how much of a pixel
// is sky. The photo can: across the outline each pixel mixes the near
// surface's colour F with the sky's B, so the near surface covers
//     α = (I − B) · (F − B) / |F − B|²
// of a pixel of colour I (the colour line of two-colour matting; Ruzon and
// Tomasi, "Alpha Estimation in Natural Images", 2000). F and B are read along
// the line through the texel across the sky's outline, just beyond the photo's
// own ramp there: from the steepest step of the photo's colour near the
// model's outline, outward on each side to where the step has flattened to
// SCENE_SKY_RAMP_END of its steepest. α follows the photo's ramp at any width
// and does not depend on the lens blur's fit. Beyond the ramp's ends the
// photo shows one surface: coverage is 1 on the near side and 0 on the sky's,
// whatever the colour (a pale patch of foliage, a dark cloud). Each texel
// reads its own line (bake_scene_sky_colours), and where the photo's ramp is
// faint, neighbouring lines disagree; so the coverage pass averages the
// texels' F, B and sides of the ramp over SCENE_SKY_SMOOTHING texels before
// using them. Where F and B are too alike to
// tell apart (SCENE_SKY_MIN_CONTRAST), or no outline is found, the outline is
// an edge only where the sky probability crosses 1/2 within
// SCENE_BAND_RADIUS, and the mask is 1 minus the sky probability, as at a
// depth edge. The far surface of an outline is the sky (bake_layers.wgsl).
//
// The first pass leaves each texel's split and kind of edge in the scratch
// buffer for the second, which sets the constant texels and writes each
// texel's kind of edge to the FAR layer for the layers.

// Window radius in working texels, from twice the lens blur's deviation.
const SCENE_COVERAGE_LEAST_RADIUS: i32 = 2;
const SCENE_COVERAGE_MOST_RADIUS: i32 = 12;
// Regularization: guide differences below about this fraction of the
// window's mean guide count as none, so texture does not cut a surface. Being
// relative, it also bounds |a · guide| to about 1 / (4 · this), which keeps
// the half-float coefficients within 0.01 of coverage.
const SCENE_COVERAGE_EPSILON: f32 = 0.03;
const SCENE_COVERAGE_EPSILON_FLOOR: f32 = 1e-5;
// The share of the depths' variance two groups must explain to be an edge: a
// uniform spread of depth gives 3/4, a clean step 1.
const SCENE_EDGE_BIMODALITY: f32 = 0.85;
// Iterations of two-means.
const SCENE_EDGE_ITERATIONS: i32 = 4;

// Kinds of edge (Bake::SCENE_EDGE_*) are kept in the scratch buffer's y.
const SCENE_EDGE_DEPTH: u32 = Bake::SCENE_EDGE_DEPTH;
const SCENE_EDGE_SKY: u32 = Bake::SCENE_EDGE_SKY;

// The sky's outline. Half the length of the line read across it, in working
// texels, from the model's outline.
const SCENE_SKY_PROFILE: i32 = 40;
// How far from the model's outline the photo's steepest step is looked for.
const SCENE_SKY_SEARCH: i32 = 16;
// The photo's ramp ends where its step has flattened to this share of the
// steepest; a Gaussian blur's has at 1.95 deviations, 97 % of the way.
const SCENE_SKY_RAMP_END: f32 = 0.15;
// Texels averaged beyond each end of the ramp for F and B.
const SCENE_SKY_SAMPLES: i32 = 3;
// F and B count as different when their guides differ by at least this share
// of the brighter one's.
const SCENE_SKY_MIN_CONTRAST: f32 = 0.08;
// How far along the line the model's outline is looked for: the band's reach
// along a diagonal, SCENE_SKY_BAND_RADIUS · √2.
const SCENE_SKY_OUTLINE_SEARCH: i32 = 46;
// The least change of sky probability per texel, at the scale it is measured,
// that gives the outline's direction.
const SCENE_SKY_MIN_SLOPE: f32 = 0.15;

fn scene_log_depth(d: f32) -> f32 {
    return log(Bake::scene_bake_frame_depth(d));
}

// The near surface's colour F and the sky's B at a sky outline, in the
// working image's colours, if the photo tells them apart: one texel's line
// across the outline.
struct SceneSkyColours {
    near: vec3<f32>,
    sky: vec3<f32>,
    // The line across the outline: a point on the model's outline (texels)
    // and the unit direction toward the sky; the photo's ramp spans
    // `ramp.x` to `ramp.y` texels along it from that point.
    origin: vec2<f32>,
    direction: vec2<f32>,
    ramp: vec2<f32>,
    found: bool,
}

const SCENE_NO_SKY_COLOURS: SceneSkyColours = SceneSkyColours(
    vec3<f32>(0.0), vec3<f32>(0.0), vec2<f32>(0.0), vec2<f32>(0.0), vec2<f32>(0.0), false,
);

// Working texel position (texel units, texel centres at integers) as uv.
fn scene_position_uv(position: vec2<f32>) -> vec2<f32> {
    return (position + vec2<f32>(0.5)) / vec2<f32>(Bake::scene_bake_size());
}

fn scene_sky_at(position: vec2<f32>) -> f32 {
    return textureSampleLevel(Bake::scene_geometry_tex, Bake::scene_bake_sampler, scene_position_uv(position), 0.0).y;
}

fn scene_rgb_at(position: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(Bake::scene_working_tex, Bake::scene_bake_sampler, scene_position_uv(position), 0.0).rgb;
}

// The direction toward the sky across its outline at `texel`, from the sky
// probability's slope at the finest scale that shows one, or zero.
fn scene_sky_direction(texel: vec2<i32>) -> vec2<f32> {
    let centre = vec2<f32>(texel);
    let scales = array<f32, 3>(6.0, 16.0, 32.0);
    for (var k = 0; k < 3; k = k + 1) {
        let s = scales[k];
        let slope = vec2<f32>(
            scene_sky_at(centre + vec2<f32>(s, 0.0)) - scene_sky_at(centre - vec2<f32>(s, 0.0)),
            scene_sky_at(centre + vec2<f32>(0.0, s)) - scene_sky_at(centre - vec2<f32>(0.0, s)),
        );
        if length(slope) > SCENE_SKY_MIN_SLOPE {
            return normalize(slope);
        }
    }
    return vec2<f32>(0.0);
}

// F and B for a texel at the sky's outline (file comment).
fn scene_sky_colours(texel: vec2<i32>) -> SceneSkyColours {
    let direction = scene_sky_direction(texel);
    if all(direction == vec2<f32>(0.0)) {
        return SCENE_NO_SKY_COLOURS;
    }
    // The model's outline on the line: the crossing of 1/2 nearest the texel,
    // as far as the band reaches along a diagonal.
    let centre = vec2<f32>(texel);
    var outline = 1e9;
    let reach = SCENE_SKY_OUTLINE_SEARCH;
    var below = scene_sky_at(centre - f32(reach) * direction);
    for (var t = -reach; t < reach; t = t + 1) {
        let above = scene_sky_at(centre + f32(t + 1) * direction);
        if below < 0.5 && above >= 0.5 && abs(f32(t) + 0.5) < abs(outline) {
            outline = f32(t) + (0.5 - below) / max(above - below, 1e-6);
        }
        below = above;
    }
    if outline > 1e8 {
        return SCENE_NO_SKY_COLOURS;
    }
    let origin = centre + outline * direction;

    // The photo's step from the line's ends: its direction in guide space.
    var ground_mean = vec3<f32>(0.0);
    var sky_mean = vec3<f32>(0.0);
    let half = SCENE_SKY_PROFILE / 2;
    for (var u = half; u <= SCENE_SKY_PROFILE; u = u + 1) {
        ground_mean += Bake::scene_bake_guide(scene_rgb_at(origin - f32(u) * direction));
        sky_mean += Bake::scene_bake_guide(scene_rgb_at(origin + f32(u) * direction));
    }
    let step = sky_mean - ground_mean;
    if length(step) <= 1e-6 {
        return SCENE_NO_SKY_COLOURS;
    }
    let across = normalize(step);
    // The photo along the line, projected onto that direction.
    var profile: array<f32, 81>;
    for (var u = -SCENE_SKY_PROFILE; u <= SCENE_SKY_PROFILE; u = u + 1) {
        profile[u + SCENE_SKY_PROFILE] = dot(Bake::scene_bake_guide(scene_rgb_at(origin + f32(u) * direction)), across);
    }
    // Its steepest step near the model's outline.
    var peak = 0;
    var steepest = 0.0;
    for (var u = -SCENE_SKY_SEARCH; u <= SCENE_SKY_SEARCH; u = u + 1) {
        let slope = scene_profile_slope(&profile, u);
        if slope > steepest {
            steepest = slope;
            peak = u;
        }
    }
    if steepest <= 0.0 {
        return SCENE_NO_SKY_COLOURS;
    }
    // Out to where the step has flattened, on each side.
    let last = SCENE_SKY_PROFILE - 1 - SCENE_SKY_SAMPLES;
    var sky_end = peak;
    while sky_end < last && scene_profile_slope(&profile, sky_end + 1) > SCENE_SKY_RAMP_END * steepest {
        sky_end = sky_end + 1;
    }
    var ground_end = peak;
    while ground_end > -last && scene_profile_slope(&profile, ground_end - 1) > SCENE_SKY_RAMP_END * steepest {
        ground_end = ground_end - 1;
    }
    var near = vec3<f32>(0.0);
    var sky = vec3<f32>(0.0);
    for (var k = 1; k <= SCENE_SKY_SAMPLES; k = k + 1) {
        near += scene_rgb_at(origin + f32(ground_end - k) * direction);
        sky += scene_rgb_at(origin + f32(sky_end + k) * direction);
    }
    near /= f32(SCENE_SKY_SAMPLES);
    sky /= f32(SCENE_SKY_SAMPLES);
    let guides = array<vec3<f32>, 2>(Bake::scene_bake_guide(near), Bake::scene_bake_guide(sky));
    let contrast = length(guides[0] - guides[1]);
    if contrast < SCENE_SKY_MIN_CONTRAST * max(length(guides[0]), length(guides[1])) || contrast <= 1e-6 {
        return SCENE_NO_SKY_COLOURS;
    }
    return SceneSkyColours(near, sky, origin, direction, vec2<f32>(f32(ground_end), f32(sky_end)), true);
}

// Whether the sky probability crosses 1/2 within SCENE_BAND_RADIUS of
// `texel`, on a lattice of every other texel.
fn scene_sky_crosses_near(texel: vec2<i32>) -> bool {
    var low = 1.0;
    var high = 0.0;
    for (var dy = -Bake::SCENE_BAND_RADIUS; dy <= Bake::SCENE_BAND_RADIUS; dy = dy + 2) {
        for (var dx = -Bake::SCENE_BAND_RADIUS; dx <= Bake::SCENE_BAND_RADIUS; dx = dx + 2) {
            let sky = textureLoad(Bake::scene_geometry_tex, Bake::scene_bake_clamp(texel + vec2<i32>(dx, dy)), 0).y;
            low = min(low, sky);
            high = max(high, sky);
        }
    }
    return low < 0.5 && high > 0.5;
}

// The step of a profile at `u`: the difference of its neighbours.
fn scene_profile_slope(profile: ptr<function, array<f32, 81>>, u: i32) -> f32 {
    return (*profile)[u + 1 + SCENE_SKY_PROFILE] - (*profile)[u - 1 + SCENE_SKY_PROFILE];
}

// Where a texel lies on its line (NORMALS.w of bake_scene_sky_colours):
// beyond the ramp's near end, within the ramp, beyond its sky end.
const SCENE_SKY_SIDE_NEAR: f32 = 1.0;
const SCENE_SKY_SIDE_RAMP: f32 = 0.5;
const SCENE_SKY_SIDE_SKY: f32 = 0.0;

// Each texel's own line across the sky's outline, for the coverage pass to
// average: NEAR = (F, 1) and NORMALS = (B, side) where the line found them,
// else zero. These layers hold nothing else until the layers are made.
@compute @workgroup_size(8, 8, 1)
fn bake_scene_sky_colours(@builtin(global_invocation_id) gid: vec3<u32>) {
    let texel = vec2<i32>(gid.xy);
    if any(texel >= Bake::scene_bake_size()) { return; }
    let band_texel = textureLoad(Bake::scene_far_tex, texel, 0);
    let band = u32(band_texel.x + 0.5);
    var colours = SCENE_NO_SKY_COLOURS;
    if (band & Bake::SCENE_BAND_SKY) != 0u && band_texel.y < 0.5 && band_texel.z > 0.5 {
        colours = scene_sky_colours(texel);
    }
    if !colours.found {
        textureStore(Bake::scene_near_out, texel, vec4<f32>(0.0));
        textureStore(Bake::scene_normals_layer_out, texel, vec4<f32>(0.0));
        return;
    }
    let along = dot(vec2<f32>(texel) - colours.origin, colours.direction);
    var side = SCENE_SKY_SIDE_RAMP;
    if along <= colours.ramp.x {
        side = SCENE_SKY_SIDE_NEAR;
    } else if along >= colours.ramp.y {
        side = SCENE_SKY_SIDE_SKY;
    }
    textureStore(Bake::scene_near_out, texel, vec4<f32>(colours.near, 1.0));
    textureStore(Bake::scene_normals_layer_out, texel, vec4<f32>(colours.sky, side));
}

// The texels' lines around a sky outline texel, averaged: what its window's
// mask follows.
struct SceneSkyModel {
    near: vec3<f32>,
    sky: vec3<f32>,
    // Of the lines found: the shares on which the texel lies beyond the
    // ramp's near end (x) and within the ramp (y).
    shares: vec2<f32>,
    // The share of texels around whose line was found.
    found: f32,
    // The sky probability crosses 1/2 within SCENE_BAND_RADIUS: the outline
    // is an edge without the photo's colours too.
    crosses_near: bool,
    // The texel lies on the near side of the model's outline.
    near_side: bool,
}

// Radius in working texels over which the lines are averaged.
const SCENE_SKY_SMOOTHING: i32 = 6;

fn scene_sky_model(texel: vec2<i32>) -> SceneSkyModel {
    var weights = 0.0;
    var found = 0.0;
    var near = vec3<f32>(0.0);
    var sky = vec3<f32>(0.0);
    var shares = vec2<f32>(0.0);
    let spread = 1.0 / (0.5 * f32(SCENE_SKY_SMOOTHING * SCENE_SKY_SMOOTHING));
    for (var dy = -SCENE_SKY_SMOOTHING; dy <= SCENE_SKY_SMOOTHING; dy = dy + 1) {
        for (var dx = -SCENE_SKY_SMOOTHING; dx <= SCENE_SKY_SMOOTHING; dx = dx + 1) {
            let offset = vec2<f32>(f32(dx), f32(dy));
            let weight = exp(-dot(offset, offset) * spread);
            let tap = Bake::scene_bake_clamp(texel + vec2<i32>(dx, dy));
            let line_near = textureLoad(Bake::scene_near_tex, tap, 0);
            weights += weight;
            if line_near.w < 0.5 {
                continue;
            }
            let line_sky = textureLoad(Bake::scene_normals_layer_tex, tap, 0);
            found += weight;
            near += weight * line_near.rgb;
            sky += weight * line_sky.rgb;
            shares += weight * vec2<f32>(
                select(0.0, 1.0, line_sky.w > 0.75),
                select(0.0, 1.0, line_sky.w > 0.25 && line_sky.w < 0.75),
            );
        }
    }
    let own_sky = textureLoad(Bake::scene_geometry_tex, texel, 0).y;
    var model = SceneSkyModel(
        vec3<f32>(0.0), vec3<f32>(0.0), vec2<f32>(0.0), 0.0, scene_sky_crosses_near(texel), own_sky < 0.5,
    );
    if found <= 0.0 {
        return model;
    }
    model.near = near / found;
    model.sky = sky / found;
    model.shares = shares / found;
    model.found = found / weights;
    return model;
}

// The mask at a sky outline where the photo's colours do not tell the
// surfaces apart: 1 minus the sky probability within SCENE_BAND_RADIUS of the
// model's outline, beyond it the side of the outline the texel lies on.
fn scene_sky_fallback(model: SceneSkyModel, sky: f32) -> f32 {
    if model.crosses_near {
        return 1.0 - sky;
    }
    return select(0.0, 1.0, model.near_side);
}

// The near surface's share at a sky outline of a tap of colour `rgb` and sky
// probability `sky`.
fn scene_sky_mask(model: SceneSkyModel, rgb: vec3<f32>, sky: f32) -> f32 {
    var colour = 0.0;
    if model.found > 0.0 {
        let line = model.near - model.sky;
        let along_line = clamp(dot(rgb - model.sky, line) / max(dot(line, line), 1e-12), 0.0, 1.0);
        colour = model.shares.x + model.shares.y * along_line;
    }
    return mix(scene_sky_fallback(model, sky), colour, model.found);
}

// The near surface's mask at a tap with geometry `tap` (GEOMETRY layer) and
// colour `rgb`, for an edge of kind `kind` split at log depth `split`, with
// the sky outline's `model`.
fn scene_near_mask(tap: vec4<f32>, rgb: vec3<f32>, kind: u32, split: f32, model: SceneSkyModel) -> f32 {
    var mask = 1.0;
    if (kind & SCENE_EDGE_DEPTH) != 0u {
        mask = select(0.0, 1.0, scene_log_depth(tap.x) < split);
    }
    if (kind & SCENE_EDGE_SKY) != 0u {
        mask = min(mask, scene_sky_mask(model, rgb, tap.y));
    }
    return mask;
}

// A model for a texel whose mask does not depend on it.
const SCENE_NO_SKY_MODEL: SceneSkyModel = SceneSkyModel(
    vec3<f32>(0.0), vec3<f32>(0.0), vec2<f32>(0.0), 0.0, true, true,
);

fn scene_store_edge(texel: vec2<i32>, kind: u32, split: f32) {
    Bake::scene_bake_scratch[Bake::scene_bake_index(texel)] = vec4<u32>(bitcast<u32>(split), kind, 0u, 0u);
}

@compute @workgroup_size(8, 8, 1)
fn bake_scene_coverage(@builtin(global_invocation_id) gid: vec3<u32>) {
    let texel = vec2<i32>(gid.xy);
    if any(texel >= Bake::scene_bake_size()) { return; }
    let band_texel = textureLoad(Bake::scene_far_tex, texel, 0);
    let band = u32(band_texel.x + 0.5);
    if band == 0u {
        scene_store_edge(texel, 0u, 0.0);
        return;
    }

    var kind = 0u;
    var split = 0.0;
    var near_depth = 0.0;
    if (band & Bake::SCENE_BAND_DEPTH) != 0u {
        // The depth edge test, on a lattice of every other texel within the
        // band radius.
        var lowest = 1e9;
        var highest = -1e9;
        for (var dy = -Bake::SCENE_BAND_RADIUS; dy <= Bake::SCENE_BAND_RADIUS; dy = dy + 2) {
            for (var dx = -Bake::SCENE_BAND_RADIUS; dx <= Bake::SCENE_BAND_RADIUS; dx = dx + 2) {
                let tap = textureLoad(Bake::scene_geometry_tex, Bake::scene_bake_clamp(texel + vec2<i32>(dx, dy)), 0);
                let depth = scene_log_depth(tap.x);
                lowest = min(lowest, depth);
                highest = max(highest, depth);
            }
        }
        split = 0.5 * (lowest + highest);
        var groups = vec2<f32>(lowest, highest);
        var shares = vec2<f32>(0.0);
        var spread = 0.0;
        for (var iteration = 0; iteration < SCENE_EDGE_ITERATIONS; iteration = iteration + 1) {
            var sums = vec4<f32>(0.0); // near sum, near count, far sum, far count
            var square = 0.0;
            for (var dy = -Bake::SCENE_BAND_RADIUS; dy <= Bake::SCENE_BAND_RADIUS; dy = dy + 2) {
                for (var dx = -Bake::SCENE_BAND_RADIUS; dx <= Bake::SCENE_BAND_RADIUS; dx = dx + 2) {
                    let tap = textureLoad(Bake::scene_geometry_tex, Bake::scene_bake_clamp(texel + vec2<i32>(dx, dy)), 0);
                    let depth = scene_log_depth(tap.x);
                    square += depth * depth;
                    if depth < split {
                        sums += vec4<f32>(depth, 1.0, 0.0, 0.0);
                    } else {
                        sums += vec4<f32>(0.0, 0.0, depth, 1.0);
                    }
                }
            }
            let count = sums.y + sums.w;
            groups = vec2<f32>(
                select(lowest, sums.x / sums.y, sums.y > 0.0),
                select(highest, sums.z / sums.w, sums.w > 0.0),
            );
            shares = vec2<f32>(sums.y, sums.w) / count;
            let mean = (sums.x + sums.z) / count;
            spread = square / count - mean * mean;
            split = 0.5 * (groups.x + groups.y);
        }
        let gap = groups.y - groups.x;
        let between = shares.x * shares.y * gap * gap;
        if gap > log(1.0 + Bake::SCENE_BAND_RELIEF) && between > SCENE_EDGE_BIMODALITY * spread {
            kind |= SCENE_EDGE_DEPTH;
        }
        near_depth = groups.x;
    }
    // The sky's outline: sky probability within SCENE_SKY_BAND_RADIUS crosses
    // 1/2 (B2's extent).
    var model = SCENE_NO_SKY_MODEL;
    if (band & Bake::SCENE_BAND_SKY) != 0u && band_texel.y < 0.5 && band_texel.z > 0.5 {
        model = scene_sky_model(texel);
        if model.found > 0.0 || model.crosses_near {
            kind |= SCENE_EDGE_SKY;
        }
    }
    scene_store_edge(texel, kind, split);
    if kind == 0u {
        return;
    }

    // The window: the lens blur's at a depth edge; at a sky outline whose
    // mask follows the photo's colours already, the least.
    var radius = SCENE_COVERAGE_LEAST_RADIUS;
    if (kind & SCENE_EDGE_DEPTH) != 0u || model.found < 1.0 {
        var depth = near_depth;
        if (kind & SCENE_EDGE_DEPTH) == 0u {
            depth = scene_log_depth(textureLoad(Bake::scene_geometry_tex, texel, 0).x);
        }
        let blur = Bake::scene_focus_blur(exp(depth));
        radius = clamp(i32(round(2.0 * blur)), SCENE_COVERAGE_LEAST_RADIUS, SCENE_COVERAGE_MOST_RADIUS);
    }
    let step = max(1, (radius + 2) / 4);
    var count = 0.0;
    var mean_guide = vec3<f32>(0.0);
    var mean_mask = 0.0;
    var guide_mask = vec3<f32>(0.0);
    // Second moments of the guide: xx, yy, zz and xy, xz, yz.
    var square = vec3<f32>(0.0);
    var product = vec3<f32>(0.0);
    for (var dy = -radius; dy <= radius; dy = dy + step) {
        for (var dx = -radius; dx <= radius; dx = dx + step) {
            let tap = Bake::scene_bake_clamp(texel + vec2<i32>(dx, dy));
            let rgb = textureLoad(Bake::scene_working_tex, tap, 0).rgb;
            let guide = Bake::scene_bake_guide(rgb);
            let mask = scene_near_mask(textureLoad(Bake::scene_geometry_tex, tap, 0), rgb, kind, split, model);
            count += 1.0;
            mean_guide += guide;
            mean_mask += mask;
            guide_mask += guide * mask;
            square += guide * guide;
            product += vec3<f32>(guide.x * guide.y, guide.x * guide.z, guide.y * guide.z);
        }
    }
    mean_guide /= count;
    mean_mask /= count;
    let covariance = guide_mask / count - mean_guide * mean_mask;
    let level = dot(mean_guide, vec3<f32>(1.0 / 3.0));
    let epsilon = SCENE_COVERAGE_EPSILON * SCENE_COVERAGE_EPSILON * level * level
        + SCENE_COVERAGE_EPSILON_FLOOR;
    // The guide's covariance in the window plus epsilon on the diagonal,
    // inverted through its cofactors.
    let diagonal = square / count - mean_guide * mean_guide + vec3<f32>(epsilon);
    let off = product / count - vec3<f32>(
        mean_guide.x * mean_guide.y,
        mean_guide.x * mean_guide.z,
        mean_guide.y * mean_guide.z,
    );
    let c00 = diagonal.y * diagonal.z - off.z * off.z;
    let c01 = off.y * off.z - off.x * diagonal.z;
    let c02 = off.x * off.z - off.y * diagonal.y;
    let c11 = diagonal.x * diagonal.z - off.y * off.y;
    let c12 = off.x * off.y - diagonal.x * off.z;
    let c22 = diagonal.x * diagonal.y - off.x * off.x;
    let determinant = diagonal.x * c00 + off.x * c01 + off.y * c02;
    let a = vec3<f32>(
        c00 * covariance.x + c01 * covariance.y + c02 * covariance.z,
        c01 * covariance.x + c11 * covariance.y + c12 * covariance.z,
        c02 * covariance.x + c12 * covariance.y + c22 * covariance.z,
    ) / determinant;
    var b = mean_mask - dot(a, mean_guide);
    // A window whose mask the guide cannot follow at all still holds two
    // surfaces: keep its constant coverage off the values that mark one.
    if all(a == vec3<f32>(0.0)) {
        b = clamp(b, 1e-3, 1.0 - 1e-3);
    }
    textureStore(Bake::scene_coverage_out, texel, vec4<f32>(a, b));
}

// The constant coverage of texels at no edge: the side of the first edge
// texel's split among their neighbours, else near. Every texel's kind of edge
// goes to the FAR layer, where the layers read it (bake_layers.wgsl).
@compute @workgroup_size(8, 8, 1)
fn bake_scene_coverage_sides(@builtin(global_invocation_id) gid: vec3<u32>) {
    let texel = vec2<i32>(gid.xy);
    if any(texel >= Bake::scene_bake_size()) { return; }
    let own = Bake::scene_bake_scratch[Bake::scene_bake_index(texel)].y;
    textureStore(Bake::scene_far_out, texel, vec4<f32>(f32(own), 0.0, 0.0, 0.0));
    if own != 0u {
        return;
    }
    let geometry = textureLoad(Bake::scene_geometry_tex, texel, 0);
    var side = 1.0;
    var found = false;
    for (var dy = -1; dy <= 1; dy = dy + 1) {
        for (var dx = -1; dx <= 1; dx = dx + 1) {
            let edge = Bake::scene_bake_scratch[Bake::scene_bake_index(Bake::scene_bake_clamp(texel + vec2<i32>(dx, dy)))];
            if edge.y != 0u && !found {
                // At the band's rim the sky probability tells the side; the
                // photo's ramp lies well inside the band.
                side = round(scene_near_mask(geometry, vec3<f32>(0.0), edge.y, bitcast<f32>(edge.x), SCENE_NO_SKY_MODEL));
                found = true;
            }
        }
    }
    textureStore(Bake::scene_coverage_out, texel, vec4<f32>(0.0, 0.0, 0.0, side));
}
