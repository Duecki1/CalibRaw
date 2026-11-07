#import calibraw::scene::camera as Camera

// The light bake (pipeline/gpu/scene/lights.rs, docs/rework/DEPTH_EFFECTS_PLAN.md
// §4.4): per light a shadow cube of the scene's visible surface (L1,
// shadow_raster.wgsl) and from it the visibility of each surface of the scene
// bake (L2, visibility.wgsl), and from the cubes the share of each light
// along every view ray that the scene does not shadow (L3, air_light.wgsl).
//
// Frame. Lights and the scene share the shading frame of camera.wgsl at the
// light's relief scale E: the camera at the origin, x right, y down, z away,
// the nearest surface at z = 1. Positions and distances are in that frame's
// units, the nearest surface's distance.
//
// Cube. A light's cube is six layers of a Depth32Float 2D array, from
// `SceneShadowLight.shape.z` on, one per face. Face k looks along axis k / 2,
// toward + for even k (`scene_cube_basis`). A face covers face coordinates
// x / z and y / z in ±(1 + SCENE_CUBE_MARGIN), a guard band past the 90° the
// face owns, so that filters around a point near a face's edge stay on that
// face. Faces are drawn into the top-left `shape.y` texels of each layer
// (Draft tier: fewer texels, same layer). Depth is reversed and infinite:
// the stored value is SCENE_CUBE_NEAR / z along the face's axis, 0 where
// nothing was drawn, so nearer is larger and precision is even in log z.
// Rows run down the face's y.

// The lights of the bake, in their channels (lights.rs `SceneShadowLights`).
struct SceneShadowLight {
    // Position in the frame of the light's relief scale (xyz), and that scale
    // E (w).
    position: vec4<f32>,
    // The light's radius in frame units (x); its cube's face edge in texels
    // in the current tier (y), 0 when the channel holds no light; its cube's
    // first layer (z); the mesh level it is drawn from (w: 0 the working
    // size, 1 half of it).
    shape: vec4<f32>,
    // The distance at which the light's falloff has halved, in frame units
    // (x): what the air light weighs the view ray by. yzw unused.
    air: vec4<f32>,
}

struct SceneShadowLights {
    lights: array<SceneShadowLight, 8>,
    // Taps of the visibility filter (x: blocker search, y: penumbra), set by
    // the tier.
    taps: vec4<u32>,
}

const SCENE_SHADOW_LIGHTS: u32 = 8u;
// The reversed depth's near plane in frame units: geometry closer to the
// light than this is not drawn.
const SCENE_CUBE_NEAR: f32 = 1e-3;
const SCENE_CUBE_MARGIN: f32 = 0.1;

@group(0) @binding(104) var<uniform> scene_shadow_lights: SceneShadowLights;
@group(0) @binding(105) var scene_cube_tex: texture_depth_2d_array;
// Compares with GreaterEqual, linear: 1 where the reference is at least the
// stored depth, i.e. lies no farther than what the face saw.
@group(0) @binding(106) var scene_cube_compare: sampler_comparison;

// The light in `channel`; inactive past the last channel.
fn scene_shadow_light(channel: u32) -> SceneShadowLight {
    if channel >= SCENE_SHADOW_LIGHTS {
        return SceneShadowLight(vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0));
    }
    return scene_shadow_lights.lights[channel];
}

fn scene_shadow_light_active(light: SceneShadowLight) -> bool {
    return light.shape.y > 0.0;
}

// Face k's basis: right (x), up (y) and forward (z) as columns. Any
// orthonormal basis serves, as drawing and reading use this one.
fn scene_cube_basis(face: u32) -> mat3x3<f32> {
    let sign = select(-1.0, 1.0, face % 2u == 0u);
    switch face / 2u {
        case 0u: {
            return mat3x3<f32>(vec3<f32>(0.0, 0.0, -sign), vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(sign, 0.0, 0.0));
        }
        case 1u: {
            return mat3x3<f32>(vec3<f32>(sign, 0.0, 0.0), vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(0.0, sign, 0.0));
        }
        default: {
            return mat3x3<f32>(vec3<f32>(sign, 0.0, 0.0), vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(0.0, 0.0, sign));
        }
    }
}

