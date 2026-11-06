// Scene shadows: how much of each scene light (scene_lights.wgsl) reaches a
// point, past the surfaces that scene depth describes, so that a light hidden
// behind a subject lights neither the air nor the surfaces in front of it.
// Receivers ask through `scene_shadow_air_at` for a point in the air (drops,
// Fog, Rain's mist) and `scene_shadow_surface_at` for a point on the pixel's
// surface (Smoke, splashes, wet ground), and pick their light's channel with
// `scene_shadow_of`. Relight's own shading reads the surface half with its
// Shadows strength; every other receiver is always shadowed.
//
// The shadow map. Shadows depend only on scene depth and the lights, so they
// are traced once into a full-image map, for up to four lights (one per
// channel; `SceneLight::channel`), instead of per image pixel. Lights beyond
// the four reach every receiver unshadowed, and Relight traces its own shading
// per pixel. The map is shared by export tiles and rebuilt only when depth or a
// light's position, size or relief changes (encode_scene_shadow_map in
// gpu.rs). It holds two halves of the level-0 scene-depth grid's size:
//
// - The surface (rows [0, n)): one texel per level-0 scene-depth texel, traced
//   from its own smooth depth. Pixels read it with the same taps as the surface
//   (`scene_surface_taps_at`), so shadows follow the photo's edges along with
//   depth.
// - The air (the rows below): SCENE_SHADOW_SLICES slices of normalized depth
//   across the frame, each a tile of 1 / SCENE_SHADOW_TILES of the grid's
//   edge, SCENE_SHADOW_TILES to a row, read with trilinear filtering. What lies
//   in the air (drops, mist) is soft, so the coarse grid suffices; it keeps the
//   map quick to rebuild while a light moves, and filtering it costs two reads.
//   The map's height is `scene_shadow_map_height` in resources.rs.
@group(0) @binding(46) var scene_shadow_map: texture_2d<f32>;
@group(0) @binding(47) var scene_shadow_map_out: texture_storage_2d<rgba16float, write>;

// Lights the shadow map holds, one per channel.
const SCENE_SHADOW_MAP_CHANNELS: u32 = 4u;
// Air slices, in rows of SCENE_SHADOW_TILES tiles of the grid's edge divided
// by SCENE_SHADOW_TILES.
const SCENE_SHADOW_SLICES: u32 = 16u;
const SCENE_SHADOW_TILES: u32 = 8u;
// Slices in the open air before the nearest surface, spaced evenly down to
// -0.9; the rest are spaced evenly from the nearest surface (0) to the
// farthest (1), as the medium's cells are (MEDIUM_CELLS).
const SCENE_SHADOW_NEAR_SLICES: f32 = 3.0;
const SCENE_SHADOW_NEAR_SPACING: f32 = 0.3;
// Steps of the march from a point in the air to a light, and the growth of its
// penumbra per unit of travel. Fewer steps skip over thin occluders (a roof's
// edge, overhead wires) at some voxels and not at their neighbours, which shows
// as blotches in lit mist. The air has a sixteenth of the surface's texels, so
// it stays cheaper to trace than the surface.
const SCENE_SHADOW_AIR_STEPS: f32 = 64.0;
const SCENE_SHADOW_AIR_PENUMBRA: f32 = 0.1;
// Normalized depth before the nearest surface around a voxel from which a point
// of the air on or behind it traces. Such points lie hidden behind the surface,
// and receivers read them only by filtering between voxels; traced where they
// are, they would be shadowed by the surface itself, and depth noise along it
// would shadow alternate voxels.
const SCENE_SHADOW_SURFACE_MARGIN: f32 = 0.02;
// A surface texel whose 3×3 neighbourhood lies at least this deep is sky or far
// scenery, and no light reaches it. A pixel beside it that reads it nonetheless
// counts as unshadowed, on the fading edge of the light's reach.
const SCENE_SHADOW_UNLIT_TEXEL_DEPTH: f32 = RELIGHT_REACH_LIMIT + 0.001;

// The shadow-map channel of a scene light's slot plus one, or zero when the map
// holds no channel for it (`assign_scene_light_channels`).
fn scene_shadow_channel(params: MaskEffectParams) -> u32 {
    let option = params.options[SCENE_LIGHT_CHANNEL_OPTION];
    return u32(clamp(option, 0.0, f32(SCENE_SHADOW_MAP_CHANNELS)) + 0.5);
}

