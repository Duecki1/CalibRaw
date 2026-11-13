// SPDX-License-Identifier: GPL-3.0-or-later
// Adapted from darktable 5.6.0 src/iop/hlreconstruct/segbased.c and segmentation.c.
// Copyright (C) 2010-2026 darktable developers.
// Copyright (C) 2026 CalibRaw contributors (Rust adaptation).

//! darktable's segmentation-based choice of highlight chroma (darktable 5.6.0,
//! `src/iop/hlreconstruct/segbased.c` and `segmentation.c`).
//!
//! It reads blocks of 3×3 photosites, darktable's superpixels, as cube roots of their mean
//! sensor level per colour. Per colour, the blocks whose mean clipped, dilated, form segments;
//! a segment takes the chroma of its best candidate block, one close to the clip level and
//! smooth. Chroma is a colour's root less the mean root of the other two.
//!
//! Where this differs from darktable: a segment without a candidate, and clipping outside any
//! segment, take chroma 0, the plain opposed estimate. darktable gives them its opposed chroma,
//! one mean over the near-clip photosites of the whole photo, which carries the colour of
//! unrelated areas into them: a blue sky's into white clouds, or coloured highlights' into a
//! neutral gradient. Blocks are measured from the recorded levels, where darktable's
//! segmentation reads its opposed result; the number of segments is not limited; and the result
//! is held per model cell, each cell taking the segment most of its segment blocks belong to.

use super::super::{CfaKind, LoadedRaw};
use super::{other_channels, SensorSaturation};
use rayon::prelude::*;

/// Edge of a block in photosites.
const BLOCK_EDGE: usize = 3;
/// darktable's default "combine": the clipped blocks of a colour are dilated by this many
/// blocks before they are segmented.
const SEGMENT_DILATION: usize = 2;
/// darktable keeps segments of more blocks than this.
const SEGMENT_MIN_BLOCKS: usize = 3;
/// darktable's default "candidating": a segment's best candidate must weigh more than 1 less
/// this, else the segment's chroma is 0.
const CANDIDATING: f32 = 0.4;
/// darktable's weight of a candidate inside its segment; one on the segment's border weighs 1.
const INSIDE_CANDIDATE_WEIGHT: f32 = 0.75;
/// darktable's lowest candidate level, as a share of the clip level's cube root.
const CANDIDATE_LOWEST_ROOT: f32 = 0.125;
/// darktable's binomial weights of the 5×5 blocks a candidate's level is read from.
const LEVEL_WEIGHTS: [f32; 5] = [1.0, 4.0, 6.0, 4.0, 1.0];

/// The blocks of a mosaic. Block `(x, y)` covers rows `3y..3y + 3` and columns
/// `o + 3x..o + 3x + 3`, `o` being the row's offset: 1 on a Bayer row starting with red or
/// blue, as darktable aligns blocks so each holds greens, reds and blues 5 : 2 : 2, else 0.
struct Blocks {
    width: usize,
    height: usize,
    /// Column offset of each row of blocks.
    row_offsets: Vec<usize>,
    /// Cube roots of each colour's mean sensor level.
    roots: Vec<[f32; 3]>,
}

impl Blocks {
    /// The root of `colour` at block `(x, y)`; positions outside take the nearest block, as
    /// darktable extends its planes' borders.
    fn root(&self, x: isize, y: isize, colour: usize) -> f32 {
        let x = x.clamp(0, self.width as isize - 1) as usize;
        let y = y.clamp(0, self.height as isize - 1) as usize;
        self.roots[y * self.width + x][colour]
    }

    /// darktable's dilation of the blocks `marked` says by `radius` (at least 1) blocks: the
    /// square of edge `2 * radius + 1` without its four corners, the union of two rectangles.
    fn dilate(&self, marked: &[bool], radius: usize) -> Vec<bool> {
        let wide = self.dilate_rectangle(marked, radius, radius - 1);
        let tall = self.dilate_rectangle(marked, radius - 1, radius);
        wide.into_iter()
            .zip(tall)
            .map(|(wide, tall)| wide || tall)
            .collect()
    }

