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
// depth-rework-bridge: delete in P7. The scene map's four maps are the layers
// of one rgba32float array, padded to the largest, so that the creative pass
// keeps to its sampled textures while it also binds the scene bake
// (docs/rework/DEPTH_EFFECTS_PLAN.md §4.6). Read them with
// `scene_map_layer_load`.
@group(0) @binding(71) var scene_map_layers_tex: texture_2d_array<f32>;
// Per scene map cell: the guided filter's slope and offset, the surface's roughness
// and the mean guide around it. From row Common::SCENE_MAP_SKY_ROW, the sky block:
// the brightest luminance what stands before a brighter sky around the cell may
// have (x) times y, which is 1 where it was measured, and the share of sky pixels
// around the cell (w; scene_analysis.wgsl `scene_sky_window`).
const SCENE_MAP_DEPTH_LAYER: i32 = 0;
// Per scene map cell, around it: the nearest (x) and farthest (y) stored depth, how
// well the guide explains depth (z) and how much of a depth edge there is (w).
const SCENE_MAP_RANGE_LAYER: i32 = 1;
// Per scene map cell: how much it is ground, the distance along the ground to what
// stands on it, its smooth depth and its terrain height.
const SCENE_MAP_GROUND_LAYER: i32 = 2;
// Per depth bin, along row 0: the lens blur in shorter edges.
const SCENE_MAP_FOCUS_LAYER: i32 = 3;

fn scene_map_layer_load(layer: i32, texel: vec2<i32>, level: i32) -> vec4<f32> {
    return textureLoad(scene_map_layers_tex, texel, layer, level);
}

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

// A block of the depth map, starting at `row`, at full-image `uv`, interpolated:
// see `SCENE_MAP_DEPTH_LAYER`.
fn scene_depth_map_block_near(uv: vec2<f32>, row: i32) -> vec4<f32> {
    let cells = scene_map_cells_at(uv);
    let base = cells.base + vec2<i32>(0, row);
    let next = cells.next + vec2<i32>(0, row);
    return mix(
        mix(
            scene_map_layer_load(SCENE_MAP_DEPTH_LAYER, base, 0),
            scene_map_layer_load(SCENE_MAP_DEPTH_LAYER, vec2<i32>(next.x, base.y), 0),
            cells.between.x,
        ),
        mix(
            scene_map_layer_load(SCENE_MAP_DEPTH_LAYER, vec2<i32>(base.x, next.y), 0),
            scene_map_layer_load(SCENE_MAP_DEPTH_LAYER, next, 0),
            cells.between.x,
        ),
        cells.between.y,
    );
}

// The depth map at full-image `uv`, interpolated.
fn scene_depth_map_near(uv: vec2<f32>) -> vec4<f32> {
    return scene_depth_map_block_near(uv, 0);
}

// The stored depth's range at full-image `uv`, interpolated: see
// `SCENE_MAP_RANGE_LAYER`.
fn scene_depth_range_near(uv: vec2<f32>) -> vec4<f32> {
    let cells = scene_map_cells_at(uv);
    return mix(
        mix(
            scene_map_layer_load(SCENE_MAP_RANGE_LAYER, cells.base, 0),
            scene_map_layer_load(SCENE_MAP_RANGE_LAYER, vec2<i32>(cells.next.x, cells.base.y), 0),
            cells.between.x,
        ),
        mix(
            scene_map_layer_load(SCENE_MAP_RANGE_LAYER, vec2<i32>(cells.base.x, cells.next.y), 0),
            scene_map_layer_load(SCENE_MAP_RANGE_LAYER, cells.next, 0),
            cells.between.x,
        ),
        cells.between.y,
    );
}

fn scene_depth_map_at(pos: vec2<i32>) -> vec4<f32> {
    return scene_depth_map_near(full_image_uv(pos));
}

