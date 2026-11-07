// Scene lights: light sources that effects place in the scene and that other
// effects receive, so effects react to each other's light regardless of
// their order in the edit.
//
// Emitters. `scene_light_at` turns an effect slot into a light: one of
// Relight's point lights, and a Light Rays source as a point light at the
// farthest surface. A new emitter is one more case there. Effects that emit
// over an area of the image (a self-illuminating Glow, Neon) are lights of
// scene_emission.wgsl, which receivers add through the same functions.
// Receivers loop over the effect slots (`Common::mask_slot_count`), skip lights
// that do not emit and weight each light by `scene_light_coverage`. Fog, Smoke
// and Rain's mist scatter it (medium.wgsl), drops and wet ground reflect it.
// The scene's surfaces shadow it through the light bake (scene/lights.rs): a
// light has a channel there (`SceneLight::channel`), from which a surface reads
// its visibility (`Geometry::scene_visibility`), a point in the air the cube
// (`Geometry::scene_point_visibility`) and a view ray the air light
// (`Geometry::scene_air_light`).
//
// Scene frame. Lights and receivers share the shading frame of
// scene/camera.wgsl at the light's relief scale E: the camera at the origin,
// x right and y down as in the image, z forward, the nearest surface at z = 1
// and the farthest at z = 1 + E · R, so linear depth d maps linearly to z, and
// negative control depths lie in the open air before the nearest surface.
// Without scene depth the scene is a plane (`scene_light_camera`).
// Image points are in shorter-edge units from the image's centre.
//
// Intensities are scene-linear Rec.2020 relative to the scene's ambient level
// (`scene_ambient_level`, scene_environment.wgsl), so a light keeps its
// strength across exposures. Everything depends only on full-image
// coordinates, the effect uniforms and global tone statistics, so export tiles
// match the preview.

// Relative intensity of Amount 100 on a surface facing a nearby light.
const SCENE_LIGHT_GAIN: f32 = 2.5;
// The Light Rays source relative to a Relight light of the same Amount, and
// its reach relative to the rays' Length (`rays_reach` in scene/lights.rs).
// Light Rays already draw their own shafts; as a distant light it adds a glow
// around its source rather than flooding a whole scene that lies within its
// reach.
const LIGHT_RAYS_LIGHT_INTENSITY: f32 = 0.4;
const LIGHT_RAYS_LIGHT_REACH: f32 = 0.35;
const SCENE_LIGHT_UNCONFINED: u32 = 0xffffffffu;
// The distance an effect assumes until depth is generated: a restrained
// constant, as no screen-height or luminance heuristic can know the scene's
// geometry. Linear depth.
const SCENE_DEPTH_UNKNOWN: f32 = 0.35;

// The camera lights and receivers share: the bake's, or the plane Relight
// lights without scene depth (relight.wgsl).
fn scene_light_camera() -> Camera::SceneCamera {
    if Geometry::scene_geometry_present() {
        return Bake::scene_bake.camera;
    }
    return relight_flat_camera();
}

// A scene light in the shading frame at its own relief scale.
struct SceneLight {
    emits: bool,
    position: vec3<f32>,
    // Distance at which the light has halved.
    reach: f32,
    // Colour times strength, relative to the ambient level.
    intensity: vec3<f32>,
    // Its light bake channel plus one, or 0 when the bake does not hold it.
    channel: u32,
    // The effect slot whose mask confines the light, or SCENE_LIGHT_UNCONFINED.
    confined_to: u32,
    // The frame's relief scale E.
    relief_scale: f32,
}

fn scene_light_none() -> SceneLight {
    return SceneLight(
        false, vec3<f32>(0.0), 1.0, vec3<f32>(0.0), 0u, SCENE_LIGHT_UNCONFINED, 1.0,
    );
}

