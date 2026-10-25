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
    let endpoints = selected_profile
        .or(embedded_profile)
        .map_or([None, None], |profile| {
            let fallback_calibration = std::array::from_fn(|index| {
                embedded_profile
                    .and_then(|embedded| embedded.matrices[index].camera_calibration)
                    .unwrap_or_else(identity_4x4)
            });
            parsed_dng_endpoints(
                profile,
                fallback_calibration,
                profile.calibration_is_compatible(),
            )
        });
    let (matrix, weight, color) = if let Some(color) = dng_color_model(endpoints, analog_balance) {
        let profile = as_shot_dng_profile(endpoints, analog_balance, wb_coeffs)
            .ok_or_else(|| anyhow!("DCP profile has no usable color matrix"))?;
        let matrix = dng_camera_to_working(profile, analog_balance, wb_coeffs, wb_coeffs, cdesc)?;
        (matrix, profile.weight, color)
    } else {
        (
            cam_to_working(xyz_to_cam, cdesc),
            0.0,
            CameraColorModel::Matrix {
                xyz_to_camera: xyz_to_cam,
            },
        )
    };
    if matrix.iter().flatten().any(|value| !value.is_finite())
        || matrix.iter().flatten().all(|value| value.abs() <= 1e-12)
    {
        return Err(anyhow!("camera colour matrix is invalid or singular"));
    }
    Ok((
        matrix,
        weight,
        Some(CameraWhiteBalanceModel {
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

/// One DNG colour calibration of LibRaw's colour data.
#[derive(Clone, Copy, Debug)]
pub(in crate::pipeline::raw_loader) struct LibRawDngColor {
    pub(super) illuminant: u16,
    pub(super) calibration: [[f32; 4]; 4],
    pub(super) colormatrix: [[f32; 3]; 4],
    pub(super) forwardmatrix: [[f32; 4]; 3],
}

/// The parts of LibRaw's colour data the camera-to-working matrix is built
/// from, copied out of its context so the photo can take another camera
/// profile after the context is gone.
#[derive(Clone, Debug)]
pub(in crate::pipeline::raw_loader) struct LibRawColor {
    pub(super) cam_xyz: [[f32; 3]; 4],
    pub(super) dng_color: [LibRawDngColor; 2],
    pub(super) dng_analog_balance: [f32; 4],
    /// The camera's white balance presets by temperature: kelvin, then four
    /// multipliers.
    pub(super) wbct_coeffs: [[f32; 5]; 64],
}

impl LibRawColor {
    pub(super) fn from_libraw(color: &ffi::libraw_colordata_t) -> Self {
        Self {
            cam_xyz: color.cam_xyz,
            dng_color: std::array::from_fn(|index| {
                let set = &color.dng_color[index];
                LibRawDngColor {
                    illuminant: set.illuminant,
                    calibration: set.calibration,
                    colormatrix: set.colormatrix,
                    forwardmatrix: set.forwardmatrix,
                }
            }),
            dng_analog_balance: color.dng_levels.analogbalance,
            wbct_coeffs: color.WBCT_Coeffs,
        }
    }
}

pub(super) fn camera_to_working_matrix(
    color: &LibRawColor,
    wb_coeffs: [f32; 4],
    cdesc: [u8; 4],
    parsed_profile: Option<&DcpProfile>,
    calibration_compatible: bool,
) -> Result<([[f32; 4]; 3], f32, Option<CameraWhiteBalanceModel>)> {
    let analog_balance = analog_balance_matrix(color.dng_analog_balance);
    let parsed_endpoints = parsed_profile.map_or([None, None], |profile| {
        parsed_dng_endpoints(
            profile,
            [
                color.dng_color[0].calibration,
                color.dng_color[1].calibration,
            ],
            calibration_compatible,
        )
    });
    let libraw_endpoints = libraw_dng_endpoints(color, calibration_compatible);
    let dng_profile = as_shot_dng_profile(parsed_endpoints, analog_balance, wb_coeffs)
        .or_else(|| as_shot_dng_profile(libraw_endpoints, analog_balance, wb_coeffs));
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
    let color_model = dng_color_model(parsed_endpoints, analog_balance)
        .or_else(|| dng_color_model(libraw_endpoints, analog_balance))
        .unwrap_or(CameraColorModel::Matrix {
            xyz_to_camera: color.cam_xyz,
        });
    let base_cct = cct_from_profile_weight(&color_model, weight)
        .or_else(|| estimate_scene_cct(color, wb_coeffs, cdesc))
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
