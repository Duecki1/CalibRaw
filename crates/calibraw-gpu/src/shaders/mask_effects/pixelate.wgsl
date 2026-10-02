
fn mask_pixelated_source_at(pos: vec2<i32>, reference_block_size: f32) -> vec3<f32> {
    let block_size = SceneAdjustments::presence_step(reference_block_size, 96);
    let global_pos = clamp(pos + Common::tile_origin(), vec2<i32>(0), Common::full_image_max());
    let cell_min = (global_pos / vec2<i32>(block_size)) * vec2<i32>(block_size);
    let cell_end = min(cell_min + vec2<i32>(block_size), Common::full_image_max() + vec2<i32>(1));
    let size = cell_end - cell_min;
    // Integrate every source pixel, including truncated blocks at image edges.
    // Nine regularly spaced samples alias checkerboards and miss small lights.
    // Integer global cells keep preview/export tiles on exactly the same grid.
    var sum = vec3<f32>(0.0);
    for (var y = 0; y < size.y; y += 1) {
        var row = vec3<f32>(0.0);
        for (var x = 0; x < size.x; x += 1) {
            row += SceneAdjustments::local_effects_at(cell_min + vec2<i32>(x, y) - Common::tile_origin());
        }
        sum += row / f32(size.x);
    }
    return sum / f32(size.y);
}

fn apply_pixelate(
    pos: vec2<i32>,
    source_rgb: vec3<f32>,
    primary: vec4<f32>,
) -> vec3<f32> {
    let amount = clamp(primary.x / 100.0, 0.0, 1.0);
    if amount <= 1e-6 || primary.y <= 1.0 { return source_rgb; }
    return mix(source_rgb, mask_pixelated_source_at(pos, primary.y), amount);
}
