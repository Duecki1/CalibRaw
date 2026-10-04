//! Camera-to-working matrices from DNG/DCP profiles and LibRaw colour data.

use super::*;

pub(super) fn cam_to_working(xyz_to_cam: [[f32; 3]; 4], cdesc: [u8; 4]) -> [[f32; 4]; 3] {
    fold_physical_camera_planes(camera_to_working_physical(xyz_to_cam), cdesc)
}

fn camera_to_working_physical(xyz_to_cam: [[f32; 3]; 4]) -> [[f32; 4]; 3] {
    let cam_to_xyz = normalized_pseudoinverse(xyz_to_cam);
    let mut physical = [[0.0; 4]; 3];
    for row in 0..3 {
        for col in 0..4 {
            physical[row][col] = XYZ_TO_REC2020[row][0] * cam_to_xyz[0][col]
                + XYZ_TO_REC2020[row][1] * cam_to_xyz[1][col]
                + XYZ_TO_REC2020[row][2] * cam_to_xyz[2][col];
        }
    }
    physical
}

/// Builds the camera colour stage from decoder-neutral metadata.  Rawler and
/// LibRaw can therefore share the same DCP and working-space treatment without
/// manufacturing a LibRaw FFI colour-data value.
pub(in crate::pipeline::raw_loader) fn camera_to_working_matrix_from_profiles(
    xyz_to_cam: [[f32; 3]; 4],
    wb_coeffs: [f32; 4],
    cdesc: [u8; 4],
    embedded_profile: Option<&DcpProfile>,
    selected_profile: Option<&DcpProfile>,
    analog_balance: [[f32; 4]; 4],
) -> Result<([[f32; 4]; 3], f32, Option<CameraWhiteBalanceModel>)> {
    let profile = selected_profile.or(embedded_profile);
    let (matrix, weight, color_model) = if let Some(profile) = profile {
        let endpoints = profile.matrices.iter().filter_map(|set| {
            Some(DngColorEndpoint {
                cct: set.illuminant.and_then(calibration_illuminant_cct),
                color_matrix: set
                    .color_matrix
                    .filter(|matrix| matrix4x3_is_valid(*matrix))?,
                calibration: parsed_calibration(
                    set,
                    identity_4x4(),
                    profile.calibration_is_compatible(),
                ),
                forward_matrix: set
                    .forward_matrix
                    .filter(|matrix| matrix3x4_is_valid(*matrix)),
            })
        });
        let endpoints = endpoints.collect::<Vec<_>>();
        if let Some(first) = endpoints.first().copied() {
            let second = endpoints.get(1).copied().unwrap_or(first);
            let fallback_calibration = std::array::from_fn(|index| {
                embedded_profile
                    .and_then(|embedded| embedded.matrices[index].camera_calibration)
                    .unwrap_or_else(identity_4x4)
            });
            let initial_model = CameraColorModel::Dng {
                endpoints: Box::new([first, second]),
                analog_balance,
            };
            let interpolated = interpolated_parsed_dng_profile_with_fallbacks(
                profile,
                wb_coeffs,
                analog_balance,
                profile.calibration_is_compatible(),
                fallback_calibration,
                estimate_cct_from_model(&initial_model, wb_coeffs),
            )
            .ok_or_else(|| anyhow!("DCP profile has no usable color matrix"))?;
            let weight = interpolated.weight;
            let matrix =
                dng_camera_to_working(interpolated, analog_balance, wb_coeffs, wb_coeffs, cdesc)?;
            let color_model = Some(CameraColorModel::Dng {
                endpoints: Box::new([first, second]),
                analog_balance,
            });
            (matrix, weight, color_model)
        } else {
            (cam_to_working(xyz_to_cam, cdesc), 0.0, None)
        }
    } else {
        (cam_to_working(xyz_to_cam, cdesc), 0.0, None)
    };
    if matrix.iter().flatten().any(|value| !value.is_finite())
        || matrix.iter().flatten().all(|value| value.abs() <= 1e-12)
    {
        return Err(anyhow!("camera colour matrix is invalid or singular"));
    }
    let color_model = color_model.or(Some(CameraColorModel::Matrix {
        xyz_to_camera: xyz_to_cam,
    }));
    Ok((
        matrix,
        weight,
        color_model.map(|color| CameraWhiteBalanceModel {
            base_wb: wb_coeffs,
            cdesc,
            base_cct: cct_from_profile_weight(&color, weight).unwrap_or(6504.0),
            color,
        }),
    ))
}

