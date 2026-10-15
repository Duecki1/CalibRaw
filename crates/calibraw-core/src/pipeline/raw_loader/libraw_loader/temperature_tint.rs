//! White balance models: daylight, temperature/tint ranges and conversions.

use super::*;
use std::cell::RefCell;

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

/// Temperature and tint of the camera's as-shot white.
pub(in crate::pipeline::raw_loader) fn as_shot_temperature_tint(
    model: &CameraWhiteBalanceModel,
) -> (f32, f32) {
    temperature_tint_from_coefficients(model, model.base_wb).unwrap_or((model.base_cct, 1.0))
}

pub(in crate::pipeline::raw_loader) fn adjusted_white_balance_coefficients(
    model: &CameraWhiteBalanceModel,
    temperature: f32,
    tint: f32,
) -> Option<[f32; 4]> {
    let (base_cct, base_tint) = as_shot_temperature_tint(model);
    let (target_cct, target_tint) = rendered_white_balance_temperature_tint(
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

// Keep slider endpoints strictly inside the positive camera-response domain.
// Rounding inward to the displayed precision also prevents a drag/typed value
// from rounding onto a white beyond the usable limits.
const MIN_WB_CAMERA_RESPONSE: f32 = 1e-8;
const TINT_PRECISION: f64 = 1_000.0;

/// Largest ratio between the strongest and the weakest channel gain that the
/// temperature/tint controls produce. Gains are reciprocal camera responses,
/// so where a channel's response approaches zero (the magenta end on most
/// sensors) its gain grows without bound long before the response turns
/// negative. Measured across ten cameras, as-shot whites stay below 3 and
/// 1901-25000 K at neutral tint below 7, while the unlimited magenta end
/// reached several thousand.
pub(super) const MAX_WB_GAIN_RATIO: f64 = 8.0;

/// Margin kept above an as-shot white that already exceeds `MAX_WB_GAIN_RATIO`
/// (IR-converted bodies, unusual lamps), so the image stays adjustable around
/// its own white instead of sitting on the limit.
const AS_SHOT_GAIN_RATIO_HEADROOM: f64 = 1.25;

fn missing_second_green(model: &CameraWhiteBalanceModel, index: usize, row: [f32; 3]) -> bool {
    index == 3 && matches!(model.cdesc[index] as char, 'G' | 'g') && row == [0.0; 3]
}

/// Fixed slider ranges for this camera and as-shot white. Each is the usable
/// interval through the as-shot white with the other value held at its
/// as-shot setting, so neither range depends on the other slider.
pub(in crate::pipeline::raw_loader) fn white_balance_ranges(
    model: &CameraWhiteBalanceModel,
) -> Option<WhiteBalanceRanges> {
    // The UI asks for the ranges many times per frame (display, presets) and
    // every render asks again, but they only change with the model, and the
    // one-Kelvin temperature scan costs about 0.3 ms. Remember the last
    // model's ranges per thread, keyed on its full contents so a changed
    // model is never served stale ranges.
    thread_local! {
        static LAST: RefCell<Option<(CameraWhiteBalanceModel, Option<WhiteBalanceRanges>)>> =
            const { RefCell::new(None) };
    }
    LAST.with_borrow_mut(|last| {
        if let Some((_, ranges)) = last.as_ref().filter(|(cached, _)| cached == model) {
            return ranges.clone();
        }
        let ranges = UsableWhiteBalance::new(model).slider_ranges();
        *last = Some((model.clone(), ranges.clone()));
        ranges
    })
}

/// The white rendered for a selected temperature (K) and tint. Each value is
/// first limited to its own slider range. In the corners where both ranges
/// meet whites the camera cannot balance, the tint is then limited to the
/// usable whites at that temperature. Only the render is limited; the stored
/// selection stays as the user set it.
pub(in crate::pipeline::raw_loader) fn rendered_white_balance_temperature_tint(
    model: &CameraWhiteBalanceModel,
    temperature: f32,
    tint: f32,
) -> Option<(f32, f32)> {
    let (temperature, tint) = white_balance_ranges(model)?.clamp(temperature, tint);
    let (low, high) = UsableWhiteBalance::new(model).tint_bounds(temperature)?;
    Some((temperature, tint.clamp(low as f32, high as f32)))
}

/// Whites whose white-balance gains are usable: every balanced channel
/// responds positively and no gain exceeds another by more than
/// `max_gain_ratio`.
struct UsableWhiteBalance {
    xyz_to_camera: [[f32; 3]; 4],
    /// Rows of `xyz_to_camera` that receive a white-balance gain.
    balanced: [bool; 4],
    max_gain_ratio: f64,
    as_shot: (f32, f32),
}

impl UsableWhiteBalance {
    fn new(model: &CameraWhiteBalanceModel) -> Self {
        let xyz_to_camera = white_balance_xyz_to_camera(model);
        let balanced = std::array::from_fn(|index| {
            logical_rgb_channel(model.cdesc, index).is_some()
                && !missing_second_green(model, index, xyz_to_camera[index])
        });
        let as_shot = as_shot_temperature_tint(model);
        let mut usable = Self {
            xyz_to_camera,
            balanced,
            max_gain_ratio: MAX_WB_GAIN_RATIO,
            as_shot,
        };
        if let Some(ratio) = darktable_temperature_tint_xyz(as_shot.0, as_shot.1)
            .and_then(|xyz| usable.gain_ratio(xyz))
        {
            usable.max_gain_ratio = usable
                .max_gain_ratio
                .max(ratio * AS_SHOT_GAIN_RATIO_HEADROOM);
        }
        usable
    }

    fn balanced_rows(&self) -> impl Iterator<Item = usize> + '_ {
        (0..4).filter(|&index| self.balanced[index])
    }

    /// Ratio between the largest and the smallest gain balancing `xyz`, or
    /// `None` when a balanced channel does not respond positively.
    fn gain_ratio(&self, xyz: [f32; 3]) -> Option<f64> {
        let camera = matrix::transform(self.xyz_to_camera, xyz);
        let mut weakest = f64::INFINITY;
        let mut strongest = 0.0f64;
        for index in self.balanced_rows() {
            let response = camera[index];
            if !(response.is_finite() && response > MIN_WB_CAMERA_RESPONSE) {
                return None;
            }
            weakest = weakest.min(f64::from(response));
            strongest = strongest.max(f64::from(response));
        }
        (strongest > 0.0).then(|| strongest / weakest)
    }

    fn is_usable(&self, xyz: [f32; 3]) -> bool {
        self.gain_ratio(xyz)
            .is_some_and(|ratio| ratio <= self.max_gain_ratio)
    }

    fn is_usable_at(&self, temperature: f32, tint: f32) -> bool {
        darktable_temperature_tint_xyz(temperature, tint).is_some_and(|xyz| self.is_usable(xyz))
    }

    fn slider_ranges(&self) -> Option<WhiteBalanceRanges> {
        let (temperature, tint) = self.as_shot;
        let (low, high) = self.tint_bounds(temperature)?;
        // The tolerance avoids losing a tick to the f32 representation of 0.135.
        let mut low = ((low * TINT_PRECISION - 1e-4).ceil() / TINT_PRECISION) as f32;
        let mut high = ((high * TINT_PRECISION + 1e-4).floor() / TINT_PRECISION) as f32;
        if !self.is_usable_at(temperature, low) {
            low += 0.001;
        }
        if !self.is_usable_at(temperature, high) {
            high -= 0.001;
        }
        if low > high
            || !self.is_usable_at(temperature, low)
            || !self.is_usable_at(temperature, high)
        {
            return None;
        }
        Some(WhiteBalanceRanges {
            temperature: self.temperature_range(temperature, tint)?,
            tint: low..=high,
        })
    }

    /// Exact bounds of the usable tints at `temperature`.
    fn tint_bounds(&self, temperature: f32) -> Option<(f64, f64)> {
        let xyz = darktable_temperature_xyz(temperature)?;
        // With Y divided by the tint t, row i responds P_i + Q_i / t.
        // Multiplied by the positive t, each requirement on the responses
        // becomes a linear inequality a*t + b > 0.
        let responses = self.xyz_to_camera.map(|row| {
            (
                f64::from(row[0]) * f64::from(xyz[0]) + f64::from(row[2]) * f64::from(xyz[2]),
                f64::from(row[1]),
            )
        });
        let mut bounds = (
            f64::from(MIN_WHITE_BALANCE_TINT),
            f64::from(MAX_WHITE_BALANCE_TINT),
        );
        for row in self.balanced_rows() {
            let (p, q) = responses[row];
            if !limit_tint(&mut bounds, p - f64::from(MIN_WB_CAMERA_RESPONSE), q) {
                return None;
            }
            // Gain ratio limit: max_gain_ratio * response(row) > response(other).
            for other in self.balanced_rows().filter(|&other| other != row) {
                let (other_p, other_q) = responses[other];
                let ratio = self.max_gain_ratio;
                if !limit_tint(&mut bounds, ratio * p - other_p, ratio * q - other_q) {
                    return None;
                }
            }
        }
        (bounds.0 <= bounds.1).then_some(bounds)
    }

    /// Whole-Kelvin temperatures connected to `temperature` whose whites at
    /// `tint` are usable.
    fn temperature_range(&self, temperature: f32, tint: f32) -> Option<RangeInclusive<f32>> {
        if !self.is_usable_at(temperature, tint) {
            return None;
        }
        let tint = tint.clamp(MIN_WHITE_BALANCE_TINT, MAX_WHITE_BALANCE_TINT);
        let xyz_ticks = white_balance_temperature_xyz_ticks();
        let usable = |kelvin: f32| {
            debug_assert_eq!(kelvin, kelvin.round());
            let index = (kelvin as u32).saturating_sub(MIN_TEMPERATURE_KELVIN as u32) as usize;
            xyz_ticks
                .get(index)
                .is_some_and(|&[x, y, z]| self.is_usable([x, y / tint, z]))
        };
        // Check the connected interval at the slider's one-Kelvin precision.
        // Checking only the two extremes misses holes at the blackbody/daylight
        // transition near 4000 K and would let the slider cross an invalid region.
        let mut low = temperature;
        let mut next = temperature.ceil() - 1.0;
        while next >= MIN_TEMPERATURE_KELVIN && usable(next) {
            low = next;
            next -= 1.0;
        }
        let mut high = temperature;
        next = temperature.floor() + 1.0;
        while next <= MAX_TEMPERATURE_KELVIN && usable(next) {
            high = next;
            next += 1.0;
        }
        Some(low..=high)
    }
}

/// Narrows `bounds` to the positive tints t with a*t + b > 0. Returns `false`
/// when no tint satisfies it.
fn limit_tint(bounds: &mut (f64, f64), a: f64, b: f64) -> bool {
    if !a.is_finite() || !b.is_finite() {
        return false;
    }
    if a > 0.0 {
        bounds.0 = bounds.0.max(-b / a);
    } else if a < 0.0 {
        bounds.1 = bounds.1.min(-b / a);
    } else if b <= 0.0 {
        return false;
    }
    true
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
