// Scene depth: the one depth every effect reads, and the geometry derived from it.
//
// Depth is normalized relative distance (near 0, far 1) over the full image,
// whatever the crop or export tile, converted from the generated map when it is
// uploaded (scene_depth.rs). "Stored depth" here is that map before the photo refines
// it.
//
// `scene_depth_at` is depth at a pixel, edge-accurate. Stored depth is reliable on
// a surface and only there: its depth edges are blocky and misplaced at full size.
//
// On a surface, a pixel takes the map's guided filter (scene_analysis.wgsl) at the
// window's mean guide: the stored depth smoothed, so the surface's texture and shading
// print nothing into it.
//
// Around a depth edge it takes the filter at its own guide, a linear function with no
// per-pixel decision to make, so edges land on the photo's own edges without
// staircases. The filter knows only brightness, so depth stays within the range stored
// depth has around the pixel: a bright patch beside a dark object lies no farther than
// what stands there.
//
// Simpler versions failed on real photos. The filter alone prints texture into depth
// and blotches Relight on fur. A hard clamp to one cell's range, or stored depth used
// directly on surfaces, draws faint contours along the cell grid in smooth sky, where
// Rain's mist turns a depth step of 0.02 into 5 levels. Gating by one cell's range
// steps where the range opens, so the edge gate is averaged over 5 x 5 cells and the
// limit uses the 3 x 3 window. Relight alone mixes stored depth in where brightness
// does not explain depth (`scene_surface_taps_at`); direct readers must not, as that
// draws the same contours.
//
// All of it reads the scene before any edit, so an edit does not move those edges, and
// export tiles match the preview.
//
// `scene_depth_tex` holds what the map is built from and what shading needs: level
// 0 channel x is the stored depth, and with the surface uploaded the other channels
// and the mip levels hold the surface (scene_surface.rs). The texture is square, so
// its texels stretch to the image's aspect.
@group(0) @binding(35) var scene_depth_tex: texture_2d<f32>;
// Per scene map cell: the guided filter's slope and offset, the surface's roughness
// and the mean guide around it.
@group(0) @binding(71) var scene_depth_map_tex: texture_2d<f32>;
// Per scene map cell, around it: the nearest (x) and farthest (y) stored depth, how
// well the guide explains depth (z) and how much of a depth edge there is (w).
@group(0) @binding(77) var scene_depth_range_tex: texture_2d<f32>;
// Per scene map cell: how much it is ground, the distance along the ground to what
// stands on it, its smooth depth and its terrain height.
@group(0) @binding(73) var scene_ground_tex: texture_2d<f32>;
// Per depth bin: the lens blur in shorter edges.
@group(0) @binding(75) var scene_focus_tex: texture_2d<f32>;

// The distance an effect assumes until depth is generated: a restrained constant,
// as no screen-height or luminance heuristic can know the scene's geometry.
const SCENE_DEPTH_UNKNOWN: f32 = 0.35;

fn scene_depth_present() -> bool {
    return Common::scene_tone_uniforms.scene_depth_present != 0u;
}

// The four scene map cells around full-image `uv` and how far it lies between them.
struct SceneMapCells {
    base: vec2<i32>,
    next: vec2<i32>,
    between: vec2<f32>,
}

fn scene_map_cells_at(uv: vec2<f32>) -> SceneMapCells {
    let grid = vec2<i32>(Common::scene_map_grid());
    let last = grid - vec2<i32>(1);
    let position = clamp(uv * vec2<f32>(grid) - vec2<f32>(0.5), vec2<f32>(0.0), vec2<f32>(last));
    let base = vec2<i32>(floor(position));
    return SceneMapCells(base, min(base + vec2<i32>(1), last), position - vec2<f32>(base));
}

// The depth map at full-image `uv`, interpolated: see `scene_depth_map_tex`.
fn scene_depth_map_near(uv: vec2<f32>) -> vec4<f32> {
    let cells = scene_map_cells_at(uv);
    return mix(
        mix(
            textureLoad(scene_depth_map_tex, cells.base, 0),
            textureLoad(scene_depth_map_tex, vec2<i32>(cells.next.x, cells.base.y), 0),
            cells.between.x,
        ),
        mix(
            textureLoad(scene_depth_map_tex, vec2<i32>(cells.base.x, cells.next.y), 0),
            textureLoad(scene_depth_map_tex, cells.next, 0),
            cells.between.x,
        ),
        cells.between.y,
    );
}

// The stored depth's range at full-image `uv`, interpolated: see
// `scene_depth_range_tex`.
fn scene_depth_range_near(uv: vec2<f32>) -> vec4<f32> {
    let cells = scene_map_cells_at(uv);
    return mix(
        mix(
            textureLoad(scene_depth_range_tex, cells.base, 0),
            textureLoad(scene_depth_range_tex, vec2<i32>(cells.next.x, cells.base.y), 0),
            cells.between.x,
        ),
        mix(
            textureLoad(scene_depth_range_tex, vec2<i32>(cells.base.x, cells.next.y), 0),
            textureLoad(scene_depth_range_tex, cells.next, 0),
            cells.between.x,
        ),
        cells.between.y,
    );
}

fn scene_depth_map_at(pos: vec2<i32>) -> vec4<f32> {
    return scene_depth_map_near(full_image_uv(pos));
}

// Depth at a pixel from the depth map's values there. A reader that also wants
// the map's roughness or mean guide reads the map once and passes it here.
fn scene_depth_from(map: vec4<f32>, pos: vec2<i32>) -> f32 {
    let range = scene_depth_range_near(full_image_uv(pos));
    var guide = map.w;
    if range.w > 0.0 {
        guide = mix(map.w, SceneSource::scene_guide_at(pos), range.w);
    }
    return clamp(map.x * guide + map.y, range.x, range.y);
}