#[derive(Clone, Copy)]
pub(super) struct InterpolatedDngProfile {
    pub(super) color_matrix: [[f32; 3]; 4],
    pub(super) calibration: [[f32; 4]; 4],
    pub(super) forward_matrix: Option<[[f32; 4]; 3]>,
    pub(super) weight: f32,
}

pub(super) fn camera_to_working_matrix(
    color: &ffi::libraw_colordata_t,
    wb_coeffs: [f32; 4],
    cdesc: [u8; 4],
    parsed_profile: Option<&DcpProfile>,
    calibration_compatible: bool,
) -> Result<([[f32; 4]; 3], f32, Option<CameraWhiteBalanceModel>)> {
    let analog_balance = analog_balance_matrix(color.dng_levels.analogbalance);
    let dng_profile = parsed_profile
        .and_then(|profile| {
            interpolated_parsed_dng_profile(
                profile,
                color,
                wb_coeffs,
                cdesc,
                analog_balance,
                calibration_compatible,
            )
        })
        .or_else(|| {
            interpolated_dng_profile(
                color,
                wb_coeffs,
                cdesc,
                analog_balance,
                calibration_compatible,
            )
        });
    let (matrix, weight) = if let Some(profile) = dng_profile {
        (
            dng_camera_to_working(profile, analog_balance, wb_coeffs, wb_coeffs, cdesc)?,
            profile.weight,
        )
    } else {
        (cam_to_working(color.cam_xyz, cdesc), 0.0)
    };

    if matrix.iter().flatten().any(|value| !value.is_finite())
        || matrix.iter().flatten().all(|value| value.abs() <= 1e-12)
    {
        return Err(anyhow!(
                "LibRaw did not provide an invertible camera colour matrix; refusing to treat camera RGB as the working colour space"
            ));
    }
    let color_model = parsed_profile
        .and_then(|profile| {
            parsed_camera_color_model(profile, color, analog_balance, calibration_compatible)
        })
        .or_else(|| libraw_camera_color_model(color, analog_balance, calibration_compatible))
        .unwrap_or(CameraColorModel::Matrix {
            xyz_to_camera: color.cam_xyz,
        });
    let base_cct = cct_from_profile_weight(&color_model, weight)
        .or_else(|| estimate_scene_cct(color, wb_coeffs, cdesc))
        .or_else(|| estimate_cct_from_model(&color_model, wb_coeffs))
        .unwrap_or(6504.0)
        .clamp(1500.0, 50_000.0);
    let model = CameraWhiteBalanceModel {
        base_wb: wb_coeffs,
        cdesc,
        base_cct,
        color: color_model,
    };
    Ok((matrix, weight, Some(model)))
}

pub(super) fn endpoint_weight(endpoints: &[DngColorEndpoint; 2], cct: f32) -> f32 {
    match (endpoints[0].cct, endpoints[1].cct) {
        (Some(first), Some(second)) => mired_interpolation_weight(cct, first, second),
        _ => 0.0,
    }
}

pub(super) fn interpolate_endpoints(
    endpoints: &[DngColorEndpoint; 2],
    weight: f32,
) -> InterpolatedDngProfile {
    InterpolatedDngProfile {
        color_matrix: matrix::lerp(endpoints[0].color_matrix, endpoints[1].color_matrix, weight),
        calibration: matrix::lerp(endpoints[0].calibration, endpoints[1].calibration, weight),
        forward_matrix: interpolate_optional_forward_matrix(
            endpoints[0].forward_matrix,
            endpoints[1].forward_matrix,
            weight,
        ),
        weight,
    }
}

fn interpolated_parsed_dng_profile(
    profile: &DcpProfile,
    color: &ffi::libraw_colordata_t,
    wb_coeffs: [f32; 4],
    cdesc: [u8; 4],
    analog_balance: [[f32; 4]; 4],
    calibration_compatible: bool,
) -> Option<InterpolatedDngProfile> {
    interpolated_parsed_dng_profile_with_fallbacks(
        profile,
        wb_coeffs,
        analog_balance,
        calibration_compatible,
        [
            color.dng_color[0].calibration,
            color.dng_color[1].calibration,
        ],
        estimate_scene_cct(color, wb_coeffs, cdesc),
    )
}

