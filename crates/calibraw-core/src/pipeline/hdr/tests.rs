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
        .add(
            &[1.0; 9],
            0.25,
            Alignment {
                x: 1.0,
                ..Alignment::default()
            },
            None,
        )
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

fn scene(x: f32, y: f32) -> [f32; 3] {
    let signal = 0.28
        + 0.10 * (x * 0.17 + y * 0.05).sin()
        + 0.06 * (x * 0.071 - y * 0.11).cos()
        + 0.17 * (-((x - 80.0).powi(2) + (y - 100.0).powi(2)) / 400.0).exp();
    [signal * 0.8, signal, signal * 0.65]
}

#[test]
fn aligns_subpixel_translation_rotation_and_exposure_change() {
    let (width, height) = (256u32, 192u32);
    let expected = Alignment {
        x: 4.25,
        y: -3.5,
        rotation: 0.014,
    };
    let (sin, cos) = expected.rotation.sin_cos();
    let (cx, cy) = ((width - 1) as f32 * 0.5, (height - 1) as f32 * 0.5);
    let mut reference = Vec::new();
    let mut source = Vec::new();
    for y in 0..height {
        for x in 0..width {
            reference.extend(scene(x as f32, y as f32));
            let (sx, sy) = (x as f32 - cx - expected.x, y as f32 - cy - expected.y);
            source.extend(
                scene(cos * sx + sin * sy + cx, -sin * sx + cos * sy + cy)
                    .map(|v| (v * 2.0).min(1.0)),
            );
        }
    }
    let reference = AlignmentReference::new(width, height, &reference, 1.0).unwrap();
    let actual = reference.align(&source, 2.0).unwrap();
    assert!((actual.x - expected.x).abs() < 0.25, "{actual:?}");
    assert!((actual.y - expected.y).abs() < 0.25, "{actual:?}");
    assert!(
        (actual.rotation - expected.rotation).abs() < 0.002,
        "{actual:?}"
    );
}

#[test]
fn rejects_brackets_with_no_usable_alignment_samples() {
    let reference = AlignmentReference::new(64, 64, &vec![0.5; 64 * 64 * 3], 1.0).unwrap();
    assert!(reference.align(&vec![1.0; 64 * 64 * 3], 2.0).is_err());
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
