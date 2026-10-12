//! Membrane (harmonic) interpolation of unknown pixels.

/// Grids at most this many pixels on their longer edge are solved directly.
const COARSEST_EDGE: usize = 16;
/// Gauss-Seidel sweeps on each finer level, which starts from the upsampled
/// coarser solution and so only has to remove local error.
const LEVEL_SWEEPS: usize = 64;
const COARSEST_SWEEPS: usize = 400;
/// Successive over-relaxation factor.
const OVER_RELAXATION: f32 = 1.8;

/// Replaces every `unknown` value with the smoothest interpolation of the
/// known values around it: the solution of Laplace's equation with known
/// pixels as Dirichlet and the grid border as Neumann boundary. It is solved
/// coarse to fine (cascadic multigrid), so large regions converge in a fixed
/// number of sweeps. Without any known pixel the values are left unchanged.
pub(super) fn fill(values: &mut [[f32; 3]], width: usize, height: usize, unknown: &[bool]) {
    debug_assert_eq!(values.len(), width * height);
    debug_assert_eq!(unknown.len(), width * height);
    let unknown_pixels = unknown
        .iter()
        .enumerate()
        .filter(|(_, unknown)| **unknown)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if unknown_pixels.is_empty() || unknown_pixels.len() == values.len() {
        return;
    }
    if width.max(height) <= COARSEST_EDGE {
        let known = values
            .iter()
            .zip(unknown)
            .filter(|(_, unknown)| !**unknown)
            .map(|(value, _)| *value)
            .collect::<Vec<_>>();
        let mean = known
            .iter()
            .fold([0.0f32; 3], |sum, value| {
                std::array::from_fn(|c| sum[c] + value[c])
            })
            .map(|sum| sum / known.len() as f32);
        for index in &unknown_pixels {
            values[*index] = mean;
        }
        relax(values, width, height, &unknown_pixels, COARSEST_SWEEPS);
        return;
    }

    let (coarse_width, coarse_height) = (width.div_ceil(2), height.div_ceil(2));
    let mut coarse = vec![[0.0f32; 3]; coarse_width * coarse_height];
    let mut coarse_unknown = vec![true; coarse_width * coarse_height];
    for coarse_y in 0..coarse_height {
        for coarse_x in 0..coarse_width {
            let mut sum = [0.0f32; 3];
            let mut count = 0usize;
            for y in 2 * coarse_y..(2 * coarse_y + 2).min(height) {
                for x in 2 * coarse_x..(2 * coarse_x + 2).min(width) {
                    let index = y * width + x;
                    if !unknown[index] {
                        for channel in 0..3 {
                            sum[channel] += values[index][channel];
                        }
                        count += 1;
                    }
                }
            }
            if count > 0 {
                let index = coarse_y * coarse_width + coarse_x;
                coarse[index] = sum.map(|value| value / count as f32);
                coarse_unknown[index] = false;
            }
        }
    }
    fill(&mut coarse, coarse_width, coarse_height, &coarse_unknown);
    for index in &unknown_pixels {
        let (x, y) = (index % width, index / width);
        values[*index] = sample_bilinear(
            &coarse,
            coarse_width,
            coarse_height,
            (x as f32 + 0.5) * 0.5 - 0.5,
            (y as f32 + 0.5) * 0.5 - 0.5,
        );
    }
    relax(values, width, height, &unknown_pixels, LEVEL_SWEEPS);
}

/// Over-relaxed Gauss-Seidel sweeps of the discrete Laplace equation over
/// `pixels`; neighbours beyond the grid border are left out (Neumann).
fn relax(values: &mut [[f32; 3]], width: usize, height: usize, pixels: &[usize], sweeps: usize) {
    for _ in 0..sweeps {
        for &index in pixels {
            let (x, y) = (index % width, index / width);
            let mut sum = [0.0f32; 3];
            let mut count = 0.0f32;
            let mut add = |neighbor: usize| {
                for channel in 0..3 {
                    sum[channel] += values[neighbor][channel];
                }
                count += 1.0;
            };
            if x > 0 {
                add(index - 1);
            }
            if x + 1 < width {
                add(index + 1);
            }
            if y > 0 {
                add(index - width);
            }
            if y + 1 < height {
                add(index + width);
            }
            if count == 0.0 {
                continue;
            }
            for channel in 0..3 {
                let target = sum[channel] / count;
                values[index][channel] += OVER_RELAXATION * (target - values[index][channel]);
            }
        }
    }
}

/// Bilinear sample at continuous pixel coordinates (pixel `i` centred on
/// `i`), clamped to the grid.
pub(super) fn sample_bilinear(
    values: &[[f32; 3]],
    width: usize,
    height: usize,
    x: f32,
    y: f32,
) -> [f32; 3] {
    let x = x.clamp(0.0, (width - 1) as f32);
    let y = y.clamp(0.0, (height - 1) as f32);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(width - 1), (y0 + 1).min(height - 1));
    let (tx, ty) = (x - x0 as f32, y - y0 as f32);
    let at = |x: usize, y: usize| values[y * width + x];
    let (a, b, c, d) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
    std::array::from_fn(|channel| {
        let top = a[channel] + (b[channel] - a[channel]) * tx;
        let bottom = c[channel] + (d[channel] - c[channel]) * tx;
        top + (bottom - top) * ty
    })
}
