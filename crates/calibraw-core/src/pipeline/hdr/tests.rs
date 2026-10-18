use super::*;

fn registration() -> Registration {
    Registration {
        alignment: Alignment::default(),
        gain: 1.0,
    }
}

fn frame<'a>(rgb: &'a [f32], peak: &'a [f32], exposure: f32) -> HdrFrame<'a> {
    HdrFrame {
        rgb,
        peak,
        exposure,
        light: exposure,
        registration: registration(),
    }
}

/// Captures linear `scene` radiance at `exposure`: clipped at white, with each pixel's
/// peak its brightest channel.
fn capture(scene: &[f32], exposure: f32) -> (Vec<f32>, Vec<f32>) {
    let rgb: Vec<f32> = scene.iter().map(|v| (v * exposure).min(1.0)).collect();
    let peak = rgb
        .chunks_exact(3)
        .map(|p| p[0].max(p[1]).max(p[2]))
        .collect();
    (rgb, peak)
}

fn merge(frames: &[(&[f32], f32)]) -> Vec<f32> {
    let pixels = frames[0].0.len() / 3;
    let mut merge = HdrAccumulator::new(pixels as u32, 1).unwrap();
    for &(scene, exposure) in frames {
        let (rgb, peak) = capture(scene, exposure);
        merge.add(frame(&rgb, &peak, exposure)).unwrap();
    }
    merge.finish().unwrap()
}

fn assert_relative(actual: &[f32], expected: &[f32], tolerance: f32) {
    for (i, (a, e)) in actual.iter().zip(expected).enumerate() {
        assert!(
            (a - e).abs() <= tolerance * e.abs().max(1e-6),
            "sample {i}: {a} != {e}"
        );
    }
}

/// A 64 × 64 frame of gray `radiance(x, y)` in reference units.
fn scene(radiance: impl Fn(usize, usize) -> f32) -> Vec<f32> {
    (0..64 * 64)
        .flat_map(|i| [radiance(i % 64, i / 64); 3])
        .collect()
}

fn merge_square(frames: &[(&[f32], f32, f32)]) -> Vec<f32> {
    let mut merge = HdrAccumulator::new(64, 64).unwrap();
    for &(scene, metadata, actual) in frames {
        let (rgb, peak) = capture(scene, actual);
        merge.add(frame(&rgb, &peak, metadata)).unwrap();
    }
    merge.finish().unwrap()
}

#[test]
fn recovers_colored_highlights_without_clipping_radiance() {
    let scene = [4.0f32, 2.0, 0.5, 0.0001, 0.0002, 0.0003, 0.2, 0.1, 0.05];
    let merged = merge(&[(&scene, 1.0), (&scene, 1.0 / 16.0), (&scene, 16.0)]);
    assert_relative(&merged, &scene, 1e-5);
}

#[test]
fn favors_long_exposure_for_quantized_shadows() {
    let scene = [0.0001f32, 0.0002, 0.0003];
    let mut merge = HdrAccumulator::new(1, 1).unwrap();
    for exposure in [1.0, 16.0] {
        let rgb: Vec<f32> = scene
            .iter()
            .map(|v| (v * exposure * 4095.0).round() / 4095.0)
            .collect();
        let peak = [rgb[2]];
        merge.add(frame(&rgb, &peak, exposure)).unwrap();
    }
    for (actual, expected) in merge.finish().unwrap().iter().zip(scene) {
        assert!((actual - expected).abs() < 1e-5, "{actual} != {expected}");
    }
}

#[test]
fn keeps_reference_borders_and_shortest_fully_clipped_exposure() {
    let mut merge = HdrAccumulator::new(3, 1).unwrap();
    merge.add(frame(&[1.0; 9], &[1.0; 3], 1.0)).unwrap();
    merge
        .add(HdrFrame {
            registration: Registration {
                alignment: Alignment::translation(1.0, 0.0),
                gain: 1.0,
            },
            ..frame(&[1.0; 9], &[1.0; 3], 0.25)
        })
        .unwrap();
    let rgb = merge.finish().unwrap();
    assert_relative(&rgb[..6], &[4.0; 6], 1e-4);
    assert_eq!(&rgb[6..], &[1.0; 3]);
}

