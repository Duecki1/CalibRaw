//! Exposure balancing between overlapping frames.
//!
//! Frames arrive normalized by their metadata exposure, as HDR merge normalizes them.
//! What remains (residual vignetting, flicker, imprecise shutter speeds) is solved with
//! the objective of OpenCV's `GainCompensator` (`modules/stitching/src/
//! exposure_compensate.cpp`, BSD-3-Clause): every overlap's mean brightness should agree
//! between its two frames, and every gain is pulled towards one. It is solved for log
//! gains, so the result is independent of the brightness scale and every gain is
//! positive, and with a weaker prior than OpenCV's, which has to start from unknown
//! 8-bit exposures; here one means "trust the metadata".

use super::math::solve_positive_definite;
use super::overview::Overview;
use rayon::prelude::*;

/// Weight of the prior relative to the overlaps a frame takes part in.
const PRIOR: f64 = 0.1;
/// Fewest measured pixels an overlap needs to count.
const MIN_OVERLAP: usize = 64;

/// A gain per frame that multiplies its exposure-normalized values.
pub(super) fn balance(overviews: &[Overview]) -> Vec<f64> {
    let n = overviews.len();
    let pairs: Vec<(usize, usize)> = (0..n)
        .flat_map(|i| (i + 1..n).map(move |j| (i, j)))
        .collect();
    let overlaps: Vec<(usize, usize, f64, f64)> = pairs
        .par_iter()
        .filter_map(|&(i, j)| {
            let (a, b) = (&overviews[i], &overviews[j]);
            let x0 = a.rect[0].max(b.rect[0]);
            let y0 = a.rect[1].max(b.rect[1]);
            let x1 = a.rect[2].min(b.rect[2]);
            let y1 = a.rect[3].min(b.rect[3]);
            let (mut count, mut sum_a, mut sum_b) = (0usize, 0.0f64, 0.0f64);
            for y in y0..y1 {
                for x in x0..x1 {
                    let (Some(ia), Some(ib)) = (a.index(x, y), b.index(x, y)) else {
                        continue;
                    };
                    if a.covered[ia] && b.covered[ib] && a.measured[ia] && b.measured[ib] {
                        count += 1;
                        sum_a += f64::from(luminance(a.rgb[ia]));
                        sum_b += f64::from(luminance(b.rgb[ib]));
                    }
                }
            }
            (count >= MIN_OVERLAP && sum_a > 0.0 && sum_b > 0.0)
                .then(|| (i, j, count as f64, (sum_a / sum_b).ln()))
        })
        .collect();
    // Normal equations of Σ N·(aᵢ − aⱼ + ln Iᵢⱼ − ln Iⱼᵢ)² + PRIOR·Σ Pᵢ·aᵢ², with Pᵢ the
    // pixels of every overlap frame i takes part in.
    let mut a = vec![0.0; n * n];
    let mut b = vec![0.0; n];
    let mut participation = vec![0.0; n];
    for &(i, j, count, log_ratio) in &overlaps {
        a[i * n + i] += count;
        a[j * n + j] += count;
        a[i * n + j] -= count;
        a[j * n + i] -= count;
        b[i] -= count * log_ratio;
        b[j] += count * log_ratio;
        participation[i] += count;
        participation[j] += count;
    }
    for i in 0..n {
        a[i * n + i] += PRIOR * participation[i].max(1.0);
    }
    let gains = solve_positive_definite(&a, &b)
        .map(|log_gains| log_gains.into_iter().map(f64::exp).collect())
        .unwrap_or_else(|| vec![1.0; n]);
    crate::diagnostics::record(format!(
        "Panorama exposure gains: {}",
        gains
            .iter()
            .map(|g: &f64| format!("{g:.3}"))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    gains
}

pub(super) fn luminance(rgb: [f32; 3]) -> f32 {
    (rgb[0] + 2.0 * rgb[1] + rgb[2]) * 0.25
}