// The normalized depth of air slice `slice`, and the inverse: the slice,
// fractional, at `depth`.
fn scene_shadow_slice_depth(slice: f32) -> f32 {
    let from_nearest = slice - SCENE_SHADOW_NEAR_SLICES;
    if from_nearest < 0.0 { return from_nearest * SCENE_SHADOW_NEAR_SPACING; }
    return from_nearest / (f32(SCENE_SHADOW_SLICES) - 1.0 - SCENE_SHADOW_NEAR_SLICES);
}

fn scene_shadow_slice_at(depth: f32) -> f32 {
    if depth < 0.0 {
        return max(SCENE_SHADOW_NEAR_SLICES + depth / SCENE_SHADOW_NEAR_SPACING, 0.0);
    }
    let far_slices = f32(SCENE_SHADOW_SLICES) - 1.0 - SCENE_SHADOW_NEAR_SLICES;
    return min(SCENE_SHADOW_NEAR_SLICES + depth * far_slices, f32(SCENE_SHADOW_SLICES) - 1.0);
}

// How much of `light` reaches `position`, a point in the air, with Relight's
// occlusion test (`relight_blocked`, which takes an occluder to be solid for a
// thickness proportional to its distance). Unlike Relight's march from a
// surface (`relight_visibility`), it also runs when the light lies straight
// behind the point in the image, which is where a forward-scattering drop
// shows it most. Sky and far scenery beyond Relight's reach limit cast no
// shadow, so a light placed there (Light Rays) is not hidden by the sky around
// it. A point on a surface leaves its own texels, which the surface's shading
// accounts for (`on_surface`). `jitter` in [0, 1) offsets the steps.
fn scene_shadow_march(light: SceneLight, position: vec3<f32>, on_surface: bool, jitter: f32) -> f32 {
    let camera = light.camera;
    let texel = relight_texel_extent(camera);
    let path_texels = length(scene_image_point(camera, light.position)
        - scene_image_point(camera, position)) / texel;
    let footprint = max(2.0 * path_texels / SCENE_SHADOW_AIR_STEPS, RELIGHT_MIN_SHADOW_FOOTPRINT);
    let top_level = f32(textureNumLevels(scene_depth_tex)) - 1.0;
    let level = i32(clamp(round(log2(footprint) - 1.0), 0.0, top_level));
    let path_length = length(light.position - position);
    let far = scene_depth_z(camera, RELIGHT_REACH_LIMIT);
    let first = select(0.0, min(1.5 / max(path_texels, 1e-4), 0.5), on_surface);
    var visibility = 1.0;
    for (var index = 0.0; index < SCENE_SHADOW_AIR_STEPS; index = index + 1.0) {
        let t = mix(first, 1.0, (index + jitter) / SCENE_SHADOW_AIR_STEPS);
        let along = mix(position, light.position, t);
        if along.z <= 0.02 { break; }
        if along.z >= far { continue; }
        let uv = scene_image_point(camera, along) / camera.image_size;
        // Nothing is known beyond the frame, and the path does not return to it.
        if any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) { break; }
        let width = 0.01 * along.z + path_length * t * SCENE_SHADOW_AIR_PENUMBRA;
        if relight_step_unoccluded(camera, along.z, width, uv, vec2<f32>(0.0), level) { continue; }
        visibility = min(visibility, 1.0 - relight_blocked(camera, along, uv, level, width));
        if visibility <= 0.0 { break; }
    }
    return visibility;
}

// Whether the surface texel `cell` needs no tracing: its whole 3×3
// neighbourhood, clamped to the grid as the resolve taps are, lies beyond the
// reach limit (SCENE_SHADOW_UNLIT_TEXEL_DEPTH). Sky is often a large share of a
// photo, and its shadows would otherwise be traced for nothing.
fn scene_shadow_texel_unlit(cell: vec2<i32>, size: vec2<i32>) -> bool {
    for (var y = -1; y <= 1; y = y + 1) {
        for (var x = -1; x <= 1; x = x + 1) {
            let neighbour = clamp(cell + vec2<i32>(x, y), vec2<i32>(0), size - vec2<i32>(1));
            if textureLoad(scene_depth_tex, neighbour, 0).y < SCENE_SHADOW_UNLIT_TEXEL_DEPTH {
                return false;
            }
        }
    }
    return true;
}

