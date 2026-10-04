//! White balance models: daylight, temperature/tint ranges and conversions.

use super::*;

pub(in crate::pipeline::raw_loader) fn daylight_white_balance(
    model: &CameraWhiteBalanceModel,
) -> Option<[f32; 3]> {
    let xyz_to_camera = match &model.color {
        CameraColorModel::Dng {
            endpoints,
            analog_balance,
        } => {
            let target = interpolate_endpoints(endpoints, endpoint_weight(endpoints, 6504.0));
            matrix::multiply(
                matrix::multiply(*analog_balance, target.calibration),
                target.color_matrix,
            )
        }
        CameraColorModel::Matrix { xyz_to_camera } => *xyz_to_camera,
    };
    let physical = matrix::transform(xyz_to_camera, D65_XYZ);
    let mut sums = [0.0f32; 3];
    let mut counts = [0u32; 3];
    for (index, response) in physical.into_iter().enumerate() {
        let Some(channel) = logical_rgb_channel(model.cdesc, index) else {
            continue;
        };
        if response.is_finite() && response > 1e-8 {
            sums[channel] += response;
            counts[channel] += 1;
        }
    }
    let response = std::array::from_fn(|channel| {
        (counts[channel] > 0).then(|| sums[channel] / counts[channel] as f32)
    });
    let [Some(red), Some(green), Some(blue)] = response else {
        return None;
    };
    let wb = [green / red, 1.0, green / blue];
    wb.into_iter()
        .all(|value| value.is_finite() && value > 0.0)
        .then_some(wb)
}

pub(super) fn parsed_camera_color_model(
    profile: &DcpProfile,
    color: &ffi::libraw_colordata_t,
    analog_balance: [[f32; 4]; 4],
    calibration_compatible: bool,
) -> Option<CameraColorModel> {
    let endpoint = |index: usize| {
        let set = &profile.matrices[index];
        Some(DngColorEndpoint {
            cct: set.illuminant.and_then(calibration_illuminant_cct),
            color_matrix: set
                .color_matrix
                .filter(|matrix| matrix4x3_is_valid(*matrix))?,
            calibration: parsed_calibration(
                set,
                color.dng_color[index].calibration,
                calibration_compatible,
            ),
            forward_matrix: set
                .forward_matrix
                .filter(|matrix| matrix3x4_is_valid(*matrix)),
        })
    };
    paired_endpoints(endpoint(0), endpoint(1)).map(|endpoints| CameraColorModel::Dng {
        endpoints: Box::new(endpoints),
        analog_balance,
    })
}

pub(super) fn libraw_camera_color_model(
    color: &ffi::libraw_colordata_t,
    analog_balance: [[f32; 4]; 4],
    calibration_compatible: bool,
) -> Option<CameraColorModel> {
    let endpoint = |index: usize| {
        let set = &color.dng_color[index];
        matrix4x3_is_valid(set.colormatrix).then(|| DngColorEndpoint {
            cct: calibration_illuminant_cct(set.illuminant),
            color_matrix: set.colormatrix,
            calibration: if calibration_compatible {
                identity_fallback_4x4(set.calibration)
            } else {
                identity_4x4()
            },
            forward_matrix: matrix3x4_is_valid(set.forwardmatrix).then_some(set.forwardmatrix),
        })
    };
    paired_endpoints(endpoint(0), endpoint(1)).map(|endpoints| CameraColorModel::Dng {
        endpoints: Box::new(endpoints),
        analog_balance,
    })
}

fn paired_endpoints(
    first: Option<DngColorEndpoint>,
    second: Option<DngColorEndpoint>,
) -> Option<[DngColorEndpoint; 2]> {
    match (first, second) {
        (Some(a), Some(b)) => Some([a, b]),
        (Some(a), None) => Some([a, a]),
        (None, Some(b)) => Some([b, b]),
        (None, None) => None,
    }
}

pub(super) fn cct_from_profile_weight(color: &CameraColorModel, weight: f32) -> Option<f32> {
    let CameraColorModel::Dng { endpoints, .. } = color else {
        return None;
    };
    let first = 1_000_000.0 / endpoints[0].cct?.max(1.0);
    let second = 1_000_000.0 / endpoints[1].cct?.max(1.0);
    Some(1_000_000.0 / (first + (second - first) * weight.clamp(0.0, 1.0)))
}