// The light the effect in slot `index` emits; none for a slot that emits none.
// Relight's is placed as relight.wgsl places it (and scene/camera.rs
// `light_position` for the light bake); the Light Rays source lies at the
// farthest surface, where rays come from, and its mask marks where rays are
// emitted, not where the light acts, so it is never confined.
fn scene_light_at(index: u32) -> SceneLight {
    let state = Common::mask_data[index].metadata;
    if !Common::mask_slot_active(state) { return scene_light_none(); }
    let effect_id = Common::mask_effect_id(state);
    let params = mask_effect_params(index);
    let camera = scene_light_camera();
    let aspect = Common::scene_image_size();
    if effect_id == MASK_EFFECT_RELIGHT_ID {
        let amount = mask_effect_percent(params, RELIGHT_AMOUNT_LANE);
        if amount <= 1e-6 { return scene_light_none(); }
        let relief_scale = clamp(mask_effect_lane(params, RELIGHT_RELIEF_SCALE_LANE) / 100.0, 0.1, 3.0);
        let control = clamp(mask_effect_lane(params, RELIGHT_LIGHT_DEPTH_LANE) / 100.0, -1.0, 1.0);
        let source = mask_effect_lane_pair(params, RELIGHT_SOURCE_X_LANE) / 100.0;
        let z = Camera::scene_light_frame_depth(camera, control, relief_scale);
        var confined = SCENE_LIGHT_UNCONFINED;
        if Common::mask_data[index].point_color_meta.z != 0xffffffffu {
            confined = index;
        }
        return SceneLight(
            true,
            Camera::scene_frame_position(camera, Camera::scene_image_point(source, aspect), z),
            // Reach is in shorter-edge widths at the nearest surface.
            clamp(mask_effect_lane(params, RELIGHT_REACH_LANE) / 100.0, 0.1, 4.0) * 2.0 * camera.lens.x,
            mask_effect_picker_color_to_working(mask_effect_color(params, RELIGHT_COLOR_LANE))
                * (amount * SCENE_LIGHT_GAIN),
            mask_effect_scene_bake_light(params),
            confined,
            relief_scale,
        );
    }
    if effect_id == MASK_EFFECT_LIGHT_RAYS_ID {
        let amount = mask_effect_percent(params, LIGHT_RAYS_AMOUNT_LANE);
        // Length is in shorter-edge widths in the image, measured at the source.
        let rays_length = clamp(mask_effect_lane(params, LIGHT_RAYS_LENGTH_LANE) / 100.0, 0.0, 2.0);
        if amount <= 1e-6 || rays_length <= 1e-6 { return scene_light_none(); }
        let source = mask_effect_lane_pair(params, LIGHT_RAYS_SOURCE_X_LANE) / 100.0;
        let z = Camera::scene_frame_depth(camera, 1.0, 1.0);
        // Its light acts only where its mask covers, as its shafts do.
        var confined = SCENE_LIGHT_UNCONFINED;
        if Common::mask_data[index].point_color_meta.z != 0xffffffffu {
            confined = index;
        }
        return SceneLight(
            true,
            Camera::scene_frame_position(camera, Camera::scene_image_point(source, aspect), z),
            max(rays_length * LIGHT_RAYS_LIGHT_REACH * 2.0 * camera.lens.x * z, 1e-3),
            mask_effect_picker_color_to_working(mask_effect_color(params, LIGHT_RAYS_COLOR_LANE))
                * (amount * SCENE_LIGHT_GAIN * LIGHT_RAYS_LIGHT_INTENSITY),
            mask_effect_scene_bake_light(params),
            confined,
            1.0,
        );
    }
    return scene_light_none();
}

// Whether any effect slot emits a scene light or emits light over an area
// (scene_emission.wgsl). It is the same for every pixel, so receivers skip
// their per-pixel setup (depth reads, volume cells) when there is nothing to
// receive.
fn scene_lights_present() -> bool {
    for (var index = 0u; index < Common::mask_slot_count(); index = index + 1u) {
        if scene_light_at(index).emits { return true; }
    }
    return scene_emission_present();
}

// How much of a light acts at an image pixel: its mask's coverage when the
// light is confined to one.
fn scene_light_coverage(light: SceneLight, pos: vec2<i32>) -> f32 {
    if light.confined_to == SCENE_LIGHT_UNCONFINED { return 1.0; }
    return SceneAdjustments::local_mask_weight(pos, light.confined_to);
}

// The light's frame depth E · R, the change in z per unit of linear depth.
fn scene_light_relief(light: SceneLight) -> f32 {
    return light.relief_scale * scene_light_camera().lens.w;
}

// The view ray of image point `point` in the light's frame, at linear depth
// 0 (x, y, z): its point at depth d is `ray · (1 + relief · d)`.
fn scene_light_ray(point: vec2<f32>) -> vec3<f32> {
    return vec3<f32>(2.0 * scene_light_camera().lens.x * point, 1.0);
}