    /// The blocks within `across` columns and `down` rows of a block `marked` says.
    fn dilate_rectangle(&self, marked: &[bool], across: usize, down: usize) -> Vec<bool> {
        let width = self.width;
        let rows: Vec<bool> = marked
            .par_chunks(width)
            .flat_map_iter(|row| {
                let mut out = vec![false; width];
                let mut last = None;
                for (x, reached) in out.iter_mut().enumerate() {
                    if row[x] {
                        last = Some(x);
                    }
                    *reached = last.is_some_and(|last| x - last <= across);
                }
                let mut next = None;
                for (x, reached) in out.iter_mut().enumerate().rev() {
                    if row[x] {
                        next = Some(x);
                    }
                    *reached |= next.is_some_and(|next| next - x <= across);
                }
                out
            })
            .collect();
        // Per column, the marked rows within `down` of each row, counted over a sliding window.
        let mut counts = vec![0u32; width];
        let window_row = |y: usize| &rows[y * width..(y + 1) * width];
        for y in 0..down.min(self.height) {
            for (count, reached) in counts.iter_mut().zip(window_row(y)) {
                *count += u32::from(*reached);
            }
        }
        let mut out = Vec::with_capacity(rows.len());
        for y in 0..self.height {
            if y + down < self.height {
                for (count, reached) in counts.iter_mut().zip(window_row(y + down)) {
                    *count += u32::from(*reached);
                }
            }
            out.extend(counts.iter().map(|count| *count > 0));
            if y >= down {
                for (count, reached) in counts.iter_mut().zip(window_row(y - down)) {
                    *count -= u32::from(*reached);
                }
            }
        }
        out
    }
}

/// A segment's candidate block: what its weight and chroma need under any white balance.
#[derive(Clone, Copy, Debug)]
struct Candidate {
    /// darktable's weight before smoothness: the square of the mean root of the 3×3 blocks
    /// around it as a share of the clip root, times [`INSIDE_CANDIDATE_WEIGHT`] inside the
    /// segment.
    closeness: f32,
    /// Standard deviation of the roots of the 5×5 blocks around it but the corners, in sensor
    /// units. White balance scales it by the cube root of the colour's multiplier.
    deviation: f32,
    /// Binomial mean of the unclipped roots of the 5×5 blocks around it, at most the clip root.
    level: f32,
    /// The roots of the other two colours at the block, `other_channels` order.
    others: [f32; 2],
}

impl Candidate {
    /// darktable's `_calc_weight` under white balance with the colour's multiplier root
    /// `wb_root`.
    fn weight(&self, wb_root: f32) -> f32 {
        let smoothness = (1.0 - 10.0 * (wb_root * self.deviation).sqrt()).max(0.0);
        self.closeness * smoothness
    }
}

/// The segments of one colour.
#[derive(Debug)]
pub(super) struct ColourSegments {
    /// Per model cell, 1 plus the segment most of its segment blocks belong to, or 0.
    cell_segment: Vec<u32>,
    /// Per segment, the candidates the best one under some white balance is among: each has a
    /// higher closeness or a lower deviation than every other.
    candidates: Vec<Vec<Candidate>>,
}

impl ColourSegments {
    /// The chroma of every model cell for `channel` under white balance whose multipliers have
    /// the cube roots `wb_roots`: its segment's best candidate's, else 0. A root of
    /// white-balanced camera RGB is the multiplier's root times the sensor level's, so sensor
    /// roots follow white balance.
    pub(super) fn cell_chroma(&self, channel: usize, wb_roots: [f32; 3]) -> Vec<f32> {
        let [a, b] = other_channels(channel);
        let relation = |own: f32, others: [f32; 2]| {
            wb_roots[channel] * own - 0.5 * (wb_roots[a] * others[0] + wb_roots[b] * others[1])
        };
        let segment_chroma: Vec<f32> = self
            .candidates
            .iter()
            .map(|candidates| {
                candidates
                    .iter()
                    .map(|candidate| (candidate.weight(wb_roots[channel]), candidate))
                    .max_by(|left, right| left.0.total_cmp(&right.0))
                    .filter(|(weight, _)| *weight > 1.0 - CANDIDATING)
                    .map_or(0.0, |(_, candidate)| {
                        relation(candidate.level, candidate.others)
                    })
            })
            .collect();
        self.cell_segment
            .iter()
            .map(|segment| match *segment {
                0 => 0.0,
                segment => segment_chroma[segment as usize - 1],
            })
            .collect()
    }
}

