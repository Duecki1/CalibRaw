#import calibraw::common as Common
#import calibraw::color as Color
#import calibraw::profile as Profile

// The demosaiced scene in camera RGB, shared by tone analysis and the scene
// adjustment passes. Some Mali Vulkan compilers crash when image operands are
// function parameters, so the binding lives here instead of being passed in.
@group(0) @binding(11) var scene_tex: texture_2d<f32>;

// Camera RGB at a tile-local pixel: the scene itself, or the positive of a
// scanned film negative (film_positive). Every scene stage reads it here, so
// tone analysis, adjustments and Dehaze all see the positive.
fn source_scene_at(pos: vec2<i32>) -> vec3<f32> {
    return film_positive(textureLoad(scene_tex, Common::clamp_pos(pos), 0).xyz);
}

// CameraUniforms::film_base.w; FILM_MODE_* in params.rs. FILM_MODE_COLOR,
// any other value, inverts each channel on its own.
const FILM_MODE_POSITIVE: u32 = 0u;
const FILM_MODE_MONOCHROME: u32 = 2u;
// FILM_MIN_TRANSMITTANCE and MONOCHROME_DENSITY_WEIGHTS in film_negative.rs.
const FILM_MIN_TRANSMITTANCE: f32 = 1.0 / 16384.0;
const FILM_MONOCHROME_WEIGHTS: vec3<f32> = vec3<f32>(0.25, 0.5, 0.25);

// The positive of a scanned film negative in camera RGB, as
// FilmNegative::positive (film_negative.rs) computes it before white
// balance; the model is documented there. `camera_rgb` is the scene
// texture's linear camera RGB, which film_base shares. The base's light
// (unexposed film) is density 0, black in the positive; denser film is stops
// of density above it, raised by each channel's exponent. Light below
// 1/16384 of the base, such as an opaque film holder or noise below black,
// counts as 14 stops, so the result stays finite; film brighter than the
// base goes below black. A black-and-white negative inverts one density, the
// weighted mean of the channels'. The white balance follows as gains, and
// the camera transform then treats the positive like any camera RGB.
fn film_positive(camera_rgb: vec3<f32>) -> vec3<f32> {
    let base = Common::camera_uniforms.film_base;
    let mode = u32(round(base.w));
    if mode == FILM_MODE_POSITIVE {
        return camera_rgb;
    }
    let transmittance = max(
        camera_rgb / max(base.xyz, vec3<f32>(1e-6)),
        vec3<f32>(FILM_MIN_TRANSMITTANCE),
    );
    var density = -log2(transmittance);
    if mode == FILM_MODE_MONOCHROME {
        density = vec3<f32>(dot(density, FILM_MONOCHROME_WEIGHTS));
    }
    let exponents = Common::camera_uniforms.film_exponents;
    let positive = exp2(exponents.w) * (exp2(exponents.xyz * density) - vec3<f32>(1.0));
    return positive * Common::camera_uniforms.film_gains.xyz;
}

// Scene radiance (RAWs, camera-linear and float rasters) rather than a
// developed photo. Rust's ViewTransformMode::select makes this decision and
// gives exactly these sources the scene view transform.
fn source_is_scene_referred() -> bool {
    return Common::camera_uniforms.view_transform_mode == Common::VIEW_TRANSFORM_SCENE_SIGMOID;
}

// Camera RGB `camera_rgb` in linear working RGB, before camera
// characterization. Global white balance reaches only sources with a camera
// white-balance model, through the CFA multipliers or the camera transform;
// developed photos have none and keep their colours. Scene-referred sources
// leave inside Rec.2020, so no later stage sees the negative channels a
// camera matrix gives narrow-band light.
fn working_from_camera(camera_rgb: vec3<f32>) -> vec3<f32> {
    var working = Color::cam_to_working(camera_rgb);

    if source_is_scene_referred() {
        working = Color::compress_scene_gamut(working);
    }
    return working;
}

// The source at a tile-local pixel in linear working RGB (working_from_camera).
fn scene_working_at(pos: vec2<i32>) -> vec3<f32> {
    return working_from_camera(source_scene_at(pos));
}

// Camera RGB `camera_rgb` after camera characterization at the profile's
// exposure, before the edit's own exposure.
fn profiled_from_camera(camera_rgb: vec3<f32>) -> vec3<f32> {
    let characterized = Profile::apply_camera_characterization(working_from_camera(camera_rgb));
    let profile_exposure_ev = bitcast<f32>(Common::camera_uniforms.profile_flags.z);
    return characterized * exp2(profile_exposure_ev);
}

// The profiled scene at a tile-local pixel (profiled_from_camera). The scene
// preparation starts from it; tone analysis and Dehaze measure it, so their
// statistics do not follow the exposure slider.
fn profiled_scene_at(pos: vec2<i32>) -> vec3<f32> {
    return profiled_from_camera(source_scene_at(pos));
}
