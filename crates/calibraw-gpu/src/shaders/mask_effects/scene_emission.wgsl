// Scene emission: effects that light the scene from an area of the image rather
// than from a point. A self-illuminating Glow and Neon's lines reach the same
// receivers as the scene lights of scene_lights.wgsl: surfaces, Fog, Smoke and
// Rain's mist and drops, and highlights on wet ground. Particles and other drawn
// light join as further emitters.
//
// Writers. `build_scene_emission` evaluates every slot on a coarse grid over the
// full image, SCENE_EMISSION_GRID_LONG cells along its longer edge, sampled
// SCENE_EMISSION_SUBSAMPLES² times per cell. An emitter gives its radiance at a
// full-image point (`scene_emission_slot_at`; a new emitter is one more case
// there and in `scene_emission_present`) and reads only what every export tile
// and zoomed view shares: its parameters, its mask in the full-image mask atlas
// (SceneAdjustments::full_image_mask_weight; `MaskEffect::reads_full_image_mask`
// keeps it there) and full-image maps such as the contour statistics
// (`scene_contour_line_share`). Radiance is exposed scene-linear Rec.2020, what
// the effect adds to the image. A cell keeps the light it emits (radiance times
// image area in shorter-edge units squared), the emitters' mean scene depth and
// their centroid in full-image uv.
//
// Light. Each cell is a disc as wide as the cell, placed at its depth in the
// scene frame (scene_lights.wgsl, relief scale 1): a scene light whose reach
// is that radius, so its light falls off with the inverse square of the distance
// beyond it and stays finite inside it. Depths are the scene bake's linear
// depth d (scene/geometry.wgsl), averaged over a cell, or 0 without scene
// depth. Two passes spread the cells once per
// render, so a receiver's cost does not depend on the emitters:
// - `build_scene_emission_surfaces` gives the light arriving at the scene's
//   surfaces on a grid twice as fine, at each cell's depth, with its mean
//   direction. Surfaces (`apply_scene_emission_light`) and wet ground read it.
// - `build_scene_emission_volume` integrates the light along each view ray over
//   the depth cells of the media (MEDIUM_CELLS), scattered evenly and with the
//   forward lobe of Fog (FOG_LIGHT_ANISOTROPY), from cells merged two by two.
//   Fog, Smoke and Rain read it.
// Light is relative to the scene's ambient level, as the scene lights' is.
// Emitters cast no shadows.
//
// Textures, in square blocks of a grid stacked downward:
// - scene_emission_tex: SCENE_EMISSION_GRID_LONG wide, two blocks: a cell's light
//   (xyz) and depth (w), then its centroid (xy).
// - scene_emission_light_tex, one texture so that the effect passes stay within
//   the sampled-texture limit: SCENE_EMISSION_SURFACE_GRID_LONG wide, first two
//   blocks of the surface grid: the arriving light (xyz) and the depth it arrives
//   at (w), then the mean direction toward it (xyz), of length 1 when it comes
//   from one direction and less the more it spreads. From row
//   SCENE_EMISSION_VOLUME_ROW, 2 × MEDIUM_CELLS blocks of the emitter grid, the
//   volume: block i the light scattered evenly over depth cell i, block
//   MEDIUM_CELLS + i the same with the forward lobe, each integrated over the
//   cell's normalized depth.

const SCENE_EMISSION_GRID_LONG: u32 = 32u;
const SCENE_EMISSION_SURFACE_GRID_LONG: u32 = 64u;
const SCENE_EMISSION_SUBSAMPLES: u32 = 4u;
const SCENE_EMISSION_VOLUME_ROW: u32 = 2u * SCENE_EMISSION_SURFACE_GRID_LONG;
// Rows of scene_emission_light_tex.
const SCENE_EMISSION_LIGHT_ROWS: u32 =
    SCENE_EMISSION_VOLUME_ROW + 2u * MEDIUM_CELLS * SCENE_EMISSION_GRID_LONG;
// Taps per axis with which a surface cell averages the bake's depth.
const SCENE_EMISSION_SURFACE_TAPS: u32 = 3u;
// Light at Scene light 100 relative to the radiance an emitter adds to the image.
const SCENE_EMISSION_GAIN: f32 = 1.0;
// Wrap of the Lambert term on surfaces, which also lets light that arrives from
// all around reach a surface facing away from its mean direction.
const SCENE_EMISSION_WRAP: f32 = 0.35;
// Normal smoothing radius in shorter-edge units: the light is broad, so it
// integrates the surface over a wide disc (`Geometry::scene_soft_normal`).
const SCENE_EMISSION_NORMAL_RADIUS: f32 = 0.03;
// Linear depth over which a surface stops sharing the light of a
// neighbouring surface cell, so light does not bleed across silhouettes.
const SCENE_EMISSION_DEPTH_TOLERANCE: f32 = 0.05;

