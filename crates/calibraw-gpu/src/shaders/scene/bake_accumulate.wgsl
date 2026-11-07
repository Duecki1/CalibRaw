#import calibraw::common as Common
#import calibraw::color as Color
#import calibraw::scene_source as SceneSource
#import calibraw::scene::bake_common as Bake

// Accumulates the scene bake's working image (bake_common.wgsl) from the
// pipeline's unedited scene: per working texel, the summed colour of the
// pixels whose centres it covers and their count, in the scratch buffer as
// f32 bits. Export tiles add only their cores, one tile after another, so
// every texel has one writer at a time and the sums equal the whole image's
// in any tiling; the bake then resolves them (bake_depth.wgsl).
@compute @workgroup_size(8, 8, 1)
fn accumulate_scene_working_image(@builtin(global_invocation_id) gid: vec3<u32>) {
    let size = Bake::scene_bake.size.xy;
    if gid.x >= size.x || gid.y >= size.y { return; }
    let span_x = Common::full_image_grid_pixel_span(gid.x, size.x, Common::camera_uniforms.full_width);
    let span_y = Common::full_image_grid_pixel_span(gid.y, size.y, Common::camera_uniforms.full_height);
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

    var sum = vec3<f32>(0.0);
    for (var y = lo.y; y < hi.y; y = y + 1) {
        for (var x = lo.x; x < hi.x; x = x + 1) {
            let rgb = Color::map_negative_gamut(SceneSource::profiled_scene_at(vec2<i32>(x, y) - origin));
            sum += max(rgb, vec3<f32>(0.0));
        }
    }
    let count = f32((hi.x - lo.x) * (hi.y - lo.y));
    let index = Bake::scene_bake_index(vec2<i32>(gid.xy));
    let before = bitcast<vec4<f32>>(Bake::scene_bake_scratch[index]);
    Bake::scene_bake_scratch[index] = bitcast<vec4<u32>>(before + vec4<f32>(sum, count));
}
