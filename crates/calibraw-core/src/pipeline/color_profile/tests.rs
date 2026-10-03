use super::{CameraProfile, HsvMap, ProfileEncoding, SrgbOutputTransform, ToneCurve};
use crate::color_math::{linear_srgb_to_oklab, linear_srgb_to_rec2020, srgb_encode};

fn assert_rgb_close(actual: [f32; 3], expected: [f32; 3], tolerance: f32) {
    for channel in 0..3 {
        assert!(
            (actual[channel] - expected[channel]).abs() <= tolerance,
            "channel {channel}: actual={actual:?}, expected={expected:?}, tolerance={tolerance}"
        );
    }
}

#[test]
fn srgb_output_is_exact_for_in_gamut_colors() {
    let transform = SrgbOutputTransform::new();
    let mut state = 0x6a09_e667_f3bc_c909_u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state as u32) as f32 / u32::MAX as f32
    };
    for _ in 0..4096 {
        // Stay inside the soft-compression knee so the expected value is exact.
        let srgb = [
            0.05 + 0.9 * next(),
            0.05 + 0.9 * next(),
            0.05 + 0.9 * next(),
        ];
        let lab = linear_srgb_to_oklab(srgb);
        if lab[1].hypot(lab[2]) > 0.08 {
            continue;
        }
        assert_rgb_close(
            transform.transform_rgb(linear_srgb_to_rec2020(srgb)),
            srgb.map(srgb_encode),
            2e-5,
        );
    }
}

#[test]
fn srgb_output_keeps_bright_yellow_saturated() {
    // A 33-point output LUT once rendered this boundary yellow as roughly
    // (251, 252, 120). Only the soft gamut knee may move it now, by about one
    // just-noticeable difference.
    let srgb = [0.991_247_97, 0.994_676_9, 0.003_443_38];
    let encoded = SrgbOutputTransform::new().transform_rgb(linear_srgb_to_rec2020(srgb));
    let expected = linear_srgb_to_oklab(srgb);
    let actual = linear_srgb_to_oklab(encoded.map(crate::color_math::srgb_decode));
    let distance = ((actual[0] - expected[0]).powi(2)
        + (actual[1] - expected[1]).powi(2)
        + (actual[2] - expected[2]).powi(2))
    .sqrt();
    assert!(distance < 0.012, "yellow moved by {distance}: {encoded:?}");
}

#[test]
fn srgb_output_maps_out_of_gamut_colors_into_the_unit_cube_preserving_hue() {
    let transform = SrgbOutputTransform::new();
    for input in [
        [1.4, -0.2, 0.3],
        [-0.5, 0.8, 1.7],
        [2.0, 2.0, 2.0],
        [0.0, 0.9, 0.0],
    ] {
        let encoded = transform.transform_rgb(input);
        assert!(encoded
            .iter()
            .all(|value| value.is_finite() && (0.0..=1.0).contains(value)));
        let source = linear_srgb_to_oklab(crate::color_math::rec2020_to_linear_srgb(input));
        let mapped = linear_srgb_to_oklab(encoded.map(crate::color_math::srgb_decode));
        let source_chroma = source[1].hypot(source[2]);
        let mapped_chroma = mapped[1].hypot(mapped[2]);
        if source_chroma > 0.02 && mapped_chroma > 0.02 {
            let cosine =
                (source[1] * mapped[1] + source[2] * mapped[2]) / (source_chroma * mapped_chroma);
            assert!(cosine > 0.999, "hue drifted for {input:?}: {cosine}");
        }
    }
}

#[test]
fn srgb_output_preserves_neutral_axis_and_near_black_transfer_curve() {
    let transform = SrgbOutputTransform::new();
    for linear in [0.0_f32, 0.000_1, 0.001, 0.003_130_8, 0.01, 0.18, 0.5, 1.0] {
        let expected = srgb_encode(linear);
        for channel in transform.transform_rgb([linear; 3]) {
            assert!(
                (channel - expected).abs() < 2e-5,
                "linear {linear}: got {channel}, expected {expected}"
            );
        }
    }
}

#[test]
fn dual_illuminant_maps_interpolate_entrywise() {
    let a = HsvMap::new(
        [1, 2, 1],
        vec![[0.0, 1.0, 1.0], [10.0, 0.5, 1.0]],
        ProfileEncoding::Linear,
    )
    .unwrap();
    let b = HsvMap::new(
        [1, 2, 1],
        vec![[20.0, 2.0, 1.0], [30.0, 1.5, 2.0]],
        ProfileEncoding::Linear,
    )
    .unwrap();
    let mixed = HsvMap::interpolate(&a, &b, 0.25).unwrap();
    assert_eq!(mixed.entries[0], [5.0, 1.25, 1.0]);
}

#[test]
fn tone_curve_sampling_is_monotonic_for_monotonic_points() {
    let curve = ToneCurve::new(vec![[0.0, 0.0], [0.2, 0.08], [0.7, 0.82], [1.0, 1.0]]).unwrap();
    let lut = curve.sampled_lut(256);
    assert!(lut.windows(2).all(|pair| pair[1] >= pair[0] - 1e-6));
}

#[test]
fn tone_curve_accepts_partial_domain_and_extends_end_values() {
    let curve = ToneCurve::new(vec![[0.2, 0.1], [0.8, 0.9]]).unwrap();
    let lut = curve.sampled_lut(11);
    assert!((lut[0] - 0.1).abs() < f32::EPSILON);
    assert!((lut[10] - 0.9).abs() < f32::EPSILON);
}

#[test]
fn gpu_layout_offsets_are_contiguous() {
    let profile = CameraProfile {
        hue_sat_maps: [
            Some(
                HsvMap::new([1, 2, 1], vec![[0.0, 1.0, 1.0]; 2], ProfileEncoding::Linear).unwrap(),
            ),
            Some(
                HsvMap::new([1, 2, 1], vec![[0.0, 1.0, 1.0]; 2], ProfileEncoding::Linear).unwrap(),
            ),
        ],
        tone_curve: Some(ToneCurve::new(vec![[0.0, 0.0], [1.0, 1.0]]).unwrap()),
        ..Default::default()
    };
    let data = profile.gpu_data();
    assert_eq!(data.layout.hue_sat[3], 1);
    assert_eq!(data.layout.hue_sat_2[3], 3);
    assert_eq!(data.words[0].map(f32::to_bits), data.layout.hue_sat_2);
    assert_eq!(data.layout.tone[1], 5);
    assert_eq!(data.words.len(), 5 + super::PROFILE_TONE_LUT_SIZE);
    data.validate().unwrap();
}