@group(0) @binding(78) var<storage, read> contour_cells_in: array<Common::ContourCell>;
@group(0) @binding(79) var scene_emission_out: texture_storage_2d<rgba16float, write>;
@group(0) @binding(80) var scene_emission_tex: texture_2d<f32>;
// The surface and volume passes each write their part.
@group(0) @binding(81) var scene_emission_light_out: texture_storage_2d<rgba16float, write>;
@group(0) @binding(83) var scene_emission_light_tex: texture_2d<f32>;

fn scene_emission_grid() -> vec2<u32> {
    return Common::full_image_grid(SCENE_EMISSION_GRID_LONG);
}

fn scene_emission_surface_grid() -> vec2<u32> {
    return Common::full_image_grid(SCENE_EMISSION_SURFACE_GRID_LONG);
}

// Whether any slot lights the scene by emission. It is the same for every
// pixel, so receivers skip reading the maps when nothing emits.
fn scene_emission_present() -> bool {
    for (var index = 0u; index < Common::mask_slot_count(); index = index + 1u) {
        let state = Common::mask_data[index].metadata;
        if Common::mask_slot_applies(state, MASK_EFFECT_GLOW_ID)
            && mask_glow_lights_scene(mask_effect_params(index)) {
            return true;
        }
        if Common::mask_slot_applies(state, MASK_EFFECT_NEON_ID)
            && neon_lights_scene(mask_effect_params(index)) {
            return true;
        }
    }
    return false;
}

// The radiance the slot at `index` emits at full-image `uv` where its mask is
// full; none for a slot that emits nothing.
fn scene_emission_slot_at(index: u32, uv: vec2<f32>) -> vec3<f32> {
    let state = Common::mask_data[index].metadata;
    if Common::mask_slot_applies(state, MASK_EFFECT_GLOW_ID) {
        return mask_glow_scene_emission(mask_effect_params(index));
    }
    if Common::mask_slot_applies(state, MASK_EFFECT_NEON_ID) {
        return neon_scene_emission(mask_effect_params(index), uv);
    }
    return vec3<f32>(0.0);
}

// The share of the contour cell around full-image `uv` that lines along the
// photo's contours cover, measured as Neon measures them (neon.wgsl): contrast
// above `threshold`, ramping in over `ramp`, on luminance with a soft black
// floor `black` at the current exposure, the Sobel taps `radius` pixels apart.
// Each bin is taken as a step between its brightness and the darker side its
// contrast implies.
fn scene_contour_line_share(
    uv: vec2<f32>,
    threshold: f32,
    ramp: f32,
    black: f32,
    radius: f32,
) -> f32 {
    let grid = Common::contour_grid();
    let cell = min(vec2<u32>(uv * vec2<f32>(grid)), grid - vec2<u32>(1u));
    let index = cell.y * Common::CONTOUR_GRID_LONG + cell.x;
    let summary = contour_cells_in[index].bins[Common::CONTOUR_CELL_VECTORS - 1u];
    if summary.x <= 0.0 { return 0.0; }
    let gain = exp2(Common::scene_tone_uniforms.exposure) / black;
    var covered = 0.0;
    for (var brightness = 0u; brightness < Common::CONTOUR_BRIGHTNESS_BINS; brightness = brightness + 1u) {
        let brighter = gain * Common::contour_brightness_at(brightness);
        let top = log2(1.0 + brighter);
        for (var contrast = 0u; contrast < Common::CONTOUR_CONTRAST_BINS; contrast = contrast + 1u) {
            let bin = contrast * Common::CONTOUR_BRIGHTNESS_BINS + brightness;
            let count = contour_cells_in[index].bins[bin / 4u][bin % 4u];
            if count <= 0.0 { continue; }
            // A step of 2c stops reads as contrast c (Common::contour_contrast_at).
            let darker = brighter * exp2(-2.0 * Common::contour_contrast_at(contrast));
            let measured = 0.5 * (top - log2(1.0 + darker));
            covered += count * smoothstep(threshold, threshold + ramp, measured);
        }
    }
    // A line is about twice its tap spacing wide. The statistics hold the
    // spacing they were measured at in reference pixels (summary.y), which a
    // zoomed view inheriting them from the full frame does not share.
    let spacing = radius / Common::resolution_scale(0.55, 3.0);
    return min(covered / summary.x * spacing / max(summary.y, 1e-3), 1.0);
}

