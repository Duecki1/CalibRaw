use super::*;
use crate::pipeline::alignment::Alignment;

const WIDTH: usize = 96;
const HEIGHT: usize = 64;

fn identity() -> Registration {
    Registration {
        alignment: Alignment::default(),
        gain: 1.0,
    }
}

/// Fine texture over a smooth gradient, in linear camera RGB.
fn sharp() -> Vec<f32> {
    let mut rgb = Vec::with_capacity(WIDTH * HEIGHT * 3);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let (x, y) = (x as f32, y as f32);
            let v = 0.2 + 0.002 * x + 0.12 * (x * 1.3).sin() * (y * 0.9).cos();
            rgb.extend([v * 0.8, v, v * 0.6]);
        }
    }
    rgb
}

/// `rgb` box-blurred over 5×5 where `blurred(x)` holds.
fn defocus(rgb: &[f32], blurred: impl Fn(usize) -> bool) -> Vec<f32> {
    let mut out = rgb.to_vec();
    for y in 0..HEIGHT {
        for x in (0..WIDTH).filter(|&x| blurred(x)) {
            for c in 0..3 {
                let mut sum = 0.0;
                for sy in y.saturating_sub(2)..=(y + 2).min(HEIGHT - 1) {
                    for sx in x.saturating_sub(2)..=(x + 2).min(WIDTH - 1) {
                        sum += rgb[(sy * WIDTH + sx) * 3 + c];
                    }
                }
                let count = (y.min(2) + (HEIGHT - 1 - y).min(2) + 1)
                    * (x.min(2) + (WIDTH - 1 - x).min(2) + 1);
                out[(y * WIDTH + x) * 3 + c] = sum / count as f32;
            }
        }
    }
    out
}

fn stack(frames: &[(&[f32], Registration)]) -> Vec<f32> {
    let mut stack = FocusStack::new(WIDTH as u32, HEIGHT as u32).unwrap();
    for (rgb, registration) in frames {
        stack.add(rgb, *registration).unwrap();
    }
    stack.finish().unwrap()
}

/// Mean absolute error over columns `columns`, rows away from the top and bottom edge.
fn error(a: &[f32], b: &[f32], columns: std::ops::Range<usize>) -> f32 {
    let mut sum = 0.0;
    let mut count = 0;
    for y in 4..HEIGHT - 4 {
        for x in columns.clone() {
            for c in 0..3 {
                let i = (y * WIDTH + x) * 3 + c;
                sum += (a[i] - b[i]).abs();
                count += 1;
            }
        }
    }
    sum / count as f32
}

#[test]
fn identical_frames_reproduce_the_input() {
    let scene = sharp();
    let fused = stack(&[
        (&scene, identity()),
        (&scene, identity()),
        (&scene, identity()),
    ]);
    for (actual, expected) in fused.iter().zip(&scene) {
        assert!((actual - expected).abs() < 1e-5, "{actual} != {expected}");
    }
}

#[test]
fn keeps_the_sharp_part_of_every_frame() {
    let scene = sharp();
    let near = defocus(&scene, |x| x >= WIDTH / 2);
    let far = defocus(&scene, |x| x < WIDTH / 2);
    let fused = stack(&[(&near, identity()), (&far, identity())]);
    let (left, right) = (4..WIDTH / 2 - 6, WIDTH / 2 + 6..WIDTH - 4);
    for columns in [left, right] {
        let fused_error = error(&fused, &scene, columns.clone());
        let best_input =
            error(&near, &scene, columns.clone()).max(error(&far, &scene, columns.clone()));
        assert!(
            fused_error < 0.05 * best_input,
            "{columns:?}: fused {fused_error} vs blurred input {best_input}"
        );
    }
}

#[test]
fn result_does_not_depend_on_frame_order() {
    let scene = sharp();
    let near = defocus(&scene, |x| x >= WIDTH / 3);
    let far = defocus(&scene, |x| x < WIDTH / 2);
    let forward = stack(&[
        (&near, identity()),
        (&far, identity()),
        (&scene, identity()),
    ]);
    let backward = stack(&[
        (&scene, identity()),
        (&far, identity()),
        (&near, identity()),
    ]);
    for (a, b) in forward.iter().zip(&backward) {
        assert!((a - b).abs() < 1e-5, "{a} != {b}");
    }
}

#[test]
fn shifted_frames_leave_no_gaps_and_divide_out_their_gain() {
    let scene = sharp();
    // The second frame is twice as bright and shifted, so it misses the right border.
    let mut shifted = vec![0.0; scene.len()];
    for y in 0..HEIGHT {
        for x in 3..WIDTH {
            for c in 0..3 {
                shifted[(y * WIDTH + x - 3) * 3 + c] = 2.0 * scene[(y * WIDTH + x) * 3 + c];
            }
        }
    }
    let registration = Registration {
        alignment: Alignment::translation(-3.0, 0.0),
        gain: 2.0,
    };
    let fused = stack(&[(&scene, identity()), (&shifted, registration)]);
    for (actual, expected) in fused.iter().zip(&scene) {
        assert!((actual - expected).abs() < 1e-4, "{actual} != {expected}");
    }
}

#[test]
fn stays_within_the_range_the_frames_measured() {
    // A bright bar in focus in one frame and spread out in the other: switching between
    // their coefficients rings, which must not leave the measured range.
    let mut sharp = vec![0.02; WIDTH * HEIGHT * 3];
    for y in 0..HEIGHT {
        for x in WIDTH / 2 - 2..WIDTH / 2 + 2 {
            sharp[(y * WIDTH + x) * 3..(y * WIDTH + x) * 3 + 3].fill(0.9);
        }
    }
    let blurred = defocus(&defocus(&sharp, |_| true), |_| true);
    let fused = stack(&[(&sharp, identity()), (&blurred, identity())]);
    for (i, pixel) in fused.chunks_exact(3).enumerate() {
        let luminance = |rgb: &[f32]| (rgb[0] + 2.0 * rgb[1] + rgb[2]) * 0.25;
        let inputs = [
            luminance(&sharp[i * 3..i * 3 + 3]),
            luminance(&blurred[i * 3..i * 3 + 3]),
        ];
        let (low, high) = (inputs[0].min(inputs[1]), inputs[0].max(inputs[1]));
        let value = luminance(pixel);
        assert!(
            value >= low * 0.999 - 1e-6 && value <= high * 1.001 + 1e-6,
            "pixel {i}: {value} outside {low}..{high}"
        );
    }
}

#[test]
fn rejects_invalid_frames() {
    assert!(FocusStack::new(0, 1).is_err());
    assert!(FocusStack::new(2, 1).unwrap().finish().is_err());
    let mut stack = FocusStack::new(2, 1).unwrap();
    assert!(stack.add(&[0.5; 3], identity()).is_err());
    assert!(stack.add(&[f32::NAN; 6], identity()).is_err());
    let invalid = Registration {
        gain: 0.0,
        ..identity()
    };
    assert!(stack.add(&[0.5; 6], invalid).is_err());
}
