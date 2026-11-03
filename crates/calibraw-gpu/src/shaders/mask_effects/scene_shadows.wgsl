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
// reach limit (RELIGHT_UNLIT_TEXEL_DEPTH). Sky is often a large share of a
// photo, and its shadows would otherwise be traced for nothing.
fn scene_shadow_texel_unlit(cell: vec2<i32>, size: vec2<i32>) -> bool {
    for (var y = -1; y <= 1; y = y + 1) {
        for (var x = -1; x <= 1; x = x + 1) {
            let neighbour = clamp(cell + vec2<i32>(x, y), vec2<i32>(0), size - vec2<i32>(1));
            if textureLoad(scene_depth_tex, neighbour, 0).y < RELIGHT_UNLIT_TEXEL_DEPTH {
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
