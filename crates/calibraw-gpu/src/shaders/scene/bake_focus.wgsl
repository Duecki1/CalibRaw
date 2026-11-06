#import calibraw::scene::bake_common as Bake

// B7: the lens blur by depth, measured on the working image (bake_common.wgsl).
//
// At an edge, blurring the image again by known Gaussians σ1 < σ2 changes the
// gradient by the ratio R = √(σ² + σ2²) / √(σ² + σ1²), so
// σ² = (σ2² − R² σ1²) / (R² − 1), after Zhuo and Sim ("Defocus Map Estimation
// from a Single Image", 2011), who compare the unblurred gradient instead.
// Blurring both sides of the ratio keeps noise from inflating the gradient at
// its peak, which would make every wide blur read sharp. It needs only the
// edge's own neighbourhood, so texture beside it does not disturb it. Each
// edge, where the gradient peaks across it, adds its gradient to a histogram
// bin of its depth and blur. Nothing at a depth is sharper than the lens
// there, while shading and soft shadows only widen edges, so the fit takes a
// low quantile of each depth's blur (SCENE_FOCUS_QUANTILE) and fits the
// thin-lens form σ(z) = c·|1/z − 1/z_f| + σ0 to those, robustly, every depth
// with enough edges counting alike. Every surface then takes the blur of its
// depth, featureless ones included. The working image's own resampling and
// the gradient's differences add a little blur everywhere, which σ0 takes up.
//
// Only edges within one surface are measured, where both sides lie at about
// the same depth: at an occluding edge the two surfaces blur differently. The
// sky is no surface and is not measured.

// The known blurs, in working texels, and the radius the wider one's weights
// reach.
const SCENE_FOCUS_REBLUR: vec2<f32> = vec2<f32>(1.0, 3.0);
const SCENE_FOCUS_REBLUR_RADIUS: i32 = 6;
// The least gradient of an edge measured, per texel in guide units (square
// root of luminance): fainter ones are mostly noise.
const SCENE_FOCUS_MIN_GRADIENT: f32 = 0.004;
// Gradient ratios this close to 1 tell no blur apart from noise.
const SCENE_FOCUS_MIN_RATIO: f32 = 1.005;
// Sky probability above which a texel is sky.
const SCENE_FOCUS_SKY: f32 = 0.5;
// Rec.2020 luminance weights.
const SCENE_FOCUS_LUMINANCE: vec3<f32> = vec3<f32>(0.2627, 0.6780, 0.0593);

fn scene_focus_guide(texel: vec2<i32>) -> f32 {
    let rgb = textureLoad(Bake::scene_working_tex, Bake::scene_bake_clamp(texel), 0).rgb;
    return sqrt(max(dot(rgb, SCENE_FOCUS_LUMINANCE), 0.0));
}

fn scene_focus_gradient(texel: vec2<i32>) -> vec2<f32> {
    return 0.5 * vec2<f32>(
        scene_focus_guide(texel + vec2<i32>(1, 0)) - scene_focus_guide(texel - vec2<i32>(1, 0)),
        scene_focus_guide(texel + vec2<i32>(0, 1)) - scene_focus_guide(texel - vec2<i32>(0, 1)),
    );
}

