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
// model's. The mask is 1 nearer than the split, and for the sky's outline
// 1 minus the sky probability; with both, the smaller. The coefficients are
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
// The first pass leaves each texel's split and kind of edge in the scratch
// buffer for the second, which sets the constant texels.

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

// Kinds of edge, in the scratch buffer's y.
const SCENE_EDGE_DEPTH: u32 = 1u;
const SCENE_EDGE_SKY: u32 = 2u;

fn scene_log_depth(d: f32) -> f32 {
    return log(Bake::scene_bake_frame_depth(d));
}

// The near surface's mask at a tap with geometry `tap` (GEOMETRY layer), for
// an edge of kind `kind` split at log depth `split`.
fn scene_near_mask(tap: vec4<f32>, kind: u32, split: f32) -> f32 {
    var mask = 1.0;
    if (kind & SCENE_EDGE_DEPTH) != 0u {
        mask = select(0.0, 1.0, scene_log_depth(tap.x) < split);
    }
    if (kind & SCENE_EDGE_SKY) != 0u {
        mask = min(mask, 1.0 - tap.y);
    }
    return mask;
}

fn scene_store_edge(texel: vec2<i32>, kind: u32, split: f32) {
    Bake::scene_bake_scratch[Bake::scene_bake_index(texel)] = vec4<u32>(bitcast<u32>(split), kind, 0u, 0u);
}

@compute @workgroup_size(8, 8, 1)
fn bake_scene_coverage(@builtin(global_invocation_id) gid: vec3<u32>) {
    let texel = vec2<i32>(gid.xy);
    if any(texel >= Bake::scene_bake_size()) { return; }
    let band = u32(textureLoad(Bake::scene_far_tex, texel, 0).x + 0.5);
    if band == 0u {
        scene_store_edge(texel, 0u, 0.0);
        return;
    }

    // The edge test, on a lattice of every other texel within the band radius.
    var lowest = 1e9;
    var highest = -1e9;
    var sky_low = 1.0;
    var sky_high = 0.0;
    for (var dy = -Bake::SCENE_BAND_RADIUS; dy <= Bake::SCENE_BAND_RADIUS; dy = dy + 2) {
        for (var dx = -Bake::SCENE_BAND_RADIUS; dx <= Bake::SCENE_BAND_RADIUS; dx = dx + 2) {
            let tap = textureLoad(Bake::scene_geometry_tex, Bake::scene_bake_clamp(texel + vec2<i32>(dx, dy)), 0);
            let depth = scene_log_depth(tap.x);
            lowest = min(lowest, depth);
            highest = max(highest, depth);
            sky_low = min(sky_low, tap.y);
            sky_high = max(sky_high, tap.y);
        }
    }
    var split = 0.5 * (lowest + highest);
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
    var kind = 0u;
    if gap > log(1.0 + Bake::SCENE_BAND_RELIEF) && between > SCENE_EDGE_BIMODALITY * spread {
        kind |= SCENE_EDGE_DEPTH;
    }
    if (band & Bake::SCENE_BAND_SKY) != 0u && sky_low < 0.5 && sky_high > 0.5 {
        kind |= SCENE_EDGE_SKY;
    }
    scene_store_edge(texel, kind, split);
    if kind == 0u {
        return;
    }

    let blur = Bake::scene_focus_blur(exp(groups.x));
    let radius = clamp(i32(round(2.0 * blur)), SCENE_COVERAGE_LEAST_RADIUS, SCENE_COVERAGE_MOST_RADIUS);
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
            let guide = Bake::scene_bake_guide(textureLoad(Bake::scene_working_tex, tap, 0).rgb);
            let mask = scene_near_mask(textureLoad(Bake::scene_geometry_tex, tap, 0), kind, split);
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
// texel's split among their neighbours, else near.
@compute @workgroup_size(8, 8, 1)
fn bake_scene_coverage_sides(@builtin(global_invocation_id) gid: vec3<u32>) {
    let texel = vec2<i32>(gid.xy);
    if any(texel >= Bake::scene_bake_size()) { return; }
    if Bake::scene_bake_scratch[Bake::scene_bake_index(texel)].y != 0u {
        return;
    }
    let geometry = textureLoad(Bake::scene_geometry_tex, texel, 0);
    var side = 1.0;
    var found = false;
    for (var dy = -1; dy <= 1; dy = dy + 1) {
        for (var dx = -1; dx <= 1; dx = dx + 1) {
            let edge = Bake::scene_bake_scratch[Bake::scene_bake_index(Bake::scene_bake_clamp(texel + vec2<i32>(dx, dy)))];
            if edge.y != 0u && !found {
                side = round(scene_near_mask(geometry, edge.y, bitcast<f32>(edge.x)));
                found = true;
            }
        }
    }
    textureStore(Bake::scene_coverage_out, texel, vec4<f32>(0.0, 0.0, 0.0, side));
}