// A point of the air on or behind the nearest surface around its voxel traces
// from just before that surface instead (`scene_shadow_air_depth`).
fn scene_shadow_air_depth(voxel: vec2<i32>, depth: f32) -> f32 {
    // The scene-depth level whose texels are the voxels; above level 0 channel
    // x holds the nearest depth of each texel's footprint (scene_surface.rs).
    let level = i32(firstTrailingBit(SCENE_SHADOW_TILES));
    let size = relight_depth_level_size(level);
    var nearest = depth + SCENE_SHADOW_SURFACE_MARGIN;
    for (var y = -1; y <= 1; y = y + 1) {
        for (var x = -1; x <= 1; x = x + 1) {
            let cell = clamp(voxel + vec2<i32>(x, y), vec2<i32>(0), size - vec2<i32>(1));
            nearest = min(nearest, textureLoad(scene_depth_tex, cell, level).x);
        }
    }
    return nearest - SCENE_SHADOW_SURFACE_MARGIN;
}

// Each texel traces from its own point. Surface texels dither with the jitter
// of the full-image pixel at their centre; voxels do not, as a voxel spans many
// pixels and its neighbours would differ in blocks. On the surface a Relight
// light traces as its shading does, with its own size; everything else marches
// through the air (`scene_shadow_march`).
@compute @workgroup_size(8, 8, 1)
fn build_scene_shadow_map(@builtin(global_invocation_id) gid: vec3<u32>) {
    let size = textureDimensions(scene_depth_tex);
    if any(gid.xy >= textureDimensions(scene_shadow_map_out)) { return; }
    let cell = vec2<i32>(gid.xy);
    let on_surface = gid.y < size.y;
    var uv: vec2<f32>;
    var depth: f32;
    if on_surface {
        if scene_shadow_texel_unlit(cell, vec2<i32>(size)) {
            textureStore(scene_shadow_map_out, cell, vec4<f32>(1.0));
            return;
        }
        uv = (vec2<f32>(gid.xy) + vec2<f32>(0.5)) / vec2<f32>(size);
        depth = textureLoad(scene_depth_tex, cell, 0).y;
    } else {
        let tile = size / SCENE_SHADOW_TILES;
        let air = vec2<u32>(gid.x, gid.y - size.y);
        let slice = (air.y / tile.y) * SCENE_SHADOW_TILES + air.x / tile.x;
        uv = (vec2<f32>(air % tile) + vec2<f32>(0.5)) / vec2<f32>(tile);
        depth = scene_shadow_air_depth(
            vec2<i32>(air % tile), scene_shadow_slice_depth(f32(slice)),
        );
    }
    let jitter = select(0.5, relight_jitter(uv), on_surface);
    var visibility = vec4<f32>(1.0);
    for (var index = 0u; index < Common::mask_slot_count(); index = index + 1u) {
        let params = mask_effect_params(index);
        let channel = scene_shadow_channel(params);
        if channel == 0u { continue; }
        let light = scene_light_at(index);
        if !light.emits { continue; }
        let camera = light.camera;
        let position = scene_camera_point(camera, uv * camera.image_size, scene_depth_z(camera, depth));
        let is_relight = Common::mask_effect_id(Common::mask_data[index].metadata) == MASK_EFFECT_RELIGHT_ID;
        if on_surface && is_relight {
            visibility[channel - 1u] = relight_visibility(
                camera, position, light.position, relight_size(params), jitter,
            );
        } else {
            visibility[channel - 1u] = scene_shadow_march(light, position, on_surface, jitter);
        }
    }
    textureStore(scene_shadow_map_out, cell, visibility);
}

// The surface half upsampled with the surface's `taps`, one channel per light.
fn scene_shadow_surface_resolve(taps: SceneSurfaceTaps) -> vec4<f32> {
    var total = vec4<f32>(0.0);
    for (var tap = 0u; tap < SCENE_SURFACE_TAPS; tap = tap + 1u) {
        total += textureLoad(scene_shadow_map, scene_surface_tap_cell(taps, tap), 0)
            * scene_surface_tap_weight(taps, tap);
    }
    return total;
}