impl LoadedRaw {
    /// The blocks of `pixels`, a mosaic of this photo, at `black_point`. Only blocks centred in
    /// a model cell of `cells` that `near_clipping` marks are read; the others hold roots of 0.
    fn highlight_blocks(
        &self,
        black_point: f32,
        pixels: &[u16],
        cells: [u32; 2],
        near_clipping: &[bool],
    ) -> Blocks {
        let width = self.width as usize;
        let height = self.height as usize;
        let blocks_x = width.saturating_sub(1) / BLOCK_EDGE;
        let blocks_y = height / BLOCK_EDGE;
        let [cells_x, cells_y] = cells.map(|count| count as usize);
        let rows: Vec<(usize, Vec<[f32; 3]>)> = (0..blocks_y)
            .into_par_iter()
            .map(|block_row| {
                let top = block_row * BLOCK_EDGE;
                let left = usize::from(
                    self.cfa_kind == CfaKind::Bayer && self.logical_color(top * width) != 1,
                );
                let cell_row = (top + 1) * cells_y / height * cells_x;
                let roots = (0..blocks_x)
                    .map(|block_column| {
                        let first = left + block_column * BLOCK_EDGE;
                        if !near_clipping[cell_row + (first + 1) * cells_x / width] {
                            return [0.0; 3];
                        }
                        let mut sum = [0.0f32; 3];
                        let mut count = [0u32; 3];
                        for y in top..top + BLOCK_EDGE {
                            for x in first..first + BLOCK_EDGE {
                                let index = y * width + x;
                                let color = self.logical_color(index);
                                sum[color] +=
                                    self.highlight_sensor_value(index, black_point, pixels);
                                count[color] += 1;
                            }
                        }
                        std::array::from_fn(|color| {
                            (sum[color] / count[color].max(1) as f32).cbrt()
                        })
                    })
                    .collect();
                (left, roots)
            })
            .collect();
        let mut blocks = Blocks {
            width: blocks_x,
            height: blocks_y,
            row_offsets: Vec::with_capacity(blocks_y),
            roots: Vec::with_capacity(blocks_x * blocks_y),
        };
        for (left, roots) in rows {
            blocks.row_offsets.push(left);
            blocks.roots.extend(roots);
        }
        blocks
    }

    /// The segments of each colour of `pixels`, a mosaic of this photo, for a model of `cells`
    /// cells; photosites clip at `clip_threshold` in normalized sensor units. `clipped_cells`
    /// marks the cells with a clipped photosite: segments, their borders and the windows their
    /// candidates read lie within 5 blocks, 15 photosites, of a clipped block, so only blocks
    /// within two cells of those are read.
    pub(super) fn highlight_segments(
        &self,
        cells: [u32; 2],
        clipped_cells: &[bool],
        black_point: f32,
        clip_threshold: f32,
        pixels: &[u16],
    ) -> [ColourSegments; 3] {
        let [cells_x, cells_y] = cells.map(|count| count as usize);
        let near_clipping: Vec<bool> = (0..clipped_cells.len())
            .map(|index| {
                let (x, y) = (index % cells_x, index / cells_x);
                (y.saturating_sub(2)..=(y + 2).min(cells_y - 1)).any(|row| {
                    (x.saturating_sub(2)..=(x + 2).min(cells_x - 1))
                        .any(|column| clipped_cells[row * cells_x + column])
                })
            })
            .collect();
        let blocks = self.highlight_blocks(black_point, pixels, cells, &near_clipping);
        let clip_root = (SensorSaturation::CLIP_DETECTION * clip_threshold).cbrt();
        std::array::from_fn(|colour| self.colour_segments(&blocks, colour, cells, clip_root))
    }

