// The scene's light as effects receive it, at the current exposure, in scene-linear
// Rec.2020: its ambient level, the photo's coarse colour around a point, and the
// light sources in the photo. None of it depends on the crop or export tile.

// The scene's ambient light level: scene-linear middle grey at the exposure of the
// image's ambient tone. Global tone statistics are shared by export tiles.
fn scene_ambient_level() -> f32 {
    let ambient_ev = Tonemap::tone_stats.percentiles_0_field.w + Common::scene_tone_uniforms.exposure;
    return ToneCommon::SCENE_MIDDLE_GREY * exp2(clamp(ambient_ev, -12.0, 6.0));
}

// The photo's coarse colour and a wide blur of it, read from the scene bake's
// working image (scene/geometry.wgsl `scene_image`): what Rain's mist and drops
// take their surroundings from.
const SCENE_ENVIRONMENT_CELLS: f32 = 256.0;
const SCENE_ENVIRONMENT_BLUR: f32 = 0.08;

// The photo's light at full-image `uv`: its coarse colour, a few percent of the
// longer edge across. Black while no scene bake is bound.
fn scene_environment_at(uv: vec2<f32>) -> vec3<f32> {
    if !Geometry::scene_image_present() { return vec3<f32>(0.0); }
    let aspect = Geometry::scene_aspect();
    let cell = max(aspect.x, aspect.y) / SCENE_ENVIRONMENT_CELLS;
    return Geometry::scene_image(uv, cell / Geometry::SCENE_IMAGE_BOX_PER_DEVIATION)
        * exp2(Common::scene_tone_uniforms.exposure);
}

// The same, spread over a wide part of the photo: a Gaussian of 8% of the
// longer edge.
fn scene_environment_blurred_at(uv: vec2<f32>) -> vec3<f32> {
    if !Geometry::scene_image_present() { return vec3<f32>(0.0); }
    let aspect = Geometry::scene_aspect();
    return Geometry::scene_image(uv, SCENE_ENVIRONMENT_BLUR * max(aspect.x, aspect.y))
        * exp2(Common::scene_tone_uniforms.exposure);
}

// Light from light sources in the photograph (tone_analysis.wgsl) arriving around
// image pixel `pos`, spread into a halo.
@group(0) @binding(45) var image_light_tex: texture_2d<f32>;

fn image_light_at(pos: vec2<i32>) -> vec3<f32> {
    let grid = vec2<f32>(Common::image_light_grid());
    let edge = f32(Common::IMAGE_LIGHT_GRID_LONG);
    let cell = clamp(full_image_uv(pos) * grid, vec2<f32>(0.5), grid - vec2<f32>(0.5));
    let light = textureSampleLevel(
        image_light_tex, SceneAdjustments::local_mask_sampler, cell / edge, 0.0,
    ).xyz;
    return max(light, vec3<f32>(0.0)) * exp2(Common::scene_tone_uniforms.exposure);
}
