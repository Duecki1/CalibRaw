//! How wide a feather's fade may be around each part of a generated mask: about the
//! half thickness of the part or gap at that edge. A strand then stays solid and
//! grows no halo, while a broad edge fades over the whole feather radius. The room
//! is measured once per mask on a coarse grid of it, so neither zoom nor tile size
//! changes it.

use super::box_filter::gaussian_blur;
use super::{chamfer_distance, mask_feather_radius, MaskImage};
use rayon::prelude::*;
use std::f32::consts::SQRT_2;
use std::iter::successors;
use std::mem::size_of;

/// Longer edge of the grid, in cells.
const GRID_EDGE: usize = 512;
/// How far outside an opening a pixel still counts as inside it, relative to the
/// disc's radius, so bumps on an edge do not shrink the room around them.
const OPENING_SLACK: f32 = 0.5;
/// Sigma, in cells, that blends the room across the steps between disc radii.
const ROOM_SMOOTHING: f32 = 1.0;

const NEIGHBOURS: [(isize, isize, f32); 8] = [
    (-1, 0, 1.0),
    (1, 0, 1.0),
    (0, -1, 1.0),
    (0, 1, 1.0),
    (-1, -1, SQRT_2),
    (1, -1, SQRT_2),
    (-1, 1, SQRT_2),
    (1, 1, SQRT_2),
];

pub(super) struct EdgeRoom {
    width: usize,
    height: usize,
    /// Source pixels per cell.
    cell: f32,
    /// Half thickness, in source pixels, of the selected part each cell is on or
    /// nearest to.
    selected: Vec<f32>,
    /// The same for the unselected gaps.
    gap: Vec<f32>,
}

impl EdgeRoom {
    pub(super) fn measure(source: &MaskImage) -> Self {
        let (source_width, source_height) = (source.width as usize, source.height as usize);
        let cell = source_width.max(source_height).div_ceil(GRID_EDGE).max(1);
        let (width, height) = (source_width.div_ceil(cell), source_height.div_ceil(cell));
        // A cell holding both counts for both, so nothing thinner than a cell is lost.
        let mut selected = vec![0_u8; width * height];
        let mut gap = vec![0_u8; width * height];
        for (y, row) in source.pixels.chunks_exact(source_width.max(1)).enumerate() {
            for (x, pixel) in row.iter().enumerate() {
                let part = if *pixel >= 128 {
                    &mut selected
                } else {
                    &mut gap
                };
                part[(y / cell) * width + x / cell] = 1;
            }
        }
        let widest = mask_feather_radius(source_width.min(source_height) as f32, 1.0) / cell as f32;
        let (selected, gap) = rayon::join(
            || part_room(&selected, width, height, widest),
            || part_room(&gap, width, height, widest),
        );
        let to_source_pixels = |room: Vec<f32>| -> Vec<f32> {
            room.into_iter().map(|cells| cells * cell as f32).collect()
        };
        Self {
            width,
            height,
            cell: cell as f32,
            selected: to_source_pixels(selected),
            gap: to_source_pixels(gap),
        }
    }

    /// The room at source pixel (`x`, `y`) of a mask grown by `grow` source pixels,
    /// which thickens its parts and narrows its gaps. Negative where nothing is left.
    pub(super) fn at(&self, x: f32, y: f32, grow: f32) -> f32 {
        if self.selected.is_empty() {
            return f32::INFINITY;
        }
        let axis = |position: f32, cells: usize| {
            let position = (position / self.cell - 0.5).clamp(0.0, (cells - 1) as f32);
            let lower = position as usize;
            (lower, (lower + 1).min(cells - 1), position - lower as f32)
        };
        let (x0, x1, fx) = axis(x, self.width);
        let (y0, y1, fy) = axis(y, self.height);
        let sample = |field: &[f32]| {
            let top = field[y0 * self.width + x0] * (1.0 - fx) + field[y0 * self.width + x1] * fx;
            let bottom =
                field[y1 * self.width + x0] * (1.0 - fx) + field[y1 * self.width + x1] * fx;
            top * (1.0 - fy) + bottom * fy
        };
        (sample(&self.selected) + grow).min(sample(&self.gap) - grow)
    }

    pub(super) fn byte_len(&self) -> usize {
        size_of::<Self>() + (self.selected.capacity() + self.gap.capacity()) * size_of::<f32>()
    }
}

/// The room, in cells, around the 1-cells of `part`: on the part, the largest disc
/// whose opening of the part holds the cell, which unlike a medial axis does not
/// jump at every bump of an edge. Off the part, the room of its nearest cell on it.
/// Without a part nothing limits the fade, up to the `widest` feather radius.
fn part_room(part: &[u8], width: usize, height: usize, widest: f32) -> Vec<f32> {
    if !part.contains(&1) {
        return vec![widest; part.len()];
    }
    let depth = chamfer_distance(part, width, height, 0);
    // One disc past the widest radius, whose room still covers that radius.
    let discs: Vec<f32> = successors(Some(1.0_f32), |disc| {
        (*disc < widest).then_some(*disc * SQRT_2)
    })
    .collect();
    let mut room = discs
        .into_par_iter()
        .map(|disc| opening_room(part, &depth, width, height, disc))
        .reduce(
            || vec![0.0; part.len()],
            |mut largest, room| {
                for (largest, room) in largest.iter_mut().zip(room) {
                    *largest = largest.max(room);
                }
                largest
            },
        );
    spread_from_part(&mut room, part, width, height);
    gaussian_blur(&mut room, width, height, ROOM_SMOOTHING);
    room
}

/// The room a disc of radius `disc` cells gives the cells of `part` in its opening,
/// falling off beyond the opening. Depths count to the centre of the first cell off
/// the part, half a cell past its edge.
fn opening_room(part: &[u8], depth: &[f32], width: usize, height: usize, disc: f32) -> Vec<f32> {
    let core: Vec<u8> = depth.iter().map(|depth| u8::from(*depth >= disc)).collect();
    if !core.contains(&1) {
        return vec![0.0; part.len()];
    }
    let half_thickness = (disc - 0.5).max(0.5);
    chamfer_distance(&core, width, height, 1)
        .iter()
        .zip(part)
        .map(|(from_core, on_part)| {
            let inside = ((1.0 + OPENING_SLACK) * disc - from_core) / (OPENING_SLACK * disc);
            half_thickness * inside.clamp(0.0, 1.0) * f32::from(*on_part)
        })
        .collect()
}

/// Gives every cell off `part` the room of its nearest cell on it.
fn spread_from_part(room: &mut [f32], part: &[u8], width: usize, height: usize) {
    let distance = chamfer_distance(part, width, height, 1);
    let mut order: Vec<usize> = (0..part.len()).filter(|index| part[*index] == 0).collect();
    order.par_sort_unstable_by(|a, b| distance[*a].total_cmp(&distance[*b]));
    for index in order {
        let (x, y) = ((index % width) as isize, (index / width) as isize);
        let mut nearest = 0.0_f32;
        for (dx, dy, step) in NEIGHBOURS {
            let (nx, ny) = (x + dx, y + dy);
            if nx < 0 || ny < 0 || nx >= width as isize || ny >= height as isize {
                continue;
            }
            let neighbour = ny as usize * width + nx as usize;
            // On a shortest path to the part: the distance grew by exactly this step.
            if (distance[index] - step - distance[neighbour]).abs() < 1e-3 {
                nearest = nearest.max(room[neighbour]);
            }
        }
        room[index] = nearest;
    }
}
