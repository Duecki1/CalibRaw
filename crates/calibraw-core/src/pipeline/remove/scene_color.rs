//! Colour conversions between the pipeline scene, the canonical remove scene and model sRGB.

use super::*;

pub fn remove_scene_white_balance(raw: &LoadedRaw, exposure: &ExposureParams) -> [f32; 3] {
    if raw.is_pre_demosaiced_raster() {
        return [1.0; 3];
    }
    let (wb, _, _) =
        raw.adjusted_white_balance_and_camera_transform(exposure.temperature, exposure.tint);
    let green = 0.5 * (wb[1] + wb[3]);
    [wb[0].max(1e-8), green.max(1e-8), wb[2].max(1e-8)]
}

pub fn pipeline_scene_to_canonical_remove_scene(
    raw: &LoadedRaw,
    exposure: &ExposureParams,
    rgb: [f32; 3],
) -> [f32; 3] {
    if raw.is_pre_demosaiced_raster() {
        return rgb;
    }
    let wb = remove_scene_white_balance(raw, exposure);
    [rgb[0] / wb[0], rgb[1] / wb[1], rgb[2] / wb[2]]
}

pub fn canonical_remove_scene_to_pipeline_scene(
    raw: &LoadedRaw,
    exposure: &ExposureParams,
    rgb: [f32; 3],
) -> [f32; 3] {
    if raw.is_pre_demosaiced_raster() {
        return rgb;
    }
    let wb = remove_scene_white_balance(raw, exposure);
    [rgb[0] * wb[0], rgb[1] * wb[1], rgb[2] * wb[2]]
}

pub fn pipeline_scene_to_working_rec2020(raw: &LoadedRaw, rgb: [f32; 3]) -> [f32; 3] {
    if raw.is_pre_demosaiced_raster() && !raw.is_camera_linear_raster() {
        return rgb;
    }
    let m = remove_camera_transform(raw);
    [
        m[0][0] * rgb[0] + m[0][1] * rgb[1] + m[0][2] * rgb[2],
        m[1][0] * rgb[0] + m[1][1] * rgb[1] + m[1][2] * rgb[2],
        m[2][0] * rgb[0] + m[2][1] * rgb[1] + m[2][2] * rgb[2],
    ]
}

// Sensor pipeline scenes have WB applied already; camera rasters do not.
fn remove_camera_transform(raw: &LoadedRaw) -> [[f32; 4]; 3] {
    let mut transform = raw.cam_to_srgb;
    if raw.is_camera_linear_raster() {
        for row in &mut transform {
            for (value, gain) in row.iter_mut().zip(raw.wb_coeffs) {
                *value *= gain;
            }
        }
    }
    transform
}

fn invert_remove_camera_matrix(raw: &LoadedRaw) -> Option<[[f32; 3]; 3]> {
    if raw.is_pre_demosaiced_raster() && !raw.is_camera_linear_raster() {
        return Some(matrix::IDENTITY3);
    }
    let rgb_columns = remove_camera_transform(raw).map(|row| [row[0], row[1], row[2]]);
    matrix::invert(rgb_columns)
}

pub fn working_rec2020_to_canonical_remove_scene(
    raw: &LoadedRaw,
    exposure: &ExposureParams,
    rgb: [f32; 3],
) -> [f32; 3] {
    if raw.is_pre_demosaiced_raster() && !raw.is_camera_linear_raster() {
        return rgb;
    }
    let Some(m) = invert_remove_camera_matrix(raw) else {
        return rgb;
    };
    let camera_wb = [
        m[0][0] * rgb[0] + m[0][1] * rgb[1] + m[0][2] * rgb[2],
        m[1][0] * rgb[0] + m[1][1] * rgb[1] + m[1][2] * rgb[2],
        m[2][0] * rgb[0] + m[2][1] * rgb[1] + m[2][2] * rgb[2],
    ];
    pipeline_scene_to_canonical_remove_scene(raw, exposure, camera_wb)
}

pub fn remove_scene_to_model_srgb(
    raw: &LoadedRaw,
    scene_rgb: [f32; 3],
    view_gain: f32,
) -> [f32; 3] {
    let working = pipeline_scene_to_working_rec2020(raw, scene_rgb);
    let scaled = working.map(|value| value.max(0.0) * view_gain.max(1e-6));
    let luma = (scaled[0] * 0.2627 + scaled[1] * 0.6780 + scaled[2] * 0.0593).max(0.0);
    let shoulder = 1.0 / (1.0 + luma);
    display_linear_rec2020_to_model_srgb(scaled.map(|value| value * shoulder))
}

pub fn remove_model_srgb_to_canonical_scene(
    raw: &LoadedRaw,
    exposure: &ExposureParams,
    srgb: [f32; 3],
    view_gain: f32,
) -> [f32; 3] {
    let mapped = model_srgb_to_display_linear_rec2020(srgb);
    let mapped_luma =
        (mapped[0] * 0.2627 + mapped[1] * 0.6780 + mapped[2] * 0.0593).clamp(0.0, 0.985);
    let undo_shoulder = 1.0 / (1.0 - mapped_luma).max(0.015);
    let gain = view_gain.max(1e-6);
    let working = mapped.map(|value| value * undo_shoulder / gain);
    working_rec2020_to_canonical_remove_scene(raw, exposure, working)
}

pub fn remove_model_view_gain(raw: &LoadedRaw, scene_rgb: &[f32]) -> f32 {
    let mut luminance = Vec::new();
    for pixel in scene_rgb.chunks_exact(3).step_by(4) {
        let working = pipeline_scene_to_working_rec2020(raw, [pixel[0], pixel[1], pixel[2]]);
        let value = working[0] * 0.2627 + working[1] * 0.6780 + working[2] * 0.0593;
        if value.is_finite() && value > 1e-6 {
            luminance.push(value.min(64.0));
        }
    }
    if luminance.is_empty() {
        return 1.0;
    }
    luminance.sort_by(f32::total_cmp);
    let index = ((luminance.len() - 1) as f32 * 0.75).round() as usize;
    let p75 = luminance[index].max(1e-5);
    // Place the upper quartile like a normally exposed photo (about 0.63 in
    // sRGB), the kind of image Big-LaMa was trained on.
    let target_display = 0.35;
    let target_linear = target_display / (1.0 - target_display);
    (target_linear / p75).clamp(0.25, 64.0)
}