pub(super) fn interpolated_parsed_dng_profile_with_fallbacks(
    profile: &DcpProfile,
    wb_coeffs: [f32; 4],
    analog_balance: [[f32; 4]; 4],
    calibration_compatible: bool,
    fallback_calibration: [[[f32; 4]; 4]; 2],
    initial_scene_cct: Option<f32>,
) -> Option<InterpolatedDngProfile> {
    let first = &profile.matrices[0];
    let second = &profile.matrices[1];
    let valid = [
        first.color_matrix.is_some_and(matrix4x3_is_valid),
        second.color_matrix.is_some_and(matrix4x3_is_valid),
    ];
    match valid {
        [false, false] => return None,
        [true, false] => {
            return parsed_single_dng_profile(
                first,
                fallback_calibration[0],
                0.0,
                calibration_compatible,
            )
        }
        [false, true] => {
            return parsed_single_dng_profile(
                second,
                fallback_calibration[1],
                1.0,
                calibration_compatible,
            )
        }
        [true, true] => {}
    }

    let cct0 = calibration_illuminant_cct(first.illuminant?)?;
    let cct1 = calibration_illuminant_cct(second.illuminant?)?;
    let mut scene_cct = initial_scene_cct.unwrap_or_else(|| (cct0 * cct1).sqrt());
    let neutral = camera_neutral(wb_coeffs);
    let first_color = first.color_matrix?;
    let second_color = second.color_matrix?;
    let first_calibration =
        parsed_calibration(first, fallback_calibration[0], calibration_compatible);
    let second_calibration =
        parsed_calibration(second, fallback_calibration[1], calibration_compatible);

    let mut weight = mired_interpolation_weight(scene_cct, cct0, cct1);
    for _ in 0..6 {
        let color_matrix = matrix::lerp(first_color, second_color, weight);
        let calibration = matrix::lerp(first_calibration, second_calibration, weight);
        let abcc = matrix::multiply(analog_balance, calibration);
        let xyz_to_camera = matrix::multiply(abcc, color_matrix);
        let camera_to_xyz = pseudoinverse(xyz_to_camera);
        let white_xyz = matrix::transform(camera_to_xyz, neutral);
        if let Some(refined) = xyz_to_cct(white_xyz) {
            scene_cct = refined.clamp(1500.0, 50_000.0);
            weight = mired_interpolation_weight(scene_cct, cct0, cct1);
        }
    }

    Some(InterpolatedDngProfile {
        color_matrix: matrix::lerp(first_color, second_color, weight),
        calibration: matrix::lerp(first_calibration, second_calibration, weight),
        forward_matrix: interpolate_optional_forward_matrix(
            first.forward_matrix,
            second.forward_matrix,
            weight,
        ),
        weight,
    })
}

fn parsed_single_dng_profile(
    set: &DcpMatrixSet,
    fallback_calibration: [[f32; 4]; 4],
    weight: f32,
    calibration_compatible: bool,
) -> Option<InterpolatedDngProfile> {
    Some(InterpolatedDngProfile {
        color_matrix: set.color_matrix?,
        calibration: parsed_calibration(set, fallback_calibration, calibration_compatible),
        forward_matrix: set
            .forward_matrix
            .filter(|matrix| matrix3x4_is_valid(*matrix)),
        weight,
    })
}

pub(super) fn parsed_calibration(
    set: &DcpMatrixSet,
    fallback: [[f32; 4]; 4],
    calibration_compatible: bool,
) -> [[f32; 4]; 4] {
    if calibration_compatible {
        set.camera_calibration
            .filter(|matrix| matrix4x4_is_valid(*matrix))
            .unwrap_or_else(|| identity_fallback_4x4(fallback))
    } else {
        identity_4x4()
    }
}

