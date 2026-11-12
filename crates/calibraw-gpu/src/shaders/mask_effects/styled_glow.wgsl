// Glow with styles: Bloom, Diffusion, Halation, Motion and Streak
// (styled_glow.rs in calibraw-core sets what the controls mean and packs
// them).
//
// Colour: everything is scene-linear Rec.2020, the creative pass's input, in
// which the glow's energy adds up physically.
//
// Source. A slot emits S = C · knee(Y) · mask, where C is the creative pass's
// input with negatives cut, Y its luminance and knee a smoothstep two stops
// wide around the Range threshold, in log2 luminance. The mask weighs the
// emitters only: the spread light reaches past it.
//
// Spread (Bloom, Diffusion, Halation). The glow's point spread function K is
// a sum of Gaussians whose
// energy per octave of deviation follows 2^(SLOPE · (u − WIDEST)) for
// u = log2 σ between FINEST and WIDEST (σ in shorter edges), normalized.
// It is evaluated on two pyramids of S, one layer per Glow slot in slot order:
// - near: built in this tile from its own pixels at half, quarter, eighth and
//   sixteenth resolution (`styled_glow_near_*`), each level's texel grid
//   aligned to the full image so every export tile reads the same values. Its
//   reach is 63 pixels at most, within the export halo (GLOW_SUPPORT in
//   calibraw-core tiles.rs).
// - far: the full-image map, S averaged over a coarse grid of the whole image
//   (`styled_glow_map_*`), then halved level by level. The pipeline that renders
//   the whole image builds it from its own creative input; an export builds it
//   in a prepass over its tiles, and zoomed views share the full frame's.
// Every level is read with a cubic B-spline, so its footprint is smooth; its
// deviation follows from the filters that built it (`styled_glow_*_deviation`).
// Each level stands for the octaves of the profile closest to its deviation
// (`styled_glow_profile_mass`), so K keeps its shape and its unit energy at
// every resolution. Without the map (a spread the near pyramid covers at this
// resolution), the widest near level takes the rest.
//
// Streaks (Motion, Streak). Built from the full-image map alone, so every
// export tile and zoomed view reads the same streaks; they are as thin as
// one cell of it. The map texture holds them in its second half of layers,
// mip 0 (`styled_glow_streak_layer`). Kawase's streak filter (DOUBLE-S.T.E.A.L,
// GDC 2003; Blender's Streaks glare) spreads the map along the direction d:
// pass n gathers 4 taps 4^n cells apart, weighted a^(j·4^n) and normalized,
// so six passes give the one-sided kernel ∝ a^t over t < 4096 cells, with
// a = e^(−cell/λ). Motion runs them once, light trailing toward d. Streak
// runs them again toward −d; the two one-sided exponentials convolve to
// exactly the two-sided e^(−|t|/λ). A Gaussian across d (Softness) runs first.
// A pass's taps are bilinear, which adds a little width off the grid's axes.
//
// Edges. Levels clamp at the tile and image edges: light beyond the frame
// continues the edge, so Diffusion does not darken the borders and streaks
// do not fade toward them.
//
// Composite, with K*S the spread light and a the STRENGTH lane:
// - Bloom:     C + a · tint · K*S
// - Diffusion: C + a · (tint · K*S − S), which moves light without adding it
//              (tint has luminance 1);
// - Halation:  C + a · tint · max(Y(K*S) − Y(S), 0), the halo beyond bright
//              edges, so a flat bright area takes no cast;
// - Motion, Streak: C + a · tint · streaks.

const STYLED_GLOW_BLOOM: f32 = 0.0;
const STYLED_GLOW_DIFFUSION: f32 = 1.0;
const STYLED_GLOW_HALATION: f32 = 2.0;
const STYLED_GLOW_MOTION: f32 = 3.0;
const STYLED_GLOW_STREAK: f32 = 4.0;
// Most taps on either side of the Gaussian across a streak; Softness 100
// needs about 3 deviations of 20 cells on a 3:2 photo.
const STYLED_GLOW_ACROSS_TAPS: i32 = 64;
// Stops of the Range knee on either side of its threshold.
const STYLED_GLOW_KNEE_STOPS: f32 = 1.0;
// A profile bound past every level (log2 shorter edges).
const STYLED_GLOW_UNBOUNDED: f32 = 64.0;

