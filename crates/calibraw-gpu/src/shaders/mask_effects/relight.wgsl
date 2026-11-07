// Relight: virtual point lights that shade the scene's surfaces
// (docs/rework/DEPTH_EFFECTS_PLAN.md §5).
//
// Surfaces. A pixel is up to two surfaces of the scene bake
// (scene/geometry.wgsl), each with its own depth, normal and colour. Each is
// shaded on its own and the two are blended by how much of the pixel the near
// one covers, so the light ends on the photo's own outline and nothing is lit
// at a depth between a subject and what lies behind it. The sky is no surface
// and receives no light.
//
// Frame. The card's Relief is the relief scale E of the shading frame
// (scene/camera.wgsl): the nearest surface at z = 1, x right and y down. A
// light lies at its Light X and Y over the full image and at its Light depth,
// in control depth. Positions, normals and the light share that frame.
//
// Shadows. A light that casts shadows holds a channel of the light bake
// (pipeline/gpu/scene/lights.rs); this reads the visibility of each surface
// from it, with the light's Size setting the penumbra there.
//
// Shading is scene-linear Rec.2020 before tone mapping. The added light is
// proportional to an estimate of the surface's reflectance (its colour with
// part of its existing shading divided out), so lit shadows recover their
// texture and colour instead of receiving a flat wash. Everything depends
// only on full-image coordinates, the bakes and global tone statistics, so
// export tiles match the preview, and every pixel reads only itself.
//
// Without scene depth (still generating, or failed) the scene is a plane
// facing the camera at the nearest distance: the lights keep their falloff
// but cast no shadows.

// The field of view of the plane without scene depth: about a 30 mm lens on
// full frame.
const RELIGHT_FLAT_FOCAL_TAN: f32 = 0.4;
// Radius in shorter edges over which Size 100 averages a surface's normal.
const RELIGHT_SOFT_NORMAL_RADIUS: f32 = 0.03;

// One of the card's lights in the shading frame.
struct RelightLight {
    emits: bool,
    position: vec3<f32>,
    // Distance at which the light has halved.
    reach: f32,
    // Colour times strength, relative to the ambient level.
    intensity: vec3<f32>,
    size: f32,
    // Shadow strength; 0 when the light casts none.
    shadows: f32,
    // Its light bake channel plus one, or 0.
    channel: u32,
}

fn relight_light(
    params: MaskEffectParams,
    camera: Camera::SceneCamera,
    relief_scale: f32,
    aspect: vec2<f32>,
) -> RelightLight {
    let amount = mask_effect_percent(params, RELIGHT_AMOUNT_LANE);
    let source = mask_effect_lane_pair(params, RELIGHT_SOURCE_X_LANE) / 100.0;
    let control = clamp(mask_effect_lane(params, RELIGHT_LIGHT_DEPTH_LANE) / 100.0, -1.0, 1.0);
    // As scene/camera.rs `light_position` places it for the light bake.
    let z = Camera::scene_light_frame_depth(camera, control, relief_scale);
    let position = Camera::scene_frame_position(camera, Camera::scene_image_point(source, aspect), z);
    // Reach is in shorter-edge widths at the nearest surface.
    let reach = clamp(mask_effect_lane(params, RELIGHT_REACH_LANE) / 100.0, 0.1, 4.0) * 2.0 * camera.lens.x;
    let intensity = mask_effect_picker_color_to_working(mask_effect_color(params, RELIGHT_COLOR_LANE))
        * (amount * SCENE_LIGHT_GAIN);
    return RelightLight(
        amount > 1e-6,
        position,
        reach,
        intensity,
        mask_effect_percent(params, RELIGHT_SIZE_LANE),
        mask_effect_percent(params, RELIGHT_SHADOWS_LANE),
        mask_effect_scene_bake_light(params),
    );
}

// The plane Relight lights without scene depth: one nearest distance deep,
// so lights behind it keep a distance to it.
fn relight_flat_camera() -> Camera::SceneCamera {
    return Camera::SceneCamera(vec4<f32>(RELIGHT_FLAT_FOCAL_TAN, 1.0, 1.0, 1.0), vec4<f32>(1.0, 0.0, 0.5, 1.0));
}