pub(super) fn estimate_cct_from_model(color: &CameraColorModel, wb: [f32; 4]) -> Option<f32> {
    let neutral = camera_neutral(wb);
    let xyz_to_camera = match color {
        CameraColorModel::Dng {
            endpoints,
            analog_balance,
        } => matrix::multiply(
            matrix::multiply(*analog_balance, endpoints[0].calibration),
            endpoints[0].color_matrix,
        ),
        CameraColorModel::Matrix { xyz_to_camera } => *xyz_to_camera,
    };
    xyz_to_cct(matrix::transform(pseudoinverse(xyz_to_camera), neutral))
}

pub(in crate::pipeline::raw_loader) fn adjusted_white_balance_coefficients(
    model: &CameraWhiteBalanceModel,
    temperature: f32,
    tint: f32,
) -> Option<[f32; 4]> {
    let (base_cct, base_tint) =
        temperature_tint_from_coefficients(model, model.base_wb).unwrap_or((model.base_cct, 1.0));
    let (target_cct, target_tint) = clamp_white_balance_temperature_tint(
        model,
        temperature_kelvin_from_offset(base_cct, temperature),
        white_balance_tint_from_offset(base_tint, tint),
    )?;
    let target_wb = temperature_tint_to_coefficients(model, target_cct, target_tint)?;
    Some(canonicalize_f32x4(
        target_wb,
        canonical_cfa_map(model.cdesc).ok()?,
    ))
}

fn white_balance_xyz_to_camera(model: &CameraWhiteBalanceModel) -> [[f32; 3]; 4] {
    match &model.color {
        CameraColorModel::Dng {
            endpoints,
            analog_balance,
        } => {
            let reference = interpolate_endpoints(endpoints, endpoint_weight(endpoints, 6504.0));
            matrix::multiply(
                matrix::multiply(*analog_balance, reference.calibration),
                reference.color_matrix,
            )
        }
        CameraColorModel::Matrix { xyz_to_camera } => *xyz_to_camera,
    }
}

fn darktable_temperature_xyz(temperature: f32) -> Option<[f32; 3]> {
    let t = temperature.clamp(MIN_TEMPERATURE_KELVIN, MAX_TEMPERATURE_KELVIN);
    let [x, y] = if t < 4_000.0 {
        planckian_xy(t)?
    } else {
        let x = if t <= 7_000.0 {
            -4.607e9 / t.powi(3) + 2.9678e6 / t.powi(2) + 0.09911e3 / t + 0.244_063
        } else {
            -2.0064e9 / t.powi(3) + 1.9018e6 / t.powi(2) + 0.24748e3 / t + 0.237_04
        };
        let y = -3.0 * x * x + 2.87 * x - 0.275;
        [x, y]
    };
    (x.is_finite() && y.is_finite() && y > 1e-10).then_some([x / y, 1.0, (1.0 - x - y) / y])
}

fn darktable_temperature_tint_xyz(temperature: f32, tint: f32) -> Option<[f32; 3]> {
    let mut xyz = darktable_temperature_xyz(temperature)?;
    xyz[1] /= tint.clamp(MIN_WHITE_BALANCE_TINT, MAX_WHITE_BALANCE_TINT);
    Some(xyz)
}

// Temperature slider limits are evaluated at whole-Kelvin ticks.  Their
// daylight/blackbody coordinates depend only on that tick, not on the camera
// or current edit, so avoid repeating the comparatively expensive polynomial
// evaluation for every tick of every UI frame.
static WHITE_BALANCE_TEMPERATURE_XYZ: OnceLock<Box<[[f32; 3]]>> = OnceLock::new();

fn white_balance_temperature_xyz_ticks() -> &'static [[f32; 3]] {
    WHITE_BALANCE_TEMPERATURE_XYZ.get_or_init(|| {
        ((MIN_TEMPERATURE_KELVIN as u32)..=(MAX_TEMPERATURE_KELVIN as u32))
            .map(|kelvin| {
                darktable_temperature_xyz(kelvin as f32)
                    .expect("the supported whole-Kelvin range must have valid coordinates")
            })
            .collect::<Vec<_>>()
            .into_boxed_slice()
    })
}

