#import calibraw::scene::bake_common as Bake

// The scene bake's first passes (bake_common.wgsl): the working image from the
// accumulated sums, then depth per texel (B1) and the band around depth edges
// and the sky's outline (B2). The band's extent is a separable minimum and
// maximum, of depth over SCENE_BAND_RADIUS texels and of sky probability over
// SCENE_SKY_BAND_RADIUS: rows into the scratch buffer, then columns. Edges
// clamp.

// The working image's level 0: each texel's mean colour. Alpha holds nothing
// at level 0 (the mips' alpha is B6's, bake_mips.wgsl).
@compute @workgroup_size(8, 8, 1)
fn resolve_scene_working_image(@builtin(global_invocation_id) gid: vec3<u32>) {
    let texel = vec2<i32>(gid.xy);
    if any(texel >= Bake::scene_bake_size()) { return; }
    let sums = bitcast<vec4<f32>>(Bake::scene_bake_scratch[Bake::scene_bake_index(texel)]);
    let rgb = sums.xyz / max(sums.w, 1.0);
    textureStore(Bake::scene_working_out, texel, vec4<f32>(rgb, 0.0));
}

// B1: linear depth and sky probability per texel, into the COVERAGE layer
// until B3 replaces them.
@compute @workgroup_size(8, 8, 1)
fn bake_scene_distance(@builtin(global_invocation_id) gid: vec3<u32>) {
    let texel = vec2<i32>(gid.xy);
    if any(texel >= Bake::scene_bake_size()) { return; }
    let depth = Bake::scene_texel_depth(texel);
    textureStore(Bake::scene_coverage_out, texel, vec4<f32>(depth, 0.0, 0.0));
}

// An extent (least and greatest depth, least and greatest sky probability)
// widened by a tap's `values` (in the same order) `distance` texels away:
// depth within SCENE_BAND_RADIUS, sky probability within
// SCENE_SKY_BAND_RADIUS.
fn scene_extend(extent: vec4<f32>, values: vec4<f32>, distance: i32) -> vec4<f32> {
    var result = vec4<f32>(extent.xy, min(extent.z, values.z), max(extent.w, values.w));
    if distance <= Bake::SCENE_BAND_RADIUS {
        result = vec4<f32>(min(extent.x, values.x), max(extent.y, values.y), result.zw);
    }
    return result;
}

// B2, rows: the least and greatest depth (xy) within SCENE_BAND_RADIUS and sky
// probability (zw) within SCENE_SKY_BAND_RADIUS along the row, into the
// scratch buffer.
@compute @workgroup_size(8, 8, 1)
fn bake_scene_extent_rows(@builtin(global_invocation_id) gid: vec3<u32>) {
    let texel = vec2<i32>(gid.xy);
    if any(texel >= Bake::scene_bake_size()) { return; }
    var extent = vec4<f32>(1.0, 0.0, 1.0, 0.0);
    for (var dx = -Bake::SCENE_SKY_BAND_RADIUS; dx <= Bake::SCENE_SKY_BAND_RADIUS; dx = dx + 1) {
        let tap = Bake::scene_bake_clamp(texel + vec2<i32>(dx, 0));
        let depth = textureLoad(Bake::scene_coverage_tex, tap, 0).xy;
        extent = scene_extend(extent, vec4<f32>(depth.xx, depth.yy), abs(dx));
    }
    Bake::scene_bake_scratch[Bake::scene_bake_index(texel)] = bitcast<vec4<u32>>(extent);
}

// B2, columns: completes the extents and writes the GEOMETRY layer as
// (d, sky, least d, greatest d), and the band code (x) with the sky
// probability's extent (yz) into the FAR layer until the layers replace them
// (bake_layers.wgsl).
@compute @workgroup_size(8, 8, 1)
fn bake_scene_extent_columns(@builtin(global_invocation_id) gid: vec3<u32>) {
    let texel = vec2<i32>(gid.xy);
    if any(texel >= Bake::scene_bake_size()) { return; }
    var extent = vec4<f32>(1.0, 0.0, 1.0, 0.0);
    for (var dy = -Bake::SCENE_SKY_BAND_RADIUS; dy <= Bake::SCENE_SKY_BAND_RADIUS; dy = dy + 1) {
        let tap = Bake::scene_bake_clamp(texel + vec2<i32>(0, dy));
        let rows = bitcast<vec4<f32>>(Bake::scene_bake_scratch[Bake::scene_bake_index(tap)]);
        extent = scene_extend(extent, rows, abs(dy));
    }
    let depth = textureLoad(Bake::scene_coverage_tex, texel, 0).xy;
    var band = 0u;
    let relief = Bake::scene_bake_frame_depth(extent.y) / Bake::scene_bake_frame_depth(extent.x) - 1.0;
    if relief > Bake::SCENE_BAND_RELIEF {
        band |= Bake::SCENE_BAND_DEPTH;
    }
    if extent.z < Bake::SCENE_SKY_EDGE.y && extent.w > Bake::SCENE_SKY_EDGE.x {
        band |= Bake::SCENE_BAND_SKY;
    }
    textureStore(Bake::scene_geometry_out, texel, vec4<f32>(depth, extent.xy));
    textureStore(Bake::scene_far_out, texel, vec4<f32>(f32(band), extent.zw, 0.0));
}
