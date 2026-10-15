use super::*;

const WIDTH: u32 = 256;
const HEIGHT: u32 = 192;

/// Smooth structure plus fine texture at several orientations and scales.
fn scene(x: f64, y: f64) -> f32 {
    (0.28
        + 0.10 * (x * 0.17 + y * 0.05).sin()
        + 0.06 * (x * 0.071 - y * 0.11).cos()
        + 0.04 * (x * 0.53 + y * 0.31).sin() * (y * 0.47 - x * 0.12).cos()
        + 0.17 * (-((x - 80.0).powi(2) + (y - 100.0).powi(2)) / 400.0).exp()) as f32
}

fn inverse(m: &Matrix) -> Matrix {
    let cofactor = |r: usize, c: usize| {
        let (r0, r1) = ((r + 1) % 3, (r + 2) % 3);
        let (c0, c1) = ((c + 1) % 3, (c + 2) % 3);
        m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0]
    };
    let det: f64 = (0..3).map(|c| m[0][c] * cofactor(0, c)).sum();
    std::array::from_fn(|r| std::array::from_fn(|c| cofactor(c, r) / det))
}

/// The scene as seen by a source frame that `alignment` (reference pixel → source pixel)
/// registers, scaled by `exposure`, optionally defocused over a disc of radius `blur`,
/// and clipped at white like a sensor.
fn frame(alignment: Alignment, exposure: f32, blur: f64) -> Vec<f32> {
    let to_reference = inverse(&alignment.matrix);
    let mut rgb = Vec::with_capacity(WIDTH as usize * HEIGHT as usize * 3);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let m = &to_reference;
            let (x, y) = (f64::from(x), f64::from(y));
            let w = m[2][0] * x + m[2][1] * y + m[2][2];
            let px = (m[0][0] * x + m[0][1] * y + m[0][2]) / w;
            let py = (m[1][0] * x + m[1][1] * y + m[1][2]) / w;
            let mut sum = 0.0;
            let mut count = 0.0;
            for dy in -3..=3 {
                for dx in -3..=3 {
                    let (ox, oy) = (f64::from(dx) * blur / 3.0, f64::from(dy) * blur / 3.0);
                    if ox.hypot(oy) <= blur || (dx == 0 && dy == 0) {
                        sum += scene(px + ox, py + oy);
                        count += 1.0;
                    }
                }
            }
            let signal = sum / count;
            rgb.extend([signal * 0.8, signal, signal * 0.65].map(|v| (v * exposure).min(1.0)));
        }
    }
    rgb
}

fn reference() -> AlignmentReference {
    AlignmentReference::new(WIDTH, HEIGHT, &frame(Alignment::default(), 1.0, 0.0), 1.0).unwrap()
}

/// Both alignments map the corners and the center within `pixels` of each other.
fn assert_close(actual: Alignment, expected: Alignment, pixels: f32) {
    let (w, h) = ((WIDTH - 1) as f32, (HEIGHT - 1) as f32);
    for (x, y) in [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h), (w / 2.0, h / 2.0)] {
        let (ax, ay) = actual.map(x, y);
        let (ex, ey) = expected.map(x, y);
        assert!(
            (ax - ex).hypot(ay - ey) < pixels,
            "({x}, {y}) maps to ({ax}, {ay}), expected ({ex}, {ey})"
        );
    }
}

#[test]
fn aligns_subpixel_translation_rotation_and_exposure_change() {
    let expected = Alignment::similarity(WIDTH, HEIGHT, 4.25, -3.5, 0.014, 1.0);
    let actual = reference().align(&frame(expected, 2.0, 0.0), 2.0).unwrap();
    assert_close(actual.alignment, expected, 0.1);
    assert!((actual.gain - 1.0).abs() < 0.01, "{actual:?}");
}

#[test]
fn aligns_small_and_large_shifts() {
    let reference = reference();
    for (x, y) in [
        (0.3, -0.2),
        (1.0, 0.0),
        (-2.6, 1.7),
        (17.0, -11.0),
        (-30.0, 24.0),
    ] {
        let expected = Alignment::translation(x, y);
        let actual = reference.align(&frame(expected, 1.0, 0.0), 1.0).unwrap();
        assert_close(actual.alignment, expected, 0.1);
    }
}

#[test]
fn aligns_a_turned_wide_angle_camera() {
    // A 100° wide view turned 3.5° sideways and tilted 1°: the near edge moves twice as
    // far as the center, which no similarity transform can follow.
    let f = f64::from(WIDTH) * 0.5 / 50f64.to_radians().tan();
    let (cx, cy) = (f64::from(WIDTH - 1) * 0.5, f64::from(HEIGHT - 1) * 0.5);
    let camera = [[f, 0.0, cx], [0.0, f, cy], [0.0, 0.0, 1.0]];
    let (yaw, pitch) = (3.5f64.to_radians(), 1f64.to_radians());
    let turn = multiply(
        &[
            [yaw.cos(), 0.0, yaw.sin()],
            [0.0, 1.0, 0.0],
            [-yaw.sin(), 0.0, yaw.cos()],
        ],
        &[
            [1.0, 0.0, 0.0],
            [0.0, pitch.cos(), -pitch.sin()],
            [0.0, pitch.sin(), pitch.cos()],
        ],
    );
    let expected = Alignment {
        matrix: multiply(&multiply(&camera, &turn), &inverse(&camera)),
    };
    let actual = reference().align(&frame(expected, 1.0, 0.0), 1.0).unwrap();
    assert_close(actual.alignment, expected, 0.15);
}

#[test]
fn aligns_focus_breathing_across_defocus_and_brightness_change() {
    let expected = Alignment::similarity(WIDTH, HEIGHT, -1.5, 2.25, -0.004, 1.035);
    // Same nominal exposure, but 10% brighter (flicker) and visibly out of focus.
    let actual = reference().align(&frame(expected, 1.1, 2.0), 1.0).unwrap();
    assert_close(actual.alignment, expected, 0.15);
    assert!((actual.gain - 1.1).abs() < 0.02, "{actual:?}");
}

#[test]
fn aligns_exposures_whose_shadows_or_highlights_are_masked() {
    let reference = reference();
    let expected = Alignment::similarity(WIDTH, HEIGHT, 1.5, -0.75, 0.002, 1.0);
    // At −5 EV most of the frame is below the noise floor, at +2 EV most of it clips.
    for exposure in [1.0 / 32.0, 4.0] {
        let actual = reference
            .align(&frame(expected, exposure, 0.0), exposure)
            .unwrap();
        assert_close(actual.alignment, expected, 0.15);
    }
}

#[test]
fn rejects_frames_with_no_usable_alignment_samples() {
    let reference = AlignmentReference::new(64, 64, &vec![0.5; 64 * 64 * 3], 1.0).unwrap();
    assert!(reference.align(&vec![1.0; 64 * 64 * 3], 2.0).is_err());
}

#[test]
fn rejects_unrelated_frames() {
    let mut unrelated = Vec::new();
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let v = 0.3 + 0.2 * ((x * 7 + y * 13) as f32 * 0.37).sin() * (y as f32 * 0.21).cos();
            unrelated.extend([v; 3]);
        }
    }
    assert!(reference().align(&unrelated, 1.0).is_err());
}
