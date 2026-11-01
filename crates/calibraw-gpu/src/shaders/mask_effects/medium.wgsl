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

// Light from scene lights that the medium scatters toward the camera, relative to
// the ambient level, before its albedo and Light glow. Single scattering: each cell
// scatters what its extinction removes from the light, attenuated by the medium
// between it and the camera. Light reaching the medium is not shadowed.
fn medium_light_scattering(
    pos: vec2<i32>,
    volume: MediumVolume,
    distance: f32,
    extinction: f32,
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
    var scattered = vec3<f32>(0.0);
    for (var index = 0u; index < Common::mask_slot_count(); index = index + 1u) {
        let light = scene_light_at(index);
        if !light.emits { continue; }
        let coverage = scene_light_coverage(light, pos);
        if coverage <= 1e-6 { continue; }
        let point = uv * light.camera.image_size;
        var received = 0.0;
        for (var i = 0u; i < MEDIUM_CELLS; i = i + 1u) {
            let cell = cells[i];
            if cell.z <= 0.0 { continue; }
            let middle = scene_camera_point(
                light.camera, point, scene_depth_z(light.camera, 0.5 * (cell.x + cell.y)),
            );
            received += cell.z * cell.w * scene_light_phase_at(light, middle, FOG_LIGHT_ANISOTROPY)
                * scene_light_falloff_along(light, point, cell.x, cell.y);
        }
        scattered += light.intensity * (received * coverage);
    }
    return scattered;
}

// Image light a medium scatters at Light glow 100, relative to the halo's mean
// light: a lamp's halo outshines its share of the spread light.
const IMAGE_LIGHT_SCATTER_GAIN: f32 = 4.0;
// The Image lights switch of Fog, Smoke and Rain (effect_lanes.rs).
const MEDIUM_IMAGE_LIGHTS_OPTION: u32 = 3u;

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