// The stored sky at full-image `uv`: the sky texels around it (level-0 distance
// of at least Common::SCENE_SKY_DISTANCE), filtered bilinearly.
fn scene_stored_sky_at(uv: vec2<f32>) -> f32 {
    let size = vec2<f32>(textureDimensions(scene_depth_tex));
    let p = uv * size - vec2<f32>(0.5);
    let base = floor(p);
    let f = p - base;
    // Gathered at the middle of the four texels, so that the sampler's coarser
    // rounding of `uv` cannot pick other texels than `f` weighs. In the order (left,
    // lower), (right, lower), (right, upper), (left, upper), lower being the larger
    // row.
    let texels = textureGather(
        0,
        scene_depth_tex,
        SceneAdjustments::local_mask_sampler,
        (base + vec2<f32>(1.0)) / size,
    );
    let sky = select(vec4<f32>(0.0), vec4<f32>(1.0), texels >= vec4<f32>(Common::SCENE_SKY_DISTANCE));
    let weights = vec4<f32>((1.0 - f.x) * f.y, f.x * f.y, f.x * (1.0 - f.y), (1.0 - f.x) * (1.0 - f.y));
    return dot(sky, weights);
}

// Lens blur of what stands before the sky, in depth texels, over which its
// outline comes to fade (`scene_sky_at`). Below it the photo's outline is as
// sharp as stored depth's.
const SCENE_SKY_SOFT_EDGE: vec2<f32> = vec2<f32>(1.0, 3.0);
// Width of the fade in lens blurs: a blurred outline mixes over two deviations
// on either side of the photo's edge, and stored sky starts at its far side. At
// most the sky windows' reach in scene map cells. The samples of the disc the fade
// averages stored sky over (`scene_stored_sky_around`).
const SCENE_SKY_FADE_BLURS: f32 = 4.0;
const SCENE_SKY_FADE_CELLS: f32 = f32(Common::SCENE_SKY_RADIUS);
const SCENE_SKY_FADE_TAPS: u32 = 12u;
// Angle between successive samples spiralling over a disc (golden angle).
const SCENE_GOLDEN_ANGLE: f32 = 2.399963229728653;
// Offset in depth texels, along both axes, of the points `scene_stored_sky_near`
// reads: about the texels stored depth may misplace a sharp outline by.
const SCENE_SKY_NEAR_OFFSET: f32 = 1.5;

// How close stored sky is to full-image `uv`, 0 to 1: the stored sky at four
// diagonal points around it, summed and halved, so that it is whole up to about
// a texel beyond the outline and fades over the next.
fn scene_stored_sky_near(uv: vec2<f32>) -> f32 {
    let offset = SCENE_SKY_NEAR_OFFSET / vec2<f32>(textureDimensions(scene_depth_tex));
    let sky = scene_stored_sky_at(uv + offset)
        + scene_stored_sky_at(uv - offset)
        + scene_stored_sky_at(uv + vec2<f32>(offset.x, -offset.y))
        + scene_stored_sky_at(uv + vec2<f32>(-offset.x, offset.y));
    return min(0.5 * sky, 1.0);
}

// Stored sky averaged over a disc of `radius` (shorter edges) around full-image
// `uv`: a half at stored depth's outline, falling to none `radius` beyond it,
// whatever the outline's shape.
fn scene_stored_sky_around(uv: vec2<f32>, radius: f32) -> f32 {
    let image = Common::scene_image_size();
    var sum = scene_stored_sky_at(uv);
    for (var tap = 0u; tap < SCENE_SKY_FADE_TAPS; tap = tap + 1u) {
        let distance = radius * sqrt((f32(tap) + 0.5) / f32(SCENE_SKY_FADE_TAPS));
        let angle = f32(tap) * SCENE_GOLDEN_ANGLE;
        sum += scene_stored_sky_at(uv + distance * vec2<f32>(cos(angle), sin(angle)) / image);
    }
    return sum / f32(SCENE_SKY_FADE_TAPS + 1u);
}

// The sky's part of a pixel: how much of the pixel it covers (`share`, 0 to 1),
// and how bright what stands before it is relative to the whole pixel (`solid`,
// 0 to 1), as an estimate of its colour.
struct SceneSky {
    share: f32,
    solid: f32,
}