// The near pyramid's levels, one mip each from level 1 (mip 0), and the
// full-image map with its mips; one layer per Glow slot.
@group(0) @binding(118) var styled_glow_near_tex: texture_2d_array<f32>;
@group(0) @binding(119) var styled_glow_map_tex: texture_2d_array<f32>;
// The level a pyramid pass writes, the finer level it reads, and that level's
// number (x: 1 to 4 for the near pyramid). For a streak pass along d, the
// level it writes and reads are streak layers, and x is the taps' spacing in
// cells, y 1 when it gathers toward −d.
@group(0) @binding(120) var styled_glow_level_out: texture_storage_2d_array<rgba16float, write>;
@group(0) @binding(121) var styled_glow_parent_tex: texture_2d_array<f32>;
@group(0) @binding(123) var<uniform> styled_glow_level: vec4<u32>;
// The map's sums while tiles add to it: per layer and cell, the summed S and
// the pixel count.
@group(0) @binding(122) var<storage, read_write> styled_glow_cells: array<vec4<f32>>;

// --- Slots ---

// Whether the pyramids are built for this render (`effects_for_upload` in
// params.rs); until they are allocated placeholders are bound.
fn styled_glow_near_ready() -> bool {
    return Common::effects_uniforms.glow_diffusion.y > 0.5;
}

// Whether the full-image map is built and the spread reaches it.
fn styled_glow_map_ready() -> bool {
    return Common::effects_uniforms.glow_diffusion.z > 0.5;
}

// Layers the pyramids hold: the Glow slots, up to the textures' layers.
fn styled_glow_layer_count(capacity: u32) -> u32 {
    var layers = 0u;
    for (var index = 0u; index < Common::mask_slot_count(); index = index + 1u) {
        if Common::mask_slot_applies(Common::mask_data[index].metadata, MASK_EFFECT_STYLED_GLOW_ID) {
            layers = layers + 1u;
        }
    }
    return min(layers, capacity);
}

// The slot whose pyramids are `layer`: the layer-th Glow slot.
fn styled_glow_slot(layer: u32) -> u32 {
    var seen = 0u;
    for (var index = 0u; index < Common::mask_slot_count(); index = index + 1u) {
        if !Common::mask_slot_applies(Common::mask_data[index].metadata, MASK_EFFECT_STYLED_GLOW_ID) {
            continue;
        }
        if seen == layer { return index; }
        seen = seen + 1u;
    }
    return 0u;
}

// What slot `index` emits at tile pixel `pos` whose creative input is `rgb`.
fn styled_glow_source(rgb: vec3<f32>, pos: vec2<i32>, index: u32, params: MaskEffectParams) -> vec3<f32> {
    let positive = max(rgb, vec3<f32>(0.0));
    let threshold = mask_effect_lane(params, STYLED_GLOW_THRESHOLD_LANE);
    let knee = smoothstep(
        threshold - STYLED_GLOW_KNEE_STOPS,
        threshold + STYLED_GLOW_KNEE_STOPS,
        Common::log_luminance(positive),
    );
    if knee <= 0.0 { return vec3<f32>(0.0); }
    return positive * (knee * SceneAdjustments::local_mask_weight(pos, index));
}

fn styled_glow_source_at(pos: vec2<i32>, index: u32, params: MaskEffectParams) -> vec3<f32> {
    let clamped = Common::clamp_pos(pos);
    return styled_glow_source(SceneAdjustments::local_effects_at(clamped), clamped, index, params);
}

// --- Near pyramid ---

