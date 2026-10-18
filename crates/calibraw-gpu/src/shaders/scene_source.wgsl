#import calibraw::common as Common
#import calibraw::color as Color
#import calibraw::profile as Profile

// The demosaiced scene in camera RGB, shared by tone analysis and the scene
// adjustment passes. Some Mali Vulkan compilers crash when image operands are
// function parameters, so the binding lives here instead of being passed in.
@group(0) @binding(11) var scene_tex: texture_2d<f32>;

// Bilinear camera RGB at a tile-local pixel position; taps clamp to the tile.
fn scene_bilinear(pos: vec2<f32>) -> vec3<f32> {
    let base = floor(pos);
    let p0 = vec2<i32>(i32(base.x), i32(base.y));
    let p1 = p0 + vec2<i32>(1, 1);
    let f = fract(pos);
    let a = textureLoad(scene_tex, Common::clamp_pos(p0), 0).xyz;
    let b = textureLoad(scene_tex, Common::clamp_pos(vec2<i32>(p1.x, p0.y)), 0).xyz;
    let c = textureLoad(scene_tex, Common::clamp_pos(vec2<i32>(p0.x, p1.y)), 0).xyz;
    let d = textureLoad(scene_tex, Common::clamp_pos(p1), 0).xyz;
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

// Camera RGB at a tile-local pixel. Pre-demosaiced rasters get lateral CA
// correction here; CFA sources were corrected when they were demosaiced.
fn source_scene_at(pos: vec2<i32>) -> vec3<f32> {
    var rgb = textureLoad(scene_tex, Common::clamp_pos(pos), 0).xyz;
    if Common::camera_uniforms.pre_demosaiced_raster <= 0.5 {
        return rgb;
    }
    if abs(Common::camera_uniforms.ca_red) > 1e-6 {
        rgb.r = scene_bilinear(
            Common::ca_warped_pos(pos, Common::camera_uniforms.ca_red),
        ).r;
    }
    if abs(Common::camera_uniforms.ca_blue) > 1e-6 {
        rgb.b = scene_bilinear(
            Common::ca_warped_pos(pos, Common::camera_uniforms.ca_blue),
        ).b;
    }
    return rgb;
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