// The shadows on the surface pixel `pos` lies on, one channel per light; 1
// without scene depth.
fn scene_shadow_surface_at(pos: vec2<i32>) -> vec4<f32> {
    if !scene_depth_present() { return vec4<f32>(1.0); }
    return scene_shadow_surface_resolve(scene_surface_taps_at(pos));
}

// Air slice `slice` at full-image `uv`, filtered within its tile.
fn scene_shadow_air_slice(uv: vec2<f32>, slice: u32) -> vec4<f32> {
    let size = vec2<f32>(textureDimensions(scene_depth_tex));
    let tile = size / f32(SCENE_SHADOW_TILES);
    // Keep the filter inside the tile.
    let inside = clamp(uv * tile, vec2<f32>(0.5), tile - vec2<f32>(0.5));
    let origin = vec2<f32>(f32(slice % SCENE_SHADOW_TILES), f32(slice / SCENE_SHADOW_TILES)) * tile
        + vec2<f32>(0.0, size.y);
    return textureSampleLevel(
        scene_shadow_map,
        SceneAdjustments::local_mask_sampler,
        (origin + inside) / vec2<f32>(textureDimensions(scene_shadow_map)),
        0.0,
    );
}

// The shadows at normalized `depth` on the view ray of pixel `pos`, one channel
// per light; 1 without scene depth.
fn scene_shadow_air_at(pos: vec2<i32>, depth: f32) -> vec4<f32> {
    if !scene_depth_present() { return vec4<f32>(1.0); }
    let uv = full_image_uv(pos);
    let slice = scene_shadow_slice_at(depth);
    let lower = u32(slice);
    let upper = min(lower + 1u, SCENE_SHADOW_SLICES - 1u);
    return mix(
        scene_shadow_air_slice(uv, lower),
        scene_shadow_air_slice(uv, upper),
        slice - f32(lower),
    );
}

// The share of `light` that `shadows` (`scene_shadow_air_at`,
// `scene_shadow_surface_at`) let through.
fn scene_shadow_of(shadows: vec4<f32>, light: SceneLight) -> f32 {
    if light.channel == 0u { return 1.0; }
    return shadows[light.channel - 1u];
}

// depth-rework-bridge: delete in P7. The surface and shadow helpers of the
// Relight before docs/rework/DEPTH_EFFECTS_PLAN.md P4, which the scene shadow
// map above and scene emission (scene_emission.wgsl) still use until P5 and
// P7. Relight itself no longer reads them.

const RELIGHT_NORMAL_TAPS: u32 = 12u;
// Shadow-march steps: one per few depth texels of the ray's image path, within
// these bounds. Too few steps skip over a silhouette between them and leave
// banded copies of its shadow.
const RELIGHT_MIN_SHADOW_STEPS: f32 = 24.0;
const RELIGHT_MAX_SHADOW_STEPS: f32 = 96.0;
const RELIGHT_TEXELS_PER_SHADOW_STEP: f32 = 4.0;
// Narrowest shadow-sample footprint in level-0 texels: about one pixel of the
// depth model, whose silhouettes were blocky, when it predicted roughly 700
// pixels across the image. Not retuned for today's 1540.
const RELIGHT_MIN_SHADOW_FOOTPRINT: f32 = 4.0;
// Normalized depths where the light fades out and beyond which nothing is lit
// (`apply_relight`).
const RELIGHT_REACH_FADE_START: f32 = 0.92;
const RELIGHT_REACH_LIMIT: f32 = 0.96;
// Radius of the disc `relight_reach` averages over, in deviations of the lens
// blur, and its samples. A blurred step runs over two deviations on either side
// of it.
const RELIGHT_REACH_BLUR_RADIUS: f32 = 4.0;
const RELIGHT_REACH_TAPS: u32 = 8u;
fn relight_surface(uv: vec2<f32>, level: f32) -> vec4<f32> {
    return textureSampleLevel(scene_depth_tex, SceneAdjustments::local_mask_sampler, uv, level);
}

// Texels of scene-depth mip `level`. Derived from level 0 because
// lavapipe in Mesa 25 reports wrong sizes for higher levels.
fn relight_depth_level_size(level: i32) -> vec2<i32> {
    let base = vec2<i32>(textureDimensions(scene_depth_tex));
    return max(base >> vec2<u32>(u32(level)), vec2<i32>(1));
}