// Depth at a pixel, for scene_depth_present() scenes. Near 0, far 1.
fn scene_depth_at(pos: vec2<i32>) -> f32 {
    return scene_depth_from(scene_depth_map_at(pos), pos);
}

// Depth difference at which a coarser map's texel stops contributing to a pixel.
const SCENE_DEPTH_RANGE: f32 = 0.025;

// How much a texel of a coarser map, holding depth `texel_depth`, may contribute at
// a pixel of edge-accurate depth `depth`. Weighting by it keeps background values
// out of a foreground silhouette, so such maps follow the photo's edges too. It is
// smooth in both depths, so it leaves no staircase along the texel grid.
fn scene_depth_closeness(texel_depth: f32, depth: f32) -> f32 {
    let separation = (texel_depth - depth) / SCENE_DEPTH_RANGE;
    return max(exp(-separation * separation), 1e-4);
}

// The ground at full-image `uv`, without regard to edge-accurate depth.
fn scene_ground_near(uv: vec2<f32>) -> vec4<f32> {
    let cells = scene_map_cells_at(uv);
    return mix(
        mix(
            textureLoad(scene_ground_tex, cells.base, 0),
            textureLoad(scene_ground_tex, vec2<i32>(cells.next.x, cells.base.y), 0),
            cells.between.x,
        ),
        mix(
            textureLoad(scene_ground_tex, vec2<i32>(cells.base.x, cells.next.y), 0),
            textureLoad(scene_ground_tex, cells.next, 0),
            cells.between.x,
        ),
        cells.between.y,
    );
}

// The lens blur at a depth, as a Gaussian's deviation in shorter edges.
fn scene_focus_blur(depth: f32) -> f32 {
    let last = i32(Common::SCENE_FOCUS_BINS) - 1;
    let position = clamp(depth * f32(Common::SCENE_FOCUS_BINS) - 0.5, 0.0, f32(last));
    let base = i32(position);
    return mix(
        textureLoad(scene_focus_tex, vec2<i32>(base, 0), 0).x,
        textureLoad(scene_focus_tex, vec2<i32>(min(base + 1, last), 0), 0).x,
        position - f32(base),
    );
}

// The relighting surface upsampled at a pixel: the 4×4 level-0 texels around it,
// their weights and their weighted mean. Weights are a tent two texels in radius
// scaled by `scene_depth_closeness` and normalized to sum to one. The tent's
// overlap between neighbouring pixels makes transitions ramp over several pixels
// instead of snapping at texel boundaries. No depth-range mask curve applies: a
// selection is not a measurement of distance. Tap `i` is the texel at
// `base + (i % 4, i / 4)`, clamped to the grid.
const SCENE_SURFACE_TAPS: u32 = 16u;

struct SceneSurfaceTaps {
    base: vec2<i32>,
    // Row y holds the weights of the texels at `base + (0..3, y)`.
    weights: array<vec4<f32>, 4>,
    // The surface channels at the pixel (scene_surface.rs).
    surface: vec4<f32>,
}

fn scene_surface_tap_cell(taps: SceneSurfaceTaps, tap: u32) -> vec2<i32> {
    let size = vec2<i32>(textureDimensions(scene_depth_tex));
    let offset = vec2<i32>(i32(tap % 4u), i32(tap / 4u));
    return clamp(taps.base + offset, vec2<i32>(0), size - vec2<i32>(1));
}

fn scene_surface_tap_weight(taps: SceneSurfaceTaps, tap: u32) -> f32 {
    var weights = taps.weights;
    return weights[tap / 4u][tap % 4u];
}

// Depth says which surface the pixel lies on. Where brightness does not explain depth
// around the pixel, at an edge between two surfaces of one brightness or one the photo
// does not show, the filter gives a depth between them that nothing has, so stored
// depth says instead. Readers of other maps on this grid repeat the loop of
// `scene_shadow_surface_resolve` over their own texture: a texture passed to a function
// crashes the Mali-G710 shader compiler (`no_handle_arguments` test).
fn scene_surface_taps_at(pos: vec2<i32>) -> SceneSurfaceTaps {
    let size = vec2<i32>(textureDimensions(scene_depth_tex));
    let uv = full_image_uv(pos);
    let p = uv * vec2<f32>(size) - vec2<f32>(0.5);
    let stored = textureSampleLevel(scene_depth_tex, SceneAdjustments::local_mask_sampler, uv, 0.0).x;
    let depth = mix(stored, scene_depth_at(pos), scene_depth_range_near(uv).z);
    var taps = SceneSurfaceTaps(vec2<i32>(floor(p)) - vec2<i32>(1), array<vec4<f32>, 4>(), vec4<f32>(0.0));
    var weights = array<vec4<f32>, 4>();
    var surface = vec4<f32>(0.0);
    var total = 0.0;
    for (var tap = 0u; tap < SCENE_SURFACE_TAPS; tap = tap + 1u) {
        let texel = taps.base + vec2<i32>(i32(tap % 4u), i32(tap / 4u));
        let offset = abs(vec2<f32>(texel) - p);
        let tent = max(1.0 - offset.x * 0.5, 0.0) * max(1.0 - offset.y * 0.5, 0.0);
        let value = textureLoad(scene_depth_tex, scene_surface_tap_cell(taps, tap), 0);
        let weight = tent * scene_depth_closeness(value.y, depth);
        weights[tap / 4u][tap % 4u] = weight;
        surface += value * weight;
        total += weight;
    }
    // The tent is positive on at least the four nearest texels, and every
    // closeness is at least 1e-4, so `total` is positive.
    for (var row = 0u; row < 4u; row = row + 1u) {
        weights[row] /= total;
    }
    taps.weights = weights;
    taps.surface = surface / total;
    return taps;
}