fn interpolated_dng_profile(
    color: &ffi::libraw_colordata_t,
    wb_coeffs: [f32; 4],
    cdesc: [u8; 4],
    analog_balance: [[f32; 4]; 4],
    calibration_compatible: bool,
) -> Option<InterpolatedDngProfile> {
    let valid = [
        matrix4x3_is_valid(color.dng_color[0].colormatrix),
        matrix4x3_is_valid(color.dng_color[1].colormatrix),
    ];
    match valid {
        [false, false] => return None,
        [true, false] => {
            return Some(single_dng_profile(
                &color.dng_color[0],
                0.0,
                calibration_compatible,
            ));
        }
        [false, true] => {
            return Some(single_dng_profile(
                &color.dng_color[1],
                1.0,
                calibration_compatible,
            ));
        }
        [true, true] => {}
    }

    let cct0 = calibration_illuminant_cct(color.dng_color[0].illuminant)?;
    let cct1 = calibration_illuminant_cct(color.dng_color[1].illuminant)?;
    let mut scene_cct =
        estimate_scene_cct(color, wb_coeffs, cdesc).unwrap_or_else(|| (cct0 * cct1).sqrt());
    let neutral = camera_neutral(wb_coeffs);

    let mut weight = mired_interpolation_weight(scene_cct, cct0, cct1);
    for _ in 0..6 {
        let color_matrix = matrix::lerp(
            color.dng_color[0].colormatrix,
            color.dng_color[1].colormatrix,
            weight,
        );
        let calibration = if calibration_compatible {
            matrix::lerp(
                identity_fallback_4x4(color.dng_color[0].calibration),
                identity_fallback_4x4(color.dng_color[1].calibration),
                weight,
            )
        } else {
            identity_4x4()
        };
        let abcc = matrix::multiply(analog_balance, calibration);
        let xyz_to_camera = matrix::multiply(abcc, color_matrix);
        let camera_to_xyz = pseudoinverse(xyz_to_camera);
        let white_xyz = matrix::transform(camera_to_xyz, neutral);
        if let Some(refined) = xyz_to_cct(white_xyz) {
            scene_cct = refined.clamp(1500.0, 50_000.0);
            weight = mired_interpolation_weight(scene_cct, cct0, cct1);
        }
    }

    let color_matrix = matrix::lerp(
        color.dng_color[0].colormatrix,
        color.dng_color[1].colormatrix,
        weight,
    );
    let calibration = if calibration_compatible {
        matrix::lerp(
            identity_fallback_4x4(color.dng_color[0].calibration),
            identity_fallback_4x4(color.dng_color[1].calibration),
            weight,
        )
    } else {
        identity_4x4()
    };
    let forward_matrix = interpolate_forward_matrix(
        color.dng_color[0].forwardmatrix,
        color.dng_color[1].forwardmatrix,
        weight,
    );
    Some(InterpolatedDngProfile {
        color_matrix,
        calibration,
        forward_matrix,
        weight,
    })
}

fn single_dng_profile(
    dng: &ffi::libraw_dng_color_t,
    weight: f32,
    calibration_compatible: bool,
) -> InterpolatedDngProfile {
    InterpolatedDngProfile {
        color_matrix: dng.colormatrix,
        calibration: if calibration_compatible {
            identity_fallback_4x4(dng.calibration)
        } else {
            identity_4x4()
        },
        forward_matrix: matrix3x4_is_valid(dng.forwardmatrix).then_some(dng.forwardmatrix),
        weight,
    }
}

