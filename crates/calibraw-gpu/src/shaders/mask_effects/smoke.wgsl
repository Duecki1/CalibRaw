// Smoke: drifting sheets anchored to the full image, not to the scene's depth.
// It reads the scene only through the effect API (effect_api.wgsl, medium.wgsl).

// Smoke particles scatter less strongly forward than fog droplets.
const SMOKE_LIGHT_ANISOTROPY: f32 = 0.4;
// Smoke is darker than fog by default (its colour is its albedo); this lets
// Light glow show on it at a similar strength.
const SMOKE_LIGHT_GLOW_GAIN: f32 = 4.0;

// Light from scene lights at the smoke, relative to the ambient level, before
// the smoke's albedo and Light glow. Smoke drifts over the scene's surfaces,
// so it is lit where it lies over them: on the surface of the scene bake it
// lies over (medium.wgsl), or at the nearest surface without scene depth.
fn smoke_light_scattering(pos: vec2<i32>, receiver: MediumTarget) -> vec3<f32> {
    return medium_surface_light(pos, SMOKE_LIGHT_ANISOTROPY, receiver);
}

fn apply_smoke(pos: vec2<i32>, input_rgb: vec3<f32>, params: MaskEffectParams) -> vec3<f32> {
    let amount = mask_effect_percent(params, SMOKE_AMOUNT_LANE);
    let density = mask_effect_percent(params, SMOKE_DENSITY_LANE);
    if amount <= 1e-6 || density <= 1e-6 {
        return input_rgb;
    }

    let angle = radians(clamp(mask_effect_lane(params, SMOKE_ANGLE_LANE), -180.0, 180.0));
    let cosine = cos(angle);
    let sine = sin(angle);
    let image_point = effect_image_point(pos);
    var point = vec2<f32>(
        cosine * image_point.x - sine * image_point.y,
        sine * image_point.x + cosine * image_point.y,
    );
    point = point * vec2<f32>(0.78, 1.18);

    let scale = clamp(mask_effect_lane(params, SMOKE_SCALE_LANE) / 100.0, 0.01, 1.0);
    let frequency = mix(11.0, 2.4, scale);
    let turbulence = mask_effect_percent(params, SMOKE_TURBULENCE_LANE);
    let softness = mask_effect_percent(params, SMOKE_SOFTNESS_LANE);
    let seed = clamp(mask_effect_lane(params, SMOKE_SEED_LANE), 0.0, 1000.0);
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
            effect_fbm(base * 0.42 + layer_offset),
            effect_fbm(base * 0.42 + layer_offset + vec2<f32>(17.2, 8.3)),
        ) - vec2<f32>(0.5);
        let warped = base + flow * mix(0.3, 3.5, turbulence);
        let sheet = effect_fbm(warped * vec2<f32>(1.0, 0.32) + layer_offset);
        let filament = abs(sheet - 0.5);
        let width = mix(0.035, 0.105, softness);
        let wisps = exp(-filament * filament / (width * width));
        let envelope = smoothstep(0.30, 0.72,
            effect_noise(base * 0.38 + layer_offset + vec2<f32>(4.3, 23.1)));
        let erosion = effect_noise(warped * 2.7 + layer_offset);
        let detail = mix(1.0, smoothstep(0.12, 0.72, erosion), turbulence * (1.0 - softness) * 0.65);
        optical_depth += wisps * envelope * detail / layer_scale;
    }
    let transmission = exp(-optical_depth * density * density * amount * 2.8);
    // Match smoke illumination to scene ambience, just as fog does; bright
    // picker colors therefore do not turn dark photographs into white paint.
    let ambient = effect_ambient();
    let albedo = mask_effect_picker_color_to_working(mask_effect_color(params, SMOKE_COLOR_LANE));
    let color = albedo * ambient;
    let smoked = input_rgb * transmission + color * (1.0 - transmission);
    // Light glow: the smoke also scatters scene lights (Relight, Light Rays)
    // and, with Image lights, the photograph's own light sources.
    let glow = mask_effect_percent(params, SMOKE_LIGHT_GLOW_LANE);
    if glow <= 1e-6 { return smoked; }
    let strength = glow * SMOKE_LIGHT_GLOW_GAIN;
    let brightness = effect_luma(albedo);
    let scattering = 1.0 - transmission;
    let uv = full_image_uv(pos);
    var lit = vec3<f32>(0.0);
    if Geometry::scene_geometry_present() {
        let scene_rgb = Geometry::scene_unedited_at(pos);
        let pixel = Geometry::scene_pixel(uv, scene_rgb, 1.0);
        lit = smoke_light_scattering(pos, medium_target(pixel, pixel.near, uv));
        if pixel.in_band {
            let far = smoke_light_scattering(pos, medium_target(pixel, pixel.far, uv));
            lit = mix(far, lit, pixel.coverage);
        }
    } else {
        lit = smoke_light_scattering(pos, medium_unbaked(uv));
    }
    return smoked + lit * (strength * ambient * brightness * scattering)
        + medium_image_light(pos, params, strength, brightness, scattering);
}
