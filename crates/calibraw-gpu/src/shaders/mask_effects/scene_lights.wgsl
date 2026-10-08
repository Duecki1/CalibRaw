// Scene lights: light sources that effects place in the scene and that other
// effects receive, so effects react to each other's light regardless of
// their order in the edit.
//
// Emitters. `scene_light_at` turns an effect slot into a light: Relight's
// point light, and the Light Rays source as a distant light behind the scene.
// A new emitter is one more case there. Receivers loop over the effect slots
// (`scene_light_slots`), skip lights that do not emit and weight each light by
// `scene_light_coverage`. Fog and Smoke scatter it (atmosphere.wgsl).
//
// Scene frame. Lights and receivers share a pinhole camera in relative units:
// x right and y down as in the image, z forward, the nearest surface at z = 1
// and the farthest at z = 1 + relief, so normalized scene depth (0 near,
// 1 far) maps linearly to z. Image points are in shorter-edge units with
// (0, 0) at the top-left corner. Each light carries the frame it was placed
// in (its relief), and receivers measure distances to it in that frame.
//
// Intensities are scene-linear Rec.2020 relative to the scene's ambient level
// (`scene_ambient_level`), so a light keeps its strength across exposures.
// Everything depends only on full-image coordinates, the effect uniforms and
// global tone statistics, so export tiles match the preview.

// tan(half the field of view) across the shorter edge, about a 30 mm lens on
// full frame. Monocular depth carries no focal length.
const SCENE_FOCAL_TAN: f32 = 0.4;
// Relight's Relief control (0–1) for an emitter without one of its own.
const SCENE_DEFAULT_RELIEF_CONTROL: f32 = 0.5;
// Relative intensity of Amount 100 on a surface facing a nearby light.
const SCENE_LIGHT_GAIN: f32 = 2.5;
// The Light Rays source relative to a Relight light of the same Amount, and
// its reach relative to the rays' Length. Light Rays already draw their own
// shafts; as a distant light it adds a glow around its source rather than
// flooding a whole scene that lies within its reach.
const LIGHT_RAYS_LIGHT_INTENSITY: f32 = 0.4;
const LIGHT_RAYS_LIGHT_REACH: f32 = 0.35;
const SCENE_LIGHT_UNCONFINED: u32 = 0xffffffffu;

struct SceneCamera {
    // Full image in shorter-edge units.
    image_size: vec2<f32>,
    // Depth range in units of the nearest surface's distance.
    relief: f32,
}

// The scene frame for a Relief control in [0, 1].
fn scene_camera(relief_control: f32) -> SceneCamera {
    let full_size = max(vec2<f32>(
        f32(Common::camera_uniforms.full_width),
        f32(Common::camera_uniforms.full_height),
    ), vec2<f32>(1.0));
    return SceneCamera(
        full_size / min(full_size.x, full_size.y),
        mix(0.25, 6.0, pow(clamp(relief_control, 0.0, 1.0), 1.5)),
    );
}

fn scene_lateral(camera: SceneCamera, point: vec2<f32>) -> vec2<f32> {
    return 2.0 * SCENE_FOCAL_TAN * (point - 0.5 * camera.image_size);
}

fn scene_camera_point(camera: SceneCamera, point: vec2<f32>, z: f32) -> vec3<f32> {
    return vec3<f32>(scene_lateral(camera, point) * z, z);
}

fn scene_image_point(camera: SceneCamera, position: vec3<f32>) -> vec2<f32> {
    return 0.5 * camera.image_size + position.xy / (2.0 * SCENE_FOCAL_TAN * position.z);
}

fn scene_depth_z(camera: SceneCamera, depth: f32) -> f32 {
    return 1.0 + camera.relief * depth;
}

// The scene's ambient light level: scene-linear middle grey at the exposure
// of the image's ambient tone. Global tone statistics are shared by export
// tiles.
fn scene_ambient_level() -> f32 {
    let ambient_ev = Tonemap::tone_stats.percentiles_0_field.w + Common::scene_tone_uniforms.exposure;
    return ToneCommon::SCENE_MIDDLE_GREY * exp2(clamp(ambient_ev, -12.0, 6.0));
}