// The first texel of near level `level` in this tile, on the full image's grid
// of that level.
fn styled_glow_near_origin(level: u32) -> vec2<i32> {
    let origin = Common::tile_origin();
    return vec2<i32>(floor(vec2<f32>(origin) / f32(1u << level)));
}

// Level 1 from the creative input: per texel, the 4 × 4 pixels around its
// centre weighted 1 3 3 1 on each axis.
@compute @workgroup_size(8, 8, 1)
fn build_styled_glow_near_first(@builtin(global_invocation_id) gid: vec3<u32>) {
    let size = textureDimensions(styled_glow_level_out);
    if any(gid.xy >= size) { return; }
    let layers = styled_glow_layer_count(textureNumLayers(styled_glow_level_out));
    // Global texel g covers pixels 2g and 2g + 1; its taps are 2g - 1 to 2g + 2.
    let global = vec2<i32>(gid.xy) + styled_glow_near_origin(1u);
    let first = 2 * global - vec2<i32>(1) - Common::tile_origin();
    let weights = vec4<f32>(1.0, 3.0, 3.0, 1.0) / 8.0;
    for (var layer = 0u; layer < layers; layer = layer + 1u) {
        let index = styled_glow_slot(layer);
        let params = mask_effect_params(index);
        var sum = vec3<f32>(0.0);
        for (var y = 0; y < 4; y = y + 1) {
            for (var x = 0; x < 4; x = x + 1) {
                let source = styled_glow_source_at(first + vec2<i32>(x, y), index, params);
                sum += source * (weights[x] * weights[y]);
            }
        }
        textureStore(styled_glow_level_out, vec2<i32>(gid.xy), layer, vec4<f32>(sum, 0.0));
    }
}

// The average of the four bilinear taps ±0.75 texels around `centre` (in
// `parent`'s texel space, centres at integers), which weighs the 4 × 4 parent
// texels around it 1 3 3 1 on each axis when the centre lies between two.
fn styled_glow_halve(centre: vec2<f32>, layer: u32) -> vec4<f32> {
    let size = vec2<f32>(textureDimensions(styled_glow_parent_tex));
    var sum = vec4<f32>(0.0);
    for (var y = -1; y <= 1; y = y + 2) {
        for (var x = -1; x <= 1; x = x + 2) {
            let uv = (centre + 0.75 * vec2<f32>(f32(x), f32(y)) + vec2<f32>(0.5)) / size;
            sum += textureSampleLevel(
                styled_glow_parent_tex,
                SceneAdjustments::local_mask_sampler,
                uv,
                layer,
                0.0,
            );
        }
    }
    return 0.25 * sum;
}

// Near level `styled_glow_level.x` (2 to 4) from the one before.
@compute @workgroup_size(8, 8, 1)
fn build_styled_glow_near_level(@builtin(global_invocation_id) gid: vec3<u32>) {
    let size = textureDimensions(styled_glow_level_out);
    if any(gid.xy >= size) { return; }
    let level = styled_glow_level.x;
    let layers = styled_glow_layer_count(textureNumLayers(styled_glow_level_out));
    let global = vec2<i32>(gid.xy) + styled_glow_near_origin(level);
    // The centre of global texel g lies between parent texels 2g and 2g + 1.
    let centre = vec2<f32>(2 * global - styled_glow_near_origin(level - 1u)) + vec2<f32>(0.5);
    for (var layer = 0u; layer < layers; layer = layer + 1u) {
        textureStore(styled_glow_level_out, vec2<i32>(gid.xy), layer, styled_glow_halve(centre, layer));
    }
}

// --- Full-image map ---

// Cells of the map along the full image's longer edge, and the side of the
// square each layer takes in the sums (STYLED_GLOW_MAP_LONG in resources.rs).
const STYLED_GLOW_MAP_LONG: u32 = 768u;

fn styled_glow_map_grid() -> vec2<u32> {
    return Common::full_image_grid(STYLED_GLOW_MAP_LONG);
}

