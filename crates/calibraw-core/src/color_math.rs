use crate::matrix::{transform, Matrix3};

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

fn oklab_to_linear_srgb(lab: [f32; 3]) -> [f32; 3] {
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

pub fn rec2020_to_oklab(rgb: [f32; 3]) -> [f32; 3] {
    linear_srgb_to_oklab(rec2020_to_linear_srgb(rgb))
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

pub fn rec2020_from_oklab(lab: [f32; 3]) -> [f32; 3] {
    linear_srgb_to_rec2020(oklab_to_linear_srgb(lab))
}

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
    fn rec2020_oklab_round_trips() {
        let sample = [0.2, 0.4, 0.8];
        let reconstructed = rec2020_from_oklab(rec2020_to_oklab(sample));
        for (actual, expected) in reconstructed.into_iter().zip(sample) {
            assert!((actual - expected).abs() <= 2e-5);
        }
    }
}
