#import calibraw::common as Common
#import calibraw::color as Color
#import calibraw::basic_adjustments as BasicAdjustments

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

// The source in linear working RGB, before camera characterization. Rasters
// that are not camera-linear take their white balance here.
fn scene_working_at(pos: vec2<i32>) -> vec3<f32> {
    let camera_rgb = source_scene_at(pos);
    var working = Color::cam_to_working(camera_rgb);

    if Common::camera_uniforms.pre_demosaiced_raster > 0.5
        && Common::camera_uniforms.camera_linear_raster <= 0.5 {
        working = BasicAdjustments::apply_temperature_tint_values(
            working,
            Common::camera_uniforms.temperature,
            Common::camera_uniforms.tint,
        );
    }
    return working;
}