fn styled_glow_cell_index(cell: vec2<u32>, layer: u32) -> u32 {
    return (layer * STYLED_GLOW_MAP_LONG + cell.y) * STYLED_GLOW_MAP_LONG + cell.x;
}

// Adds this tile's core to the map's sums: per cell, the S of the pixels
// whose centres it covers. Export tiles add only their cores, one tile after
// another, so the sums equal the whole image's in any tiling.
@compute @workgroup_size(8, 8, 1)
fn accumulate_styled_glow_map(@builtin(global_invocation_id) gid: vec3<u32>) {
    let grid = styled_glow_map_grid();
    if any(gid.xy >= grid) { return; }
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
    let count = f32((hi.x - lo.x) * (hi.y - lo.y));
    let capacity = arrayLength(&styled_glow_cells) / (STYLED_GLOW_MAP_LONG * STYLED_GLOW_MAP_LONG);
    let layers = styled_glow_layer_count(capacity);
    for (var layer = 0u; layer < layers; layer = layer + 1u) {
        let index = styled_glow_slot(layer);
        let params = mask_effect_params(index);
        var sum = vec3<f32>(0.0);
        for (var y = lo.y; y < hi.y; y = y + 1) {
            for (var x = lo.x; x < hi.x; x = x + 1) {
                sum += styled_glow_source_at(vec2<i32>(x, y) - origin, index, params);
            }
        }
        let cell = styled_glow_cell_index(gid.xy, layer);
        styled_glow_cells[cell] = styled_glow_cells[cell] + vec4<f32>(sum, count);
    }
}

// The map's first level: each cell's mean S.
@compute @workgroup_size(8, 8, 1)
fn resolve_styled_glow_map(@builtin(global_invocation_id) gid: vec3<u32>) {
    let size = textureDimensions(styled_glow_level_out);
    if any(gid.xy >= size) { return; }
    let layers = styled_glow_layer_count(textureNumLayers(styled_glow_level_out));
    for (var layer = 0u; layer < layers; layer = layer + 1u) {
        let sums = styled_glow_cells[styled_glow_cell_index(gid.xy, layer)];
        let mean = select(vec3<f32>(0.0), sums.xyz / sums.w, sums.w > 0.0);
        textureStore(styled_glow_level_out, vec2<i32>(gid.xy), layer, vec4<f32>(mean, 0.0));
    }
}

// Map level `styled_glow_level.x` from the one before. Sizes halve and round
// down, so texel centres are placed through the levels' common uv.
@compute @workgroup_size(8, 8, 1)
fn build_styled_glow_map_level(@builtin(global_invocation_id) gid: vec3<u32>) {
    let size = textureDimensions(styled_glow_level_out);
    if any(gid.xy >= size) { return; }
    let parent = vec2<f32>(textureDimensions(styled_glow_parent_tex));
    let uv = (vec2<f32>(gid.xy) + vec2<f32>(0.5)) / vec2<f32>(size);
    let centre = uv * parent - vec2<f32>(0.5);
    let layers = styled_glow_layer_count(textureNumLayers(styled_glow_level_out));
    for (var layer = 0u; layer < layers; layer = layer + 1u) {
        textureStore(styled_glow_level_out, vec2<i32>(gid.xy), layer, styled_glow_halve(centre, layer));
    }
}

// --- Streaks ---

fn styled_glow_streaks(params: MaskEffectParams) -> bool {
    let style = mask_effect_lane(params, STYLED_GLOW_STYLE_LANE);
    return style == STYLED_GLOW_MOTION || style == STYLED_GLOW_STREAK;
}

// The map layer that holds the streaks of Glow layer `layer`: the map holds
// as many streak layers as map layers, after them.
fn styled_glow_streak_layer(layer: u32) -> u32 {
    return textureNumLayers(styled_glow_map_tex) / 2u + layer;
}