    /// darktable's segmentation of `colour`: its clipped blocks, those whose mean root reaches
    /// `clip_root`, dilated by [`SEGMENT_DILATION`] and labelled 4-connected; a segment's border
    /// is the blocks 4-adjacent to it outside the dilation.
    fn colour_segments(
        &self,
        blocks: &Blocks,
        colour: usize,
        cells: [u32; 2],
        clip_root: f32,
    ) -> ColourSegments {
        const NONE: u32 = u32::MAX;
        let (width, height) = (blocks.width, blocks.height);
        let clipped: Vec<bool> = blocks
            .roots
            .iter()
            .map(|roots| roots[colour] >= clip_root)
            .collect();
        if !clipped.contains(&true) {
            return ColourSegments {
                cell_segment: vec![0; (cells[0] * cells[1]) as usize],
                candidates: Vec::new(),
            };
        }
        let dilated = blocks.dilate(&clipped, SEGMENT_DILATION);
        let mut member = vec![NONE; dilated.len()];
        let mut border = vec![NONE; dilated.len()];
        let mut segments: Vec<Vec<usize>> = Vec::new();
        let mut borders: Vec<Vec<usize>> = Vec::new();
        let mut stack = Vec::new();
        for seed in 0..dilated.len() {
            if !dilated[seed] || member[seed] != NONE {
                continue;
            }
            let id = segments.len() as u32;
            let mut blocks_of_segment = Vec::new();
            let mut border_of_segment = Vec::new();
            stack.push(seed);
            member[seed] = id;
            while let Some(index) = stack.pop() {
                blocks_of_segment.push(index);
                let (x, y) = (index % width, index / width);
                let neighbours = [
                    (x > 0).then(|| index - 1),
                    (x + 1 < width).then(|| index + 1),
                    (y > 0).then(|| index - width),
                    (y + 1 < height).then(|| index + width),
                ];
                for neighbour in neighbours.into_iter().flatten() {
                    if dilated[neighbour] {
                        if member[neighbour] == NONE {
                            member[neighbour] = id;
                            stack.push(neighbour);
                        }
                    } else if border[neighbour] == NONE {
                        border[neighbour] = id;
                        border_of_segment.push(neighbour);
                    }
                }
            }
            if blocks_of_segment.len() <= SEGMENT_MIN_BLOCKS {
                // Too small: its blocks belong to no segment, as darktable reverts them.
                for index in blocks_of_segment {
                    member[index] = NONE - 1;
                }
                for index in border_of_segment {
                    border[index] = NONE;
                }
                continue;
            }
            segments.push(blocks_of_segment);
            borders.push(border_of_segment);
        }
        let candidates = segments
            .par_iter()
            .zip(&borders)
            .map(|(inside, outside)| {
                let found = inside
                    .iter()
                    .map(|index| (*index, INSIDE_CANDIDATE_WEIGHT))
                    .chain(outside.iter().map(|index| (*index, 1.0)))
                    .filter_map(|(index, factor)| {
                        Self::candidate(blocks, colour, index, factor, clip_root)
                    })
                    .collect();
                undominated(found)
            })
            .collect();
        let cell_segment =
            cell_segments(blocks, &member, &segments, cells, self.width, self.height);
        ColourSegments {
            cell_segment,
            candidates,
        }
    }

