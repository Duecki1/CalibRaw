// Light Rays (light_shafts.rs): shafts of light from one or more sources, in its
// Scene style here, and in its 2D style through the mask's openings without
// depth (light_shafts_flat.wgsl, `LIGHT_SHAFTS_FLAT_OPTION`).
//
// Scene: light from sources behind the scene that the air scatters toward the
// camera, shadowed by the scene, so trees and window frames cut the rays.
//
// Model. Single scattering in a thin, even haze that dims nothing. Along the view
// ray of a pixel, from the nearest surface's depth (d = 0) to the surface the
// pixel shows, the haze scatters toward the camera
//     L = I · σ · ∫ phase(θ(d)) · falloff(d) · V(d) dd,
// with I the source's colour and strength, falloff its inverse-square falloff
// (`scene_light_falloff`), V the share of it that reaches the air at d past the
// scene's shadows and θ the angle between its light and the view ray. This is the
// in-scattering of GPU Gems 3 ch. 13 (Mitchell, "Volumetric Light Scattering as a
// Post-Process", eq. 1 and 2), with the occlusion taken from the scene's shadow
// cubes instead of the photo's brightness: rays start at the gaps between
// objects, lie over nearer objects only where the air in front of them is lit,
// and do not streak with the photo's texture.
//
// The air light (scene/air_light.wgsl) holds for every view ray the share
//     S = ∫ falloff · V dd / ∫ falloff dd,
// so L ≈ I · σ · S · ∫ phase · falloff dd, whose unshadowed integral is exact per
// segment (`scene_light_falloff_along`) with the phase at each segment's middle,
// as media weigh scene lights (medium.wgsl). A pixel that shows two surfaces
// gets the rays in front of each, blended by its coverage.
//
// A source is a point light of the light bake at the farthest surface; its
// position, reach (Length) and Penetration come from its channel there
// (scene/lights.rs), which every source draws from. Without scene depth or a
// channel it draws nothing. The rays show only where the mask covers.
//
// Colour is scene-linear Rec.2020 relative to the ambient level
// (`effect_ambient`), so the rays keep their strength across exposures; tone
// mapping rolls them into the highlights. Precision f32.

// Scattering of the haze per unit of linear depth at Amount 100, relative to a
// scene light's intensity.
const LIGHT_SHAFTS_HAZE: f32 = 0.25;
// Forward scattering of the haze (Henyey–Greenstein g): rays are brightest
// looking toward their source, as haze and fine aerosols scatter.
const LIGHT_SHAFTS_ANISOTROPY: f32 = 0.5;
// Segments of a view ray for the phase.
const LIGHT_SHAFTS_SEGMENTS: u32 = 8u;
// 2D: how quickly the rays fade with distance from their source, the legacy
// Light Rays' default Fade, and their strength at Amount 100 as there.
const LIGHT_SHAFTS_FLAT_FADE: f32 = 0.6;
const LIGHT_SHAFTS_FLAT_GAIN: f32 = 0.32;

// Whether the slot draws Light Rays' 2D style.
fn light_shafts_is_flat(params: MaskEffectParams) -> bool {
    return params.options[LIGHT_SHAFTS_FLAT_OPTION] > 0.5;
}

// The scene light of the Light Rays source in slot `index`
// (`scene_light_at`): its light bake channel's, or none without one, which is
// always so in the 2D style.
fn light_shafts_source(index: u32, params: MaskEffectParams) -> SceneLight {
    let amount = mask_effect_percent(params, LIGHT_SHAFTS_AMOUNT_LANE);
    let channel = mask_effect_scene_bake_light(params);
    if amount <= 1e-6 || mask_effect_lane(params, LIGHT_SHAFTS_LENGTH_LANE) <= 1e-6
        || channel == 0u || !Geometry::scene_geometry_present() {
        return scene_light_none();
    }
    let baked = Lights::scene_shadow_light(channel - 1u);
    if !Lights::scene_shadow_light_active(baked) { return scene_light_none(); }
    // Its light acts only where its mask covers, as its rays do.
    var confined = SCENE_LIGHT_UNCONFINED;
    if Common::mask_data[index].point_color_meta.z != 0xffffffffu {
        confined = index;
    }
    return SceneLight(
        true,
        baked.position.xyz,
        baked.air.x,
        mask_effect_color(params, LIGHT_SHAFTS_TINT_LANE) * (amount * SCENE_LIGHT_GAIN),
        channel,
        confined,
        baked.position.w,
    );
}