// The point of a view ray at linear depth `d`, in the light's frame; negative
// depths lie in the air before the nearest surface, down to -1 at the camera.
fn scene_light_ray_position(light: SceneLight, ray: vec3<f32>, d: f32) -> vec3<f32> {
    // Before the nearest surface depth is a share of the way to the camera.
    if d < 0.0 { return ray * (1.0 + 0.9 * d); }
    return ray * (1.0 + scene_light_relief(light) * d);
}

// Inverse-square falloff with a finite core: 1 at the light, 1/2 at its reach.
fn scene_light_falloff(light: SceneLight, position: vec3<f32>) -> f32 {
    let offset = position - light.position;
    return 1.0 / (1.0 + dot(offset, offset) / (light.reach * light.reach));
}

// ∫ falloff dt along the view ray `ray` between linear depths `near` and
// `far`. The ray's position is linear in depth, so the integral has a closed
// form and a light close to the ray leaves no sampling bands.
fn scene_light_falloff_along(light: SceneLight, ray: vec3<f32>, near: f32, far: f32) -> f32 {
    if far <= near { return 0.0; }
    // position(t) = start + direction * t
    let start = ray - light.position;
    let direction = ray * scene_light_relief(light);
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
    let lengths = max(length(travel) * length(position), 1e-6);
    return scene_light_phase(dot(travel, -position) / lengths, anisotropy);
}

// Light from every scene light that a point of the view ray of pixel `pos`
// at linear depth `d` in the air scatters toward the camera, relative to the
// ambient level: the light bake's cubes say how much of each light reaches
// it, and the emission of Glow and Neon adds its own (it casts no shadows).
// `uv` is the pixel's full-image position.
fn scene_light_in_air(pos: vec2<i32>, uv: vec2<f32>, d: f32, anisotropy: f32) -> vec3<f32> {
    let camera = scene_light_camera();
    let baked = Geometry::scene_geometry_present();
    let point = Camera::scene_image_point(uv, Common::scene_image_size());
    let ray = scene_light_ray(point);
    var received = vec3<f32>(0.0);
    for (var index = 0u; index < Common::mask_slot_count(); index = index + 1u) {
        let light = scene_light_at(index);
        if !light.emits { continue; }
        let coverage = scene_light_coverage(light, pos);
        if coverage <= 1e-6 { continue; }
        let position = scene_light_ray_position(light, ray, d);
        var visibility = 1.0;
        if baked && light.channel != 0u {
            visibility = Geometry::scene_point_visibility(position, light.channel - 1u);
        }
        received += light.intensity * (scene_light_falloff(light, position)
            * scene_light_phase_at(light, position, anisotropy) * coverage * visibility);
    }
    if scene_emission_present() {
        received += scene_emission_received(pos, d, anisotropy);
    }
    return received;
}

// Light from every scene light that a medium over a surface of the pixel
// scatters toward the camera where it lies on the surface, relative to the
// ambient level: the surface's visibility from the light bake (L2). `surface`
// and `pixel` are as the scene bake gives them; without scene depth, a
// surface at SCENE_DEPTH_UNKNOWN.
fn scene_light_on_surface(
    pos: vec2<i32>,
    uv: vec2<f32>,
    pixel: Geometry::ScenePixel,
    surface: Geometry::SceneSurface,
    anisotropy: f32,
) -> vec3<f32> {
    let camera = scene_light_camera();
    let baked = Geometry::scene_geometry_present();
    let point = Camera::scene_image_point(uv, Common::scene_image_size());
    var depth = SCENE_DEPTH_UNKNOWN;
    if baked {
        depth = mix(surface.d, 1.0, surface.sky);
    }
    var received = vec3<f32>(0.0);
    for (var index = 0u; index < Common::mask_slot_count(); index = index + 1u) {
        let light = scene_light_at(index);
        if !light.emits { continue; }
        let coverage = scene_light_coverage(light, pos);
        if coverage <= 1e-6 { continue; }
        let z = Camera::scene_frame_depth(camera, depth, light.relief_scale);
        let position = Camera::scene_frame_position(camera, point, z);
        var visibility = 1.0;
        if baked && light.channel != 0u {
            visibility = Geometry::scene_visibility(pixel, surface, light.channel - 1u);
        }
        received += light.intensity * (scene_light_falloff(light, position)
            * scene_light_phase_at(light, position, anisotropy) * coverage * visibility);
    }
    if scene_emission_present() {
        received += scene_emission_received(pos, depth, anisotropy);
    }
    return received;
}