// The scene bake's linear depth of the visible surface's texel at full-image
// `uv`; 0 without scene depth. The nearer surface of an edge texel counts: the
// light of an emitter lies on what stands before the rest.
fn scene_emission_depth_at(uv: vec2<f32>) -> f32 {
    if !Geometry::scene_geometry_present() { return 0.0; }
    let texel = vec2<i32>(floor(clamp(uv, vec2<f32>(0.0), vec2<f32>(0.9999)) * vec2<f32>(Bake::scene_bake_size())));
    return Geometry::scene_layer(texel, Bake::SCENE_LAYER_GEOMETRY).x;
}

@compute @workgroup_size(8, 8, 1)
fn build_scene_emission(@builtin(global_invocation_id) gid: vec3<u32>) {
    let grid = scene_emission_grid();
    if gid.x >= grid.x || gid.y >= grid.y { return; }
    let samples = SCENE_EMISSION_SUBSAMPLES;
    var light = vec3<f32>(0.0);
    var weights = 0.0;
    var weighted_depth = 0.0;
    var weighted_uv = vec2<f32>(0.0);
    var mean_depth = 0.0;
    for (var sy = 0u; sy < samples; sy = sy + 1u) {
        for (var sx = 0u; sx < samples; sx = sx + 1u) {
            let offset = (vec2<f32>(f32(sx), f32(sy)) + vec2<f32>(0.5)) / f32(samples);
            let uv = (vec2<f32>(gid.xy) + offset) / vec2<f32>(grid);
            let depth = scene_emission_depth_at(uv);
            mean_depth += depth;
            var radiance = vec3<f32>(0.0);
            for (var index = 0u; index < Common::mask_slot_count(); index = index + 1u) {
                let emitted = scene_emission_slot_at(index, uv);
                if all(emitted <= vec3<f32>(0.0)) { continue; }
                radiance += emitted * SceneAdjustments::full_image_mask_weight(uv, index);
            }
            let weight = dot(radiance, Common::LUMA);
            light += radiance;
            weights += weight;
            weighted_depth += weight * depth;
            weighted_uv += weight * uv;
        }
    }
    let count = f32(samples * samples);
    let cell_size = Common::scene_image_size() / vec2<f32>(grid);
    let sample_area = cell_size.x * cell_size.y / count;
    var depth = mean_depth / count;
    var center = (vec2<f32>(gid.xy) + vec2<f32>(0.5)) / vec2<f32>(grid);
    if weights > 1e-9 {
        depth = weighted_depth / weights;
        center = weighted_uv / weights;
    }
    let cell = vec2<i32>(gid.xy);
    textureStore(scene_emission_out, cell, vec4<f32>(light * (sample_area * SCENE_EMISSION_GAIN), depth));
    textureStore(
        scene_emission_out,
        cell + vec2<i32>(0, i32(SCENE_EMISSION_GRID_LONG)),
        vec4<f32>(center, 0.0, 0.0),
    );
}

// The centroid of emitter grid cell `cell` in full-image uv, as
// `build_scene_emission` stores it.
fn scene_emission_center(cell: vec2<i32>) -> vec2<f32> {
    return textureLoad(scene_emission_tex, cell + vec2<i32>(0, i32(SCENE_EMISSION_GRID_LONG)), 0).xy;
}

// A grid cell's light as a scene light in the frame at relief scale 1:
// `light` and `depth` as `build_scene_emission` stores them, `center` its
// centroid and `extent` the width of the cell in shorter-edge units. Its
// intensity is relative to `ambient`.
fn scene_emitter(
    light: vec3<f32>,
    depth: f32,
    center: vec2<f32>,
    extent: f32,
    ambient: f32,
) -> SceneLight {
    let camera = scene_light_camera();
    let z = Camera::scene_frame_depth(camera, depth, 1.0);
    // Scene units per shorter-edge unit of the image at that depth.
    let scale = 2.0 * camera.lens.x * z;
    let core = max(0.5 * extent * scale, 1e-4);
    // Light (radiance × area) over the core's square, so that intensity ×
    // falloff = light / (core² + distance²).
    let power = light * (scale * scale);
    return SceneLight(
        true,
        Camera::scene_frame_position(camera, Camera::scene_image_point(center, Common::scene_image_size()), z),
        core,
        power / (core * core * ambient),
        0u,
        SCENE_LIGHT_UNCONFINED,
        1.0,
    );
}