    /// The candidate at block `index` of `colour`, weighted by `factor` for its place in the
    /// segment; `None` where it clipped or cannot pass darktable's limits under any white
    /// balance.
    fn candidate(
        blocks: &Blocks,
        colour: usize,
        index: usize,
        factor: f32,
        clip_root: f32,
    ) -> Option<Candidate> {
        let root = blocks.roots[index][colour];
        if root >= clip_root {
            return None;
        }
        let (x, y) = (
            (index % blocks.width) as isize,
            (index / blocks.width) as isize,
        );
        let at = |dx: isize, dy: isize| blocks.root(x + dx, y + dy, colour);
        let mean_3x3 = (-1..=1)
            .flat_map(|dy| (-1..=1).map(move |dx| (dx, dy)))
            .map(|(dx, dy)| at(dx, dy))
            .sum::<f32>()
            / 9.0;
        let share = mean_3x3 / clip_root;
        let closeness = factor * if share < 1.0 { share * share } else { 0.0 };
        // Smoothness is at most 1, so a lower closeness never passes.
        if closeness <= 1.0 - CANDIDATING {
            return None;
        }
        let window: Vec<f32> = (-2..=2)
            .flat_map(|dy| (-2..=2).map(move |dx| (dx, dy)))
            .filter(|(dx, dy): &(isize, isize)| dx.abs() < 2 || dy.abs() < 2)
            .map(|(dx, dy)| at(dx, dy))
            .collect();
        let mean = window.iter().sum::<f32>() / window.len() as f32;
        let variance = window
            .iter()
            .map(|value| (value - mean).powi(2))
            .sum::<f32>()
            / window.len() as f32;
        let (mut sum, mut weights) = (0.0f32, 0.0f32);
        for (dy, weight_y) in (-2..=2).zip(LEVEL_WEIGHTS) {
            for (dx, weight_x) in (-2..=2).zip(LEVEL_WEIGHTS) {
                let value = at(dx, dy);
                if value < clip_root {
                    sum += value * weight_x * weight_y;
                    weights += weight_x * weight_y;
                }
            }
        }
        let level = sum / weights.max(1.0);
        if level <= CANDIDATE_LOWEST_ROOT * clip_root {
            return None;
        }
        let [a, b] = other_channels(colour);
        Some(Candidate {
            closeness,
            deviation: variance.sqrt(),
            level: level.min(clip_root),
            others: [blocks.roots[index][a], blocks.roots[index][b]],
        })
    }
}

/// `candidates` without those another beats under every white balance: one with at least its
/// closeness and at most its deviation.
fn undominated(mut candidates: Vec<Candidate>) -> Vec<Candidate> {
    candidates.sort_by(|left, right| {
        right
            .closeness
            .total_cmp(&left.closeness)
            .then(left.deviation.total_cmp(&right.deviation))
    });
    let mut kept: Vec<Candidate> = Vec::new();
    for candidate in candidates {
        if kept
            .last()
            .is_none_or(|last| candidate.deviation < last.deviation)
        {
            kept.push(candidate);
        }
    }
    kept
}

/// Per model cell of `cells` over a `width` × `height` photo, 1 plus the segment most of the
/// segment blocks centred in it belong to, or 0. `member` holds each block's segment, `u32`
/// values beyond the segments for none.
fn cell_segments(
    blocks: &Blocks,
    member: &[u32],
    segments: &[Vec<usize>],
    [cells_x, cells_y]: [u32; 2],
    width: u32,
    height: u32,
) -> Vec<u32> {
    let (cells_x, cells_y) = (cells_x as usize, cells_y as usize);
    let (width, height) = (width as usize, height as usize);
    let mut pairs: Vec<(usize, u32)> = member
        .iter()
        .enumerate()
        .filter(|(_, segment)| (**segment as usize) < segments.len())
        .map(|(index, segment)| {
            let row = index / blocks.width;
            let x = blocks.row_offsets[row] + (index % blocks.width) * BLOCK_EDGE + 1;
            let y = row * BLOCK_EDGE + 1;
            let cell = (y * cells_y / height) * cells_x + x * cells_x / width;
            (cell, *segment)
        })
        .collect();
    pairs.par_sort_unstable();
    let mut cell_segment = vec![0u32; cells_x * cells_y];
    let mut best_count = vec![0usize; cells_x * cells_y];
    for run in pairs.chunk_by(|left, right| left == right) {
        let (cell, segment) = run[0];
        if run.len() > best_count[cell] {
            best_count[cell] = run.len();
            cell_segment[cell] = segment + 1;
        }
    }
    cell_segment
}
