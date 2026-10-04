use crate::matrix::{multiply, transform, Matrix3};

/// CIE D65 white (x = 0.3127, y = 0.3290) normalized to Y = 1.
pub const D65_XYZ: [f32; 3] = [0.950_455_9, 1.0, 1.089_057_8];

/// D50 white (x = 0.3457, y = 0.3585) used by the DNG SDK as its profile
/// connection space; DNG forward matrices map camera neutral to this value.
pub const DNG_PCS_D50_XYZ: [f32; 3] = [0.964_295_7, 1.0, 0.825_104_6];

/// Linear Bradford chromatic adaptation that maps `source` white to `target`
/// white, preserving luminance. Returns `None` for degenerate whites.
pub fn bradford_adaptation(source: [f32; 3], target: [f32; 3]) -> Option<Matrix3> {
    const BRADFORD: Matrix3 = [
        [0.8951, 0.2664, -0.1614],
        [-0.7502, 1.7135, 0.0367],
        [0.0389, -0.0685, 1.0296],
    ];
    const BRADFORD_INV: Matrix3 = [
        [0.986_992_9, -0.147_054_3, 0.159_962_7],
        [0.432_305_3, 0.518_360_3, 0.049_291_2],
        [-0.008_528_7, 0.040_042_8, 0.968_486_7],
    ];
    if !source.iter().chain(&target).all(|v| v.is_finite())
        || source[1].abs() < 1e-10
        || target[1].abs() < 1e-10
    {
        return None;
    }
    let source_lms = transform(BRADFORD, source.map(|v| v / source[1]));
    let target_lms = transform(BRADFORD, target.map(|v| v / target[1]));
    if source_lms.iter().any(|v| !v.is_finite() || v.abs() < 1e-10) {
        return None;
    }
    let diagonal = [
        [target_lms[0] / source_lms[0], 0.0, 0.0],
        [0.0, target_lms[1] / source_lms[1], 0.0],
        [0.0, 0.0, target_lms[2] / source_lms[2]],
    ];
    Some(multiply(BRADFORD_INV, multiply(diagonal, BRADFORD)))
}

/// IEC 61966-2-1 sRGB encoding of a linear value, clamped to `[0, 1]`.
pub fn srgb_encode(linear: f32) -> f32 {
    srgb_encode_signed(linear.clamp(0.0, 1.0))
}

/// IEC 61966-2-1 sRGB decoding of an encoded value, clamped to `[0, 1]`.
pub fn srgb_decode(encoded: f32) -> f32 {
    srgb_decode_signed(encoded.clamp(0.0, 1.0))
}

/// sRGB encoding mirrored around zero and left unclamped, for intermediate
/// values that may fall outside the display range.
pub fn srgb_encode_signed(linear: f32) -> f32 {
    let magnitude = linear.abs();
    let encoded = if magnitude <= 0.003_130_8 {
        magnitude * 12.92
    } else {
        1.055 * magnitude.powf(1.0 / 2.4) - 0.055
    };
    encoded.copysign(linear)
}

/// Inverse of [`srgb_encode_signed`].
pub fn srgb_decode_signed(encoded: f32) -> f32 {
    let magnitude = encoded.abs();
    let linear = if magnitude <= 0.040_45 {
        magnitude / 12.92
    } else {
        ((magnitude + 0.055) / 1.055).powf(2.4)
    };
    linear.copysign(encoded)
}

pub fn linear_srgb_to_oklab(rgb: [f32; 3]) -> [f32; 3] {
    let lms = transform(
        [
            [0.412_221_46, 0.536_332_55, 0.051_445_99],
            [0.211_903_5, 0.680_699_5, 0.107_396_96],
            [0.088_302_46, 0.281_718_85, 0.629_978_7],
        ],
        rgb,
    )
    .map(f32::cbrt);
    transform(
        [
            [0.210_454_26, 0.793_617_8, -0.004_072_05],
            [1.977_998_5, -2.428_592_2, 0.450_593_7],
            [0.025_904_04, 0.782_771_77, -0.808_675_77],
        ],
        lms,
    )
}

