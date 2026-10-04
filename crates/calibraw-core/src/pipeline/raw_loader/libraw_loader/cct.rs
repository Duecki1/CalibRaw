//! Correlated colour temperature estimation and illuminant interpolation.

use super::*;

pub(super) fn planckian_xy(cct: f32) -> Option<[f32; 2]> {
    let t = cct.clamp(1667.0, 25_000.0);
    let x = if t <= 4000.0 {
        -0.266_123_9e9 / t.powi(3) - 0.234_358e6 / t.powi(2) + 0.877_695_6e3 / t + 0.179_91
    } else {
        -3.025_846_9e9 / t.powi(3) + 2.107_038e6 / t.powi(2) + 0.222_634_7e3 / t + 0.240_39
    };
    let y = if t <= 2222.0 {
        -1.106_381_4 * x.powi(3) - 1.348_110_2 * x.powi(2) + 2.185_558_3 * x - 0.202_196_8
    } else if t <= 4000.0 {
        -0.954_947_6 * x.powi(3) - 1.374_185_9 * x.powi(2) + 2.091_37 * x - 0.167_488_7
    } else {
        3.081_758 * x.powi(3) - 5.873_387 * x.powi(2) + 3.751_129_9 * x - 0.370_014_8
    };
    (x.is_finite() && y.is_finite() && x > 0.0 && y > 0.0).then_some([x, y])
}

pub(super) fn estimate_scene_cct(
    color: &ffi::libraw_colordata_t,
    wb_coeffs: [f32; 4],
    cdesc: [u8; 4],
) -> Option<f32> {
    let mut best_cct = 0.0;
    let mut best_error = f32::INFINITY;

    for row in color.WBCT_Coeffs {
        let cct = row[0];
        if !cct.is_finite() || cct <= 0.0 {
            continue;
        }

        let candidate = white_balance([row[1], row[2], row[3], row[4]], cdesc);
        let error = (candidate[0].ln() - wb_coeffs[0].ln()).abs()
            + (candidate[2].ln() - wb_coeffs[2].ln()).abs();

        if error < best_error {
            best_error = error;
            best_cct = cct;
        }
    }

    if best_cct > 0.0 {
        Some(best_cct.clamp(1500.0, 50000.0))
    } else {
        None
    }
}

/// Temperatures the DNG SDK assigns to EXIF LightSource values when it
/// interpolates dual-illuminant profiles.
pub(super) fn calibration_illuminant_cct(illuminant: u16) -> Option<f32> {
    match illuminant {
        // Daylight, flash, fine weather, standard light B, D55.
        1 | 4 | 9 | 18 | 20 => Some(5500.0),
        // Fluorescent and cool white fluorescent (3800-4500 K).
        2 | 14 => Some(4150.0),
        // Tungsten and standard light A.
        3 | 17 => Some(2850.0),
        // Cloudy, standard light C, D65.
        10 | 19 | 21 => Some(6500.0),
        // Shade, D75.
        11 | 22 => Some(7500.0),
        // Daylight fluorescent (5700-7100 K).
        12 => Some(6400.0),
        // Day white fluorescent (4600-5500 K).
        13 => Some(5050.0),
        // White fluorescent (3250-3800 K).
        15 => Some(3525.0),
        // Warm white fluorescent (2600-3250 K).
        16 => Some(2925.0),
        23 => Some(5000.0),
        24 => Some(3200.0),
        _ => None,
    }
}

pub(super) fn mired_interpolation_weight(cct: f32, first_cct: f32, second_cct: f32) -> f32 {
    let first = 1_000_000.0 / first_cct.max(1.0);
    let second = 1_000_000.0 / second_cct.max(1.0);
    let scene = 1_000_000.0 / cct.max(1.0);
    let denominator = second - first;
    if denominator.abs() < 1e-8 {
        0.0
    } else {
        ((scene - first) / denominator).clamp(0.0, 1.0)
    }
}

/// Robertson (1968) isotemperature lines: reciprocal temperature in mired,
/// CIE 1960 u and v of the Planckian locus, and the isotherm slope. The DNG
/// SDK uses the same method to interpolate dual-illuminant profiles.
const ROBERTSON_ISOTHERMS: [[f64; 4]; 31] = [
    [0.0, 0.18006, 0.26352, -0.24341],
    [10.0, 0.18066, 0.26589, -0.25479],
    [20.0, 0.18133, 0.26846, -0.26876],
    [30.0, 0.18208, 0.27119, -0.28539],
    [40.0, 0.18293, 0.27407, -0.30470],
    [50.0, 0.18388, 0.27709, -0.32675],
    [60.0, 0.18494, 0.28021, -0.35156],
    [70.0, 0.18611, 0.28342, -0.37915],
    [80.0, 0.18740, 0.28668, -0.40955],
    [90.0, 0.18880, 0.28997, -0.44278],
    [100.0, 0.19032, 0.29326, -0.47888],
    [125.0, 0.19462, 0.30141, -0.58204],
    [150.0, 0.19962, 0.30921, -0.70471],
    [175.0, 0.20525, 0.31647, -0.84901],
    [200.0, 0.21142, 0.32312, -1.0182],
    [225.0, 0.21807, 0.32909, -1.2168],
    [250.0, 0.22511, 0.33439, -1.4512],
    [275.0, 0.23247, 0.33904, -1.7298],
    [300.0, 0.24010, 0.34308, -2.0637],
    [325.0, 0.24792, 0.34655, -2.4681],
    [350.0, 0.25591, 0.34951, -2.9641],
    [375.0, 0.26400, 0.35200, -3.5814],
    [400.0, 0.27218, 0.35407, -4.3633],
    [425.0, 0.28039, 0.35577, -5.3762],
    [450.0, 0.28863, 0.35714, -6.7262],
    [475.0, 0.29685, 0.35823, -8.5955],
    [500.0, 0.30505, 0.35907, -11.324],
    [525.0, 0.31320, 0.35968, -15.628],
    [550.0, 0.32129, 0.36011, -23.325],
    [575.0, 0.32931, 0.36038, -40.770],
    [600.0, 0.33724, 0.36051, -116.45],
];

/// Correlated colour temperature of an XYZ white by Robertson's method.
pub(super) fn xyz_to_cct(xyz: [f32; 3]) -> Option<f32> {
    let [x, y, z] = xyz.map(f64::from);
    let denominator = x + 15.0 * y + 3.0 * z;
    if !denominator.is_finite() || denominator.abs() < 1e-10 {
        return None;
    }
    let u = 4.0 * x / denominator;
    let v = 6.0 * y / denominator;

    let mut previous_distance = 0.0;
    for index in 1..ROBERTSON_ISOTHERMS.len() {
        let [mired, line_u, line_v, slope] = ROBERTSON_ISOTHERMS[index];
        let length = (1.0 + slope * slope).sqrt();
        // Signed distance from this isotherm; it changes sign between the
        // two isotherms that bracket the white.
        let distance = (-(u - line_u) * slope + (v - line_v)) / length;
        if distance <= 0.0 || index == ROBERTSON_ISOTHERMS.len() - 1 {
            let distance = -distance.min(0.0);
            let fraction = if index == 1 {
                0.0
            } else {
                distance / (previous_distance + distance)
            };
            let previous_mired = ROBERTSON_ISOTHERMS[index - 1][0];
            let interpolated = previous_mired * fraction + mired * (1.0 - fraction);
            let cct = (1.0e6 / interpolated) as f32;
            return (cct.is_finite() && cct > 0.0).then_some(cct);
        }
        previous_distance = distance;
    }
    None
}