pub(super) fn dng_camera_to_working(
    profile: InterpolatedDngProfile,
    analog_balance: [[f32; 4]; 4],
    neutral_wb: [f32; 4],
    applied_wb: [f32; 4],
    cdesc: [u8; 4],
) -> Result<[[f32; 4]; 3]> {
    let abcc = matrix::multiply(analog_balance, profile.calibration);
    let neutral = camera_neutral(neutral_wb);

    let camera_to_xyz_d50 = if let Some(forward) = profile.forward_matrix {
        let forward = normalize_forward_matrix(forward)?;
        let inverse_abcc = matrix::invert(abcc)
            .ok_or_else(|| anyhow!("DNG AnalogBalance * CameraCalibration is singular"))?;
        let reference_neutral = matrix::transform(inverse_abcc, neutral);
        let mut balanced_reference_to_xyz = forward;
        for column in 0..4 {
            let value = reference_neutral[column];
            if !value.is_finite() || value.abs() < 1e-10 {
                return Err(anyhow!("DNG ReferenceNeutral contains an invalid channel"));
            }
            for row in &mut balanced_reference_to_xyz {
                row[column] /= value;
            }
        }
        matrix::multiply(balanced_reference_to_xyz, inverse_abcc)
    } else {
        let xyz_to_camera = matrix::multiply(abcc, profile.color_matrix);
        let camera_to_xyz = pseudoinverse(xyz_to_camera);
        if camera_to_xyz
            .iter()
            .flatten()
            .all(|value| value.abs() <= 1e-12)
        {
            return Err(anyhow!("DNG XYZ-to-camera matrix is singular"));
        }
        let source_white = matrix::transform(camera_to_xyz, neutral);
        let adaptation = bradford_adaptation(source_white, DNG_PCS_D50_XYZ)
            .ok_or_else(|| anyhow!("DNG CameraNeutral does not define a valid white point"))?;
        // Bradford preserves the white's luminance; rescale so CameraNeutral
        // reaches PCS white at Y = 1, as the ForwardMatrix path does.
        let white_luminance = source_white[1];
        matrix::multiply(adaptation, camera_to_xyz).map(|row| row.map(|v| v / white_luminance))
    };

    // Bradford from DNG_PCS_D50_XYZ to D65_XYZ.
    const D50_TO_D65: [[f32; 3]; 3] = [
        [0.955_473_4, -0.023_098_5, 0.063_259_3],
        [-0.028_369_7, 1.009_995_5, 0.021_041_4],
        [0.012_314, -0.020_507_7, 1.330_365_9],
    ];
    let xyz_d50_to_rec2020 = matrix::multiply(XYZ_TO_REC2020, D50_TO_D65);
    let mut physical = matrix::multiply(xyz_d50_to_rec2020, camera_to_xyz_d50);
    for column in 0..4 {
        let gain = applied_wb[column].max(1e-8);
        for row in &mut physical {
            row[column] /= gain;
        }
    }
    Ok(fold_physical_camera_planes(physical, cdesc))
}

/// Scales ForwardMatrix rows so camera white maps exactly to the DNG PCS
/// white, matching the DNG SDK's `NormalizeForwardMatrix`. Stored matrices are
/// rounded and some third-party profiles are not normalized at all.
fn normalize_forward_matrix(forward: [[f32; 4]; 3]) -> Result<[[f32; 4]; 3]> {
    let mut normalized = forward;
    for (row, target) in normalized.iter_mut().zip(DNG_PCS_D50_XYZ) {
        let white = row.iter().sum::<f32>();
        if !white.is_finite() || white <= 1e-6 {
            return Err(anyhow!(
                "DNG ForwardMatrix does not map camera white to a valid XYZ"
            ));
        }
        for value in row {
            *value *= target / white;
        }
    }
    Ok(normalized)
}

fn fold_physical_camera_planes(physical: [[f32; 4]; 3], cdesc: [u8; 4]) -> [[f32; 4]; 3] {
    let mut out = [[0.0; 4]; 3];
    for (physical_col, _) in cdesc.iter().enumerate() {
        let Some(rgb_col) = logical_rgb_channel(cdesc, physical_col) else {
            continue;
        };
        for row in 0..3 {
            out[row][rgb_col] += physical[row][physical_col];
        }
    }
    out
}

fn analog_balance_matrix(values: [f32; 4]) -> [[f32; 4]; 4] {
    let mut out = [[0.0; 4]; 4];
    for index in 0..4 {
        out[index][index] = if values[index].is_finite() && values[index] > 1e-8 {
            values[index]
        } else {
            1.0
        };
    }
    out
}

pub(super) fn camera_neutral(wb_coeffs: [f32; 4]) -> [f32; 4] {
    wb_coeffs.map(|gain| 1.0 / gain.max(1e-8))
}

fn interpolate_forward_matrix(
    first: [[f32; 4]; 3],
    second: [[f32; 4]; 3],
    weight: f32,
) -> Option<[[f32; 4]; 3]> {
    match (matrix3x4_is_valid(first), matrix3x4_is_valid(second)) {
        (true, true) => Some(matrix::lerp(first, second, weight)),
        (true, false) => Some(first),
        (false, true) => Some(second),
        (false, false) => None,
    }
}

fn interpolate_optional_forward_matrix(
    first: Option<[[f32; 4]; 3]>,
    second: Option<[[f32; 4]; 3]>,
    weight: f32,
) -> Option<[[f32; 4]; 3]> {
    match (
        first.filter(|matrix| matrix3x4_is_valid(*matrix)),
        second.filter(|matrix| matrix3x4_is_valid(*matrix)),
    ) {
        (Some(a), Some(b)) => Some(matrix::lerp(a, b, weight)),
        (Some(matrix), None) | (None, Some(matrix)) => Some(matrix),
        (None, None) => None,
    }
}