// Mean extent of one level-0 texel in shorter-edge units.
fn relight_texel_extent(camera: SceneCamera) -> f32 {
    let texels = vec2<f32>(textureDimensions(scene_depth_tex));
    let extent = camera.image_size / texels;
    return sqrt(extent.x * extent.y);
}

// Averages the depth gradient over a disc, as a light of that size integrates
// the surface. Taps that leave the pixel's own surface (their depth departs
// from the tangent plane) are rejected, so a background slope does not tilt
// a foreground edge and the light leaves no halo around silhouettes.
fn relight_smoothed_gradient(
    camera: SceneCamera,
    uv: vec2<f32>,
    depth: f32,
    gradient: vec2<f32>,
    radius: f32,
) -> vec2<f32> {
    let spacing = radius * 0.5 / relight_texel_extent(camera);
    let levels = f32(textureNumLevels(scene_depth_tex));
    let level = clamp(log2(max(spacing, 1.0)), 0.0, levels - 1.0);
    let tolerance = 0.01 + radius * (0.25 + 0.5 * length(gradient));
    var sum = gradient;
    var weights = 1.0;
    for (var tap = 0u; tap < RELIGHT_NORMAL_TAPS; tap = tap + 1u) {
        let distance = radius * sqrt((f32(tap) + 0.5) / f32(RELIGHT_NORMAL_TAPS));
        let angle = f32(tap) * SCENE_GOLDEN_ANGLE;
        let offset = distance * vec2<f32>(cos(angle), sin(angle));
        let tap_surface = relight_surface(uv + offset / camera.image_size, level);
        let residual = (tap_surface.y - depth - dot(gradient, offset)) / tolerance;
        let weight = exp(-residual * residual);
        sum += tap_surface.zw * weight;
        weights += weight;
    }
    return sum / weights;
}

// How much of a light a surface at `depth` can receive at all: none beyond the
// far end of reach (`relight_contribution`). Where the lens blurred that depth,
// the reach is averaged over a disc as wide as the blur, as the photo's own
// light is, and the pixel keeps the lesser of that and its own: the light fades
// on toward a blurred outline, while a sharp nearer object beside far scenery
// lends it no reach. Its own reach still ends the light at the far cutoff, where
// the disc's mean is about a half.
fn relight_reach(uv: vec2<f32>, depth: f32) -> f32 {
    let own = 1.0 - smoothstep(RELIGHT_REACH_FADE_START, RELIGHT_REACH_LIMIT, depth);
    let image = Common::scene_image_size();
    let texel = max(image.x, image.y) / f32(textureDimensions(scene_depth_tex).x);
    let radius = RELIGHT_REACH_BLUR_RADIUS * scene_focus_blur(depth);
    if own <= 0.0 || radius <= texel { return own; }
    // Each sample reads depth averaged over about its share of the disc.
    let spacing = radius / sqrt(f32(RELIGHT_REACH_TAPS));
    let level = clamp(log2(spacing / texel), 0.0, f32(textureNumLevels(scene_depth_tex)) - 1.0);
    var sum = own;
    for (var tap = 0u; tap < RELIGHT_REACH_TAPS; tap = tap + 1u) {
        let distance = radius * sqrt((f32(tap) + 0.5) / f32(RELIGHT_REACH_TAPS));
        let angle = f32(tap) * SCENE_GOLDEN_ANGLE;
        let offset = distance * vec2<f32>(cos(angle), sin(angle));
        let sample_depth = relight_surface(uv + offset / image, level).y;
        sum += 1.0 - smoothstep(RELIGHT_REACH_FADE_START, RELIGHT_REACH_LIMIT, sample_depth);
    }
    return min(own, sum / f32(RELIGHT_REACH_TAPS + 1u));
}

// Surface normal from the depth gradient (depth per shorter-edge length),
// facing the camera.
fn relight_normal(camera: SceneCamera, point: vec2<f32>, z: f32, gradient: vec2<f32>) -> vec3<f32> {
    let focal = 2.0 * SCENE_FOCAL_TAN;
    let lateral = scene_lateral(camera, point);
    let slope = camera.relief * gradient;
    let along_x = vec3<f32>(focal * z + lateral.x * slope.x, lateral.y * slope.x, slope.x);
    let along_y = vec3<f32>(lateral.x * slope.y, focal * z + lateral.y * slope.y, slope.y);
    let normal = normalize(cross(along_x, along_y));
    return select(normal, -normal, dot(normal, vec3<f32>(lateral * z, z)) > 0.0);
}