// Keep the UI endpoints strictly inside the positive camera-response domain.
// Rounding inward to the displayed precision also prevents a drag/typed value
// from rounding onto a zero response, where reciprocal WB gains are undefined.
const MIN_WB_CAMERA_RESPONSE: f32 = 1e-8;
const TINT_PRECISION: f64 = 1_000.0;

fn missing_second_green(model: &CameraWhiteBalanceModel, index: usize, row: [f32; 3]) -> bool {
    index == 3 && matches!(model.cdesc[index] as char, 'G' | 'g') && row == [0.0; 3]
}

pub(in crate::pipeline::raw_loader) fn white_balance_tint_range(
    model: &CameraWhiteBalanceModel,
    temperature: f32,
) -> Option<RangeInclusive<f32>> {
    let xyz = darktable_temperature_xyz(temperature)?;
    let matrix = white_balance_xyz_to_camera(model);
    let mut low = f64::from(MIN_WHITE_BALANCE_TINT);
    let mut high = f64::from(MAX_WHITE_BALANCE_TINT);
    for (index, row) in matrix.into_iter().enumerate() {
        if logical_rgb_channel(model.cdesc, index).is_none()
            || missing_second_green(model, index, row)
        {
            continue;
        }
        // CAM = X*mX + mY/tint + Z*mZ. For positive tint,
        // CAM > epsilon is the linear inequality a*tint + b > 0.
        let a = f64::from(row[0]) * f64::from(xyz[0]) + f64::from(row[2]) * f64::from(xyz[2])
            - f64::from(MIN_WB_CAMERA_RESPONSE);
        let b = f64::from(row[1]);
        if !a.is_finite() || !b.is_finite() {
            return None;
        }
        if a > 0.0 {
            low = low.max(-b / a);
        } else if a < 0.0 {
            high = high.min(-b / a);
        } else if b <= 0.0 {
            return None;
        }
    }
    // The tolerance avoids losing a tick to the f32 representation of 0.135.
    let mut low = ((low * TINT_PRECISION - 1e-4).ceil() / TINT_PRECISION) as f32;
    let mut high = ((high * TINT_PRECISION + 1e-4).floor() / TINT_PRECISION) as f32;
    if low > high {
        return None;
    }
    if temperature_tint_to_coefficients(model, temperature, low).is_none() {
        low += 0.001;
    }
    if temperature_tint_to_coefficients(model, temperature, high).is_none() {
        high -= 0.001;
    }
    (low <= high
        && temperature_tint_to_coefficients(model, temperature, low).is_some()
        && temperature_tint_to_coefficients(model, temperature, high).is_some())
    .then_some(low..=high)
}

pub(in crate::pipeline::raw_loader) fn clamp_white_balance_temperature_tint(
    model: &CameraWhiteBalanceModel,
    temperature: f32,
    tint: f32,
) -> Option<(f32, f32)> {
    let mut temperature = temperature.clamp(MIN_TEMPERATURE_KELVIN, MAX_TEMPERATURE_KELVIN);
    let range = if let Some(range) = white_balance_tint_range(model, temperature) {
        range
    } else {
        // Some profiles cannot describe any positive neutral at an extreme
        // temperature. Stay in the connected range around their as-shot white.
        let (base_temperature, base_tint) =
            temperature_tint_from_coefficients(model, model.base_wb)?;
        let range = white_balance_temperature_range(model, base_temperature, base_tint)?;
        temperature = temperature.clamp(*range.start(), *range.end());
        white_balance_tint_range(model, temperature)?
    };
    Some((temperature, tint.clamp(*range.start(), *range.end())))
}