#[test]
fn rejects_invalid_inputs_and_uncovered_output() {
    assert!(HdrAccumulator::new(0, 1).is_err());
    let mut merge = HdrAccumulator::new(2, 1).unwrap();
    assert!(merge.add(frame(&[0.5; 3], &[0.5; 2], 1.0)).is_err());
    assert!(merge.add(frame(&[0.5; 6], &[0.5; 1], 1.0)).is_err());
    assert!(merge.add(frame(&[f32::NAN; 6], &[0.5; 2], 1.0)).is_err());
    assert!(merge.add(frame(&[0.5; 6], &[0.5; 2], 0.0)).is_err());
    let warped_reference = HdrFrame {
        registration: Registration {
            alignment: Alignment::translation(1.0, 0.0),
            gain: 1.0,
        },
        ..frame(&[0.5; 6], &[0.5; 2], 1.0)
    };
    assert!(merge.add(warped_reference).is_err());
    assert!(merge.finish().is_err());
    assert!(exposure_scale(0.0, 100.0, 8.0).is_err());
    assert!(exposure_scale(1.0, f32::NAN, 8.0).is_err());
    assert_eq!(exposure_scale(0.01, 100.0, 2.0).unwrap(), 0.25);
    assert_eq!(collected_light(0.01, 2.0).unwrap(), 0.0025);
}

#[test]
fn excludes_saturated_sensor_footprints_even_when_demosaicing_softens_them() {
    let mut merge = HdrAccumulator::new(1, 1).unwrap();
    merge.add(frame(&[0.25, 0.1, 0.05], &[0.25], 0.25)).unwrap();
    merge.add(frame(&[0.9, 0.4, 0.2], &[1.0], 1.0)).unwrap();
    assert_relative(&merge.finish().unwrap(), &[1.0, 0.4, 0.2], 1e-6);
}

#[test]
fn measures_the_exposure_ratio_instead_of_trusting_metadata() {
    // Six stops of radiance the reference measures; the long frame is 6 % brighter than
    // its metadata says.
    let scene = scene(|x, _| 0.01 * 2f32.powf(x as f32 / 10.0));
    let merged = merge_square(&[(&scene, 1.0, 1.0), (&scene, 4.0, 4.0 * 1.06)]);
    assert_relative(&merged, &scene, 2e-3);
}

#[test]
fn rejects_motion_the_reference_did_not_see() {
    let background = scene(|x, _| 0.02 + 0.18 * x as f32 / 63.0);
    // A dark object crosses the long frame only.
    let moved = scene(|x, y| {
        if (24..40).contains(&x) && (24..40).contains(&y) {
            0.01
        } else {
            0.02 + 0.18 * x as f32 / 63.0
        }
    });
    let merged = merge_square(&[(&background, 1.0, 1.0), (&moved, 4.0, 4.0)]);
    for (x, y) in [(32, 32), (26, 26), (38, 38), (4, 32), (60, 10)] {
        let i = (y * 64 + x) * 3;
        assert_relative(&merged[i..i + 3], &background[i..i + 3], 1e-4);
    }
}

#[test]
fn keeps_a_clipped_reference_highlight_a_shorter_frame_shows_elsewhere() {
    // A flame clipped in the reference has moved in the short frame.
    let flame = |x0: usize| {
        scene(move |x, y| {
            if (x0..x0 + 16).contains(&x) && (24..40).contains(&y) {
                4.0
            } else {
                0.1
            }
        })
    };
    let (reference, moved) = (flame(16), flame(40));
    let merged = merge_square(&[(&reference, 1.0, 1.0), (&moved, 1.0 / 16.0, 1.0 / 16.0)]);
    let at = |x: usize, y: usize| merged[(y * 64 + x) * 3 + 1];
    // Where the reference clipped, no darker ghost of the background shows through.
    assert!(at(24, 32) >= 1.0, "clipped reference: {}", at(24, 32));
    // Where only the short frame saw the flame, the reference's background stays.
    assert!(
        (at(48, 32) - 0.1).abs() < 1e-4,
        "moved flame: {}",
        at(48, 32)
    );
    assert!((at(4, 4) - 0.1).abs() < 1e-4, "background: {}", at(4, 4));
}