// The light of `light` the haze scatters toward the camera along view ray
// `ray` in front of `surface` of the pixel at full-image `uv`, relative to the
// light's intensity.
fn light_shafts_scattered(
    light: SceneLight,
    ray: vec3<f32>,
    uv: vec2<f32>,
    surface: Geometry::SceneSurface,
) -> f32 {
    let depth = medium_surface_depth(surface);
    let step = depth / f32(LIGHT_SHAFTS_SEGMENTS);
    var scattered = 0.0;
    for (var k = 0u; k < LIGHT_SHAFTS_SEGMENTS; k = k + 1u) {
        let near = f32(k) * step;
        let far = near + step;
        let middle = scene_light_ray_position(light, ray, 0.5 * (near + far));
        scattered += scene_light_phase_at(light, middle, LIGHT_SHAFTS_ANISOTROPY)
            * scene_light_falloff_along(light, ray, near, far);
    }
    return scattered * Geometry::scene_air_light(uv, surface, light.channel - 1u);
}

// The 2D style's light of the source in slot `index` at the pixel `pos`, at
// full-image `uv`: the legacy Light Rays' shafts in the source's tint.
fn light_shafts_flat_light(pos: vec2<i32>, index: u32, params: MaskEffectParams, uv: vec2<f32>) -> vec3<f32> {
    let amount = mask_effect_percent(params, LIGHT_SHAFTS_AMOUNT_LANE);
    let reach = clamp(mask_effect_lane(params, LIGHT_SHAFTS_LENGTH_LANE) / 100.0, 0.0, 2.0);
    if amount <= 1e-6 || reach <= 1e-6 { return vec3<f32>(0.0); }
    let look = LightShaftsFlatLook(
        clamp(mask_effect_lane(params, LIGHT_SHAFTS_SPREAD_LANE), 0.0, 45.0),
        LIGHT_SHAFTS_FLAT_FADE,
        mask_effect_percent(params, LIGHT_SHAFTS_SOFTNESS_LANE),
        mask_effect_percent(params, LIGHT_SHAFTS_VARIATION_LANE),
        mask_effect_lane(params, LIGHT_SHAFTS_RAY_COUNT_LANE),
    );
    let source_uv = mask_effect_lane_pair(params, LIGHT_SHAFTS_SOURCE_X_LANE) / 100.0;
    let penetration = mask_effect_percent(params, LIGHT_SHAFTS_PENETRATION_LANE);
    let ray = light_shafts_flat_ray(pos, index, uv, source_uv, reach, look, penetration);
    return mask_effect_color(params, LIGHT_SHAFTS_TINT_LANE)
        * (ray.shaft * ray.aperture * amount * LIGHT_SHAFTS_FLAT_GAIN);
}

fn apply_light_shafts(pos: vec2<i32>, input_rgb: vec3<f32>) -> vec3<f32> {
    let uv = full_image_uv(pos);
    let ray = scene_light_ray(Camera::scene_image_point(uv, Common::scene_image_size()));
    var flat = vec3<f32>(0.0);
    var pixel: Geometry::ScenePixel;
    var read = false;
    var near = vec3<f32>(0.0);
    var far = vec3<f32>(0.0);
    for (var index = 0u; index < Common::mask_slot_count(); index = index + 1u) {
        let state = Common::mask_data[index].metadata;
        if !Common::mask_slot_applies(state, MASK_EFFECT_LIGHT_SHAFTS_ID) { continue; }
        let params = mask_effect_params(index);
        if light_shafts_is_flat(params) {
            flat += light_shafts_flat_light(pos, index, params, uv);
            continue;
        }
        let light = light_shafts_source(index, params);
        if !light.emits { continue; }
        let coverage = scene_light_coverage(light, pos);
        if coverage <= 1e-6 { continue; }
        if !read {
            pixel = Geometry::scene_pixel(uv, Geometry::scene_unedited_at(pos), 1.0);
            read = true;
        }
        let intensity = light.intensity * (LIGHT_SHAFTS_HAZE * coverage);
        near += intensity * light_shafts_scattered(light, ray, uv, pixel.near);
        if pixel.in_band {
            far += intensity * light_shafts_scattered(light, ray, uv, pixel.far);
        }
    }
    var rgb = input_rgb + flat * light_shafts_flat_headroom(input_rgb);
    if read {
        rgb += Geometry::scene_blend(pixel, near, far) * effect_ambient();
    }
    return rgb;
}
