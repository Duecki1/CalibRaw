// Fog: a medium that builds with distance from the camera to each visible surface.
// It reads the scene through the scene bake (scene/geometry.wgsl, medium.wgsl):
// a pixel is up to two surfaces, each fogged to its own depth and blended by how
// much of the pixel the near one covers, so a silhouette or tree line gets no rim
// of the other surface's fog. Without scene depth it is a veil at an assumed depth.
//
// Depth is the scene's linear depth d (0 the nearest surface, 1 the farthest; sky
// counts as 1). Fog start is a control depth c (even in the log of distance,
// scene/camera.wgsl) and is converted to d once.

// Scattered scene light at Light glow 100, relative to the single-scattering
// estimate: lights in photographs read brighter in haze than their surfaces.
const FOG_LIGHT_GLOW_GAIN: f32 = 4.0;
// Fog's start control (0 to 95) of a flat scene, without depth: the range of the
// depth maps older edits were made with (`migrations::scene_depth_controls`).
const FOG_FLAT_RANGE_RATIO: f32 = 9.695652;

// What every surface of a pixel shares.
struct FogMedium {
    volume: MediumVolume,
    // 6 · density² · amount: extinction per unit of optical length and ray length.
    strength: f32,
    color: vec3<f32>,
    ambient: f32,
    glow: f32,
}

// The fog in front of a surface at linear depth `distance` seen as `rgb`.
fn fog_shade(
    pos: vec2<i32>,
    rgb: vec3<f32>,
    distance: f32,
    fog: FogMedium,
    params: MaskEffectParams,
    receiver: MediumTarget,
) -> vec3<f32> {
    let volume = fog.volume;
    if distance <= volume.start { return rgb; }
    let step = 1.0 / f32(MEDIUM_CELLS);
    var optical_length = medium_onset_integral(distance, volume.start, volume.onset_width);
    if volume.variation > 1e-6 {
        optical_length = 0.0;
        for (var i = 0u; i < MEDIUM_CELLS; i = i + 1u) {
            let lo = max(f32(i) * step, volume.start);
            let cell_end = f32(i + 1u) * step;
            let hi = min(cell_end, distance);
            if hi <= lo { continue; }
            // A partial last interval uses the same density as the full interval,
            // so increasing surface distance can never remove accumulated fog.
            let bank_density = medium_bank_density(volume, lo, cell_end);
            let segment = medium_onset_integral(hi, volume.start, volume.onset_width)
                - medium_onset_integral(lo, volume.start, volume.onset_width);
            optical_length += segment * bank_density;
        }
    }
    // Beer-Lambert extinction and constant-environment single scattering in
    // scene-linear Rec.2020. Amount changes concentration, not a screen overlay.
    // https://pbr-book.org/4ed/Volume_Scattering/Transmittance
    let ray_length = length(volume.ray);
    let transmission = exp(-fog.strength * optical_length * ray_length);
    // Airlight matches the scene's illumination, avoiding white self-luminous fog
    // in dark photographs.
    let airlight = fog.color * fog.ambient;
    let fogged = rgb * transmission + airlight * (1.0 - transmission);
    // Light glow: the fog also scatters scene lights (Relight, Light Rays),
    // brightest looking toward a light. The fog's brightness is its albedo,
    // so the glow keeps the light's colour.
    if fog.glow <= 1e-6 { return fogged; }
    let albedo = effect_luma(fog.color);
    let scattered = medium_light_scattering(pos, volume, distance, fog.strength * ray_length, receiver);
    return fogged + scattered * (fog.glow * FOG_LIGHT_GLOW_GAIN * fog.ambient * albedo)
        + medium_image_light(pos, params, fog.glow, albedo, 1.0 - transmission);
}

fn apply_fog(pos: vec2<i32>, input_rgb: vec3<f32>, params: MaskEffectParams) -> vec3<f32> {
    let amount = mask_effect_percent(params, FOG_AMOUNT_LANE);
    let density = mask_effect_percent(params, FOG_DENSITY_LANE);
    if amount <= 1e-6 || density <= 1e-6 {
        return input_rgb;
    }
    let influence = mask_effect_percent(params, FOG_DEPTH_INFLUENCE_LANE);
    let baked = Geometry::scene_geometry_present() && influence > 1e-6;
    let start_control = clamp(mask_effect_lane(params, FOG_START_LANE) / 100.0, 0.0, 0.95);
    var start = (pow(FOG_FLAT_RANGE_RATIO, start_control) - 1.0) / (FOG_FLAT_RANGE_RATIO - 1.0);
    if baked {
        start = Camera::scene_depth_of_control(Bake::scene_bake.camera, start_control);
    }

    let scale = clamp(mask_effect_lane(params, FOG_SCALE_LANE) / 100.0, 0.01, 1.0);
    let softness = mask_effect_percent(params, FOG_SOFTNESS_LANE);
    let variation = mask_effect_percent(params, FOG_VARIATION_LANE);
    let seed = clamp(mask_effect_lane(params, FOG_SEED_LANE), 0.0, 1000.0);
    let offset = vec3<f32>(seed * 0.071 + 19.3, seed * -0.113 + 47.1, seed * 0.053 + 11.7);
    let frequency = mix(7.0, 1.6, scale);
    // A perspective volume in relative scene units. Integrate only to the
    // visible surface; foreground objects truncate the same volume as the
    // background. Fixed world-space intervals preserve shared ray prefixes.
    let ray = vec3<f32>(effect_image_point(pos) * 1.25, 1.0);
    let onset_width = mix(0.025, 0.18, softness);
    let fog = FogMedium(
        MediumVolume(ray, start, onset_width, frequency, offset, variation, softness),
        6.0 * density * density * amount,
        mask_effect_picker_color_to_working(mask_effect_color(params, FOG_COLOR_LANE)),
        effect_ambient() * 1.15,
        mask_effect_percent(params, FOG_LIGHT_GLOW_LANE),
    );
    let uv = full_image_uv(pos);
    if !baked {
        // The assumed depth of a scene without depth.
        let distance = mix(1.0, EFFECT_SCENE_DEPTH_UNKNOWN, influence);
        return fog_shade(pos, input_rgb, distance, fog, params, medium_unbaked(uv));
    }

    let scene_rgb = Geometry::scene_unedited_at(pos);
    let pixel = Geometry::scene_pixel(uv, scene_rgb, 1.0);
    let colors = Geometry::scene_split(pixel, scene_rgb, input_rgb);
    let near = fog_shade(
        pos, colors[0], mix(1.0, medium_surface_depth(pixel.near), influence), fog, params,
        medium_target(pixel, pixel.near, uv),
    );
    if !pixel.in_band {
        return near;
    }
    let far = fog_shade(
        pos, colors[1], mix(1.0, medium_surface_depth(pixel.far), influence), fog, params,
        medium_target(pixel, pixel.far, uv),
    );
    return Geometry::scene_blend(pixel, near, far);
}