// One cell of the map along `pixels`, a direction in full-image pixels, as a
// vector in cells, and its length in full-image pixels in z. Cells are
// nearly square; this keeps directions exact where they are not.
fn styled_glow_cell_step(pixels: vec2<f32>, grid: vec2<f32>) -> vec3<f32> {
    let cell = vec2<f32>(
        f32(max(Common::camera_uniforms.full_width, 1u)),
        f32(max(Common::camera_uniforms.full_height, 1u)),
    ) / grid;
    let unit = normalize(pixels / cell);
    return vec3<f32>(unit, length(unit * cell));
}

fn styled_glow_streak_direction(params: MaskEffectParams) -> vec2<f32> {
    let angle = mask_effect_lane(params, STYLED_GLOW_DIRECTION_LANE);
    return vec2<f32>(cos(angle), sin(angle));
}

// `parent` at cell `cell` (centres at integers), bilinear.
fn styled_glow_parent_at(cell: vec2<f32>, layer: u32) -> vec4<f32> {
    let size = vec2<f32>(textureDimensions(styled_glow_parent_tex));
    return textureSampleLevel(
        styled_glow_parent_tex,
        SceneAdjustments::local_mask_sampler,
        (cell + vec2<f32>(0.5)) / size,
        layer,
        0.0,
    );
}

// The first streak pass: the map's first level blurred across each streak
// slot's direction by a Gaussian of the SOFTNESS deviation, into its streak
// layer. Reads map layers and writes streak layers, one per Glow slot.
@compute @workgroup_size(8, 8, 1)
fn blur_styled_glow_streak_across(@builtin(global_invocation_id) gid: vec3<u32>) {
    let size = textureDimensions(styled_glow_level_out);
    if any(gid.xy >= size) { return; }
    let grid = vec2<f32>(size);
    let short_edge = f32(max(
        min(Common::camera_uniforms.full_width, Common::camera_uniforms.full_height),
        1u,
    ));
    let centre = vec2<f32>(gid.xy);
    let layers = styled_glow_layer_count(textureNumLayers(styled_glow_level_out));
    for (var layer = 0u; layer < layers; layer = layer + 1u) {
        let params = mask_effect_params(styled_glow_slot(layer));
        if !styled_glow_streaks(params) { continue; }
        let along = styled_glow_streak_direction(params);
        let cell_step = styled_glow_cell_step(vec2<f32>(-along.y, along.x), grid);
        let deviation = mask_effect_lane(params, STYLED_GLOW_SOFTNESS_LANE) * short_edge / cell_step.z;
        var sum = vec4<f32>(0.0);
        if deviation < 0.25 {
            sum = styled_glow_parent_at(centre, layer);
        } else {
            let reach = min(i32(ceil(3.0 * deviation)), STYLED_GLOW_ACROSS_TAPS);
            var total = 0.0;
            for (var tap = -reach; tap <= reach; tap = tap + 1) {
                let offset = f32(tap);
                let weight = exp(-0.5 * offset * offset / (deviation * deviation));
                sum += weight * styled_glow_parent_at(centre + offset * cell_step.xy, layer);
                total += weight;
            }
            sum /= total;
        }
        textureStore(styled_glow_level_out, vec2<i32>(gid.xy), layer, sum);
    }
}

