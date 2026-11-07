// Participating media: what Fog, Smoke and Rain's mist share. A medium scatters
// light toward the camera along the view ray, so a lamp also glows in the air in
// front of it.

// A view ray through a medium.
struct MediumVolume {
    ray: vec3<f32>,
    start: f32,
    onset_width: f32,
    frequency: f32,
    offset: vec3<f32>,
    variation: f32,
    softness: f32,
}

// Cells along the view ray: fixed intervals of normalized depth, so rays share
// prefixes and density never jumps with surface distance.
const MEDIUM_CELLS: u32 = 12u;

// The medium's density integrated from the camera to `distance`: nothing before
// `start`, rising over `width`.
fn medium_onset_integral(distance: f32, start: f32, width: f32) -> f32 {
    let travel = max(distance - start, 0.0);
    let u = clamp(travel / width, 0.0, 1.0);
    return width * (u * u * u - 0.5 * u * u * u * u) + max(travel - width, 0.0);
}

// Relative density of the medium's banks in the cell from `lo` to `cell_end`.
fn medium_bank_density(volume: MediumVolume, lo: f32, cell_end: f32) -> f32 {
    let t = 0.5 * (lo + cell_end);
    let point = volume.ray * t * volume.frequency + volume.offset;
    let broad = effect_noise3(point);
    let detail = effect_noise3(point * 2.03 + vec3<f32>(7.1, -3.4, 13.8));
    let field = mix(broad, detail, mix(0.28, 0.08, volume.softness));
    return exp2((field - 0.5) * volume.variation * mix(5.0, 2.5, volume.softness));
}

// Forward scattering of fog droplets toward the camera (Henyey–Greenstein g).
const FOG_LIGHT_ANISOTROPY: f32 = 0.6;

// A surface of a pixel as a medium reads it from the scene bake
// (scene/geometry.wgsl), or nothing where there is no scene depth.
struct MediumTarget {
    baked: bool,
    pixel: Geometry::ScenePixel,
    surface: Geometry::SceneSurface,
    // Full-image uv of the pixel.
    uv: vec2<f32>,
}

fn medium_unbaked(uv: vec2<f32>) -> MediumTarget {
    let surface = Geometry::SceneSurface(vec3<f32>(0.0), vec3<f32>(0.0, 0.0, -1.0), vec3<f32>(0.0), 0.0, 0.0, 0u);
    let pixel = Geometry::ScenePixel(1.0, surface, surface, false, vec2<i32>(0), vec4<f32>(0.0), vec4<f32>(0.0));
    return MediumTarget(false, pixel, surface, uv);
}

fn medium_target(pixel: Geometry::ScenePixel, surface: Geometry::SceneSurface, uv: vec2<f32>) -> MediumTarget {
    return MediumTarget(true, pixel, surface, uv);
}

// The linear depth of a surface as a medium counts it: sky lies at the far end
// (d = 1).
fn medium_surface_depth(surface: Geometry::SceneSurface) -> f32 {
    return mix(surface.d, 1.0, surface.sky);
}