pub fn oklab_to_linear_srgb(lab: [f32; 3]) -> [f32; 3] {
    let root = transform(
        [
            [1.0, 0.396_337_78, 0.215_803_76],
            [1.0, -0.105_561_35, -0.063_854_17],
            [1.0, -0.089_484_18, -1.291_485_5],
        ],
        lab,
    );
    let lms = root.map(|value| value * value * value);
    transform(
        [
            [4.076_741_7, -3.307_711_6, 0.230_969_94],
            [-1.268_438, 2.609_757_4, -0.341_319_4],
            [-0.004_196_09, -0.703_418_6, 1.707_614_7],
        ],
        lms,
    )
}

/// Convert linear Rec.2020 primaries to linear sRGB without gamut mapping.
pub fn rec2020_to_linear_srgb(rgb: [f32; 3]) -> [f32; 3] {
    transform(
        [
            [1.660_491, -0.587_641_1, -0.072_849_9],
            [-0.124_550_5, 1.132_899_9, -0.008_349_4],
            [-0.018_150_8, -0.100_578_9, 1.118_729_7],
        ],
        rgb,
    )
}

/// Convert linear sRGB primaries to linear Rec.2020.
pub fn linear_srgb_to_rec2020(rgb: [f32; 3]) -> [f32; 3] {
    transform(LINEAR_SRGB_TO_REC2020, rgb)
}

pub const LINEAR_SRGB_TO_REC2020: Matrix3 = [
    [0.627_403_9, 0.329_283, 0.043_313_1],
    [0.069_097_3, 0.919_540_4, 0.011_362_3],
    [0.016_391_4, 0.088_013_3, 0.895_595_3],
];

/// Linear Display P3 (D65) primaries to linear Rec.2020, as tagged by phone
/// HEIF images that carry `nclx` colour primaries 12 instead of an ICC profile.
pub const LINEAR_DISPLAY_P3_TO_REC2020: Matrix3 = [
    [0.753_833, 0.198_597_3, 0.047_569_7],
    [0.045_743_8, 0.941_777_2, 0.012_478_9],
    [-0.001_210_3, 0.017_601_7, 0.983_608_6],
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_transfer_maps_black_white_and_mid_gray() {
        assert_eq!(srgb_encode(0.0), 0.0);
        assert!((srgb_encode(1.0) - 1.0).abs() < 1e-6);
        assert!((srgb_encode(0.18) - 0.461).abs() < 0.002);
        assert_eq!(srgb_encode(-0.5), 0.0);
        assert_eq!(srgb_encode_signed(-0.18), -srgb_encode_signed(0.18));
        for encoded in [0.0, 0.02, 0.5, 1.0] {
            assert!((srgb_encode(srgb_decode(encoded)) - encoded).abs() < 1e-6);
        }
    }

    #[test]
    fn rec2020_to_linear_srgb_maps_srgb_red() {
        let red_in_rec2020 = [0.627_403_9, 0.069_097_3, 0.016_391_4];
        let converted = rec2020_to_linear_srgb(red_in_rec2020);
        for (actual, expected) in converted.into_iter().zip([1.0, 0.0, 0.0]) {
            assert!((actual - expected).abs() < 1e-6);
        }
    }

    #[test]
    fn bradford_adaptation_maps_source_white_to_target_white() {
        let adaptation = bradford_adaptation(DNG_PCS_D50_XYZ, D65_XYZ).unwrap();
        let adapted = transform(adaptation, DNG_PCS_D50_XYZ);
        for (actual, expected) in adapted.into_iter().zip(D65_XYZ) {
            assert!((actual - expected).abs() < 1e-6);
        }
        assert!(bradford_adaptation([1.0, 0.0, 1.0], D65_XYZ).is_none());
    }
}