// One pass of the streak filter: per streak slot, 4 taps `styled_glow_level.x`
// cells apart from each cell, back along d (light trails toward d) or, when
// `styled_glow_level.y` is 1, forward along it, weighted a^(j·spacing) and
// normalized. Motion slots take no pass toward −d and are copied.
@compute @workgroup_size(8, 8, 1)
fn build_styled_glow_streak(@builtin(global_invocation_id) gid: vec3<u32>) {
    let size = textureDimensions(styled_glow_level_out);
    if any(gid.xy >= size) { return; }
    let grid = vec2<f32>(size);
    let short_edge = f32(max(
        min(Common::camera_uniforms.full_width, Common::camera_uniforms.full_height),
        1u,
    ));
    let spacing = f32(styled_glow_level.x);
    let backward = styled_glow_level.y == 1u;
    let centre = vec2<f32>(gid.xy);
    let layers = styled_glow_layer_count(textureNumLayers(styled_glow_level_out));
    for (var layer = 0u; layer < layers; layer = layer + 1u) {
        let params = mask_effect_params(styled_glow_slot(layer));
        if !styled_glow_streaks(params) { continue; }
        let style = mask_effect_lane(params, STYLED_GLOW_STYLE_LANE);
        if backward && style == STYLED_GLOW_MOTION {
            textureStore(styled_glow_level_out, vec2<i32>(gid.xy), layer, styled_glow_parent_at(centre, layer));
            continue;
        }
        let cell_step = styled_glow_cell_step(styled_glow_streak_direction(params), grid);
        let streak_length = mask_effect_lane(params, STYLED_GLOW_LENGTH_LANE) * short_edge;
        // ln a per tap: the pixels between taps over the length λ.
        let decay = -spacing * cell_step.z / max(streak_length, 1e-3);
        let toward = select(-cell_step.xy, cell_step.xy, backward) * spacing;
        var sum = vec4<f32>(0.0);
        var total = 0.0;
        for (var tap = 0; tap < 4; tap = tap + 1) {
            let weight = exp(decay * f32(tap));
            sum += weight * styled_glow_parent_at(centre + f32(tap) * toward, layer);
            total += weight;
        }
        textureStore(styled_glow_level_out, vec2<i32>(gid.xy), layer, sum / total);
    }
}

// --- Reading the levels ---

// A cubic B-spline read as four bilinear taps (Sigg and Hadwiger): the taps'
// uv corners and the weights of their columns and rows. Textures are read
// where they are declared, never passed to a function (`no_handle_arguments`).
struct StyledGlowTaps {
    low: vec2<f32>,
    high: vec2<f32>,
    low_weight: vec2<f32>,
    high_weight: vec2<f32>,
}

// The taps at `coord` of a level `size` texels large, in its texel space with
// centres at integers.
fn styled_glow_bspline_taps(coord: vec2<f32>, size: vec2<f32>) -> StyledGlowTaps {
    let base = floor(coord);
    let f = coord - base;
    let f2 = f * f;
    let f3 = f2 * f;
    let w0 = (1.0 - 3.0 * f + 3.0 * f2 - f3) / 6.0;
    let w1 = (4.0 - 6.0 * f2 + 3.0 * f3) / 6.0;
    let w2 = (1.0 + 3.0 * f + 3.0 * f2 - 3.0 * f3) / 6.0;
    let w3 = f3 / 6.0;
    let g0 = w0 + w1;
    let g1 = w2 + w3;
    return StyledGlowTaps(
        (base - vec2<f32>(1.0) + w1 / g0 + vec2<f32>(0.5)) / size,
        (base + vec2<f32>(1.0) + w3 / g1 + vec2<f32>(0.5)) / size,
        g0,
        g1,
    );
}

// Near level `level` at tile pixel `pos`.
fn styled_glow_near_at(pos: vec2<i32>, layer: u32, level: u32) -> vec3<f32> {
    let mip = level - 1u;
    let global = vec2<f32>(pos + Common::tile_origin()) + vec2<f32>(0.5);
    let coord = global / f32(1u << level) - vec2<f32>(0.5)
        - vec2<f32>(styled_glow_near_origin(level));
    let taps = styled_glow_bspline_taps(coord, vec2<f32>(textureDimensions(styled_glow_near_tex, mip)));
    let sampler_ = SceneAdjustments::local_mask_sampler;
    let lod = f32(mip);
    let low_row = textureSampleLevel(styled_glow_near_tex, sampler_, taps.low, layer, lod).rgb * taps.low_weight.x
        + textureSampleLevel(styled_glow_near_tex, sampler_, vec2<f32>(taps.high.x, taps.low.y), layer, lod).rgb * taps.high_weight.x;
    let high_row = textureSampleLevel(styled_glow_near_tex, sampler_, vec2<f32>(taps.low.x, taps.high.y), layer, lod).rgb * taps.low_weight.x
        + textureSampleLevel(styled_glow_near_tex, sampler_, taps.high, layer, lod).rgb * taps.high_weight.x;
    return low_row * taps.low_weight.y + high_row * taps.high_weight.y;
}