pub(super) fn identity_fallback_4x4(mut matrix: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
    if !matrix.iter().flatten().all(|value| value.is_finite()) {
        return identity_4x4();
    }

    // LibRaw exposes ordinary three-channel DNG CameraCalibration matrices in
    // its four-plane storage. For RGB DNGs the unused fourth row/column may be
    // all zero, which makes the storage matrix singular even though the actual
    // 3x3 calibration is perfectly usable. Complete only that inactive plane
    // as identity before the ForwardMatrix path needs a 4x4 inverse.
    let fourth_plane_unused = matrix[3].iter().all(|value| value.abs() <= 1e-8)
        && (0..3).all(|row| matrix[row][3].abs() <= 1e-8);
    if fourth_plane_unused {
        matrix[3][3] = 1.0;
    }

    if matrix.iter().flatten().any(|value| value.abs() > 1e-8) && matrix::invert(matrix).is_some() {
        matrix
    } else {
        identity_4x4()
    }
}

pub(super) fn identity_4x4() -> [[f32; 4]; 4] {
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

pub(super) fn matrix4x3_is_valid(matrix: [[f32; 3]; 4]) -> bool {
    matrix.iter().flatten().all(|v| v.is_finite())
        && matrix.iter().flatten().any(|v| v.abs() > 1e-8)
}

pub(super) fn matrix3x4_is_valid(matrix: [[f32; 4]; 3]) -> bool {
    matrix.iter().flatten().all(|v| v.is_finite())
        && matrix.iter().flatten().any(|v| v.abs() > 1e-8)
}

fn matrix4x4_is_valid(matrix: [[f32; 4]; 4]) -> bool {
    matrix.iter().flatten().all(|v| v.is_finite())
        && matrix.iter().flatten().any(|v| v.abs() > 1e-8)
}

fn normalized_pseudoinverse(mut xyz_to_cam: [[f32; 3]; 4]) -> [[f32; 4]; 3] {
    for row in &mut xyz_to_cam {
        let white_response = row[0] * D65_XYZ[0] + row[1] * D65_XYZ[1] + row[2] * D65_XYZ[2];
        if white_response.is_finite() && white_response.abs() > 1e-12 {
            for value in row {
                *value /= white_response;
            }
        }
    }

    pseudoinverse(xyz_to_cam)
}

pub(super) fn pseudoinverse(input: [[f32; 3]; 4]) -> [[f32; 4]; 3] {
    let mut temp = [[0.0f64; 6]; 3];

    for i in 0..3 {
        temp[i][i + 3] = 1.0;
        for j in 0..3 {
            for row in &input {
                temp[i][j] += f64::from(row[i]) * f64::from(row[j]);
            }
        }
    }

    for i in 0..3 {
        let mut pivot_row = i;
        let mut pivot_abs = temp[i][i].abs();
        for (row, values) in temp.iter().enumerate().skip(i + 1) {
            let candidate = values[i].abs();
            if candidate > pivot_abs {
                pivot_abs = candidate;
                pivot_row = row;
            }
        }
        if !pivot_abs.is_finite() || pivot_abs < 1e-14 {
            return [[0.0; 4]; 3];
        }
        if pivot_row != i {
            temp.swap(i, pivot_row);
        }

        let pivot = temp[i][i];
        for value in &mut temp[i] {
            *value /= pivot;
        }
        let pivot_values = temp[i];
        for (row_index, row) in temp.iter_mut().enumerate() {
            if row_index == i {
                continue;
            }
            let scale = row[i];
            for (value, pivot_value) in row.iter_mut().zip(pivot_values) {
                *value -= pivot_value * scale;
            }
        }
    }

    let mut out = [[0.0; 4]; 3];
    for col in 0..4 {
        for row in 0..3 {
            let value = (0..3)
                .map(|k| temp[row][k + 3] * f64::from(input[col][k]))
                .sum::<f64>();
            if !value.is_finite() {
                return [[0.0; 4]; 3];
            }
            out[row][col] = value as f32;
        }
    }
    out
}
