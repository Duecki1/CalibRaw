
fn atmosphere_hash(position: vec2<f32>) -> f32 {
    let signed_cell = vec2<i32>(position);
    let cell = bitcast<vec2<u32>>(signed_cell);
    return mask_effect_hash_unit(cell.x * 1597334677u ^ cell.y * 3812015801u);
}

fn atmosphere_noise(position: vec2<f32>) -> f32 {
    let cell = floor(position);
    let fraction = fract(position);
    let smooth_fraction = fraction * fraction * (vec2<f32>(3.0) - 2.0 * fraction);
    let bottom = mix(
        atmosphere_hash(cell),
        atmosphere_hash(cell + vec2<f32>(1.0, 0.0)),
        smooth_fraction.x,
    );
    let top = mix(
        atmosphere_hash(cell + vec2<f32>(0.0, 1.0)),
        atmosphere_hash(cell + vec2<f32>(1.0, 1.0)),
        smooth_fraction.x,
    );
    return mix(bottom, top, smooth_fraction.y);
}

fn atmosphere_fbm(position: vec2<f32>) -> f32 {
    var point = position;
    var amplitude = 0.5;
    var value = 0.0;
    var normalization = 0.0;
    for (var octave = 0u; octave < 5u; octave = octave + 1u) {
        value = value + atmosphere_noise(point) * amplitude;
        normalization = normalization + amplitude;
        point = vec2<f32>(
            point.x * 1.62 + point.y * 1.17,
            point.x * -1.17 + point.y * 1.62,
        ) + vec2<f32>(13.7, 9.2);
        amplitude = amplitude * 0.52;
    }
    return value / max(normalization, 1e-6);
}

fn atmosphere_image_point(pos: vec2<i32>) -> vec2<f32> {
    let full_size = max(
        vec2<f32>(
            f32(Common::camera_uniforms.full_width),
            f32(Common::camera_uniforms.full_height),
        ),
        vec2<f32>(1.0),
    );
    var point = full_image_uv(pos) - vec2<f32>(0.5);
    point.x = point.x * full_size.x / full_size.y;
    return point;
}

// Full-image depth is shared by all fog components; mask coverage remains a
// separate final blend. Scene depth is normalized relative distance (near=0, far=1).
// Level 0 channel x holds it; the other channels and mip levels hold the
// relighting surface (scene_surface.rs, relight.wgsl).
@group(0) @binding(35) var scene_depth_tex: texture_2d<f32>;

// Fog depth at an image pixel: stored depth (channel x), upsampled jointly
// with the image over 6×6 texels. Fog and its light glow change steeply
// across depth edges, and the depth model's edges are blocky and can lie a
// few texels off the image's: within a tent three texels wide, samples whose
// image colour differs from the pixel's are rejected, so an edge follows the
// image's edge where colours differ and blends smoothly where they do not,
// instead of tracing the texel grid as a staircase. Guide colours lie up to
// 3.5 texels away (SCENE_DEPTH_GUIDE_SUPPORT in tiles.rs).
fn fog_depth_at(pos: vec2<i32>) -> f32 {
    let size = vec2<i32>(textureDimensions(scene_depth_tex));
    let p = full_image_uv(pos) * vec2<f32>(size) - vec2<f32>(0.5);
    let base = vec2<i32>(floor(p)) - vec2<i32>(2);
    let full_size = vec2<f32>(
        f32(Common::camera_uniforms.full_width),
        f32(Common::camera_uniforms.full_height),
    );
    let center = sqrt(max(SceneAdjustments::local_effects_at(pos), vec3<f32>(0.0)));
    var total = 0.0;
    var weights = 0.0;
    for (var y = 0; y < 6; y = y + 1) {
        for (var x = 0; x < 6; x = x + 1) {
            let texel = base + vec2<i32>(x, y);
            let offset = abs(vec2<f32>(texel) - p);
            let tent = max(1.0 - offset.x / 3.0, 0.0) * max(1.0 - offset.y / 3.0, 0.0);
            if tent <= 0.0 { continue; }
            let cell = clamp(texel, vec2<i32>(0), size - vec2<i32>(1));
            let uv = (vec2<f32>(cell) + vec2<f32>(0.5)) / vec2<f32>(size);
            let guide_pos = uv * full_size - vec2<f32>(0.5) - vec2<f32>(Common::tile_origin());
            let guide = sqrt(max(mask_effect_source_linear_at(guide_pos), vec3<f32>(0.0)));
            let delta = (guide - center) / max(length(center), 0.15);
            let weight = tent * max(exp(-dot(delta, delta) * 64.0), 0.0001);
            total += textureLoad(scene_depth_tex, cell, 0).x * weight;
            weights += weight;
        }
    }
    return clamp(total / max(weights, 1e-6), 0.0, 1.0);
}

