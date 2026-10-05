// Pixelate replaces each pixel with the mean of its block on a global integer
// grid (block size in pixels, 1..=PIXELATE_MAX_BLOCK).
//
// Large blocks are averaged once per block by `prepare_pixelate_blocks` instead
// of once per pixel, which would cost up to 96x96 loads per pixel. Cached means
// live in the pixelate block cache texture (binding 36/37, a scratch texture of
// the image size and work format). Each block size from
// PIXELATE_CACHED_MIN_BLOCK up owns a fixed region of `pixelate_cache_stride()`
// texels in row-major order, so a size's region needs no table of offsets.
// Inside a region, blocks are stored row-major starting at the block that
// contains the texture's first pixel. Sizes whose region does not fit in the
// texture (only very thin images) and small blocks are averaged in place.
const PIXELATE_MAX_BLOCK: i32 = 96;
// Smaller blocks cost at most 11x11 loads per pixel when averaged in place.
const PIXELATE_CACHED_MIN_BLOCK: i32 = 12;

fn pixelate_block_size(reference_block_size: f32) -> i32 {
    return SceneAdjustments::presence_step(reference_block_size, PIXELATE_MAX_BLOCK);
}

// Global block grid cells that intersect this texture. A texture edge that does
// not fall on the grid adds one partial cell per axis.
fn pixelate_grid_size(block_size: i32) -> vec2<i32> {
    let extent = vec2<i32>(
        i32(Common::camera_uniforms.width),
        i32(Common::camera_uniforms.height),
    );
    return (extent - vec2<i32>(1)) / vec2<i32>(block_size) + vec2<i32>(2);
}

// The global cell containing the texture's first (clamped) pixel.
fn pixelate_first_cell(block_size: i32) -> vec2<i32> {
    let first = clamp(Common::tile_origin(), vec2<i32>(0), Common::full_image_max());
    return first / vec2<i32>(block_size);
}

// Texels per cached block size: the grid of the smallest cached size bounds
// every larger size's grid.
fn pixelate_cache_stride() -> i32 {
    let grid = pixelate_grid_size(PIXELATE_CACHED_MIN_BLOCK);
    return grid.x * grid.y;
}

// First texel index of `block_size`'s cache region, or -1 when the size is
// averaged in place.
fn pixelate_cache_offset(block_size: i32) -> i32 {
    if block_size < PIXELATE_CACHED_MIN_BLOCK { return -1; }
    let stride = pixelate_cache_stride();
    let offset = (block_size - PIXELATE_CACHED_MIN_BLOCK) * stride;
    let capacity = i32(Common::camera_uniforms.width) * i32(Common::camera_uniforms.height);
    if offset + stride > capacity { return -1; }
    return offset;
}

fn pixelate_cache_texel(index: i32) -> vec2<i32> {
    let width = i32(Common::camera_uniforms.width);
    return vec2<i32>(index % width, index / width);
}

// Mean of the block whose global top-left pixel is `cell_min`.
fn pixelate_cell_average(cell_min: vec2<i32>, block_size: i32) -> vec3<f32> {
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

fn pixelate_is_active(primary: vec4<f32>) -> bool {
    return clamp(primary.x / 100.0, 0.0, 1.0) > 1e-6 && primary.y > 1.0;
}

fn pixelate_block_size_in_use(block_size: i32) -> bool {
    let count = min(Common::scene_tone_uniforms.mask_counts.x, Common::MAX_RENDER_MASK_SLOTS);
    for (var index = 0u; index < count; index = index + 1u) {
        let state = Common::mask_data[index].metadata;
        if state.x == 0u || state.y == 0u || Common::mask_effect_id(state) != MASK_EFFECT_PIXELATE_ID {
            continue;
        }
        let primary = Common::mask_data[index].adjust_0_field;
        if pixelate_is_active(primary) && pixelate_block_size(primary.y) == block_size {
            return true;
        }
    }
    return false;
}

// One invocation per cache texel: each computes one block mean of a block size
// that an active Pixelate effect uses. Reads the creative pass's input.
@compute @workgroup_size(8, 8, 1)
fn prepare_pixelate_blocks(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= Common::camera_uniforms.width || gid.y >= Common::camera_uniforms.height { return; }
    let index = i32(gid.y) * i32(Common::camera_uniforms.width) + i32(gid.x);
    let stride = pixelate_cache_stride();
    let block_size = PIXELATE_CACHED_MIN_BLOCK + index / stride;
    if block_size > PIXELATE_MAX_BLOCK || pixelate_cache_offset(block_size) < 0 { return; }
    let grid = pixelate_grid_size(block_size);
    let local = index % stride;
    if local >= grid.x * grid.y { return; }
    let cell_min = (pixelate_first_cell(block_size) + vec2<i32>(local % grid.x, local / grid.x))
        * block_size;
    // The last cell per axis is outside the image when the texture ends on the grid.
    if any(cell_min > Common::full_image_max()) { return; }
    if !pixelate_block_size_in_use(block_size) { return; }
    textureStore(
        SceneAdjustments::pixelate_blocks_out,
        vec2<i32>(gid.xy),
        vec4<f32>(pixelate_cell_average(cell_min, block_size), 1.0),
    );
}

fn mask_pixelated_source_at(pos: vec2<i32>, reference_block_size: f32) -> vec3<f32> {
    let block_size = pixelate_block_size(reference_block_size);
    let global_pos = clamp(pos + Common::tile_origin(), vec2<i32>(0), Common::full_image_max());
    let cell = global_pos / vec2<i32>(block_size);
    let offset = pixelate_cache_offset(block_size);
    if offset < 0 {
        return pixelate_cell_average(cell * block_size, block_size);
    }
    let local = cell - pixelate_first_cell(block_size);
    let index = offset + local.y * pixelate_grid_size(block_size).x + local.x;
    return textureLoad(SceneAdjustments::pixelate_blocks_tex, pixelate_cache_texel(index), 0).xyz;
}

fn apply_pixelate(
    pos: vec2<i32>,
    source_rgb: vec3<f32>,
    primary: vec4<f32>,
) -> vec3<f32> {
    if !pixelate_is_active(primary) { return source_rgb; }
    let amount = clamp(primary.x / 100.0, 0.0, 1.0);
    return mix(source_rgb, mask_pixelated_source_at(pos, primary.y), amount);
}