struct SceneLight {
    emits: bool,
    // The frame the light was placed in.
    camera: SceneCamera,
    position: vec3<f32>,
    // Distance at which the light has halved, in scene units.
    reach: f32,
    // Colour times strength, relative to the ambient level.
    intensity: vec3<f32>,
    // The effect slot whose mask confines the light, or SCENE_LIGHT_UNCONFINED.
    confined_to: u32,
}

fn scene_light_inactive() -> SceneLight {
    return SceneLight(
        false, SceneCamera(vec2<f32>(1.0), 1.0), vec3<f32>(0.0), 1.0, vec3<f32>(0.0),
        SCENE_LIGHT_UNCONFINED,
    );
}

// Relight's light from its packed parameters.
fn relight_scene_light(params: MaskEffectParams) -> SceneLight {
    let camera = scene_camera(mask_effect_lane(params, RELIGHT_RELIEF_LANE) / 100.0);
    // Light depth: -1 at the camera, 0 at the nearest surface, 1 at the farthest.
    let light_depth = clamp(mask_effect_lane(params, RELIGHT_DEPTH_LANE) / 100.0, -1.0, 1.0);
    let z = select(
        scene_depth_z(camera, light_depth),
        1.0 + 0.9 * light_depth,
        light_depth < 0.0,
    );
    // Reach is in shorter-edge widths at the nearest surface.
    let reach = clamp(mask_effect_lane(params, RELIGHT_REACH_LANE) / 100.0, 0.1, 4.0)
        * 2.0 * SCENE_FOCAL_TAN;
    let amount = clamp(mask_effect_lane(params, RELIGHT_AMOUNT_LANE) / 100.0, 0.0, 1.0);
    let source = mask_effect_lane_pair(params, RELIGHT_SOURCE_X_LANE);
    return SceneLight(
        amount > 1e-6,
        camera,
        scene_camera_point(camera, source / 100.0 * camera.image_size, z),
        reach,
        mask_effect_picker_color_to_working(mask_effect_color(params, RELIGHT_COLOR_LANE))
            * (amount * SCENE_LIGHT_GAIN),
        SCENE_LIGHT_UNCONFINED,
    );
}

// The Light Rays source as a light just beyond the farthest surface, where
// rays come from. Its mask marks where rays are emitted, not where the light
// acts, so the light is never confined.
fn light_rays_scene_light(params: MaskEffectParams) -> SceneLight {
    let camera = scene_camera(SCENE_DEFAULT_RELIEF_CONTROL);
    let z = scene_depth_z(camera, 1.0);
    let amount = clamp(mask_effect_lane(params, LIGHT_RAYS_AMOUNT_LANE) / 100.0, 0.0, 1.0);
    // Length is in shorter-edge widths in the image, measured at the source.
    let rays_length = clamp(mask_effect_lane(params, LIGHT_RAYS_LENGTH_LANE) / 100.0, 0.0, 2.0);
    let source = mask_effect_lane_pair(params, LIGHT_RAYS_SOURCE_X_LANE);
    return SceneLight(
        amount > 1e-6 && rays_length > 1e-6,
        camera,
        scene_camera_point(camera, source / 100.0 * camera.image_size, z),
        max(rays_length * LIGHT_RAYS_LIGHT_REACH * 2.0 * SCENE_FOCAL_TAN * z, 1e-3),
        mask_effect_picker_color_to_working(mask_effect_color(params, LIGHT_RAYS_COLOR_LANE))
            * (amount * SCENE_LIGHT_GAIN * LIGHT_RAYS_LIGHT_INTENSITY),
        SCENE_LIGHT_UNCONFINED,
    );
}

fn scene_light_slots() -> u32 {
    return min(Common::scene_tone_uniforms.mask_counts.x, Common::MAX_RENDER_MASK_SLOTS);
}