// Level-0 scene-depth texels at an image pixel, and with `with_shadows` the
// relight shadow map's texels (relight.wgsl), which share their grid and
// therefore their weights. Channel x of `surface` is the stored depth;
// relighting reads the surface in the other channels.
struct SceneDepthTexels {
    surface: vec4<f32>,
    shadows: vec4<f32>,
}

fn scene_depth_texels_at(pos: vec2<i32>, with_shadows: bool) -> SceneDepthTexels {
    let size = vec2<i32>(textureDimensions(scene_depth_tex));
    let p = full_image_uv(pos) * vec2<f32>(size) - vec2<f32>(0.5);
    let base = vec2<i32>(floor(p));
    let f = fract(p);
    let full_size = vec2<f32>(
        f32(Common::camera_uniforms.full_width),
        f32(Common::camera_uniforms.full_height),
    );
    let center = sqrt(max(SceneAdjustments::local_effects_at(pos), vec3<f32>(0.0)));
    var total = vec4<f32>(0.0);
    var shadows = vec4<f32>(0.0);
    var weights = 0.0;
    // Joint upsampling rejects samples across image edges instead of blurring
    // background depth into foreground silhouettes. No depth-range mask curve
    // is applied here: a selection is not a measurement of distance.
    for (var y = 0; y < 2; y = y + 1) {
        for (var x = 0; x < 2; x = x + 1) {
            let cell = clamp(base + vec2<i32>(x, y), vec2<i32>(0), size - vec2<i32>(1));
            let uv = (vec2<f32>(cell) + vec2<f32>(0.5)) / vec2<f32>(size);
            let guide_pos = uv * full_size - vec2<f32>(0.5) - vec2<f32>(Common::tile_origin());
            let guide = sqrt(max(mask_effect_source_linear_at(guide_pos), vec3<f32>(0.0)));
            let delta = (guide - center) / max(length(center), 0.15);
            let spatial = select(1.0 - f.x, f.x, x == 1) * select(1.0 - f.y, f.y, y == 1);
            let weight = spatial * max(exp(-dot(delta, delta) * 64.0), 0.0001);
            total += textureLoad(scene_depth_tex, cell, 0) * weight;
            if with_shadows {
                shadows += textureLoad(relight_shadow_map, cell, 0) * weight;
            }
            weights += weight;
        }
    }
    let normalization = max(weights, 1e-6);
    return SceneDepthTexels(total / normalization, shadows / normalization);
}

fn fog_hash3(cell: vec3<i32>) -> f32 {
    let p = bitcast<vec3<u32>>(cell);
    return mask_effect_hash_unit(p.x * 1597334677u ^ p.y * 3812015801u ^ p.z * 2798796415u);
}