// Map mip `mip` at tile pixel `pos`.
fn styled_glow_map_at(pos: vec2<i32>, layer: u32, mip: u32) -> vec3<f32> {
    let size = vec2<f32>(textureDimensions(styled_glow_map_tex, mip));
    let taps = styled_glow_bspline_taps(full_image_uv(pos) * size - vec2<f32>(0.5), size);
    let sampler_ = SceneAdjustments::local_mask_sampler;
    let lod = f32(mip);
    let low_row = textureSampleLevel(styled_glow_map_tex, sampler_, taps.low, layer, lod).rgb * taps.low_weight.x
        + textureSampleLevel(styled_glow_map_tex, sampler_, vec2<f32>(taps.high.x, taps.low.y), layer, lod).rgb * taps.high_weight.x;
    let high_row = textureSampleLevel(styled_glow_map_tex, sampler_, vec2<f32>(taps.low.x, taps.high.y), layer, lod).rgb * taps.low_weight.x
        + textureSampleLevel(styled_glow_map_tex, sampler_, taps.high, layer, lod).rgb * taps.high_weight.x;
    return low_row * taps.low_weight.y + high_row * taps.high_weight.y;
}

// log2 of the deviation of near level `level`, in shorter edges: the 1 3 3 1
// filters of the levels before it (variance 3/4 of a parent texel each) and
// the B-spline read (1/3 of a texel).
fn styled_glow_near_deviation(level: u32) -> f32 {
    let texels = f32(1u << (2u * level));
    let variance = 0.25 * (texels - 1.0) + texels / 3.0;
    return 0.5 * log2(variance) - log2(Common::processing_short_edge());
}

// log2 of the deviation of map mip `mip`, in shorter edges: the cell's box
// (1/12 of a cell), the levels' filters and the B-spline read.
fn styled_glow_map_deviation(mip: u32) -> f32 {
    let cells = f32(1u << (2u * mip));
    let variance = 1.0 / 12.0 + 0.25 * (cells - 1.0) + cells / 3.0;
    let grid = vec2<f32>(textureDimensions(styled_glow_map_tex));
    return 0.5 * log2(variance) - log2(min(grid.x, grid.y));
}

// The profile's energy below `u`, unnormalized: 2^(slope (u − widest)) per
// octave from `finest` to `widest`.
fn styled_glow_profile_below(u: f32, finest: f32, widest: f32, slope: f32) -> f32 {
    let clamped = clamp(u, finest, widest);
    if slope < 1e-4 { return clamped - finest; }
    return (exp2(slope * (clamped - widest)) - exp2(slope * (finest - widest))) / (slope * log(2.0));
}

// The share of the profile between log2 deviations `lower` and `upper`.
fn styled_glow_profile_mass(lower: f32, upper: f32, params: MaskEffectParams) -> f32 {
    let finest = mask_effect_lane(params, STYLED_GLOW_FINEST_LANE);
    let widest = mask_effect_lane(params, STYLED_GLOW_WIDEST_LANE);
    let slope = mask_effect_lane(params, STYLED_GLOW_SLOPE_LANE);
    let total = styled_glow_profile_below(widest, finest, widest, slope);
    if total <= 0.0 { return 0.0; }
    return max(
        styled_glow_profile_below(upper, finest, widest, slope)
            - styled_glow_profile_below(lower, finest, widest, slope),
        0.0,
    ) / total;
}

