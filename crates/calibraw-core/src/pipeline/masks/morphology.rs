//! Grayscale dilation and erosion of soft coverage: each pixel takes the largest
//! or smallest value within an octagon around it, so every level of a soft edge
//! moves by the same distance and keeps its softness. The cost does not depend
//! on the radius (van Herk, 1992; Gil and Werman, 1993).

use rayon::prelude::*;

/// Moves every level of `coverage` (0 to 1) outwards by `radius` raster pixels, or
/// inwards when negative. Radii between whole pixels interpolate, so the result
/// changes continuously with `radius` from the untouched coverage at 0. The raster's
/// edges are not edges of the selection: what touches them stays selected.
pub(super) fn offset_coverage(coverage: &mut [f32], width: usize, height: usize, radius: f32) {
    if width == 0 || height == 0 {
        return;
    }
    if radius >= 0.0 {
        offset_by_extreme(coverage, width, height, radius, f32::max, 0.0);
    } else {
        offset_by_extreme(coverage, width, height, -radius, f32::min, 1.0);
    }
}

/// `off_raster` is the value `pick` ignores. The filter runs on a margin of it as
/// wide as the octagon: the octagon's segments then add up the same in any order,
/// and every pixel sees all of the octagon that lies on the raster.
fn offset_by_extreme<F>(
    coverage: &mut [f32],
    width: usize,
    height: usize,
    radius: f32,
    pick: F,
    off_raster: f32,
) where
    F: Fn(f32, f32) -> f32 + Copy + Sync,
{
    let whole = radius.floor() as usize;
    let fraction = radius - whole as f32;
    let (lower, upper) = (Octagon::reaching(whole), Octagon::reaching(whole + 1));
    let shared = lower.shared_with(upper);
    let margin = whole + 1;
    let padded_width = width + 2 * margin;
    let mut lower_levels = with_margin(coverage, width, margin, off_raster);
    let mut filter = LineFilter::new(lower_levels.len(), padded_width, height + 2 * margin, pick);
    filter.apply(&mut lower_levels, shared);
    let mut upper_levels = lower_levels.clone();
    filter.apply(&mut upper_levels, upper.beyond(shared));
    filter.apply(&mut lower_levels, lower.beyond(shared));
    coverage
        .par_chunks_mut(width)
        .enumerate()
        .for_each(|(y, row)| {
            let start = (y + margin) * padded_width + margin;
            let lower = &lower_levels[start..][..width];
            let upper = &upper_levels[start..][..width];
            for ((value, lower), upper) in row.iter_mut().zip(lower).zip(upper) {
                *value = lower + (upper - lower) * fraction;
            }
        });
}

/// A copy of the raster `values`, `width` pixels wide, inside a margin of `fill`.
fn with_margin(values: &[f32], width: usize, margin: usize, fill: f32) -> Vec<f32> {
    let padded_width = width + 2 * margin;
    let padded_height = values.len() / width + 2 * margin;
    let mut padded = vec![fill; padded_width * padded_height];
    padded
        .par_chunks_mut(padded_width)
        .skip(margin)
        .zip(values.par_chunks(width))
        .for_each(|(padded, values)| padded[margin..][..width].copy_from_slice(values));
    padded
}

/// A near-regular octagon of whole pixels, as the sum of line segments reaching
/// `axis` pixels along both axes and `diagonal` pixels along both diagonals.
#[derive(Clone, Copy)]
struct Octagon {
    axis: usize,
    diagonal: usize,
}

impl Octagon {
    /// The octagon reaching `radius` pixels along the axes. A regular octagon
    /// gives each diagonal segment 1 − 1/√2 of that. Diagonal segments alone
    /// reach only every other pixel, so the axis segments never vanish.
    fn reaching(radius: usize) -> Self {
        const DIAGONAL_SHARE: f32 = 1.0 - std::f32::consts::FRAC_1_SQRT_2;
        let diagonal =
            ((radius as f32 * DIAGONAL_SHARE).round() as usize).min(radius.saturating_sub(1) / 2);
        Self {
            axis: radius - 2 * diagonal,
            diagonal,
        }
    }

    fn shared_with(self, other: Self) -> Self {
        Self {
            axis: self.axis.min(other.axis),
            diagonal: self.diagonal.min(other.diagonal),
        }
    }

    /// The segments that extend `part` to this octagon.
    fn beyond(self, part: Self) -> Self {
        Self {
            axis: self.axis - part.axis,
            diagonal: self.diagonal - part.diagonal,
        }
    }
}

/// How a line advances per row, in columns.
#[derive(Clone, Copy)]
enum Slant {
    Vertical,
    Right,
    Left,
}

impl Slant {
    const fn columns_per_row(self) -> isize {
        match self {
            Self::Vertical => 0,
            Self::Right => 1,
            Self::Left => -1,
        }
    }
}