// How the lens softens the sky's outline at a pixel: the sky's share of it, and how
// far the brightness cap holds there on account of the fade (`scene_sky_at`).
struct SceneSkyFade {
    share: f32,
    near: f32,
}

// The fade at full-image `uv` for a pixel whose depth before the sky is `depth`,
// given the share of sky around it (`SCENE_MAP_DEPTH_LAYER`). Stored depth's sky
// outline is sharp. Where the lens blurred the edge, the photo mixes sky and what
// stands before it over tens of pixels, and stored sky starts where the mix is nearly
// all sky, so a sharp outline drew a line there (Relight lit those pixels by their
// sky colour up to it). Where the lens blurred the depth, the sky's share therefore
// fades out from stored depth's outline over the blur's width
// (SCENE_SKY_FADE_BLURS), smooth by construction: twice the stored sky averaged
// over a disc that wide, whole up to the outline.
fn scene_sky_fade(uv: vec2<f32>, depth: f32, covered: f32) -> SceneSkyFade {
    let stored = SceneSkyFade(scene_stored_sky_at(uv), 0.0);
    if covered <= 0.0 { return stored; }
    let image = Common::scene_image_size();
    let long = max(image.x, image.y);
    let blur = scene_focus_blur(depth);
    let soft = smoothstep(
        SCENE_SKY_SOFT_EDGE.x,
        SCENE_SKY_SOFT_EDGE.y,
        blur * f32(textureDimensions(scene_depth_tex).x) / long,
    );
    if soft <= 0.0 { return stored; }
    let cell = long / f32(Common::SCENE_MAP_GRID_LONG);
    let around = scene_stored_sky_around(
        uv,
        min(SCENE_SKY_FADE_BLURS * blur, SCENE_SKY_FADE_CELLS * cell),
    );
    return SceneSkyFade(
        max(stored.share, soft * min(2.0 * around, 1.0)),
        // Fully over the fade's inner half.
        soft * min(4.0 * around, 1.0),
    );
}

// The sky at a pixel whose depth before the sky is `depth`. The map's filters
// blend depth across the sky's outline, so readers set the sky's part to the
// farthest depth afterwards; light and shadow then never reach it, and nothing
// lies between the sky and what stands before it.
//
// Within the fade (`scene_sky_fade`) a pixel counts as no brighter than what
// stands before a brighter sky nearby (`scene_sky_window` in scene_analysis.wgsl),
// so a mostly sky-coloured mix lends no brightness to the light it receives. Where
// the lens kept the depth sharp, stored depth's outline is as sharp as the photo's
// but may lie a texel off it, so the cap holds within a texel or two of the outline
// alone.
fn scene_sky_at(pos: vec2<i32>, depth: f32) -> SceneSky {
    let uv = full_image_uv(pos);
    let block = scene_depth_map_block_near(uv, Common::SCENE_MAP_SKY_ROW);
    let fade = scene_sky_fade(uv, depth, block.w);
    var sky = SceneSky(fade.share, 1.0);
    if block.w <= 0.0 || block.y <= 0.0 { return sky; }
    var near = fade.near;
    if near < 1.0 {
        near = max(near, scene_stored_sky_near(uv));
    }
    if near > 0.0 {
        let cap = block.x / block.y;
        let guide = SceneSource::scene_guide_at(pos);
        sky.solid = mix(1.0, min(1.0, cap / max(guide * guide, 1e-12)), near);
    }
    return sky;
}

// Depth at a pixel from the depth map's values there, of what stands before the
// sky where the sky covers part of the pixel (`scene_sky_fade`).
fn scene_solid_depth_from(map: vec4<f32>, pos: vec2<i32>) -> f32 {
    let uv = full_image_uv(pos);
    let range = scene_depth_range_near(uv);
    var guide = map.w;
    if range.w > 0.0 {
        guide = mix(map.w, SceneSource::scene_guide_at(pos), range.w);
    }
    return clamp(map.x * guide + map.y, range.x, range.y);
}

