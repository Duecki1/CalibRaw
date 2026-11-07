// The RAW stage of a pre-demosaiced raster (JPEG, PNG, HEIF, TIFF): copies the
// raster source into the scene texture with the profiled luminance denoise
// that sensor sources get in their demosaic finish. The source is never
// written, so colour denoise and Remove always start from the original pixels.
//
// Colour space: the raster's RGB as uploaded: scene-linear Rec.2020 for
// rendered photos, and for a linear DNG camera RGB before white balance. The
// denoise reads it white balanced (`Common::scene_neutral_gains`, 1 for
// rendered photos), the units of the noise uniforms, and writes it unbalanced
// again.
// Coordinates are tile-local pixels and neighbours clamp to the tile, whose
// export halo covers the widest denoise reach (DEMOSAIC_CHAIN_SUPPORT).

#import calibraw::common as Common
#import calibraw::noise_ca_finish as NoiseCaFinish

@group(0) @binding(11) var raster_source: texture_2d<f32>;
@group(0) @binding(10) var raster_scene_write: texture_storage_2d<rgba16float /* CALIBRAW_WORK_FORMAT */, write>;

override fn NoiseCaFinish::finish_reference_at(pos: vec2<i32>) -> vec3<f32> {
    return textureLoad(raster_source, Common::clamp_pos(pos), 0).rgb * Common::scene_neutral_gains();
}

@compute @workgroup_size(8, 8, 1)
fn raster_finish(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= Common::camera_uniforms.width || gid.y >= Common::camera_uniforms.height { return; }
    let pos = vec2<i32>(i32(gid.x), i32(gid.y));
    let gains = Common::scene_neutral_gains();
    let rgb = textureLoad(raster_source, pos, 0).rgb * gains;
    textureStore(
        raster_scene_write,
        pos,
        vec4<f32>(NoiseCaFinish::finish_apply_sensor_denoise(pos, rgb) / gains, 1.0),
    );
}