/// Filters along lines of constant slant, keeping its buffers between passes.
///
/// Every line is cut into blocks at the same rows, each of a window's length.
/// `forward` holds the extreme of a line from its block's start to each pixel,
/// `backward` from each pixel to its block's end, and a window crosses at most
/// one block boundary, so two lookups answer it.
struct LineFilter<F> {
    forward: Vec<f32>,
    backward: Vec<f32>,
    transposed: Vec<f32>,
    width: usize,
    height: usize,
    pick: F,
}

impl<F> LineFilter<F>
where
    F: Fn(f32, f32) -> f32 + Copy + Sync,
{
    fn new(len: usize, width: usize, height: usize, pick: F) -> Self {
        Self {
            forward: vec![0.0; len],
            backward: vec![0.0; len],
            transposed: Vec::new(),
            width,
            height,
            pick,
        }
    }

    fn apply(&mut self, values: &mut [f32], octagon: Octagon) {
        let (width, height) = (self.width, self.height);
        if octagon.axis > 0 {
            self.filter(values, width, height, Slant::Vertical, octagon.axis);
            // Rows are the columns of the transposed raster.
            let mut transposed = std::mem::take(&mut self.transposed);
            transposed.resize(values.len(), 0.0);
            transpose(values, &mut transposed, width);
            self.filter(
                &mut transposed,
                height,
                width,
                Slant::Vertical,
                octagon.axis,
            );
            transpose(&transposed, values, height);
            self.transposed = transposed;
        }
        if octagon.diagonal > 0 {
            self.filter(values, width, height, Slant::Right, octagon.diagonal);
            self.filter(values, width, height, Slant::Left, octagon.diagonal);
        }
    }

    /// Replaces each pixel with the extreme of the `reach` pixels either side
    /// of it along its line, which ends at the edges of the raster it is given.
    fn filter(
        &mut self,
        values: &mut [f32],
        width: usize,
        height: usize,
        slant: Slant,
        reach: usize,
    ) {
        let block_rows = 2 * reach + 1;
        let pick = self.pick;
        let shift = slant.columns_per_row();
        self.forward.copy_from_slice(values);
        self.forward
            .par_chunks_mut(width * block_rows)
            .for_each(|block| {
                for y in 1..block.len() / width {
                    let (earlier, current) = block.split_at_mut(y * width);
                    let previous = &earlier[(y - 1) * width..];
                    absorb(&mut current[..width], previous, shift, pick);
                }
            });
        self.backward.copy_from_slice(values);
        self.backward
            .par_chunks_mut(width * block_rows)
            .for_each(|block| {
                for y in (0..(block.len() / width).saturating_sub(1)).rev() {
                    let (current, later) = block.split_at_mut((y + 1) * width);
                    let next = &later[..width];
                    absorb(&mut current[y * width..], next, -shift, pick);
                }
            });
        let (forward, backward) = (&self.forward, &self.backward);
        values
            .par_chunks_mut(width)
            .enumerate()
            .for_each(|(y, row)| {
                // The one multiple of the block length in the untruncated window.
                let boundary = (y + reach) / block_rows * block_rows;
                for (x, value) in row.iter_mut().enumerate() {
                    let (room_before, room_after) = match slant {
                        Slant::Vertical => (reach, reach),
                        Slant::Right => (x, width - 1 - x),
                        Slant::Left => (width - 1 - x, x),
                    };
                    let steps_before = reach.min(y).min(room_before);
                    let steps_after = reach.min(height - 1 - y).min(room_after);
                    let (first, last) = (y - steps_before, y + steps_after);
                    let at = |line_row: usize, steps: isize| {
                        line_row * width + (x as isize + steps * shift) as usize
                    };
                    let tail = || backward[at(first, -(steps_before as isize))];
                    let head = || forward[at(last, steps_after as isize)];
                    *value = if first >= boundary {
                        head()
                    } else if last < boundary {
                        tail()
                    } else {
                        pick(tail(), head())
                    };
                }
            });
    }
}

/// Merges into each pixel of `row` the pixel `shift` columns before it in
/// `neighbour`, the adjacent row of its line.
fn absorb<F>(row: &mut [f32], neighbour: &[f32], shift: isize, pick: F)
where
    F: Fn(f32, f32) -> f32,
{
    let width = row.len();
    let (row, neighbour) = match shift {
        1 => (&mut row[1..], &neighbour[..width - 1]),
        -1 => (&mut row[..width - 1], &neighbour[1..width]),
        _ => (row, &neighbour[..width]),
    };
    for (value, neighbour) in row.iter_mut().zip(neighbour) {
        *value = pick(*value, *neighbour);
    }
}

/// Writes the transpose of the raster `values`, `width` pixels wide, to `out`.
fn transpose(values: &[f32], out: &mut [f32], width: usize) {
    let height = values.len() / width;
    out.par_chunks_mut(height)
        .enumerate()
        .for_each(|(x, column)| {
            for (y, value) in column.iter_mut().enumerate() {
                *value = values[y * width + x];
            }
        });
}