// How much the depth-map texels around `uv` block the ray at `position`.
// Each of the four texels is tested on its own and the results are blended
// bilinearly (percentage-closer filtering): thresholding interpolated depth
// would trace the texel grid as a hard staircase, while blending the tests
// gives an edge that ramps smoothly across one texel of `level`.
fn relight_blocked(
    camera: SceneCamera,
    position: vec3<f32>,
    uv: vec2<f32>,
    level: i32,
    width: f32,
) -> f32 {
    let size = relight_depth_level_size(level);
    let p = uv * vec2<f32>(size) - vec2<f32>(0.5);
    let base = vec2<i32>(floor(p));
    let f = p - vec2<f32>(base);
    var blocked = 0.0;
    for (var y = 0; y < 2; y = y + 1) {
        for (var x = 0; x < 2; x = x + 1) {
            let cell = clamp(base + vec2<i32>(x, y), vec2<i32>(0), size - vec2<i32>(1));
            let occluder_z = scene_depth_z(camera, textureLoad(scene_depth_tex, cell, level).y);
            // Bias and minimum penumbra keep noisy depth (foliage) from speckling.
            let penetration = position.z - occluder_z - 0.006 * occluder_z;
            let thickness = 0.15 * occluder_z;
            let test = smoothstep(-0.5 * width, width, penetration)
                * (1.0 - smoothstep(thickness, thickness * 1.5, penetration));
            let weight = select(1.0 - f.x, f.x, x == 1) * select(1.0 - f.y, f.y, y == 1);
            blocked += test * weight;
        }
    }
    return blocked;
}

// Whether a shadow ray at depth `z` passes in front of every surface that
// one march step's occlusion tests read (`relight_blocked` around `uv` and
// `uv ± side` on `level`) by at least the test's ramp, so that every test
// reports exactly zero and the step can be skipped. Above level 0, channel x
// of the surface holds the nearest depth of each texel's footprint
// (scene_surface.rs), never farther than the depth any test reads there; the
// check reads 2×2 texels of the finest such level covering the tested cells.
fn relight_step_unoccluded(
    camera: SceneCamera,
    z: f32,
    width: f32,
    uv: vec2<f32>,
    side: vec2<f32>,
    level: i32,
) -> bool {
    let size = relight_depth_level_size(level);
    let last = size - vec2<i32>(1);
    let extent = vec2<f32>(size);
    // The cells `relight_blocked` reads for all three taps.
    let lo = clamp(
        vec2<i32>(floor(min(uv - side, uv + side) * extent - vec2<f32>(0.5))),
        vec2<i32>(0),
        last,
    );
    let hi = clamp(
        vec2<i32>(floor(max(uv - side, uv + side) * extent - vec2<f32>(0.5))) + vec2<i32>(1),
        vec2<i32>(0),
        last,
    );
    let top = i32(textureNumLevels(scene_depth_tex)) - 1;
    // Level 0 holds stored depth in channel x, not the nearest depth.
    var shift = select(0u, 1u, level == 0);
    while level + i32(shift) < top
        && any((hi >> vec2<u32>(shift)) - (lo >> vec2<u32>(shift)) > vec2<i32>(1)) {
        shift = shift + 1u;
    }
    let a = lo >> vec2<u32>(shift);
    let b = hi >> vec2<u32>(shift);
    if any(b - a > vec2<i32>(1)) { return false; }
    let coarse = level + i32(shift);
    let nearest = min(
        min(
            textureLoad(scene_depth_tex, a, coarse).x,
            textureLoad(scene_depth_tex, vec2<i32>(b.x, a.y), coarse).x,
        ),
        min(
            textureLoad(scene_depth_tex, vec2<i32>(a.x, b.y), coarse).x,
            textureLoad(scene_depth_tex, b, coarse).x,
        ),
    );
    let occluder_z = scene_depth_z(camera, nearest);
    // The largest penetration any test can see; at or below the ramp's start
    // its smoothstep is zero.
    return z - occluder_z - 0.006 * occluder_z <= -0.5 * width;
}