@compute @workgroup_size(8, 8, 1)
fn bake_scene_focus_samples(@builtin(global_invocation_id) gid: vec3<u32>) {
    let texel = vec2<i32>(gid.xy);
    let size = Bake::scene_bake_size();
    let margin = SCENE_FOCUS_REBLUR_RADIUS + 2;
    if any(texel < vec2<i32>(margin)) || any(texel >= size - vec2<i32>(margin)) { return; }

    let gradient = scene_focus_gradient(texel);
    let magnitude = length(gradient);
    if magnitude < SCENE_FOCUS_MIN_GRADIENT { return; }
    // Only where the gradient peaks across the edge.
    let across = vec2<i32>(round(gradient / magnitude));
    if length(scene_focus_gradient(texel + across)) > magnitude
        || length(scene_focus_gradient(texel - across)) > magnitude {
        return;
    }
    let own = textureLoad(Bake::scene_geometry_tex, texel, 0);
    if own.y > SCENE_FOCUS_SKY { return; }
    // Both sides on one surface.
    let reach = SCENE_FOCUS_REBLUR_RADIUS * across;
    let z_sides = vec2<f32>(
        Bake::scene_bake_frame_depth(textureLoad(Bake::scene_geometry_tex, texel - reach, 0).x),
        Bake::scene_bake_frame_depth(textureLoad(Bake::scene_geometry_tex, texel + reach, 0).x),
    );
    if max(z_sides.x, z_sides.y) > (1.0 + Bake::SCENE_BAND_RELIEF) * min(z_sides.x, z_sides.y) { return; }

    // The gradients of the image blurred again by each SCENE_FOCUS_REBLUR at
    // the texel: the gradients around it, Gaussian-weighted.
    var blurred = array<vec2<f32>, 2>(vec2<f32>(0.0), vec2<f32>(0.0));
    var weights = vec2<f32>(0.0);
    let spread = 0.5 / (SCENE_FOCUS_REBLUR * SCENE_FOCUS_REBLUR);
    for (var dy = -SCENE_FOCUS_REBLUR_RADIUS; dy <= SCENE_FOCUS_REBLUR_RADIUS; dy = dy + 1) {
        for (var dx = -SCENE_FOCUS_REBLUR_RADIUS; dx <= SCENE_FOCUS_REBLUR_RADIUS; dx = dx + 1) {
            let offset = vec2<f32>(f32(dx), f32(dy));
            let weight = exp(-dot(offset, offset) * spread);
            let tap = scene_focus_gradient(texel + vec2<i32>(dx, dy));
            blurred[0] += weight.x * tap;
            blurred[1] += weight.y * tap;
            weights += weight;
        }
    }
    let ratio = length(blurred[0] / weights.x) / max(length(blurred[1] / weights.y), 1e-6);
    let narrow = SCENE_FOCUS_REBLUR.x * SCENE_FOCUS_REBLUR.x;
    let wide = SCENE_FOCUS_REBLUR.y * SCENE_FOCUS_REBLUR.y;
    let square = ratio * ratio;
    if ratio < SCENE_FOCUS_MIN_RATIO || square * narrow >= wide { return; }
    let blur = sqrt((wide - square * narrow) / (square - 1.0));

    let blur_bin = floor(log2(blur / Bake::SCENE_FOCUS_LEAST_BLUR) * Bake::SCENE_FOCUS_BINS_PER_OCTAVE);
    if blur_bin < 0.0 || blur_bin >= f32(Bake::SCENE_FOCUS_BLUR_BINS) { return; }
    let inverse = 1.0 / Bake::scene_bake_frame_depth(own.x);
    let farthest = Bake::scene_focus_farthest_inverse();
    let depth_bin = min(
        u32(max((inverse - farthest) / max(1.0 - farthest, 1e-6), 0.0) * f32(Bake::SCENE_FOCUS_DEPTH_BINS)),
        Bake::SCENE_FOCUS_DEPTH_BINS - 1u,
    );
    // Weighted by the edge's gradient (guide units per texel, in fixed point),
    // so faint edges count less.
    let weight = u32(round(min(magnitude, 1.0) * Bake::SCENE_FOCUS_WEIGHT_SCALE));
    atomicAdd(&Bake::scene_focus.histogram[depth_bin * Bake::SCENE_FOCUS_BLUR_BINS + u32(blur_bin)], weight);
}

// Depth bins that weigh less than this many full-contrast edges are left out
// of the fit; heavier ones count as this many, so the photo's largest surface
// does not outweigh the rest.
const SCENE_FOCUS_MIN_BIN_WEIGHT: f32 = 8.0;
// The share of a depth's edges that are sharper than the blur taken for it.
const SCENE_FOCUS_QUANTILE: f32 = 0.2;
// Candidate planes in focus, even in 1/z from the farthest surface to the
// nearest; one per lane.
const SCENE_FOCUS_CANDIDATES: u32 = 64u;
const SCENE_FOCUS_ITERATIONS: i32 = 4;
// Huber threshold of a bin's residual, in working texels of deviation.
const SCENE_FOCUS_HUBER: f32 = 0.5;

var<workgroup> focus_costs: array<f32, 64>;
var<workgroup> focus_fits: array<vec3<f32>, 64>;