// The width of a cell of `grid` in shorter-edge units.
fn scene_emission_cell_extent(grid: vec2<u32>) -> f32 {
    let size = Common::scene_image_size() / vec2<f32>(grid);
    return sqrt(size.x * size.y);
}

@compute @workgroup_size(8, 8, 1)
fn build_scene_emission_surfaces(@builtin(global_invocation_id) gid: vec3<u32>) {
    let grid = scene_emission_surface_grid();
    if gid.x >= grid.x || gid.y >= grid.y { return; }
    let camera = scene_light_camera();
    let uv = (vec2<f32>(gid.xy) + vec2<f32>(0.5)) / vec2<f32>(grid);
    // The cell's depth: a few taps over its extent.
    var depth = 0.0;
    let taps = SCENE_EMISSION_SURFACE_TAPS;
    for (var ty = 0u; ty < taps; ty = ty + 1u) {
        for (var tx = 0u; tx < taps; tx = tx + 1u) {
            let offset = (vec2<f32>(f32(tx), f32(ty)) + vec2<f32>(0.5)) / f32(taps) - vec2<f32>(0.5);
            depth += scene_emission_depth_at(uv + offset / vec2<f32>(grid));
        }
    }
    depth /= f32(taps * taps);
    let surface = Camera::scene_frame_position(
        camera,
        Camera::scene_image_point(uv, Common::scene_image_size()),
        Camera::scene_frame_depth(camera, depth, 1.0),
    );
    let emitters = scene_emission_grid();
    let extent = scene_emission_cell_extent(emitters);
    let ambient = scene_ambient_level();
    var received = vec3<f32>(0.0);
    var direction = vec3<f32>(0.0);
    var weights = 0.0;
    for (var y = 0u; y < emitters.y; y = y + 1u) {
        for (var x = 0u; x < emitters.x; x = x + 1u) {
            let cell = vec2<i32>(i32(x), i32(y));
            let light = textureLoad(scene_emission_tex, cell, 0);
            if dot(light.xyz, Common::LUMA) <= 0.0 { continue; }
            let center = scene_emission_center(cell);
            let emitter = scene_emitter(light.xyz, light.w, center, extent, ambient);
            let arriving = emitter.intensity * scene_light_falloff(emitter, surface);
            received += arriving;
            let weight = dot(arriving, Common::LUMA);
            let offset = emitter.position - surface;
            direction += offset * (weight / max(length(offset), 1e-6));
            weights += weight;
        }
    }
    let cell = vec2<i32>(gid.xy);
    textureStore(scene_emission_light_out, cell, vec4<f32>(received, depth));
    textureStore(
        scene_emission_light_out,
        cell + vec2<i32>(0, i32(SCENE_EMISSION_SURFACE_GRID_LONG)),
        vec4<f32>(direction / max(weights, 1e-12), 0.0),
    );
}