// Light from scene lights that the medium scatters toward the camera, relative to
// the ambient level, before its albedo and Light glow. Single scattering: each cell
// scatters what its extinction removes from the light, attenuated by the medium
// between it and the camera. A scene light (scene_lights.wgsl): the unshadowed
// integral over the cells times the air light's share, which the scene does not
// shadow along the ray to the receiver's surface (scene/air_light.wgsl); and the
// emission of Glow and Neon, which casts no shadows (scene_emission.wgsl).
fn medium_light_scattering(
    pos: vec2<i32>,
    volume: MediumVolume,
    distance: f32,
    extinction: f32,
    receiver: MediumTarget,
) -> vec3<f32> {
    if !scene_lights_present() { return vec3<f32>(0.0); }
    // Per cell: near and far depth, extinction per unit depth, and the
    // transmission from the camera to the cell's middle.
    var cells: array<vec4<f32>, MEDIUM_CELLS>;
    var through = 0.0;
    let step = 1.0 / f32(MEDIUM_CELLS);
    for (var i = 0u; i < MEDIUM_CELLS; i = i + 1u) {
        let lo = max(f32(i) * step, volume.start);
        let cell_end = f32(i + 1u) * step;
        let hi = min(cell_end, distance);
        if hi <= lo {
            cells[i] = vec4<f32>(0.0);
            continue;
        }
        var bank_density = 1.0;
        if volume.variation > 1e-6 {
            bank_density = medium_bank_density(volume, lo, cell_end);
        }
        let segment = medium_onset_integral(hi, volume.start, volume.onset_width)
            - medium_onset_integral(lo, volume.start, volume.onset_width);
        let optical_depth = extinction * segment * bank_density;
        cells[i] = vec4<f32>(lo, hi, optical_depth / (hi - lo), exp(-(through + 0.5 * optical_depth)));
        through += optical_depth;
    }

    let uv = full_image_uv(pos);
    let ray = scene_light_ray(Camera::scene_image_point(uv, Common::scene_image_size()));
    var scattered = vec3<f32>(0.0);
    for (var index = 0u; index < Common::mask_slot_count(); index = index + 1u) {
        let light = scene_light_at(index);
        if !light.emits { continue; }
        let coverage = scene_light_coverage(light, pos);
        if coverage <= 1e-6 { continue; }
        var received = 0.0;
        for (var i = 0u; i < MEDIUM_CELLS; i = i + 1u) {
            let cell = cells[i];
            if cell.z <= 0.0 { continue; }
            let middle = scene_light_ray_position(light, ray, 0.5 * (cell.x + cell.y));
            received += cell.z * cell.w * scene_light_phase_at(light, middle, FOG_LIGHT_ANISOTROPY)
                * scene_light_falloff_along(light, ray, cell.x, cell.y);
        }
        var share = 1.0;
        if receiver.baked && light.channel != 0u {
            share = Geometry::scene_air_light(uv, receiver.surface, light.channel - 1u);
        }
        scattered += light.intensity * (received * coverage * share);
    }
    if scene_emission_present() {
        for (var i = 0u; i < MEDIUM_CELLS; i = i + 1u) {
            let cell = cells[i];
            if cell.z <= 0.0 { continue; }
            scattered += scene_emission_medium(uv, i, cell.x, cell.y) * (cell.z * cell.w);
        }
    }
    return scattered;
}

// Light from scene lights that a medium over a surface scatters toward the
// camera where it lies on the surface, relative to the ambient level (Smoke):
// the surface's own light from the scene bake's visibility (L2).
fn medium_surface_light(pos: vec2<i32>, anisotropy: f32, receiver: MediumTarget) -> vec3<f32> {
    if !scene_lights_present() { return vec3<f32>(0.0); }
    return scene_light_on_surface(pos, receiver.uv, receiver.pixel, receiver.surface, anisotropy);
}

// Image light a medium scatters at Light glow 100, relative to the halo's mean
// light: a lamp's halo outshines its share of the spread light.
const IMAGE_LIGHT_SCATTER_GAIN: f32 = 4.0;
// The Image lights switch of Fog, Smoke and Rain is `MEDIUM_IMAGE_LIGHTS_OPTION`.
fn medium_image_lights_enabled(params: MaskEffectParams) -> bool {
    return params.options[MEDIUM_IMAGE_LIGHTS_OPTION] > 0.5;
}

// Image lights: light from the photograph's light sources that a medium scatters
// toward the camera, in their own colours. `strength` is the medium's Light glow
// (with any gain for a dark albedo), `albedo` its brightness and `scattering` the
// share of light it scatters along the view ray (one minus its transmission), so
// near surfaces with little of the medium in front of them receive little of the
// halo.
fn medium_image_light(
    pos: vec2<i32>,
    params: MaskEffectParams,
    strength: f32,
    albedo: f32,
    scattering: f32,
) -> vec3<f32> {
    if !medium_image_lights_enabled(params) { return vec3<f32>(0.0); }
    return image_light_at(pos) * (strength * IMAGE_LIGHT_SCATTER_GAIN * albedo * scattering);
}
