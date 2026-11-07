#import calibraw::scene::bake_common as Bake

// B4 and B5: each surface's colour, depth, normal and sky probability in the
// band (bake_common.wgsl, layers GEOMETRY.yzw, NORMALS, NEAR and FAR).
//
// A band texel whose coverage is nearly 1 shows only the near surface, and
// one nearly 0 only the far one (SCENE_PURE_COVERAGE): its own colour, depth,
// normal and sky probability are that surface's. The far surface's sky
// probability is what tells the sky's outline from a surface at the sky's
// depth: beside the sky, the farthest depth around a texel is the sky's even
// where the texel itself shows none. Texels between them get each surface's
// values filled in from those samples by push-pull (Gortler et al., "The
// Lumigraph", 1996): a pyramid of weighted means is pushed down from the
// samples and pulled back up where a level has none. Texels outside the band
// show their own surface on both layers. At the sky's outline alone (edge
// kind SCENE_EDGE_SKY), the far surface is the sky: a texel showing it
// purely is a sample of sky probability 1 at the farthest depth, wherever the
// depth model drew its outline. Each texel's kind of edge comes from the
// FAR layer (bake_coverage.wgsl) until bake_scene_layers_near has copied it
// to NEAR.w.
//
// The pyramid lives in the scratch buffer, levels 1 to SCENE_LAYER_LEVELS,
// each ceil(size / 2^k) texels, two vec4<u32> per texel (near, then far),
// each packing a surface's colour, depth, normal x and y, sky probability and
// weight as half floats. The normal's z is not stored: visible normals face
// the camera (z < 0), so it follows from x and y, and a mean of unit normals
// lifted back onto the sphere stays a unit normal of the same xy.
// Its coarsest level is the working size / 32: farther than that from any
// sample, a texel keeps its own values.

const SCENE_LAYER_LEVELS: u32 = 5u;

// The pyramid level a push or pull pass works on (x).
@group(0) @binding(103) var<uniform> scene_layer_level: vec4<u32>;

struct SceneSurfaceSample {
    color: vec3<f32>,
    depth: f32,
    normal: vec3<f32>,
    sky: f32,
    weight: f32,
}

const SCENE_NO_SAMPLE: SceneSurfaceSample = SceneSurfaceSample(vec3<f32>(0.0), 0.0, vec3<f32>(0.0), 0.0, 0.0);

fn scene_level_size(level: u32) -> vec2<i32> {
    let size = Bake::scene_bake.size.xy;
    let scale = (1u << level) - 1u;
    return vec2<i32>((size + vec2<u32>(scale)) >> vec2<u32>(level));
}

// The index of a level's first texel, in vec4 units of the scratch buffer.
fn scene_level_offset(level: u32) -> u32 {
    var offset = 0u;
    for (var k = 1u; k < level; k = k + 1u) {
        let size = scene_level_size(k);
        offset += 2u * u32(size.x * size.y);
    }
    return offset;
}

// A half float's bits from an f32, rounded to nearest. Values too small for a
// normal half become 0 and values too large the largest half: the pyramid
// holds colours, depths, sky probabilities and weights in [0, 1], and unit
// normals.
// (pack2x16float needs a shader capability not every device has.)
fn scene_half(value: f32) -> u32 {
    let bits = bitcast<u32>(value);
    let sign = (bits >> 16u) & 0x8000u;
    // Rounding may carry into the exponent, which is then right.
    let rounded = (bits & 0x7fffffffu) + 0x1000u;
    let exponent = i32(rounded >> 23u) - 112;
    if exponent <= 0 {
        return sign;
    }
    if exponent >= 31 {
        return sign | 0x7bffu;
    }
    return sign | (u32(exponent) << 10u) | ((rounded >> 13u) & 0x3ffu);
}

fn scene_from_half(half: u32) -> f32 {
    let sign = (half & 0x8000u) << 16u;
    let exponent = (half >> 10u) & 0x1fu;
    if exponent == 0u {
        return bitcast<f32>(sign);
    }
    return bitcast<f32>(sign | ((exponent + 112u) << 23u) | ((half & 0x3ffu) << 13u));
}

fn scene_pack_halves(values: vec2<f32>) -> u32 {
    return scene_half(values.x) | (scene_half(values.y) << 16u);
}

