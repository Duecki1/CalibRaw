//! Correlated colour temperature estimation, illuminant interpolation and
//! the temperature/tint coordinates of whites.

use super::*;

pub(super) fn estimate_scene_cct(
    color: &LibRawColor,
    wb_coeffs: [f32; 4],
    cdesc: [u8; 4],
) -> Option<f32> {
    let mut best_cct = 0.0;
    let mut best_error = f32::INFINITY;

    for row in color.wbct_coeffs {
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

/// Correlated colour temperature of an XYZ white by Robertson's method, an
/// independent check of [`temperature_tint_uv`]'s locus in tests.
#[cfg(test)]
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

/// Tint per unit of CIE 1960 uv distance from the Planckian locus along the
/// isotherm, the scale of Lightroom and the DNG SDK. A white above the locus
/// (greener light) has a positive tint, and balancing it adds magenta.
const TINT_PER_UV: f64 = 3_000.0;

/// Unit direction of an isotherm with `slope` in CIE 1960 uv. It points
/// towards increasing u, which is below the locus (magenta) for every
/// tabulated slope.
fn isotherm_direction(slope: f64) -> [f64; 2] {
    let length = (1.0 + slope * slope).sqrt();
    [1.0 / length, slope / length]
}

/// CIE 1960 uv of the white at `kelvin` with `tint` (see [`TINT_PER_UV`]).
///
/// As the DNG SDK's `dng_temperature`, the locus point and the isotherm
/// direction are interpolated linearly in mired between Robertson's
/// isotherms, and the tint offsets the point along the interpolated isotherm.
/// Temperatures outside the table extend its first or last interval.
pub(super) fn temperature_tint_uv(kelvin: f64, tint: f64) -> [f64; 2] {
    let mired = 1.0e6 / kelvin;
    let last = ROBERTSON_ISOTHERMS.len() - 2;
    let index = (0..last)
        .find(|&index| mired < ROBERTSON_ISOTHERMS[index + 1][0])
        .unwrap_or(last);
    let [low_mired, low_u, low_v, low_slope] = ROBERTSON_ISOTHERMS[index];
    let [high_mired, high_u, high_v, high_slope] = ROBERTSON_ISOTHERMS[index + 1];
    let weight = (mired - low_mired) / (high_mired - low_mired);
    let low = isotherm_direction(low_slope);
    let high = isotherm_direction(high_slope);
    let direction = [
        low[0] + weight * (high[0] - low[0]),
        low[1] + weight * (high[1] - low[1]),
    ];
    let length = direction[0].hypot(direction[1]);
    let offset = -tint / TINT_PER_UV / length;
    [
        low_u + weight * (high_u - low_u) + direction[0] * offset,
        low_v + weight * (high_v - low_v) + direction[1] * offset,
    ]
}

/// Temperature (K) and tint of a CIE 1960 uv white, the exact inverse of
/// [`temperature_tint_uv`].
///
/// Within each interval of the table, the white lies on the isotherm through
/// the locus point at weight w when the offset from that point is parallel
/// to the interpolated direction. Both are linear in w, so their cross
/// product is a quadratic in w. Of the solutions inside an interval, the one
/// nearest the locus is the white's own isotherm; whites beyond either end
/// of the table use the extended first or last interval. Robertson's method
/// ([`xyz_to_cct`]) only approximates this inverse.
pub(super) fn uv_temperature_tint(uv: [f64; 2]) -> Option<(f64, f64)> {
    const WEIGHT_TOLERANCE: f64 = 1e-9;
    let cross = |a: [f64; 2], b: [f64; 2]| a[0] * b[1] - a[1] * b[0];
    let last = ROBERTSON_ISOTHERMS.len() - 2;
    let mut best: Option<(f64, f64)> = None;
    for index in 0..=last {
        let [low_mired, low_u, low_v, low_slope] = ROBERTSON_ISOTHERMS[index];
        let [high_mired, high_u, high_v, high_slope] = ROBERTSON_ISOTHERMS[index + 1];
        let low = isotherm_direction(low_slope);
        let high = isotherm_direction(high_slope);
        let offset = [uv[0] - low_u, uv[1] - low_v];
        let locus_step = [high_u - low_u, high_v - low_v];
        let direction_step = [high[0] - low[0], high[1] - low[1]];
        // (offset - w locus_step) x (low + w direction_step) = 0.
        let constant = cross(offset, low);
        let linear = cross(offset, direction_step) - cross(locus_step, low);
        let quadratic = -cross(locus_step, direction_step);
        let min_weight = if index == 0 {
            f64::NEG_INFINITY
        } else {
            -WEIGHT_TOLERANCE
        };
        let max_weight = if index == last {
            f64::INFINITY
        } else {
            1.0 + WEIGHT_TOLERANCE
        };
        for weight in quadratic_roots(quadratic, linear, constant)
            .into_iter()
            .flatten()
        {
            if !(min_weight..=max_weight).contains(&weight) {
                continue;
            }
            let direction = [
                low[0] + weight * direction_step[0],
                low[1] + weight * direction_step[1],
            ];
            let length = direction[0].hypot(direction[1]);
            let distance = ((offset[0] - weight * locus_step[0]) * direction[0]
                + (offset[1] - weight * locus_step[1]) * direction[1])
                / length;
            let mired = low_mired + weight * (high_mired - low_mired);
            let tint = -distance * TINT_PER_UV;
            if mired > 0.0 && best.is_none_or(|(_, best_tint)| tint.abs() < best_tint.abs()) {
                best = Some((1.0e6 / mired, tint));
            }
        }
    }
    best.filter(|(kelvin, tint)| kelvin.is_finite() && tint.is_finite())
}

/// Real roots of `a x^2 + b x + c`, computed without cancellation.
fn quadratic_roots(a: f64, b: f64, c: f64) -> [Option<f64>; 2] {
    let scale = a.abs().max(b.abs()).max(c.abs());
    if scale == 0.0 || !scale.is_finite() {
        return [None, None];
    }
    if a.abs() <= 1e-12 * scale {
        return [(b != 0.0).then(|| -c / b), None];
    }
    let discriminant = b * b - 4.0 * a * c;
    if discriminant < 0.0 {
        return [None, None];
    }
    let q = -0.5 * (b + b.signum() * discriminant.sqrt());
    [Some(q / a), (q != 0.0).then(|| c / q)]
}

/// CIE 1960 uv of an XYZ colour.
pub(super) fn xyz_to_uv(xyz: [f32; 3]) -> Option<[f64; 2]> {
    let [x, y, z] = xyz.map(f64::from);
    let denominator = x + 15.0 * y + 3.0 * z;
    (denominator.is_finite() && denominator.abs() > 1e-10)
        .then(|| [4.0 * x / denominator, 6.0 * y / denominator])
}

/// XYZ with Y = 1 of a CIE 1960 uv chromaticity.
pub(super) fn uv_to_xyz(uv: [f64; 2]) -> Option<[f32; 3]> {
    let [u, v] = uv;
    let xyz =
        [3.0 * u / (2.0 * v), 1.0, (4.0 - u - 10.0 * v) / (2.0 * v)].map(|value| value as f32);
    (v > 1e-10 && xyz.iter().all(|value| value.is_finite())).then_some(xyz)
}
