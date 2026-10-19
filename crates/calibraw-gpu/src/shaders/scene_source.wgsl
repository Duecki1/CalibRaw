#import calibraw::common as Common
#import calibraw::color as Color
#import calibraw::profile as Profile

// The demosaiced scene in camera RGB, shared by tone analysis and the scene
// adjustment passes. Some Mali Vulkan compilers crash when image operands are
// function parameters, so the binding lives here instead of being passed in.
@group(0) @binding(11) var scene_tex: texture_2d<f32>;

// Camera RGB at a tile-local pixel.
fn source_scene_at(pos: vec2<i32>) -> vec3<f32> {
    return textureLoad(scene_tex, Common::clamp_pos(pos), 0).xyz;
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