fn scene_unpack_halves(packed: u32) -> vec2<f32> {
    return vec2<f32>(scene_from_half(packed & 0xffffu), scene_from_half(packed >> 16u));
}

fn scene_pack(sample: SceneSurfaceSample) -> vec4<u32> {
    return vec4<u32>(
        scene_pack_halves(sample.color.rg),
        scene_pack_halves(vec2<f32>(sample.color.b, sample.depth)),
        scene_pack_halves(sample.normal.xy),
        scene_pack_halves(vec2<f32>(sample.sky, sample.weight)),
    );
}

fn scene_unpack(packed: vec4<u32>) -> SceneSurfaceSample {
    let rg = scene_unpack_halves(packed.x);
    let b_depth = scene_unpack_halves(packed.y);
    let normal_xy = scene_unpack_halves(packed.z);
    let sky_weight = scene_unpack_halves(packed.w);
    return SceneSurfaceSample(
        vec3<f32>(rg, b_depth.x),
        b_depth.y,
        vec3<f32>(normal_xy, -sqrt(max(1.0 - dot(normal_xy, normal_xy), 0.0))),
        sky_weight.x,
        sky_weight.y,
    );
}

// Surface `side` (0 near, 1 far) of a level's texel.
fn scene_level_load(level: u32, texel: vec2<i32>, side: u32) -> SceneSurfaceSample {
    let size = scene_level_size(level);
    let clamped = clamp(texel, vec2<i32>(0), size - vec2<i32>(1));
    let index = scene_level_offset(level) + 2u * u32(clamped.y * size.x + clamped.x) + side;
    return scene_unpack(Bake::scene_bake_scratch[index]);
}

fn scene_level_store(level: u32, texel: vec2<i32>, side: u32, sample: SceneSurfaceSample) {
    let size = scene_level_size(level);
    let index = scene_level_offset(level) + 2u * u32(texel.y * size.x + texel.x) + side;
    Bake::scene_bake_scratch[index] = scene_pack(sample);
}

fn scene_weighted(sample: SceneSurfaceSample, weight: f32) -> SceneSurfaceSample {
    return SceneSurfaceSample(
        sample.color * weight,
        sample.depth * weight,
        sample.normal * weight,
        sample.sky * weight,
        weight,
    );
}

fn scene_add(a: SceneSurfaceSample, b: SceneSurfaceSample) -> SceneSurfaceSample {
    return SceneSurfaceSample(
        a.color + b.color,
        a.depth + b.depth,
        a.normal + b.normal,
        a.sky + b.sky,
        a.weight + b.weight,
    );
}

// A weighted sum as a mean, its weight capped at 1.
fn scene_mean(sum: SceneSurfaceSample) -> SceneSurfaceSample {
    if sum.weight <= 0.0 {
        return SCENE_NO_SAMPLE;
    }
    return SceneSurfaceSample(
        sum.color / sum.weight,
        sum.depth / sum.weight,
        sum.normal / sum.weight,
        sum.sky / sum.weight,
        min(sum.weight, 1.0),
    );
}

// A working texel's own values, how purely each surface shows there ((near,
// far) sample weights) and whether it lies at an edge. Of the texels at no
// edge, only those beside an edge texel are samples, of the side coverage
// gives them (bake_coverage.wgsl takes it from that edge's split): an edge's
// surfaces are filled from the edge and its immediate surroundings. Where
// the edge's own texels never show one surface purely, as along a crease
// whose coverage ramps across the whole band, the texels just past the band
// are that surface's nearest samples. Texels farther out take their side by
// default, not from an edge, and are none.
struct SceneTexelSurface {
    own: SceneSurfaceSample,
    // The texel's values as a sample of the far surface: its own, or the sky's
    // at the sky's outline.
    far_own: SceneSurfaceSample,
    weights: vec2<f32>,
    edge: bool,
}

// `depth` is the texel's own linear depth (x) and sky probability (y).
// Whether one of the eight texels around `texel` lies at an edge.
fn scene_beside_edge(texel: vec2<i32>) -> bool {
    for (var dy = -1; dy <= 1; dy = dy + 1) {
        for (var dx = -1; dx <= 1; dx = dx + 1) {
            let neighbour = Bake::scene_bake_clamp(texel + vec2<i32>(dx, dy));
            if !Bake::scene_coverage_constant(textureLoad(Bake::scene_coverage_tex, neighbour, 0)) {
                return true;
            }
        }
    }
    return false;
}

