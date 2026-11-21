//! The largest axis-aligned rectangle of covered seam-grid pixels: the panorama's crop.

/// `[x0, y0, x1, y1)` of the largest rectangle whose pixels are all `inside`, found row by
/// row with the histogram-stack method; `None` when no pixel is inside.
pub(super) fn largest_rectangle(
    width: usize,
    height: usize,
    inside: &[bool],
) -> Option<[usize; 4]> {
    let mut heights = vec![0usize; width];
    let mut best: Option<([usize; 4], usize)> = None;
    let mut stack: Vec<usize> = Vec::with_capacity(width + 1);
    for y in 0..height {
        for x in 0..width {
            heights[x] = if inside[y * width + x] {
                heights[x] + 1
            } else {
                0
            };
        }
        stack.clear();
        for x in 0..=width {
            let current = if x < width { heights[x] } else { 0 };
            while let Some(&top) = stack.last() {
                if heights[top] < current {
                    break;
                }
                stack.pop();
                let tall = heights[top];
                let left = stack.last().map_or(0, |&l| l + 1);
                let area = tall * (x - left);
                if area > 0 && best.is_none_or(|(_, a)| area > a) {
                    best = Some(([left, y + 1 - tall, x, y + 1], area));
                }
            }
            stack.push(x);
        }
    }
    best.map(|(rect, _)| rect)
}

/// `inside` shrunk by one pixel: a pixel stays only when it and its eight neighbours are
/// inside, so a rectangle of the result is covered at full resolution too.
pub(super) fn erode(width: usize, height: usize, inside: &[bool]) -> Vec<bool> {
    (0..width * height)
        .map(|i| {
            let (x, y) = (i % width, i / width);
            x > 0
                && y > 0
                && x + 1 < width
                && y + 1 < height
                && (y - 1..=y + 1).all(|yy| (x - 1..=x + 1).all(|xx| inside[yy * width + xx]))
        })
        .collect()
}
