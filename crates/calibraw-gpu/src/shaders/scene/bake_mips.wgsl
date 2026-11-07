#import calibraw::scene::bake_common as Bake

// B6: the working image's mip chain (bake_common.wgsl). Colour is the box mean
// of the finer level, for reading the scene with a footprint. Alpha, from
// level 1 on, is the least linear depth of the visible surface below the
// texel, a hierarchical depth for tracing rays through the scene (Wet
// Surfaces): the near surface where it covers at least half the texel, else
// the far one. Level sizes halve and round down, as wgpu's mip levels do; the
// last texel of a level also takes the finer level's odd last row or column,
// so no depth is left out. Sizes come from the storage target and the level
// uniform, as some drivers misreport the sizes of sampled levels above 0.

// The level written (x) and the finer level's size (yz).
@group(0) @binding(103) var<uniform> scene_mip_level: vec4<u32>;

// The visible surface's depth at a working texel.
fn scene_visible_depth(texel: vec2<i32>) -> f32 {
    let geometry = textureLoad(Bake::scene_geometry_tex, texel, 0);
    let rgb = textureLoad(Bake::scene_working_tex, texel, 0).rgb;
    let coverage = Bake::scene_coverage_of(
        textureLoad(Bake::scene_coverage_tex, texel, 0),
        Bake::scene_bake_guide(rgb),
    );
    return select(geometry.w, geometry.z, coverage >= 0.5);
}

// The finer texels below `texel`: [first, last] per axis.
fn scene_mip_children(texel: vec2<i32>, size: vec2<i32>, finer: vec2<i32>) -> vec4<i32> {
    let first = 2 * texel;
    let odd = (finer % vec2<i32>(2)) == vec2<i32>(1);
    let extra = select(vec2<i32>(0), vec2<i32>(1), odd & (texel == size - vec2<i32>(1)));
    return vec4<i32>(first, min(first + vec2<i32>(1) + extra, finer - vec2<i32>(1)));
}

@compute @workgroup_size(8, 8, 1)
fn bake_scene_working_mip(@builtin(global_invocation_id) gid: vec3<u32>) {
    let texel = vec2<i32>(gid.xy);
    let size = vec2<i32>(textureDimensions(Bake::scene_working_out));
    if any(texel >= size) { return; }
    let first_level = scene_mip_level.x == 1u;
    let children = scene_mip_children(texel, size, vec2<i32>(scene_mip_level.yz));
    var color = vec3<f32>(0.0);
    var depth = 1.0;
    var count = 0.0;
    for (var y = children.y; y <= children.w; y = y + 1) {
        for (var x = children.x; x <= children.z; x = x + 1) {
            let finer = textureLoad(Bake::scene_working_tex, vec2<i32>(x, y), 0);
            color += finer.rgb;
            count += 1.0;
            if first_level {
                depth = min(depth, scene_visible_depth(vec2<i32>(x, y)));
            } else {
                depth = min(depth, finer.a);
            }
        }
    }
    textureStore(Bake::scene_working_out, texel, vec4<f32>(color / count, depth));
}

// The same mip chain for a bake without a depth result: no geometry, so the
// alpha, the nearest visible depth, is 1 at every level.
@compute @workgroup_size(8, 8, 1)
fn bake_scene_working_mip_flat(@builtin(global_invocation_id) gid: vec3<u32>) {
    let texel = vec2<i32>(gid.xy);
    let size = vec2<i32>(textureDimensions(Bake::scene_working_out));
    if any(texel >= size) { return; }
    let children = scene_mip_children(texel, size, vec2<i32>(scene_mip_level.yz));
    var color = vec3<f32>(0.0);
    var count = 0.0;
    for (var y = children.y; y <= children.w; y = y + 1) {
        for (var x = children.x; x <= children.z; x = x + 1) {
            color += textureLoad(Bake::scene_working_tex, vec2<i32>(x, y), 0).rgb;
            count += 1.0;
        }
    }
    textureStore(Bake::scene_working_out, texel, vec4<f32>(color / count, 1.0));
}