// `kind` is the texel's kind of edge.
fn scene_texel_surface(texel: vec2<i32>, depth: vec2<f32>, kind: u32) -> SceneTexelSurface {
    let rgb = textureLoad(Bake::scene_working_tex, texel, 0).rgb;
    let normal = Bake::scene_input_normal_at(Bake::scene_bake_uv(texel));
    let own = SceneSurfaceSample(rgb, depth.x, normal, depth.y, 1.0);
    var far_own = own;
    if kind == Bake::SCENE_EDGE_SKY {
        far_own.depth = 1.0;
        far_own.sky = 1.0;
    }
    let coefficients = textureLoad(Bake::scene_coverage_tex, texel, 0);
    if Bake::scene_coverage_constant(coefficients) {
        var weights = vec2<f32>(0.0);
        if scene_beside_edge(texel) {
            weights = select(vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0), coefficients.w == 0.0);
        }
        return SceneTexelSurface(own, far_own, weights, false);
    }
    let coverage = Bake::scene_coverage_of(coefficients, Bake::scene_bake_guide(rgb));
    let weights = vec2<f32>(
        select(0.0, 1.0, coverage > Bake::SCENE_PURE_COVERAGE.y),
        select(0.0, 1.0, coverage < Bake::SCENE_PURE_COVERAGE.x),
    );
    return SceneTexelSurface(own, far_own, weights, true);
}

// A texel's kind of edge, as the FAR layer holds it until the layers are made
// and NEAR.w after.
fn scene_edge_kind(stored: f32) -> u32 {
    return u32(stored + 0.5);
}

// Push, level 0 to 1: the edges' pure texels, from the bake layers.
@compute @workgroup_size(8, 8, 1)
fn bake_scene_layers_push_first(@builtin(global_invocation_id) gid: vec3<u32>) {
    let texel = vec2<i32>(gid.xy);
    if any(texel >= scene_level_size(1u)) { return; }
    var near = SCENE_NO_SAMPLE;
    var far = near;
    for (var child = 0; child < 4; child = child + 1) {
        let source = Bake::scene_bake_clamp(2 * texel + vec2<i32>(child % 2, child / 2));
        let depth = textureLoad(Bake::scene_geometry_tex, source, 0).xy;
        let kind = scene_edge_kind(textureLoad(Bake::scene_far_tex, source, 0).x);
        let surface = scene_texel_surface(source, depth, kind);
        near = scene_add(near, scene_weighted(surface.own, surface.weights.x));
        far = scene_add(far, scene_weighted(surface.far_own, surface.weights.y));
    }
    scene_level_store(1u, texel, 0u, scene_mean(near));
    scene_level_store(1u, texel, 1u, scene_mean(far));
}

// Push, level k - 1 to k (scene_layer_level.x = k).
@compute @workgroup_size(8, 8, 1)
fn bake_scene_layers_push(@builtin(global_invocation_id) gid: vec3<u32>) {
    let level = scene_layer_level.x;
    let texel = vec2<i32>(gid.xy);
    if any(texel >= scene_level_size(level)) { return; }
    for (var side = 0u; side < 2u; side = side + 1u) {
        var sum = SCENE_NO_SAMPLE;
        for (var child = 0; child < 4; child = child + 1) {
            let sample = scene_level_load(level - 1u, 2 * texel + vec2<i32>(child % 2, child / 2), side);
            sum = scene_add(sum, scene_weighted(sample, sample.weight));
        }
        scene_level_store(level, texel, side, scene_mean(sum));
    }
}

// Level `level`'s values at a finer texel's position, interpolated.
fn scene_level_up(level: u32, finer: vec2<i32>, side: u32) -> SceneSurfaceSample {
    let position = (vec2<f32>(finer) + vec2<f32>(0.5)) * 0.5 - vec2<f32>(0.5);
    let base = vec2<i32>(floor(position));
    let t = position - vec2<f32>(base);
    var sum = SCENE_NO_SAMPLE;
    for (var corner = 0; corner < 4; corner = corner + 1) {
        let offset = vec2<i32>(corner % 2, corner / 2);
        let bilinear = select(1.0 - t.x, t.x, offset.x == 1) * select(1.0 - t.y, t.y, offset.y == 1);
        let sample = scene_level_load(level, base + offset, side);
        // Weighted by the coarse texel's own weight, so empty ones add nothing.
        sum = scene_add(sum, scene_weighted(sample, bilinear * sample.weight));
    }
    return scene_mean(sum);
}

