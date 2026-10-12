//! Chessboard distance transform.

/// Chebyshev (chessboard) distance in pixels from every pixel to the nearest
/// `true` pixel of `mask`, saturating at `cap`. Two raster passes over the
/// 8-neighbourhood are exact for this metric.
pub(super) fn chebyshev(mask: &[bool], width: usize, height: usize, cap: u32) -> Vec<u32> {
    let mut distance = mask
        .iter()
        .map(|set| if *set { 0 } else { cap })
        .collect::<Vec<_>>();
    let step = |value: u32| value.saturating_add(1).min(cap);
    for y in 0..height {
        for x in 0..width {
            let index = y * width + x;
            let mut best = distance[index];
            if x > 0 {
                best = best.min(step(distance[index - 1]));
            }
            if y > 0 {
                let above = index - width;
                best = best.min(step(distance[above]));
                if x > 0 {
                    best = best.min(step(distance[above - 1]));
                }
                if x + 1 < width {
                    best = best.min(step(distance[above + 1]));
                }
            }
            distance[index] = best;
        }
    }
    for y in (0..height).rev() {
        for x in (0..width).rev() {
            let index = y * width + x;
            let mut best = distance[index];
            if x + 1 < width {
                best = best.min(step(distance[index + 1]));
            }
            if y + 1 < height {
                let below = index + width;
                best = best.min(step(distance[below]));
                if x + 1 < width {
                    best = best.min(step(distance[below + 1]));
                }
                if x > 0 {
                    best = best.min(step(distance[below - 1]));
                }
            }
            distance[index] = best;
        }
    }
    distance
}

/// Largest Chebyshev distance from a hole pixel to the nearest pixel outside
/// the hole: half the hole's thickness at its thickest. Pixels beyond the
/// image border do not count as outside.
pub(super) fn hole_radius(hole: &[bool], width: usize, height: usize) -> u32 {
    let outside = hole.iter().map(|hole| !hole).collect::<Vec<_>>();
    chebyshev(&outside, width, height, u32::MAX)
        .into_iter()
        .zip(hole)
        .filter(|(_, hole)| **hole)
        .map(|(distance, _)| distance)
        .max()
        .unwrap_or(0)
}