// The face that owns direction `v` from the light: its largest component.
fn scene_cube_face(v: vec3<f32>) -> u32 {
    let a = abs(v);
    if a.x >= a.y && a.x >= a.z {
        return select(1u, 0u, v.x >= 0.0);
    }
    if a.y >= a.z {
        return select(3u, 2u, v.y >= 0.0);
    }
    return select(5u, 4u, v.z >= 0.0);
}

// Direction `v` from the light in face `face`'s basis: (right, up, forward).
fn scene_cube_coordinates(v: vec3<f32>, face: u32) -> vec3<f32> {
    return v * scene_cube_basis(face);
}

// The clip position that draws face coordinates `c` (scene_cube_coordinates).
fn scene_cube_clip(c: vec3<f32>) -> vec4<f32> {
    let scale = 1.0 + SCENE_CUBE_MARGIN;
    return vec4<f32>(c.x, c.y, SCENE_CUBE_NEAR * scale, c.z * scale);
}

// Texture uv of face coordinates `c` in a face of `face_texels` within a layer
// of `layer_texels`.
fn scene_cube_uv(c: vec3<f32>, face_texels: f32, layer_texels: f32) -> vec2<f32> {
    let ndc = c.xy / (c.z * (1.0 + SCENE_CUBE_MARGIN));
    return vec2<f32>(0.5 + 0.5 * ndc.x, 0.5 - 0.5 * ndc.y) * (face_texels / layer_texels);
}

// The stored depth of distance `z` along a face's axis, and back.
fn scene_cube_depth(z: f32) -> f32 {
    return SCENE_CUBE_NEAR / max(z, SCENE_CUBE_NEAR);
}

fn scene_cube_distance(stored: f32) -> f32 {
    return SCENE_CUBE_NEAR / max(stored, 1e-30);
}

// How much of light `light` reaches the scene point `position`, whose surface
// faces `normal` (zero for a point in the air), from its cube: percentage-
// closer soft shadows (Fernando, "Percentage-Closer Soft Shadows", 2005).
// Blockers are searched over the light's disc seen from the point; their mean
// distance gives the penumbra's width, which a filter of that radius
// realizes. Each filter tap is a bilinear comparison. Taps follow a fixed
// Vogel spiral, so results do not depend on the pixel. `blocker_taps` 0
// skips the search and filters over one texel.
//
// Self-shadowing is avoided by moving the point along its normal by about a
// shadow texel's footprint there (normal offset), and by a bias that grows
// with the slope of the surface seen from the light (slope-scaled bias).
fn scene_cube_visibility(
    light: SceneShadowLight,
    position: vec3<f32>,
    normal: vec3<f32>,
    blocker_taps: u32,
    filter_taps: u32,
) -> f32 {
    let face_texels = light.shape.y;
    let layer_texels = f32(textureDimensions(scene_cube_tex).x);
    let raw = position - light.position.xyz;
    let raw_face = scene_cube_face(raw);
    let raw_axis = max(dot(raw, scene_cube_basis(raw_face)[2]), SCENE_CUBE_NEAR);
    // A shadow texel's extent at the point, in frame units.
    let texel = 2.0 * (1.0 + SCENE_CUBE_MARGIN) * raw_axis / face_texels;
    let toward_light = -normalize(raw);
    let facing = dot(normal, toward_light);
    let sine = sqrt(max(1.0 - facing * facing, 0.0));
    let offset_point = position + normal * (texel * (SCENE_SHADOW_NORMAL_OFFSET + SCENE_SHADOW_SLOPE_OFFSET * sine));
    let v = offset_point - light.position.xyz;
    let face = scene_cube_face(v);
    let c = scene_cube_coordinates(v, face);
    if c.z <= SCENE_CUBE_NEAR {
        return 1.0;
    }
    let slope = select(min(sine / max(facing, 1e-3), SCENE_SHADOW_MAX_SLOPE), 0.0, dot(normal, normal) < 0.5);
    let bias = texel * (SCENE_SHADOW_BIAS + SCENE_SHADOW_SLOPE_BIAS * slope);
    let receiver = c.z - bias;
    let reference = scene_cube_depth(receiver);
    let layer = i32(light.shape.z) + i32(face);
    let uv = scene_cube_uv(c, face_texels, layer_texels);
    // One face's guard band in uv, which filters stay within.
    let margin_uv = SCENE_CUBE_MARGIN / (2.0 * (1.0 + SCENE_CUBE_MARGIN)) * (face_texels / layer_texels);
    let texel_uv = 1.0 / layer_texels;
    // The narrowest filter: the mesh's outline is only as exact as its texels,
    // so a sharper shadow edge would show their steps as dots.
    let least_uv = SCENE_SHADOW_LEAST_FILTER * texel_uv;
    // Face coordinates per uv.
    let coordinates_per_uv = 2.0 * (1.0 + SCENE_CUBE_MARGIN) * (layer_texels / face_texels);
    let radius = light.shape.x;

    var filter_uv = least_uv;
    if blocker_taps > 0u && radius > 0.0 {
        // The light's disc seen from the receiver, at half its distance:
        // where the blockers that shade it most lie.
        let search_uv = clamp(2.0 * radius / receiver / coordinates_per_uv, texel_uv, margin_uv);
        var blockers = 0.0;
        var count = 0.0;
        for (var tap = 0u; tap < blocker_taps; tap = tap + 1u) {
            let sample_uv = uv + search_uv * scene_vogel(tap, blocker_taps);
            let texel_position = vec2<i32>(sample_uv * layer_texels);
            let stored = textureLoad(scene_cube_tex, texel_position, layer, 0);
            let distance = scene_cube_distance(stored);
            if stored > 0.0 && distance < receiver {
                blockers += distance;
                count += 1.0;
            }
        }
        if count == 0.0 {
            return 1.0;
        }
        let blocker = blockers / count;
        let penumbra = radius * (receiver - blocker) / (blocker * receiver);
        filter_uv = clamp(penumbra / coordinates_per_uv, least_uv, margin_uv);
    }
    var lit = 0.0;
    let taps = max(filter_taps, 1u);
    for (var tap = 0u; tap < taps; tap = tap + 1u) {
        let sample_uv = uv + filter_uv * scene_vogel(tap, taps);
        lit += textureSampleCompareLevel(scene_cube_tex, scene_cube_compare, sample_uv, layer, reference);
    }
    return lit / f32(taps);
}