fn fog_noise3(point: vec3<f32>) -> f32 {
    let cell = vec3<i32>(floor(point));
    let f = fract(point);
    let u = f * f * f * (f * (f * 6.0 - vec3<f32>(15.0)) + vec3<f32>(10.0));
    let a = mix(
        mix(fog_hash3(cell), fog_hash3(cell + vec3<i32>(1, 0, 0)), u.x),
        mix(fog_hash3(cell + vec3<i32>(0, 1, 0)), fog_hash3(cell + vec3<i32>(1, 1, 0)), u.x), u.y,
    );
    let b = mix(
        mix(fog_hash3(cell + vec3<i32>(0, 0, 1)), fog_hash3(cell + vec3<i32>(1, 0, 1)), u.x),
        mix(fog_hash3(cell + vec3<i32>(0, 1, 1)), fog_hash3(cell + vec3<i32>(1, 1, 1)), u.x), u.y,
    );
    return mix(a, b, u.z);
}

fn fog_onset_integral(distance: f32, start: f32, width: f32) -> f32 {
    let travel = max(distance - start, 0.0);
    let u = clamp(travel / width, 0.0, 1.0);
    return width * (u * u * u - 0.5 * u * u * u * u) + max(travel - width, 0.0);
}

// Forward scattering of fog droplets toward the camera (Henyey–Greenstein g).
const FOG_LIGHT_ANISOTROPY: f32 = 0.6;
// Smoke particles scatter less strongly forward than fog droplets.
const SMOKE_LIGHT_ANISOTROPY: f32 = 0.4;
// Scattered scene light at Light glow 100, relative to the single-scattering
// estimate: lights in photographs read brighter in haze than their surfaces.
const FOG_LIGHT_GLOW_GAIN: f32 = 4.0;
// Image light a medium scatters at Light glow 100, relative to the halo's
// mean light: a lamp's halo outshines its share of the spread light.
const IMAGE_LIGHT_SCATTER_GAIN: f32 = 4.0;
// Fog cells along the view ray: fixed intervals of normalized depth, so rays
// share prefixes and density never jumps with surface distance.
const FOG_CELLS: u32 = 12u;

// The fog volume along one pixel's view ray.
struct FogVolume {
    ray: vec3<f32>,
    start: f32,
    onset_width: f32,
    frequency: f32,
    offset: vec3<f32>,
    variation: f32,
    softness: f32,
}

// Relative density of the fog bank in the cell from `lo` to `cell_end`.
fn fog_bank_density(volume: FogVolume, lo: f32, cell_end: f32) -> f32 {
    let t = 0.5 * (lo + cell_end);
    let point = volume.ray * t * volume.frequency + volume.offset;
    let broad = fog_noise3(point);
    let detail = fog_noise3(point * 2.03 + vec3<f32>(7.1, -3.4, 13.8));
    let field = mix(broad, detail, mix(0.28, 0.08, volume.softness));
    return exp2((field - 0.5) * volume.variation * mix(5.0, 2.5, volume.softness));
}