// What one surface of `pixel` shows: the existing light dimmed to Ambient,
// plus every light of the card on an estimate of its reflectance. `rgb` is
// the surface's part of the pixel's colour.
fn relight_shade(
    surface: Geometry::SceneSurface,
    pixel: Geometry::ScenePixel,
    rgb: vec3<f32>,
    params: MaskEffectParams,
    frame: RelightFrame,
) -> vec3<f32> {
    let ambient_rgb = rgb * frame.ambient;
    let lit_share = 1.0 - surface.sky;
    if lit_share <= 0.0 {
        return ambient_rgb;
    }
    // Divide out part of the existing shading relative to the scene's ambient
    // level (shared by export tiles), so the estimate approaches reflectance.
    let solid = Color::gamut_project_nonnegative_rec2020(rgb);
    let relative = clamp(Common::safe_luma(solid) / frame.ambient_level, 0.02, 64.0);
    let reflectance = solid * pow(relative, -0.3);

    var added = vec3<f32>(0.0);
    for (var instance = 0u; instance < mask_effect_instance_count(params); instance = instance + 1u) {
        let light = relight_light(
            mask_effect_instance_params(params, instance), frame.camera, frame.relief_scale, frame.aspect,
        );
        if !light.emits { continue; }
        var normal = surface.normal;
        if frame.has_geometry {
            normal = Geometry::scene_soft_normal(
                surface, frame.uv, RELIGHT_SOFT_NORMAL_RADIUS * light.size * light.size, frame.relief_scale,
            );
        }
        let to_light = light.position - surface.position;
        let distance = max(length(to_light), 1e-5);
        // Wrapped Lambert: a larger source reaches further around a form.
        let wrap = 0.5 * light.size;
        let diffuse = clamp((dot(normal, to_light / distance) + wrap) / (1.0 + wrap), 0.0, 1.0);
        if diffuse <= 1e-6 { continue; }
        // Inverse-square falloff with a finite core: 1 at the light, 1/2 at its reach.
        let falloff = 1.0 / (1.0 + distance * distance / (light.reach * light.reach));
        var visibility = 1.0;
        if frame.has_geometry && light.channel != 0u && light.shadows > 1e-6 {
            visibility = mix(1.0, Geometry::scene_visibility(pixel, surface, light.channel - 1u), light.shadows);
        }
        added += light.intensity * (diffuse * falloff * visibility);
    }
    return ambient_rgb + reflectance * added * lit_share;
}

// What every surface of a pixel shares.
struct RelightFrame {
    camera: Camera::SceneCamera,
    relief_scale: f32,
    aspect: vec2<f32>,
    uv: vec2<f32>,
    ambient: f32,
    ambient_level: f32,
    has_geometry: bool,
}

// Whether any of the card's lights adds light.
fn relight_any_light(params: MaskEffectParams) -> bool {
    for (var instance = 0u; instance < mask_effect_instance_count(params); instance = instance + 1u) {
        let light = mask_effect_instance_params(params, instance);
        if mask_effect_percent(light, RELIGHT_AMOUNT_LANE) > 1e-6 {
            return true;
        }
    }
    return false;
}

// Relight shades the pixel once with all its card's lights, in the slot of the
// first light (instances.rs in calibraw-core): the existing light dims to the
// shared Ambient level once, and the lights add up on one estimate of each
// surface's reflectance. Slots of the other lights leave the image alone.
fn apply_relight(pos: vec2<i32>, input_rgb: vec3<f32>, params: MaskEffectParams) -> vec3<f32> {
    if mask_effect_instance(params) != 0u { return input_rgb; }
    let ambient = mask_effect_percent(params, RELIGHT_AMBIENT_LANE);
    if !relight_any_light(params) { return input_rgb * ambient; }

    let uv = full_image_uv(pos);
    let has_geometry = Geometry::scene_geometry_present();
    var frame = RelightFrame(
        relight_flat_camera(),
        clamp(mask_effect_lane(params, RELIGHT_RELIEF_SCALE_LANE) / 100.0, 0.1, 3.0),
        Common::scene_image_size(),
        uv,
        ambient,
        scene_ambient_level(),
        has_geometry,
    );
    if !has_geometry {
        let point = Camera::scene_image_point(uv, frame.aspect);
        let surface = Geometry::SceneSurface(
            Camera::scene_frame_position(frame.camera, point, 1.0),
            vec3<f32>(0.0, 0.0, -1.0),
            input_rgb,
            0.0,
            0.0,
            0u,
        );
        let pixel = Geometry::ScenePixel(1.0, surface, surface, false, vec2<i32>(0), vec4<f32>(0.0), vec4<f32>(0.0));
        return relight_shade(surface, pixel, input_rgb, params, frame);
    }
    frame.camera = Bake::scene_bake.camera;
    frame.aspect = Geometry::scene_aspect();
    let scene_rgb = Geometry::scene_unedited_at(pos);
    let pixel = Geometry::scene_pixel(uv, scene_rgb, frame.relief_scale);
    let colors = Geometry::scene_split(pixel, scene_rgb, input_rgb);
    let near = relight_shade(pixel.near, pixel, colors[0], params, frame);
    if !pixel.in_band {
        return near;
    }
    let far = relight_shade(pixel.far, pixel, colors[1], params, frame);
    return Geometry::scene_blend(pixel, near, far);
}