@compute @workgroup_size(8, 8, 1)
fn build_scene_emission_volume(@builtin(global_invocation_id) gid: vec3<u32>) {
    let grid = scene_emission_grid();
    if gid.x >= grid.x || gid.y >= grid.y { return; }
    let uv = (vec2<f32>(gid.xy) + vec2<f32>(0.5)) / vec2<f32>(grid);
    let ray = scene_light_ray(Camera::scene_image_point(uv, Common::scene_image_size()));
    let extent = 2.0 * scene_emission_cell_extent(grid);
    let ambient = scene_ambient_level();
    var even: array<vec3<f32>, MEDIUM_CELLS>;
    var forward: array<vec3<f32>, MEDIUM_CELLS>;
    let merged = (grid + vec2<u32>(1u)) / 2u;
    for (var my = 0u; my < merged.y; my = my + 1u) {
        for (var mx = 0u; mx < merged.x; mx = mx + 1u) {
            // Media blur light anyway: two by two cells make one emitter.
            var light = vec3<f32>(0.0);
            var weights = 0.0;
            var weighted_depth = 0.0;
            var weighted_uv = vec2<f32>(0.0);
            for (var dy = 0u; dy < 2u; dy = dy + 1u) {
                for (var dx = 0u; dx < 2u; dx = dx + 1u) {
                    let source = vec2<u32>(2u * mx + dx, 2u * my + dy);
                    if source.x >= grid.x || source.y >= grid.y { continue; }
                    let cell = vec2<i32>(source);
                    let texel = textureLoad(scene_emission_tex, cell, 0);
                    let weight = dot(texel.xyz, Common::LUMA);
                    if weight <= 0.0 { continue; }
                    let center = scene_emission_center(cell);
                    light += texel.xyz;
                    weights += weight;
                    weighted_depth += weight * texel.w;
                    weighted_uv += weight * center;
                }
            }
            if weights <= 0.0 { continue; }
            let emitter = scene_emitter(
                light, weighted_depth / weights, weighted_uv / weights, extent, ambient,
            );
            for (var i = 0u; i < MEDIUM_CELLS; i = i + 1u) {
                let near = f32(i) / f32(MEDIUM_CELLS);
                let far = f32(i + 1u) / f32(MEDIUM_CELLS);
                let along = scene_light_falloff_along(emitter, ray, near, far);
                if along <= 0.0 { continue; }
                let middle = scene_light_ray_position(emitter, ray, 0.5 * (near + far));
                even[i] += emitter.intensity * along;
                forward[i] += emitter.intensity
                    * (along * scene_light_phase_at(emitter, middle, FOG_LIGHT_ANISOTROPY));
            }
        }
    }
    let long = i32(SCENE_EMISSION_GRID_LONG);
    let cell = vec2<i32>(gid.xy) + vec2<i32>(0, i32(SCENE_EMISSION_VOLUME_ROW));
    for (var i = 0u; i < MEDIUM_CELLS; i = i + 1u) {
        textureStore(scene_emission_light_out, cell + vec2<i32>(0, i32(i) * long), vec4<f32>(even[i], 0.0));
        textureStore(
            scene_emission_light_out,
            cell + vec2<i32>(0, i32(MEDIUM_CELLS + i) * long),
            vec4<f32>(forward[i], 0.0),
        );
    }
}

// Block `block` of the volume at full-image `uv`, filtered between columns.
fn scene_emission_volume_at(uv: vec2<f32>, block: u32) -> vec3<f32> {
    let grid = vec2<f32>(scene_emission_grid());
    let long = f32(SCENE_EMISSION_GRID_LONG);
    // Clamped half a cell inside the block, so filtering stays in it.
    let cell = clamp(uv * grid, vec2<f32>(0.5), grid - vec2<f32>(0.5));
    let size = vec2<f32>(f32(SCENE_EMISSION_SURFACE_GRID_LONG), f32(SCENE_EMISSION_LIGHT_ROWS));
    let row = f32(SCENE_EMISSION_VOLUME_ROW) + f32(block) * long;
    return textureSampleLevel(
        scene_emission_light_tex,
        SceneAdjustments::local_mask_sampler,
        (cell + vec2<f32>(0.0, row)) / size,
        0.0,
    ).xyz;
}

// Light from emitters that the medium in depth cell `cell` of the view ray at
// full-image `uv` scatters toward the camera, integrated from normalized depth
// `near` to `far` inside the cell, before the medium's extinction and with
// Fog's forward lobe (medium_light_scattering).
fn scene_emission_medium(uv: vec2<f32>, cell: u32, near: f32, far: f32) -> vec3<f32> {
    let share = (far - near) * f32(MEDIUM_CELLS);
    return scene_emission_volume_at(uv, MEDIUM_CELLS + cell) * share;
}

// Light from emitters that a point on the view ray of pixel `pos` at `depth`
// scatters toward the camera, as `scene_light_received` gives it for scene
// lights. Anisotropies between none and Fog's blend the two scatterings.
fn scene_emission_received(pos: vec2<i32>, depth: f32, anisotropy: f32) -> vec3<f32> {
    let uv = full_image_uv(pos);
    let t = clamp(depth * f32(MEDIUM_CELLS) - 0.5, 0.0, f32(MEDIUM_CELLS - 1u));
    let lower = u32(t);
    let upper = min(lower + 1u, MEDIUM_CELLS - 1u);
    let f = t - f32(lower);
    let even = mix(scene_emission_volume_at(uv, lower), scene_emission_volume_at(uv, upper), f);
    let forward = mix(
        scene_emission_volume_at(uv, MEDIUM_CELLS + lower),
        scene_emission_volume_at(uv, MEDIUM_CELLS + upper),
        f,
    );
    // Each block holds a depth cell's integral; per unit depth it is that
    // times the cells per unit.
    return mix(even, forward, clamp(anisotropy / FOG_LIGHT_ANISOTROPY, 0.0, 1.0))
        * f32(MEDIUM_CELLS);
}