// Screen-space shadow: marches from the surface toward the light through the
// depth map. Depth maps hold only front surfaces, so an occluder is assumed to
// be solid for a thickness proportional to its distance; a ray passing farther
// behind it is lit.
//
// Each step is a cone section toward the light: its footprint widens with the
// distance from the receiver and with light size, as a real penumbra does, and
// is never narrower than twice the step spacing or a depth-model pixel, so
// consecutive steps cover the path without gaps. The step reads the mip level
// whose texels are half that footprint, filters the occlusion test over it and
// averages three taps across the ray, so silhouettes in the low-resolution
// depth map give soft shadow edges instead of texel staircases, and thin
// structures fade instead of striping. `jitter` in [0, 1) offsets the steps
// per pixel, so what aliasing remains becomes fine dither rather than
// contour-like copies of a silhouette's shadow. Steps where the ray provably
// passes in front of everything nearby are skipped
// (`relight_step_unoccluded`); they would leave the visibility unchanged.
fn relight_visibility(
    camera: SceneCamera,
    surface: vec3<f32>,
    light: vec3<f32>,
    size: f32,
    jitter: f32,
) -> f32 {
    let path = light - surface;
    let path_length = length(path);
    let start = scene_image_point(camera, surface);
    let end = scene_image_point(camera, light);
    let texel = relight_texel_extent(camera);
    let path_texels = length(end - start) / texel;
    if path_texels < 1.0 { return 1.0; }
    let direction = (end - start) / (path_texels * texel);
    let across = vec2<f32>(-direction.y, direction.x);
    // Leave the receiver's own texels; the tangent plane already shades them.
    let first = min(1.5 / path_texels, 0.5);
    let penumbra_rate = mix(0.01, 0.25, size);
    let top_level = f32(textureNumLevels(scene_depth_tex)) - 1.0;
    let steps = clamp(
        ceil(path_texels / RELIGHT_TEXELS_PER_SHADOW_STEP),
        RELIGHT_MIN_SHADOW_STEPS,
        RELIGHT_MAX_SHADOW_STEPS,
    );
    var visibility = 1.0;
    for (var index = 0.0; index < steps; index = index + 1.0) {
        let fraction = (index + jitter) / steps;
        // Denser near the receiver, where contact shadows are narrow.
        let t = mix(first, 1.0, fraction * fraction);
        let position = surface + path * t;
        if position.z <= 0.02 { break; }
        let point = scene_image_point(camera, position);
        let uv = point / camera.image_size;
        // Nothing is known beyond the frame: it casts no shadow.
        if any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) { break; }
        let travel = path_length * t;
        // Penumbra radius in the image at this step's distance.
        let cone = travel * penumbra_rate / (2.0 * SCENE_FOCAL_TAN * position.z);
        let spacing = path_texels * (1.0 - first) * 2.0 * fraction / steps;
        let footprint = max(max(cone / texel, 2.0 * spacing), RELIGHT_MIN_SHADOW_FOOTPRINT);
        // The filtered test ramps over one texel of the level, half the footprint.
        let level = i32(clamp(round(log2(footprint) - 1.0), 0.0, top_level));
        let width = 0.01 * position.z + travel * penumbra_rate;
        let side = across * (0.5 * footprint * texel) / camera.image_size;
        if relight_step_unoccluded(camera, position.z, width, uv, side, level) { continue; }
        let blocked = 0.5 * relight_blocked(camera, position, uv, level, width)
            + 0.25 * relight_blocked(camera, position, uv - side, level, width)
            + 0.25 * relight_blocked(camera, position, uv + side, level, width);
        visibility = min(visibility, 1.0 - blocked);
        if visibility <= 0.0 { break; }
    }
    return visibility;
}

// Interleaved gradient noise in [0, 1) per full-image pixel at full-image
// `uv`, so export tiles and the preview dither alike.
fn relight_jitter(uv: vec2<f32>) -> f32 {
    let full_size = vec2<f32>(
        f32(Common::camera_uniforms.full_width),
        f32(Common::camera_uniforms.full_height),
    );
    let p = floor(uv * full_size);
    return fract(52.9829189 * fract(dot(p, vec2<f32>(0.06711056, 0.00583715))));
}

fn relight_size(params: MaskEffectParams) -> f32 {
    return mask_effect_percent(params, RELIGHT_SIZE_LANE);
}