// Depth at a pixel from the depth map's values there. A reader that also wants
// the map's roughness or mean guide reads the map once and passes it here.
fn scene_depth_from(map: vec4<f32>, pos: vec2<i32>) -> f32 {
    let depth = scene_solid_depth_from(map, pos);
    let uv = full_image_uv(pos);
    let covered = scene_depth_map_block_near(uv, Common::SCENE_MAP_SKY_ROW).w;
    let sky = scene_sky_fade(uv, depth, covered).share;
    if sky <= 0.0 { return depth; }
    return mix(depth, 1.0, sky);
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
            scene_map_layer_load(SCENE_MAP_GROUND_LAYER, cells.base, 0),
            scene_map_layer_load(SCENE_MAP_GROUND_LAYER, vec2<i32>(cells.next.x, cells.base.y), 0),
            cells.between.x,
        ),
        mix(
            scene_map_layer_load(SCENE_MAP_GROUND_LAYER, vec2<i32>(cells.base.x, cells.next.y), 0),
            scene_map_layer_load(SCENE_MAP_GROUND_LAYER, cells.next, 0),
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
        scene_map_layer_load(SCENE_MAP_FOCUS_LAYER, vec2<i32>(base, 0), 0).x,
        scene_map_layer_load(SCENE_MAP_FOCUS_LAYER, vec2<i32>(min(base + 1, last), 0), 0).x,
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
    // Row y holds the weights of the texels at `base + (0..3, y)`: sky texels
    // count for the sky's share of the pixel and the others for the rest.
    weights: array<vec4<f32>, 4>,
    // The surface channels at the pixel (scene_surface.rs), of what stands before
    // the sky where the sky covers part of the pixel; where it covers all, the sky:
    // farthest and flat.
    surface: vec4<f32>,
    // The sky's part of the pixel. Light on the surface covers the rest.
    sky: SceneSky,
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
    let depth = mix(
        stored,
        scene_solid_depth_from(scene_depth_map_at(pos), pos),
        scene_depth_range_near(uv).z,
    );
    let sky = scene_sky_at(pos, depth);
    var taps = SceneSurfaceTaps(
        vec2<i32>(floor(p)) - vec2<i32>(1),
        array<vec4<f32>, 4>(),
        vec4<f32>(0.0),
        sky,
    );
    var weights = array<vec4<f32>, 4>();
    var surface = vec4<f32>(0.0);
    var total = 0.0;
    var solid = vec4<f32>(0.0);
    var solid_total = 0.0;
    for (var tap = 0u; tap < SCENE_SURFACE_TAPS; tap = tap + 1u) {
        let texel = taps.base + vec2<i32>(i32(tap % 4u), i32(tap / 4u));
        let offset = abs(vec2<f32>(texel) - p);
        let tent = max(1.0 - offset.x * 0.5, 0.0) * max(1.0 - offset.y * 0.5, 0.0);
        let value = textureLoad(scene_depth_tex, scene_surface_tap_cell(taps, tap), 0);
        let is_sky = value.x >= Common::SCENE_SKY_DISTANCE;
        let near = tent * scene_depth_closeness(value.y, depth);
        // Never zero, so that `total` stays positive where the fade gives the
        // pixel wholly to the sky but none of the sixteen texels is sky.
        let share = max(select(1.0 - sky.share, sky.share, is_sky), 1e-6);
        let weight = near * share;
        weights[tap / 4u][tap % 4u] = weight;
        surface += value * weight;
        total += weight;
        if !is_sky {
            solid += value * near;
            solid_total += near;
        }
    }
    for (var row = 0u; row < 4u; row = row + 1u) {
        weights[row] /= total;
    }
    taps.weights = weights;
    if sky.share >= 1.0 {
        // Sky is the farthest depth and flat.
        taps.surface = vec4<f32>(surface.x / total, 1.0, 0.0, 0.0);
    } else {
        // The sky's share is at least the stored sky's, so below a whole share
        // one of the four nearest texels is not sky; the tent is positive there
        // and every closeness is at least 1e-4, so `solid_total` is positive.
        taps.surface = solid / solid_total;
    }
    return taps;
}
