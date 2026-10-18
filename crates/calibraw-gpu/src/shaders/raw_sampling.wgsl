#import calibraw::common as Common

@group(0) @binding(3) var reconstructed_raw_tex: texture_2d<f32>;

// The loader supplies physical CFA planes in R, G1, B, G2 order.
fn cfa_channel_at(pos: vec2<i32>) -> u32 {
    return min(textureLoad(Common::color_tex, Common::clamp_pos(pos), 0).r, 3u);
}

fn color_at(pos: vec2<i32>) -> u32 {
    let channel = cfa_channel_at(pos);
    return select(channel, 1u, channel == 3u);
}

fn wb_for_cfa_channel(channel: u32) -> f32 {
    return Common::camera_uniforms.wb[min(channel, 3u)];
}

// The raw texture holds the original mosaic in R. With AI denoise on a Bayer
// sensor it is Rg16Uint and G holds the AI-denoised mosaic in the same sensor
// units, blended linearly before black/white normalization. Weight 1 selects
// G exactly so full-strength AI output does not depend on mix() rounding.
fn raw_value_at(p: vec2<i32>) -> f32 {
    let texel = textureLoad(Common::raw_tex, p, 0);
    let ai_weight = Common::camera_uniforms.ai_cfa_weight;
    if ai_weight >= 1.0 {
        return f32(texel.g);
    }
    if ai_weight > 0.0 {
        return mix(f32(texel.r), f32(texel.g), ai_weight);
    }
    return f32(texel.r);
}

fn raw_sensor_at(pos: vec2<i32>) -> f32 {
    let p = Common::clamp_pos(pos);
    let channel = cfa_channel_at(p);
    let raw = raw_value_at(p);
    let metadata_black = textureLoad(Common::black_tex, p, 0).x;
    let white = max(Common::camera_uniforms.white_levels[channel], metadata_black + 1.0);
    let sensor_range = max(white - metadata_black, 1.0);

    let black_offset = clamp(Common::camera_uniforms.black_point, -0.25, 0.25) * sensor_range;
    let calibrated_black = clamp(
        metadata_black + black_offset,
        0.0,
        white - 1.0,
    );
    // Read noise below black stays negative so demosaic, denoise and later
    // filters average it to black; clamping each sample would lift and tint
    // deep shadows. Scene stages take it (Color::compress_scene_gamut keeps
    // pixels without a positive channel). -1 only bounds a bad black level.
    return clamp((raw - calibrated_black) / (white - calibrated_black), -1.0, 4.0);
}

// Four read-noise sigmas of the noisiest channel, in the white-balanced units
// of raw_camera_at: an offset that lifts samples below black for demosaic
// steps that divide by local signal and so need positive values.
fn read_noise_pedestal() -> f32 {
    let read = Common::camera_uniforms.noise_read;
    return 4.0 * sqrt(max(max(max(read.r, read.g), max(read.b, read.a)), 0.0));
}

fn raw_camera_at(pos: vec2<i32>) -> f32 {
    let p = Common::clamp_pos(pos);
    return raw_sensor_at(p) * wb_for_cfa_channel(cfa_channel_at(p));
}

// Keep normal clipping slightly below the calibrated white point. Make the
// clipping decision in sensor space so it is independent of white balance: a
// shared WB-domain threshold would mark high-gain channels clipped while their
// sensor samples are still below white.
const SHARED_HIGHLIGHT_CLIP_SAFETY: f32 = 0.995;

fn shared_highlight_sensor_clip() -> f32 {
    return SHARED_HIGHLIGHT_CLIP_SAFETY
        * max(Common::camera_uniforms.highlight_clip, 0.01);
}

fn is_raw_clipped(pos: vec2<i32>) -> bool {
    let p = Common::clamp_pos(pos);
    if Common::camera_uniforms.highlight_options.x >= 1.5 {
        return raw_sensor_at(p) >= 0.987 * max(Common::camera_uniforms.highlight_clip, 0.01);
    }
    return raw_sensor_at(p) >= shared_highlight_sensor_clip();
}

fn raw_cfa_at(pos: vec2<i32>) -> f32 {
    return textureLoad(reconstructed_raw_tex, Common::clamp_pos(pos), 0).x;
}