pub(in crate::pipeline::raw_loader) fn white_balance_temperature_range(
    model: &CameraWhiteBalanceModel,
    temperature: f32,
    tint: f32,
) -> Option<RangeInclusive<f32>> {
    temperature_tint_to_coefficients(model, temperature, tint)?;
    let matrix = white_balance_xyz_to_camera(model);
    let xyz_ticks = white_balance_temperature_xyz_ticks();
    let tint = tint.clamp(MIN_WHITE_BALANCE_TINT, MAX_WHITE_BALANCE_TINT);
    let valid = |kelvin: f32| {
        debug_assert_eq!(kelvin, kelvin.round());
        let index = (kelvin as u32).saturating_sub(MIN_TEMPERATURE_KELVIN as u32) as usize;
        let Some(mut xyz) = xyz_ticks.get(index).copied() else {
            return false;
        };
        xyz[1] /= tint;
        let camera = matrix::transform(matrix, xyz);
        (0..4).all(|index| {
            logical_rgb_channel(model.cdesc, index).is_none()
                || missing_second_green(model, index, matrix[index])
                || (camera[index].is_finite() && camera[index] > MIN_WB_CAMERA_RESPONSE)
        })
    };
    // Check the connected interval at the slider's one-Kelvin precision.
    // Checking only the two extremes misses holes at the blackbody/daylight
    // transition near 4000 K and would let the slider cross an invalid region.
    let mut low = temperature;
    let mut next = temperature.ceil() - 1.0;
    while next >= MIN_TEMPERATURE_KELVIN && valid(next) {
        low = next;
        next -= 1.0;
    }
    let mut high = temperature;
    next = temperature.floor() + 1.0;
    while next <= MAX_TEMPERATURE_KELVIN && valid(next) {
        high = next;
        next += 1.0;
    }
    Some(low..=high)
}

pub(in crate::pipeline::raw_loader) fn temperature_tint_to_coefficients(
    model: &CameraWhiteBalanceModel,
    temperature: f32,
    tint: f32,
) -> Option<[f32; 4]> {
    let matrix = white_balance_xyz_to_camera(model);
    let camera = matrix::transform(matrix, darktable_temperature_tint_xyz(temperature, tint)?);
    let mut coefficients = [1.0; 4];
    for index in 0..4 {
        if logical_rgb_channel(model.cdesc, index).is_none() {
            coefficients[index] = model.base_wb[index];
        } else if camera[index].is_finite() && camera[index] > MIN_WB_CAMERA_RESPONSE {
            coefficients[index] = 1.0 / camera[index];
        } else if missing_second_green(model, index, matrix[index]) {
            coefficients[index] = coefficients[1];
        } else {
            return None;
        }
    }
    Some(white_balance(coefficients, model.cdesc))
}

pub(in crate::pipeline::raw_loader) fn temperature_tint_from_coefficients(
    model: &CameraWhiteBalanceModel,
    coefficients: [f32; 4],
) -> Option<(f32, f32)> {
    let mut camera_neutral = [0.0; 4];
    for index in 0..4 {
        let coefficient = coefficients[index];
        camera_neutral[index] = if coefficient.is_finite() && coefficient > 1e-8 {
            1.0 / coefficient
        } else {
            0.0
        };
    }
    let camera_to_xyz = pseudoinverse(white_balance_xyz_to_camera(model));
    let xyz = matrix::transform(camera_to_xyz, camera_neutral);
    if xyz[0].abs() <= 1e-10 || !xyz.iter().all(|value| value.is_finite()) {
        return None;
    }

    let target_ratio = xyz[2] / xyz[0];
    let mut low = MIN_TEMPERATURE_KELVIN;
    let mut high = MAX_TEMPERATURE_KELVIN;
    while high - low > 0.25 {
        let midpoint = 0.5 * (low + high);
        let reference = darktable_temperature_xyz(midpoint)?;
        if reference[2] / reference[0] > target_ratio {
            high = midpoint;
        } else {
            low = midpoint;
        }
    }
    let temperature = 0.5 * (low + high);
    let reference = darktable_temperature_xyz(temperature)?;
    let xyz_y_over_x = xyz[1] / xyz[0];
    if xyz_y_over_x.abs() <= 1e-10 {
        return None;
    }
    let tint = ((reference[1] / reference[0]) / xyz_y_over_x)
        .clamp(MIN_WHITE_BALANCE_TINT, MAX_WHITE_BALANCE_TINT);
    Some((temperature, tint))
}