// Light from scene lights that the fog scatters toward the camera, relative
// to the ambient level, before the fog's albedo and Light glow. Single
// scattering: each cell scatters what its extinction removes from the light,
// attenuated by the fog between it and the camera. Light reaching the fog is
// not shadowed.
fn fog_light_scattering(
    pos: vec2<i32>,
    volume: FogVolume,
    distance: f32,
    extinction: f32,
) -> vec3<f32> {
    if !scene_lights_present() { return vec3<f32>(0.0); }
    // Per cell: near and far depth, extinction per unit depth, and the
    // transmission from the camera to the cell's middle.
    var cells: array<vec4<f32>, FOG_CELLS>;
    var through = 0.0;
    let step = 1.0 / f32(FOG_CELLS);
    for (var i = 0u; i < FOG_CELLS; i = i + 1u) {
        let lo = max(f32(i) * step, volume.start);
        let cell_end = f32(i + 1u) * step;
        let hi = min(cell_end, distance);
        if hi <= lo {
            cells[i] = vec4<f32>(0.0);
            continue;
        }
        var bank_density = 1.0;
        if volume.variation > 1e-6 {
            bank_density = fog_bank_density(volume, lo, cell_end);
        }
        let segment = fog_onset_integral(hi, volume.start, volume.onset_width)
            - fog_onset_integral(lo, volume.start, volume.onset_width);
        let optical_depth = extinction * segment * bank_density;
        cells[i] = vec4<f32>(lo, hi, optical_depth / (hi - lo), exp(-(through + 0.5 * optical_depth)));
        through += optical_depth;
    }

    let uv = full_image_uv(pos);
    var scattered = vec3<f32>(0.0);
    for (var index = 0u; index < scene_light_slots(); index = index + 1u) {
        let light = scene_light_at(index);
        if !light.emits { continue; }
        let coverage = scene_light_coverage(light, pos);
        if coverage <= 1e-6 { continue; }
        let point = uv * light.camera.image_size;
        var received = 0.0;
        for (var i = 0u; i < FOG_CELLS; i = i + 1u) {
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

// Light from light sources in the photograph (tone_analysis.wgsl) arriving
// around image pixel `pos`, spread into a halo, at the current exposure.
@group(0) @binding(45) var image_light_tex: texture_2d<f32>;

fn image_light_at(pos: vec2<i32>) -> vec3<f32> {
    let grid = vec2<f32>(Common::image_light_grid());
    let edge = f32(Common::IMAGE_LIGHT_GRID_LONG);
    let cell = clamp(full_image_uv(pos) * grid, vec2<f32>(0.5), grid - vec2<f32>(0.5));
    let light = textureSampleLevel(
        image_light_tex, SceneAdjustments::local_mask_sampler, cell / edge, 0.0,
    ).xyz;
    return max(light, vec3<f32>(0.0)) * exp2(Common::scene_tone_uniforms.exposure);
}

// Media that scatter light (Fog, Smoke) carry their options in the last
// component of `film_effects` (`set_medium_options` in mask_params.rs).
fn medium_image_lights_enabled(options: vec4<f32>) -> bool {
    return options.w > 0.5;
}

// Image lights: light from the photograph's light sources that a medium
// scatters toward the camera, in their own colours. `strength` is the
// medium's Light glow (with any gain for a dark albedo), `albedo` its
// brightness and `scattering` the share of light it scatters along the view
// ray (one minus its transmission), so near surfaces with little of the
// medium in front of them receive little of the halo.
fn medium_image_light(
    pos: vec2<i32>,
    options: vec4<f32>,
    strength: f32,
    albedo: f32,
    scattering: f32,
) -> vec3<f32> {
    if !medium_image_lights_enabled(options) { return vec3<f32>(0.0); }
    return image_light_at(pos) * (strength * IMAGE_LIGHT_SCATTER_GAIN * albedo * scattering);
}

fn apply_fog(
    pos: vec2<i32>,
    input_rgb: vec3<f32>,
    primary: vec4<f32>,
    secondary: vec4<f32>,
    tertiary: vec4<f32>,
    options: vec4<f32>,
) -> vec3<f32> {
    let amount = clamp(primary.x / 100.0, 0.0, 1.0);
    let density = clamp(primary.y / 100.0, 0.0, 1.0);
    if amount <= 1e-6 || density <= 1e-6 {
        return input_rgb;
    }
    let influence = clamp(tertiary.z / 100.0, 0.0, 1.0);
    // Until depth is generated, use a restrained constant-distance preview.
    // No screen-height or luminance heuristic pretends to know scene geometry.
    var distance = 0.35;
    if Common::scene_tone_uniforms.scene_depth_present != 0u && influence > 1e-6 {
        distance = fog_depth_at(pos);
    }
    distance = mix(1.0, distance, influence);
    let start = clamp(tertiary.y / 100.0, 0.0, 0.95);
    if distance <= start { return input_rgb; }

    let scale = clamp(primary.z / 100.0, 0.01, 1.0);
    let softness = clamp(primary.w / 100.0, 0.0, 1.0);
    let variation = clamp(secondary.w / 100.0, 0.0, 1.0);
    let seed = clamp(tertiary.x, 0.0, 1000.0);
    let offset = vec3<f32>(seed * 0.071 + 19.3, seed * -0.113 + 47.1, seed * 0.053 + 11.7);
    let frequency = mix(7.0, 1.6, scale);
    let image_point = atmosphere_image_point(pos);
    // A perspective volume in relative scene units. Integrate only to the
    // visible surface; foreground objects truncate the same volume as the
    // background. Fixed world-space intervals preserve shared ray prefixes.
    let ray = vec3<f32>(image_point * 1.25, 1.0);
    let step = 1.0 / f32(FOG_CELLS);
    let onset_width = mix(0.025, 0.18, softness);
    let volume = FogVolume(ray, start, onset_width, frequency, offset, variation, softness);
    var optical_length = fog_onset_integral(distance, start, onset_width);
    if variation > 1e-6 {
        optical_length = 0.0;
        for (var i = 0u; i < FOG_CELLS; i = i + 1u) {
            let lo = max(f32(i) * step, start);
            let cell_end = f32(i + 1u) * step;
            let hi = min(cell_end, distance);
            if hi <= lo { continue; }
            // A partial last interval uses the same density as the full interval,
            // so increasing surface distance can never remove accumulated fog.
            let bank_density = fog_bank_density(volume, lo, cell_end);
            let segment = fog_onset_integral(hi, start, onset_width)
                - fog_onset_integral(lo, start, onset_width);
            optical_length += segment * bank_density;
        }
    }
    // Beer-Lambert extinction and constant-environment single scattering in
    // scene-linear Rec.2020. Amount changes concentration, not a screen overlay.
    // https://pbr-book.org/4ed/Volume_Scattering/Transmittance
    let optical_depth = 6.0 * density * density * amount * optical_length * length(ray);
    let transmission = exp(-optical_depth);
    // Match airlight to scene illumination, avoiding white self-luminous fog
    // in dark photographs.
    let ambient = scene_ambient_level() * 1.15;
    let color = mask_effect_picker_color_to_working(secondary.xyz);
    let airlight = color * ambient;
    let fogged = input_rgb * transmission + airlight * (1.0 - transmission);
    // Light glow: the fog also scatters scene lights (Relight, Light Rays),
    // brightest looking toward a light. The fog's brightness is its albedo,
    // so the glow keeps the light's colour.
    let glow = clamp(tertiary.w / 100.0, 0.0, 1.0);
    if glow <= 1e-6 { return fogged; }
    let albedo = Common::safe_luma(color);
    let extinction = 6.0 * density * density * amount * length(ray);
    let scattered = fog_light_scattering(pos, volume, distance, extinction);
    return fogged + scattered * (glow * FOG_LIGHT_GLOW_GAIN * ambient * albedo)
        + medium_image_light(pos, options, glow, albedo, 1.0 - transmission);
}

// Smoke is darker than fog by default (its colour is its albedo); this lets
// Light glow show on it at a similar strength.
const SMOKE_LIGHT_GLOW_GAIN: f32 = 4.0;

// Light from scene lights at the smoke, relative to the ambient level, before
// the smoke's albedo and Light glow. Smoke drifts over the scene's surfaces,
// so it is lit where it lies over them: at the pixel's scene depth, or at the
// nearest surface without depth.
fn smoke_light_scattering(pos: vec2<i32>) -> vec3<f32> {
    if !scene_lights_present() { return vec3<f32>(0.0); }
    var depth = 0.0;
    if Common::scene_tone_uniforms.scene_depth_present != 0u {
        depth = fog_depth_at(pos);
    }
    let uv = full_image_uv(pos);
    var lit = vec3<f32>(0.0);
    for (var index = 0u; index < scene_light_slots(); index = index + 1u) {
        let light = scene_light_at(index);
        if !light.emits { continue; }
        let coverage = scene_light_coverage(light, pos);
        if coverage <= 1e-6 { continue; }
        let position = scene_camera_point(
            light.camera, uv * light.camera.image_size, scene_depth_z(light.camera, depth),
        );
        lit += light.intensity * (scene_light_falloff(light, position)
            * scene_light_phase_at(light, position, SMOKE_LIGHT_ANISOTROPY) * coverage);
    }
    return lit;
}

fn apply_smoke(
    pos: vec2<i32>,
    input_rgb: vec3<f32>,
    primary: vec4<f32>,
    secondary: vec4<f32>,
    tertiary: vec4<f32>,
    options: vec4<f32>,
) -> vec3<f32> {
    let amount = clamp(primary.x / 100.0, 0.0, 1.0);
    let density = clamp(primary.y / 100.0, 0.0, 1.0);
    if amount <= 1e-6 || density <= 1e-6 {
        return input_rgb;
    }

    let angle = radians(clamp(secondary.w, -180.0, 180.0));
    let cosine = cos(angle);
    let sine = sin(angle);
    let image_point = atmosphere_image_point(pos);
    var point = vec2<f32>(
        cosine * image_point.x - sine * image_point.y,
        sine * image_point.x + cosine * image_point.y,
    );
    point = point * vec2<f32>(0.78, 1.18);

    let scale = clamp(primary.z / 100.0, 0.01, 1.0);
    let frequency = mix(11.0, 2.4, scale);
    let turbulence = clamp(primary.w / 100.0, 0.0, 1.0);
    let softness = clamp(tertiary.x / 100.0, 0.0, 1.0);
    let seed = clamp(tertiary.y, 0.0, 1000.0);
    let offset = vec2<f32>(seed * 0.097 + 31.6, seed * -0.067 + 8.9);

    // Three translucent layers carry stretched, warped density sheets.
    // Density controls optical thickness; it never lowers a threshold until
    // every pixel becomes an opaque wash. Seed and scale live in image space.
    var optical_depth = 0.0;
    for (var layer = 0u; layer < 3u; layer += 1u) {
        let layer_scale = 1.0 + f32(layer) * 0.47;
        let layer_offset = offset + vec2<f32>(19.7, -13.1) * f32(layer);
        let base = point * frequency * layer_scale;
        let flow = vec2<f32>(
            atmosphere_fbm(base * 0.42 + layer_offset),
            atmosphere_fbm(base * 0.42 + layer_offset + vec2<f32>(17.2, 8.3)),
        ) - vec2<f32>(0.5);
        let warped = base + flow * mix(0.3, 3.5, turbulence);
        let sheet = atmosphere_fbm(warped * vec2<f32>(1.0, 0.32) + layer_offset);
        let filament = abs(sheet - 0.5);
        let width = mix(0.035, 0.105, softness);
        let wisps = exp(-filament * filament / (width * width));
        let envelope = smoothstep(0.30, 0.72,
            atmosphere_noise(base * 0.38 + layer_offset + vec2<f32>(4.3, 23.1)));
        let erosion = atmosphere_noise(warped * 2.7 + layer_offset);
        let detail = mix(1.0, smoothstep(0.12, 0.72, erosion), turbulence * (1.0 - softness) * 0.65);
        optical_depth += wisps * envelope * detail / layer_scale;
    }
    let transmission = exp(-optical_depth * density * density * amount * 2.8);
    // Match smoke illumination to scene ambience, just as fog does; bright
    // picker colors therefore do not turn dark photographs into white paint.
    let ambient = scene_ambient_level();
    let albedo = mask_effect_picker_color_to_working(secondary.xyz);
    let color = albedo * ambient;
    let smoked = input_rgb * transmission + color * (1.0 - transmission);
    // Light glow: the smoke also scatters scene lights (Relight, Light Rays)
    // and, with Image lights, the photograph's own light sources.
    let glow = clamp(tertiary.z / 100.0, 0.0, 1.0);
    if glow <= 1e-6 { return smoked; }
    let strength = glow * SMOKE_LIGHT_GLOW_GAIN;
    let brightness = Common::safe_luma(albedo);
    let scattering = 1.0 - transmission;
    let lit = smoke_light_scattering(pos);
    return smoked + lit * (strength * ambient * brightness * scattering)
        + medium_image_light(pos, options, strength, brightness, scattering);
}