struct SceneEmissionArrival {
    // Relative to the ambient level.
    light: vec3<f32>,
    // Toward the light, shorter the more it spreads.
    direction: vec3<f32>,
}

// The light from emitters arriving at the surface at full-image `uv` and
// normalized depth `depth`. Neighbouring surface cells count by how close
// their depth is, so light does not cross silhouettes; a small floor falls back
// to plain bilinear weights where none is close.
fn scene_emission_surface_at(uv: vec2<f32>, depth: f32) -> SceneEmissionArrival {
    let grid = vec2<i32>(scene_emission_surface_grid());
    let p = uv * vec2<f32>(grid) - vec2<f32>(0.5);
    let base = vec2<i32>(floor(p));
    let f = p - floor(p);
    var light = vec3<f32>(0.0);
    var direction = vec3<f32>(0.0);
    var weights = 0.0;
    for (var y = 0; y < 2; y = y + 1) {
        for (var x = 0; x < 2; x = x + 1) {
            let cell = clamp(base + vec2<i32>(x, y), vec2<i32>(0), grid - vec2<i32>(1));
            let bilinear = select(1.0 - f.x, f.x, x == 1) * select(1.0 - f.y, f.y, y == 1);
            let texel = textureLoad(scene_emission_light_tex, cell, 0);
            let toward = textureLoad(
                scene_emission_light_tex,
                cell + vec2<i32>(0, i32(SCENE_EMISSION_SURFACE_GRID_LONG)),
                0,
            ).xyz;
            let difference = (texel.w - depth) / SCENE_EMISSION_DEPTH_TOLERANCE;
            let weight = bilinear * (exp(-difference * difference) + 1e-3);
            light += texel.xyz * weight;
            direction += toward * weight;
            weights += weight;
        }
    }
    return SceneEmissionArrival(light / weights, direction / weights);
}

// One surface of a pixel lit by the emitters, in proportion to an estimate of
// its reflectance as Relight's (`relight_shade`); `rgb` is its part of the
// pixel's colour. The sky stays unlit, as Relight leaves it.
fn scene_emission_shade(
    pos: vec2<i32>,
    uv: vec2<f32>,
    surface: Geometry::SceneSurface,
    normal: vec3<f32>,
    rgb: vec3<f32>,
) -> vec3<f32> {
    let arrival = scene_emission_surface_at(uv, surface.d);
    let facing = clamp(
        (dot(normal, arrival.direction) + SCENE_EMISSION_WRAP) / (1.0 + SCENE_EMISSION_WRAP),
        0.0,
        1.0,
    );
    let solid = Color::gamut_project_nonnegative_rec2020(rgb);
    let relative = clamp(Common::safe_luma(solid) / scene_ambient_level(), 0.02, 64.0);
    let reflectance = solid * pow(relative, -0.3) * (1.0 - surface.sky);
    return rgb + reflectance * arrival.light * facing;
}

// Light from emitters on the scene's surfaces. It is added before the creative
// effects (creative_effects.wgsl), so Relight's Ambient dims it with
// everything else and Fog veils it. Each of a pixel's surfaces is lit with
// its own normal, softened over the wide disc of the light; without scene depth the
// scene is a plane facing the camera.
fn apply_scene_emission_light(pos: vec2<i32>, input_rgb: vec3<f32>) -> vec3<f32> {
    if !scene_emission_present() { return input_rgb; }
    let uv = full_image_uv(pos);
    if !Geometry::scene_geometry_present() {
        let plane = Geometry::SceneSurface(
            vec3<f32>(0.0), vec3<f32>(0.0, 0.0, -1.0), input_rgb, 0.0, 0.0, 0u,
        );
        return scene_emission_shade(pos, uv, plane, plane.normal, input_rgb);
    }
    let scene_rgb = Geometry::scene_unedited_at(pos);
    let pixel = Geometry::scene_pixel(uv, scene_rgb, 1.0);
    let colors = Geometry::scene_split(pixel, scene_rgb, input_rgb);
    let near = scene_emission_shade(
        pos, uv, pixel.near,
        Geometry::scene_soft_normal(pixel.near, uv, SCENE_EMISSION_NORMAL_RADIUS, 1.0), colors[0],
    );
    if !pixel.in_band {
        return near;
    }
    let far = scene_emission_shade(
        pos, uv, pixel.far,
        Geometry::scene_soft_normal(pixel.far, uv, SCENE_EMISSION_NORMAL_RADIUS, 1.0), colors[1],
    );
    return Geometry::scene_blend(pixel, near, far);
}
