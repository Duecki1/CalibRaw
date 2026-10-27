//! Distances to the nearest of some pixels of a plane, chamfer-approximated.

/// No seed pixel in the plane.
pub(super) const NO_SEED: u32 = u32::MAX;

/// For each pixel of a `width` x `height` plane, by row: the distance in
/// pixels to the nearest pixel `is_seed` accepts, and that pixel's index.
/// Infinity and `NO_SEED` where no pixel is a seed. Two chamfer passes with
/// steps of 1 and √2.
pub(super) fn nearest_seeds(
    width: usize,
    height: usize,
    is_seed: impl Fn(usize) -> bool,
) -> (Vec<f32>, Vec<u32>) {
    let mut nearest: Vec<u32> = (0..width * height)
        .map(|index| {
            if is_seed(index) {
                index as u32
            } else {
                NO_SEED
            }
        })
        .collect();
    let mut distance: Vec<f32> = nearest
        .iter()
        .map(|&seed| if seed == NO_SEED { f32::INFINITY } else { 0.0 })
        .collect();
    let diagonal = std::f32::consts::SQRT_2;
    let forward = [
        (-1isize, 0isize, 1.0f32),
        (-1, -1, diagonal),
        (0, -1, 1.0),
        (1, -1, diagonal),
    ];
    let backward = forward.map(|(dx, dy, cost)| (-dx, -dy, cost));
    let mut relax = |x: usize, y: usize, steps: &[(isize, isize, f32)]| {
        let index = y * width + x;
        for &(dx, dy, cost) in steps {
            let (nx, ny) = (x as isize + dx, y as isize + dy);
            if nx < 0 || ny < 0 || nx >= width as isize || ny >= height as isize {
                continue;
            }
            let neighbour = ny as usize * width + nx as usize;
            if distance[neighbour] + cost < distance[index] {
                distance[index] = distance[neighbour] + cost;
                nearest[index] = nearest[neighbour];
            }
        }
    };
    for y in 0..height {
        for x in 0..width {
            relax(x, y, &forward);
        }
    }
    for y in (0..height).rev() {
        for x in (0..width).rev() {
            relax(x, y, &backward);
        }
    }
    (distance, nearest)
}