// K*S of `layer` at `pos`: every level weighed by the profile's energy over
// the octaves nearest its deviation, the near levels and then the map's mips
// wider than the widest near level.
fn styled_glow_spread_at(pos: vec2<i32>, layer: u32, params: MaskEffectParams) -> vec3<f32> {
    let near_levels = textureNumLevels(styled_glow_near_tex);
    let map_levels = select(0u, textureNumLevels(styled_glow_map_tex), styled_glow_map_ready());
    let near_top = styled_glow_near_deviation(near_levels);
    // The levels in order of deviation: near 1 to near_levels, then the map's
    // mips above near_top. `next` is the deviation of the level after the
    // current one, or unbounded.
    var spread = vec3<f32>(0.0);
    var lower = -STYLED_GLOW_UNBOUNDED;
    var mip = 0u;
    while mip < map_levels && styled_glow_map_deviation(mip) <= near_top {
        mip = mip + 1u;
    }
    for (var level = 1u; level <= near_levels; level = level + 1u) {
        let deviation = styled_glow_near_deviation(level);
        var next = STYLED_GLOW_UNBOUNDED;
        if level < near_levels {
            next = styled_glow_near_deviation(level + 1u);
        } else if mip < map_levels {
            next = styled_glow_map_deviation(mip);
        }
        let upper = select(0.5 * (deviation + next), STYLED_GLOW_UNBOUNDED, next >= STYLED_GLOW_UNBOUNDED);
        let weight = styled_glow_profile_mass(lower, upper, params);
        if weight > 1e-5 {
            spread += weight * styled_glow_near_at(pos, layer, level);
        }
        lower = upper;
    }
    for (; mip < map_levels; mip = mip + 1u) {
        let deviation = styled_glow_map_deviation(mip);
        var upper = STYLED_GLOW_UNBOUNDED;
        if mip + 1u < map_levels {
            upper = 0.5 * (deviation + styled_glow_map_deviation(mip + 1u));
        }
        let weight = styled_glow_profile_mass(lower, upper, params);
        if weight > 1e-5 {
            spread += weight * styled_glow_map_at(pos, layer, mip);
        }
        lower = upper;
    }
    return spread;
}

// --- Composite ---

// Adds the light of every Glow slot to `rgb` at tile pixel `pos`. Runs after
// the per-slot creative effects and reads their common input, so its result
// does not depend on the order of the cards.
fn apply_styled_glow(pos: vec2<i32>, rgb: vec3<f32>) -> vec3<f32> {
    if !styled_glow_near_ready() { return rgb; }
    let layers = styled_glow_layer_count(textureNumLayers(styled_glow_near_tex));
    if layers == 0u { return rgb; }
    let input = SceneAdjustments::local_effects_at(pos);
    var result = rgb;
    for (var layer = 0u; layer < layers; layer = layer + 1u) {
        let index = styled_glow_slot(layer);
        let params = mask_effect_params(index);
        let strength = mask_effect_lane(params, STYLED_GLOW_STRENGTH_LANE);
        if strength <= 0.0 { continue; }
        let tint = mask_effect_color(params, STYLED_GLOW_TINT_LANE);
        if styled_glow_streaks(params) {
            if styled_glow_map_ready() {
                let streaks = styled_glow_map_at(pos, styled_glow_streak_layer(layer), 0u);
                result += strength * tint * streaks;
            }
            continue;
        }
        let source = styled_glow_source(input, pos, index, params);
        let spread = styled_glow_spread_at(pos, layer, params);
        let style = mask_effect_lane(params, STYLED_GLOW_STYLE_LANE);
        if style == STYLED_GLOW_BLOOM {
            result += strength * tint * spread;
        } else if style == STYLED_GLOW_DIFFUSION {
            result += strength * (tint * spread - source);
        } else {
            let halo = max(dot(spread, Common::LUMA) - dot(source, Common::LUMA), 0.0);
            result += strength * halo * tint;
        }
    }
    return result;
}