// Pull, level k + 1 into k (scene_layer_level.x = k): where k has less than
// full weight, the coarser mean fills the rest.
@compute @workgroup_size(8, 8, 1)
fn bake_scene_layers_pull(@builtin(global_invocation_id) gid: vec3<u32>) {
    let level = scene_layer_level.x;
    let texel = vec2<i32>(gid.xy);
    if any(texel >= scene_level_size(level)) { return; }
    for (var side = 0u; side < 2u; side = side + 1u) {
        let own = scene_level_load(level, texel, side);
        let coarse = scene_level_up(level + 1u, texel, side);
        let keep = own.weight;
        let filled = SceneSurfaceSample(
            own.color * keep + coarse.color * (1.0 - keep),
            own.depth * keep + coarse.depth * (1.0 - keep),
            own.normal * keep + coarse.normal * (1.0 - keep),
            own.sky * keep + coarse.sky * (1.0 - keep),
            keep + coarse.weight * (1.0 - keep),
        );
        scene_level_store(level, texel, side, filled);
    }
}

// A surface's values at a band texel: its own where it shows purely, else
// filled from the pyramid's level 1, else its own.
fn scene_layer_value(surface: SceneTexelSurface, texel: vec2<i32>, side: u32) -> SceneSurfaceSample {
    let pure = select(surface.weights.x, surface.weights.y, side == 1u);
    var own = surface.own;
    if side == 1u {
        own = surface.far_own;
    }
    if pure > 0.0 {
        return own;
    }
    let filled = scene_level_up(1u, texel, side);
    if filled.weight <= 0.0 {
        return own;
    }
    return filled;
}

// The near surface's colour and both normals: layers NEAR and NORMALS.
@compute @workgroup_size(8, 8, 1)
fn bake_scene_layers_near(@builtin(global_invocation_id) gid: vec3<u32>) {
    let texel = vec2<i32>(gid.xy);
    if any(texel >= Bake::scene_bake_size()) { return; }
    let depth = textureLoad(Bake::scene_geometry_tex, texel, 0).xy;
    let kind = scene_edge_kind(textureLoad(Bake::scene_far_tex, texel, 0).x);
    let surface = scene_texel_surface(texel, depth, kind);
    var near = surface.own;
    var far = surface.own;
    if surface.edge {
        near = scene_layer_value(surface, texel, 0u);
        far = scene_layer_value(surface, texel, 1u);
    }
    textureStore(Bake::scene_normals_layer_out, texel, vec4<f32>(
        Bake::scene_octahedral(normalize(near.normal)),
        Bake::scene_octahedral(normalize(far.normal)),
    ));
    textureStore(Bake::scene_near_out, texel, vec4<f32>(near.color, f32(select(0u, kind, surface.edge))));
}

// The far surface's colour and sky probability and both surfaces' depths:
// layers FAR and GEOMETRY. The texel's own depth and sky come from the depth
// result again, as this pass writes the layer that held them.
@compute @workgroup_size(8, 8, 1)
fn bake_scene_layers_far(@builtin(global_invocation_id) gid: vec3<u32>) {
    let texel = vec2<i32>(gid.xy);
    if any(texel >= Bake::scene_bake_size()) { return; }
    let depth = Bake::scene_texel_depth(texel);
    let kind = scene_edge_kind(textureLoad(Bake::scene_near_tex, texel, 0).w);
    let surface = scene_texel_surface(texel, depth, kind);
    var near = surface.own;
    var far = surface.own;
    if surface.edge {
        near = scene_layer_value(surface, texel, 0u);
        far = scene_layer_value(surface, texel, 1u);
    }
    textureStore(Bake::scene_geometry_out, texel, vec4<f32>(depth.x, far.sky, near.depth, far.depth));
    textureStore(Bake::scene_far_out, texel, vec4<f32>(far.color, f32(select(0u, kind, surface.edge))));
}