// The least filter radius in shadow texels.
const SCENE_SHADOW_LEAST_FILTER: f32 = 2.0;
// Normal offset and bias in shadow texels at the receiver. Depth from a single
// photo is noisy by about a percent of distance, which is a shadow texel or
// two wherever the light lies near the camera; less bias than that leaves
// stripes of self-shadow on distant surfaces.
const SCENE_SHADOW_NORMAL_OFFSET: f32 = 1.0;
const SCENE_SHADOW_SLOPE_OFFSET: f32 = 1.0;
const SCENE_SHADOW_BIAS: f32 = 1.5;
const SCENE_SHADOW_SLOPE_BIAS: f32 = 1.5;
const SCENE_SHADOW_MAX_SLOPE: f32 = 4.0;

// Point `tap` of `count` on a Vogel spiral in the unit disc.
fn scene_vogel(tap: u32, count: u32) -> vec2<f32> {
    let radius = sqrt((f32(tap) + 0.5) / f32(count));
    let angle = f32(tap) * 2.39996323;
    return radius * vec2<f32>(cos(angle), sin(angle));
}

// The air light (L3, air_light.wgsl) of one pipeline: a grid over the full
// image whose texel (i, j) covers full-image pixels
// [i, i + 1) · `march.x` × [j, j + 1) · `march.x`, of which a texture array
// holds the part `region` names. Layer 0 holds the linear depth of the near
// (x) and the far (y) surface the march ended at; then, per group of four
// light bake channels, a layer of each channel's shadowed share along the
// view ray to the near surface and one to the far surface.
struct SceneAirLight {
    // The texture's first texel in grid coordinates (xy) and its size (zw).
    region: vec4<i32>,
    // Full-image pixels per grid texel (x), 0 when no air light is built;
    // steps along each ray (y); cube taps per step (z); groups of four lights
    // (w).
    march: vec4<u32>,
    // The full image's size in pixels (xy).
    image: vec4<u32>,
}

@group(0) @binding(115) var<uniform> scene_air: SceneAirLight;