// A depth bin's 1/z (x), the SCENE_FOCUS_QUANTILE of its deviations in
// working texels (y) and its weight in the fit (z), or zero weight.
fn scene_focus_bin(bin: u32) -> vec3<f32> {
    var total = 0.0;
    for (var k = 0u; k < Bake::SCENE_FOCUS_BLUR_BINS; k = k + 1u) {
        total += f32(atomicLoad(&Bake::scene_focus.histogram[bin * Bake::SCENE_FOCUS_BLUR_BINS + k]));
    }
    let weight = total / Bake::SCENE_FOCUS_WEIGHT_SCALE;
    let farthest = Bake::scene_focus_farthest_inverse();
    let inverse = farthest + (f32(bin) + 0.5) / f32(Bake::SCENE_FOCUS_DEPTH_BINS) * (1.0 - farthest);
    if weight < SCENE_FOCUS_MIN_BIN_WEIGHT {
        return vec3<f32>(inverse, 0.0, 0.0);
    }
    let wanted = SCENE_FOCUS_QUANTILE * total;
    var below = 0.0;
    var quantile = 0.0;
    for (var k = 0u; k < Bake::SCENE_FOCUS_BLUR_BINS; k = k + 1u) {
        let count = f32(atomicLoad(&Bake::scene_focus.histogram[bin * Bake::SCENE_FOCUS_BLUR_BINS + k]));
        if below + count >= wanted {
            // Even in log blur within the bin.
            let within = (wanted - below) / max(count, 1.0);
            quantile = Bake::SCENE_FOCUS_LEAST_BLUR * exp2((f32(k) + within) / Bake::SCENE_FOCUS_BINS_PER_OCTAVE);
            break;
        }
        below += count;
    }
    return vec3<f32>(inverse, quantile, SCENE_FOCUS_MIN_BIN_WEIGHT);
}

@compute @workgroup_size(64, 1, 1)
fn bake_scene_focus_fit(@builtin(local_invocation_index) lane: u32) {
    var bins: array<vec3<f32>, 32>;
    var used = 0u;
    for (var bin = 0u; bin < Bake::SCENE_FOCUS_DEPTH_BINS; bin = bin + 1u) {
        bins[bin] = scene_focus_bin(bin);
        used += select(0u, 1u, bins[bin].z > 0.0);
    }
    let farthest = Bake::scene_focus_farthest_inverse();
    let focus = mix(farthest, 1.0, f32(lane) / f32(SCENE_FOCUS_CANDIDATES - 1u));
    // Iteratively reweighted least squares of σ = c·x + σ0 with
    // x = |1/z − 1/z_f|, both terms held at or above 0.
    var c = 0.0;
    var flat = 0.0;
    for (var iteration = 0; iteration < SCENE_FOCUS_ITERATIONS; iteration = iteration + 1) {
        var sums = vec4<f32>(0.0); // w, w·x, w·x², w·σ
        var cross = 0.0; // w·x·σ
        for (var bin = 0u; bin < Bake::SCENE_FOCUS_DEPTH_BINS; bin = bin + 1u) {
            let sample = bins[bin];
            if sample.z == 0.0 { continue; }
            let x = abs(sample.x - focus);
            var weight = sample.z;
            if iteration > 0 {
                let residual = abs(c * x + flat - sample.y);
                weight *= min(1.0, SCENE_FOCUS_HUBER / max(residual, 1e-6));
            }
            sums += weight * vec4<f32>(1.0, x, x * x, sample.y);
            cross += weight * x * sample.y;
        }
        let determinant = sums.x * sums.z - sums.y * sums.y;
        if sums.x <= 0.0 { break; }
        if determinant > 1e-9 * sums.x * sums.z {
            c = (sums.x * cross - sums.y * sums.w) / determinant;
            flat = (sums.w - c * sums.y) / sums.x;
        } else {
            c = 0.0;
            flat = sums.w / sums.x;
        }
        if c < 0.0 {
            c = 0.0;
            flat = sums.w / sums.x;
        }
        if flat < 0.0 {
            flat = 0.0;
            c = select(0.0, cross / sums.z, sums.z > 0.0);
        }
    }
    var cost = 0.0;
    for (var bin = 0u; bin < Bake::SCENE_FOCUS_DEPTH_BINS; bin = bin + 1u) {
        let sample = bins[bin];
        let residual = abs(c * abs(sample.x - focus) + flat - sample.y);
        // Huber's loss.
        cost += sample.z * select(
            SCENE_FOCUS_HUBER * (residual - 0.5 * SCENE_FOCUS_HUBER),
            0.5 * residual * residual,
            residual <= SCENE_FOCUS_HUBER,
        );
    }
    focus_costs[lane] = cost;
    focus_fits[lane] = vec3<f32>(c, focus, flat);
    workgroupBarrier();
    if lane != 0u { return; }
    var best = 0u;
    for (var candidate = 1u; candidate < SCENE_FOCUS_CANDIDATES; candidate = candidate + 1u) {
        if focus_costs[candidate] < focus_costs[best] {
            best = candidate;
        }
    }
    let short = f32(min(Bake::scene_bake.size.x, Bake::scene_bake.size.y));
    let fit = focus_fits[best];
    Bake::scene_focus.fit = select(
        vec4<f32>(0.0),
        vec4<f32>(fit.x / short, fit.y, fit.z / short, 1.0),
        used > 0u,
    );
}