// The light the effect in slot `index` emits; inactive when it emits none.
fn scene_light_at(index: u32) -> SceneLight {
    let state = Common::mask_data[index].metadata;
    if state.x == 0u || state.y == 0u { return scene_light_inactive(); }
    let params = mask_effect_params(index);
    let effect_id = Common::mask_effect_id(state);
    if effect_id == MASK_EFFECT_RELIGHT_ID {
        var light = relight_scene_light(params);
        // A masked Relight lights only its mask.
        if Common::mask_data[index].point_color_meta.z != 0xffffffffu {
            light.confined_to = index;
        }
        return light;
    }
    if effect_id == MASK_EFFECT_LIGHT_RAYS_ID {
        return light_rays_scene_light(params);
    }
    return scene_light_inactive();
}

// Whether any effect slot emits a scene light. It is the same for every
// pixel, so receivers skip their per-pixel setup (depth reads, volume cells)
// when there is nothing to receive.
fn scene_lights_present() -> bool {
    for (var index = 0u; index < scene_light_slots(); index = index + 1u) {
        if scene_light_at(index).emits { return true; }
    }
    return false;
}

// How much of a light acts at an image pixel: its mask's coverage when the
// light is confined to one.
fn scene_light_coverage(light: SceneLight, pos: vec2<i32>) -> f32 {
    if light.confined_to == SCENE_LIGHT_UNCONFINED { return 1.0; }
    return SceneAdjustments::local_mask_weight(pos, light.confined_to);
}

// Inverse-square falloff with a finite core: 1 at the light, 1/2 at its reach.
fn scene_light_falloff(light: SceneLight, position: vec3<f32>) -> f32 {
    let offset = position - light.position;
    return 1.0 / (1.0 + dot(offset, offset) / (light.reach * light.reach));
}

// ∫ falloff dt along the view ray of image point `point` between normalized
// depths `near` and `far`. The ray's position is linear in depth
// (scene_camera_point), so the integral has a closed form and a light close
// to the ray leaves no sampling bands.
fn scene_light_falloff_along(light: SceneLight, point: vec2<f32>, near: f32, far: f32) -> f32 {
    if far <= near { return 0.0; }
    // position(t) = start + direction * t
    let ray = vec3<f32>(scene_lateral(light.camera, point), 1.0);
    let start = ray - light.position;
    let direction = ray * light.camera.relief;
    let speed_squared = dot(direction, direction);
    let speed = sqrt(speed_squared);
    let closest = -dot(start, direction) / speed_squared;
    let miss_squared = max(dot(start, start) - speed_squared * closest * closest, 0.0);
    let reach_squared = light.reach * light.reach;
    let core = sqrt(reach_squared + miss_squared);
    return reach_squared / (speed * core)
        * (atan(speed * (far - closest) / core) - atan(speed * (near - closest) / core));
}

// Share of scattered light that leaves evenly in all directions: multiple
// scattering in a dense medium spreads part of the light, so a lamp also
// lights the haze in front of it as seen from the camera.
const SCENE_LIGHT_ISOTROPIC_SHARE: f32 = 0.3;

// Phase function scaled so that isotropic scattering is 1: a Henyey–Greenstein
// lobe plus the isotropic share. `cos_angle` is between the light's direction
// of travel and the direction toward the camera; `anisotropy` > 0 scatters
// forward, so a medium glows most when one looks toward the light through it.
fn scene_light_phase(cos_angle: f32, anisotropy: f32) -> f32 {
    let g = clamp(anisotropy, -0.95, 0.95);
    let denominator = max(1.0 + g * g - 2.0 * g * cos_angle, 1e-4);
    let lobe = (1.0 - g * g) / (denominator * sqrt(denominator));
    return mix(lobe, 1.0, SCENE_LIGHT_ISOTROPIC_SHARE);
}

// Phase of light from `light` scattered at `position` toward the camera.
fn scene_light_phase_at(light: SceneLight, position: vec3<f32>, anisotropy: f32) -> f32 {
    let travel = position - light.position;
    let toward_camera = -position;
    let lengths = max(length(travel) * length(toward_camera), 1e-6);
    return scene_light_phase(dot(travel, toward_camera) / lengths, anisotropy);
}
