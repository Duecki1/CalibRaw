use super::*;

#[test]
fn recovers_colored_highlights_without_clipping_radiance() {
    let scene = [4.0f32, 2.0, 0.5, 0.0001, 0.0002, 0.0003, 0.2, 0.1, 0.05];
    let mut merge = HdrAccumulator::new(3, 1).unwrap();
    for exposure in [1.0, 1.0 / 16.0, 16.0] {
        let frame: Vec<_> = scene.iter().map(|v| (v * exposure).min(1.0)).collect();
        merge
            .add(&frame, exposure, Alignment::default(), None)
            .unwrap();
    }
    for (actual, expected) in merge.finish().unwrap().iter().zip(scene) {
        assert!((actual - expected).abs() < 1e-6, "{actual} != {expected}");
    }
}

#[test]
fn favors_long_exposure_for_quantized_shadows() {
    let scene = [0.0001f32, 0.0002, 0.0003];
    let mut merge = HdrAccumulator::new(1, 1).unwrap();
    for exposure in [1.0, 16.0] {
        let frame: Vec<_> = scene
            .iter()
            .map(|v| (v * exposure * 4095.0).round() / 4095.0)
            .collect();
        merge
            .add(&frame, exposure, Alignment::default(), None)
            .unwrap();
    }
    for (actual, expected) in merge.finish().unwrap().iter().zip(scene) {
        assert!((actual - expected).abs() < 1e-5, "{actual} != {expected}");
    }
}

#[test]
fn keeps_reference_borders_and_shortest_fully_clipped_exposure() {
    let mut merge = HdrAccumulator::new(3, 1).unwrap();
    merge
        .add(&[1.0; 9], 1.0, Alignment::default(), None)
        .unwrap();
    merge
        .add(&[1.0; 9], 0.25, Alignment::translation(1.0, 0.0), None)
        .unwrap();
    let rgb = merge.finish().unwrap();
    assert_eq!(&rgb[..6], &[4.0; 6]);
    assert_eq!(&rgb[6..], &[1.0; 3]);
}

#[test]
fn rejects_invalid_inputs_and_uncovered_output() {
    assert!(HdrAccumulator::new(0, 1).is_err());
    let mut merge = HdrAccumulator::new(2, 1).unwrap();
    assert!(merge
        .add(&[0.5; 3], 1.0, Alignment::default(), None)
        .is_err());
    assert!(merge
        .add(&[f32::NAN; 6], 1.0, Alignment::default(), None)
        .is_err());
    assert!(merge
        .add(&[0.5; 6], 0.0, Alignment::default(), None)
        .is_err());
    assert!(merge.finish().is_err());
    assert!(exposure_scale(0.0, 100.0, 8.0).is_err());
    assert!(exposure_scale(1.0, f32::NAN, 8.0).is_err());
    assert_eq!(exposure_scale(0.01, 100.0, 2.0).unwrap(), 0.25);
}

#[test]
fn excludes_saturated_sensor_footprints_even_when_demosaicing_softens_them() {
    let mut merge = HdrAccumulator::new(1, 1).unwrap();
    merge
        .add(&[0.25, 0.1, 0.05], 0.25, Alignment::default(), None)
        .unwrap();
    merge
        .add(&[0.9, 0.4, 0.2], 1.0, Alignment::default(), Some(&[1]))
        .unwrap();
    assert_eq!(merge.finish().unwrap(), [1.0, 0.4, 0.2]);
}

#[test]
fn marks_only_highlights_without_any_unclipped_exposure() {
    let mut merge = HdrAccumulator::new(4, 1).unwrap();
    merge
        .add(
            &[1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.2, 0.1, 0.05],
            1.0,
            Alignment::default(),
            None,
        )
        .unwrap();
    merge
        .add(
            &[
                1.0, 0.7, 0.9, 0.5, 0.25, 0.125, 0.0, 0.0, 0.0, 0.05, 0.025, 0.0125,
            ],
            0.25,
            Alignment::default(),
            None,
        )
        .unwrap();
    let (rgb, clipped) = merge.finish_with_clipped_highlights().unwrap();
    assert_eq!(clipped, [true, false, false, false]);
    assert_eq!(&rgb[..3], &[4.0, 2.8, 3.6]);
    assert_eq!(&rgb[3..6], &[2.0, 1.0, 0.5]);
    assert_eq!(&rgb[6..9], &[0.0; 3]);
}
