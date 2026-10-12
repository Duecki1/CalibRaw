//! The nearest-neighbour field: for every patch that overlaps the hole, the
//! most similar fully known patch, found with PatchMatch.

use super::distance::chebyshev;
use super::pyramid::{Feature, Level, FEATURES};
use super::{FilledPixel, PATCH_FILL_BAND, PATCH_RADIUS};
use rayon::prelude::*;

const RADIUS: usize = PATCH_RADIUS;
/// Rows of the field searched by one task. Propagation crosses band borders
/// through the previous sweep's field, so results do not depend on threads.
const BAND_ROWS: usize = 16;
/// Upsampled matches that land on an invalid source are nudged this far, a
/// few times, before falling back to a random source.
const UPSAMPLE_JITTER: usize = 2;
const UPSAMPLE_ATTEMPTS: usize = 4;
/// Locality cost of a source one `reach` from the hole, in patch distance
/// units (mean squared perceptual difference per pixel, summed over
/// features): about a 2% colour difference.
const LOCALITY_WEIGHT: f32 = 1e-3;

/// Patch centres that may be copied from: the whole patch lies inside the
/// level and every pixel of it may be copied. Each carries a locality cost.
pub(super) struct Sources {
    valid: Vec<bool>,
    list: Vec<u32>,
    /// Added to the patch distance of a source: zero beside the hole and
    /// growing with the square of the distance from it, in units of `reach`.
    /// Like the automatic sampling area of other editors, this prefers the
    /// hole's own surroundings, while a much better match further away still
    /// wins. Without it, a distant region that only resembles the
    /// surroundings at a coarse scale (a reflection of a forest for the
    /// forest) can be copied wholesale.
    locality: Vec<f32>,
}

impl Sources {
    /// `reach` is the hole's half-thickness in level pixels, or more.
    pub(super) fn new(
        allowed: &[bool],
        hole: &[bool],
        width: usize,
        height: usize,
        reach: f32,
    ) -> Self {
        let mut valid = vec![false; width * height];
        let mut list = Vec::new();
        let edge = 2 * RADIUS + 1;
        if width < edge || height < edge {
            return Self {
                valid,
                list,
                locality: Vec::new(),
            };
        }
        // Summed-area table of pixels that may not be copied.
        let stride = width + 1;
        let mut blocked = vec![0u32; stride * (height + 1)];
        for y in 0..height {
            let mut row = 0u32;
            for x in 0..width {
                row += u32::from(!allowed[y * width + x]);
                blocked[(y + 1) * stride + x + 1] = blocked[y * stride + x + 1] + row;
            }
        }
        for y in RADIUS..height - RADIUS {
            for x in RADIUS..width - RADIUS {
                let (top, bottom) = (y - RADIUS, y + RADIUS + 1);
                let (left, right) = (x - RADIUS, x + RADIUS + 1);
                let count = blocked[bottom * stride + right] + blocked[top * stride + left]
                    - blocked[top * stride + right]
                    - blocked[bottom * stride + left];
                if count == 0 {
                    valid[y * width + x] = true;
                    list.push((y * width + x) as u32);
                }
            }
        }
        let reach = reach.max(1.0);
        let locality = chebyshev(hole, width, height, u32::MAX)
            .into_iter()
            .map(|distance| {
                let distance = (distance as f32 - (RADIUS + 1) as f32).max(0.0) / reach;
                LOCALITY_WEIGHT * distance * distance
            })
            .collect();
        Self {
            valid,
            list,
            locality,
        }
    }

    pub(super) fn len(&self) -> usize {
        self.list.len()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub(super) fn locality(&self, index: usize) -> f32 {
        self.locality[index]
    }

    fn contains(&self, index: usize) -> bool {
        self.valid[index]
    }

    fn random(&self, rng: &mut Rng) -> u32 {
        self.list[rng.below(self.list.len())]
    }

    /// Up to `count` source centres: all of them, or an evenly spread
    /// deterministic sample.
    pub(super) fn sample(&self, count: usize, seed: u64) -> Vec<usize> {
        if self.list.len() <= count {
            return self.list.iter().map(|index| *index as usize).collect();
        }
        let mut rng = Rng::new(seed, 0x0A10);
        let stride = self.list.len() as f64 / count as f64;
        (0..count)
            .map(|slot| {
                let jitter = rng.below(stride.floor().max(1.0) as usize);
                let position = (slot as f64 * stride) as usize + jitter;
                self.list[position.min(self.list.len() - 1)] as usize
            })
            .collect()
    }
}

/// How a hole pixel is re-estimated from the patches covering it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Vote {
    /// Mean weighted by match quality.
    Mean,
    /// Plain mean, for an estimate the match distances do not describe yet.
    Uniform,
    /// The best match's proposal alone.
    Best,
}

#[derive(Clone, Copy, Debug)]
struct Match {
    /// Level pixel index of the source patch centre.
    source: u32,
    /// Mean squared feature difference per pixel.
    distance: f32,
}

impl Match {
    const NONE: Self = Self {
        source: u32::MAX,
        distance: f32::INFINITY,
    };
}

/// Matches for every patch centre within [`RADIUS`] of the hole (the
/// domain), stored densely over the domain's bounding box.
pub(super) struct Field {
    left: usize,
    top: usize,
    width: usize,
    height: usize,
    level_width: usize,
    active: Vec<bool>,
    matches: Vec<Match>,
}

impl Field {
    /// A field of random sources.
    pub(super) fn random(level: &Level, seed: u64) -> Self {
        Self::initialised(level, seed, |_, _, rng| level.sources.random(rng))
    }

    /// The coarser level's field doubled in scale, with every offset kept.
    pub(super) fn upsampled(
        level: &Level,
        coarse: &Level,
        coarse_field: &Field,
        seed: u64,
    ) -> Self {
        let (width, height) = (level.width, level.height);
        Self::initialised(level, seed, |x, y, rng| {
            let (coarse_x, coarse_y) = (
                (x / 2).min(coarse.width - 1),
                (y / 2).min(coarse.height - 1),
            );
            if let Some(found) = coarse_field.get(coarse_x as isize, coarse_y as isize) {
                let source = found.source as usize;
                let (source_x, source_y) = (source % coarse.width, source / coarse.width);
                let fine_x = (2 * source_x + x - 2 * coarse_x).clamp(RADIUS, width - 1 - RADIUS);
                let fine_y = (2 * source_y + y - 2 * coarse_y).clamp(RADIUS, height - 1 - RADIUS);
                for attempt in 0..=UPSAMPLE_ATTEMPTS {
                    let (jitter_x, jitter_y) = if attempt == 0 {
                        (0, 0)
                    } else {
                        (rng.offset(UPSAMPLE_JITTER), rng.offset(UPSAMPLE_JITTER))
                    };
                    let candidate_x = (fine_x as isize + jitter_x)
                        .clamp(RADIUS as isize, (width - 1 - RADIUS) as isize)
                        as usize;
                    let candidate_y = (fine_y as isize + jitter_y)
                        .clamp(RADIUS as isize, (height - 1 - RADIUS) as isize)
                        as usize;
                    let candidate = candidate_y * width + candidate_x;
                    if level.sources.contains(candidate) {
                        return candidate as u32;
                    }
                }
            }
            level.sources.random(rng)
        })
    }

    fn initialised(
        level: &Level,
        seed: u64,
        propose: impl Fn(usize, usize, &mut Rng) -> u32 + Sync,
    ) -> Self {
        let near = chebyshev(&level.hole, level.width, level.height, RADIUS as u32 + 1);
        let in_domain = |index: usize| near[index] as usize <= RADIUS;
        // Bounding box of the domain; the caller guarantees a hole, so it is
        // never empty, and an empty one would make an empty field.
        let (left, top, right, bottom) = (0..near.len()).filter(|index| in_domain(*index)).fold(
            (level.width, level.height, 0, 0),
            |(left, top, right, bottom), index| {
                let (x, y) = (index % level.width, index / level.width);
                (left.min(x), top.min(y), right.max(x + 1), bottom.max(y + 1))
            },
        );
        let (left, top) = (left.min(right), top.min(bottom));
        let (width, height) = (right - left, bottom - top);
        let active = (0..width * height)
            .map(|index| in_domain((top + index / width) * level.width + left + index % width))
            .collect::<Vec<_>>();
        let mut matches = vec![Match::NONE; width * height];
        matches
            .par_chunks_mut(BAND_ROWS * width.max(1))
            .enumerate()
            .for_each(|(band, rows)| {
                let mut rng = Rng::new(seed, band as u64);
                let first = band * BAND_ROWS * width;
                for (offset, slot) in rows.iter_mut().enumerate() {
                    let index = first + offset;
                    if !active[index] {
                        continue;
                    }
                    let (x, y) = (left + index % width, top + index / width);
                    let source = propose(x, y, &mut rng);
                    *slot = Match {
                        source,
                        distance: level.patch_distance((x, y), source as usize, f32::INFINITY),
                    };
                }
            });
        Self {
            left,
            top,
            width,
            height,
            level_width: level.width,
            active,
            matches,
        }
    }

    /// Recomputes every match distance against the level's current features.
    pub(super) fn refresh_distances(&mut self, level: &Level) {
        let (left, top, width) = (self.left, self.top, self.width);
        let active = &self.active;
        self.matches
            .par_iter_mut()
            .enumerate()
            .filter(|(index, _)| active[*index])
            .for_each(|(index, found)| {
                let target = (left + index % width, top + index / width);
                found.distance = level.patch_distance(target, found.source as usize, f32::INFINITY);
            });
    }

    /// The match of the patch centred on level pixel `(x, y)`, if in the domain.
    fn get(&self, x: isize, y: isize) -> Option<Match> {
        let local_x = usize::try_from(x).ok()?.checked_sub(self.left)?;
        let local_y = usize::try_from(y).ok()?.checked_sub(self.top)?;
        if local_x >= self.width || local_y >= self.height {
            return None;
        }
        let index = local_y * self.width + local_x;
        self.active[index].then(|| self.matches[index])
    }

    /// One PatchMatch sweep: each patch tries the shifted matches of its
    /// already-visited neighbours (propagation), then sources sampled around
    /// its best match at exponentially shrinking radii (random search).
    /// Odd sweeps run backwards so information spreads in every direction.
    pub(super) fn search(&mut self, level: &Level, sweep: u64, seed: u64) {
        let reverse = sweep % 2 == 1;
        let step: isize = if reverse { 1 } else { -1 };
        let (left, top, width, height) = (self.left, self.top, self.width, self.height);
        let (level_width, level_height) = (level.width, level.height);
        let max_radius = level_width.max(level_height);
        let previous = self.matches.clone();
        let active = &self.active;
        self.matches
            .par_chunks_mut(BAND_ROWS * width.max(1))
            .enumerate()
            .for_each(|(band, rows)| {
                let mut rng = Rng::new(seed ^ sweep.rotate_left(29), band as u64);
                let band_top = band * BAND_ROWS;
                let band_rows = rows.len() / width;
                for row_step in 0..band_rows {
                    let local_y = if reverse {
                        band_rows - 1 - row_step
                    } else {
                        row_step
                    };
                    let domain_y = band_top + local_y;
                    for column_step in 0..width {
                        let local_x = if reverse {
                            width - 1 - column_step
                        } else {
                            column_step
                        };
                        if !active[domain_y * width + local_x] {
                            continue;
                        }
                        let target = (left + local_x, top + domain_y);
                        let mut best = rows[local_y * width + local_x];
                        let consider = |x: isize, y: isize, best: &mut Match| {
                            if x < RADIUS as isize
                                || y < RADIUS as isize
                                || x >= (level_width - RADIUS) as isize
                                || y >= (level_height - RADIUS) as isize
                            {
                                return;
                            }
                            let candidate = y as usize * level_width + x as usize;
                            if candidate as u32 == best.source || !level.sources.contains(candidate)
                            {
                                return;
                            }
                            let distance = level.patch_distance(target, candidate, best.distance);
                            if distance < best.distance {
                                *best = Match {
                                    source: candidate as u32,
                                    distance,
                                };
                            }
                        };

                        // Propagation from the neighbours visited before.
                        let neighbor_x = local_x as isize + step;
                        if (0..width as isize).contains(&neighbor_x)
                            && active[domain_y * width + neighbor_x as usize]
                        {
                            let found = rows[local_y * width + neighbor_x as usize];
                            let (x, y) = source_position(found.source, level_width);
                            consider(x - step, y, &mut best);
                        }
                        let neighbor_y = domain_y as isize + step;
                        if (0..height as isize).contains(&neighbor_y)
                            && active[neighbor_y as usize * width + local_x]
                        {
                            let neighbor_y = neighbor_y as usize;
                            let found = if (band_top..band_top + band_rows).contains(&neighbor_y) {
                                rows[(neighbor_y - band_top) * width + local_x]
                            } else {
                                previous[neighbor_y * width + local_x]
                            };
                            let (x, y) = source_position(found.source, level_width);
                            consider(x, y - step, &mut best);
                        }

                        // Random search around the best match.
                        let mut radius = max_radius;
                        while radius >= 1 {
                            let (x, y) = source_position(best.source, level_width);
                            consider(x + rng.offset(radius), y + rng.offset(radius), &mut best);
                            radius /= 2;
                        }
                        rows[local_y * width + local_x] = best;
                    }
                }
            });
    }

    /// Re-estimates every hole pixel from what the patches covering it
    /// propose. [`Vote::Mean`] averages them (Wexler et al.): patches that
    /// match well count more, with the scale set by the 75th percentile of
    /// match distances.
    /// Returns the mean absolute change of the hole's perceptual colour.
    pub(super) fn vote(&self, level: &mut Level, vote: Vote) -> f32 {
        let scale = 2.0 * self.distance_percentile(0.75).max(1e-6);
        let level_width = level.width;
        let features = &level.features;
        let updated = level
            .hole_pixels
            .par_iter()
            .map(|&index| {
                let index = index as usize;
                let (x, y) = (
                    (index % level_width) as isize,
                    (index / level_width) as isize,
                );
                let mut best: Option<(Match, (isize, isize))> = None;
                self.for_each_covering(x, y, |found, offset| {
                    if best.is_none_or(|(current, _)| found.distance < current.distance) {
                        best = Some((found, offset));
                    }
                });
                let Some((nearest, nearest_offset)) = best else {
                    return features[index];
                };
                if vote == Vote::Best {
                    return features[self.copied_pixel(nearest.source, nearest_offset)];
                }
                let mut sum = [0.0f32; FEATURES];
                let mut total = 0.0f32;
                self.for_each_covering(x, y, |found, offset| {
                    // Relative to the best covering patch, so the weights
                    // never all underflow.
                    let weight = if vote == Vote::Uniform {
                        1.0
                    } else {
                        (-(found.distance - nearest.distance) / scale).exp()
                    };
                    let feature = features[self.copied_pixel(found.source, offset)];
                    for channel in 0..FEATURES {
                        sum[channel] += feature[channel] * weight;
                    }
                    total += weight;
                });
                sum.map(|value| value / total)
            })
            .collect::<Vec<Feature>>();
        let mut change = 0.0f32;
        for (&index, feature) in level.hole_pixels.iter().zip(updated) {
            let index = index as usize;
            let guide = &mut level.guide[index];
            for channel in 0..3 {
                change += (feature[channel] - guide[channel]).abs();
                guide[channel] = feature[channel];
            }
            level.features[index] = feature;
        }
        change / (level.hole_pixels.len().max(1) * 3) as f32
    }

    /// The fill of the finest level: each pixel within the band of the hole
    /// is copied from the payload under the single best patch covering it
    /// (Newson et al.), so texture and grain arrive unaveraged.
    pub(super) fn reconstruct(
        &self,
        level: &Level,
        payload: &[[f32; 3]],
        band: &[u32],
    ) -> Vec<FilledPixel> {
        let level_width = level.width;
        let pixels = band
            .iter()
            .enumerate()
            .filter(|(_, distance)| **distance as usize <= PATCH_FILL_BAND)
            .map(|(index, distance)| (index, *distance as usize))
            .collect::<Vec<_>>();
        pixels
            .into_par_iter()
            .map(|(index, distance)| {
                let (x, y) = (index % level_width, index / level_width);
                let mut best: Option<(Match, (isize, isize))> = None;
                self.for_each_covering(x as isize, y as isize, |found, offset| {
                    if best.is_none_or(|(current, _)| found.distance < current.distance) {
                        best = Some((found, offset));
                    }
                });
                let value = best.map_or(payload[index], |(found, offset)| {
                    payload[self.copied_pixel(found.source, offset)]
                });
                FilledPixel {
                    x,
                    y,
                    distance,
                    value,
                }
            })
            .collect()
    }

    /// Visits the match of every patch covering level pixel `(x, y)`, with
    /// the pixel's offset from that patch's centre.
    fn for_each_covering(&self, x: isize, y: isize, mut visit: impl FnMut(Match, (isize, isize))) {
        let radius = RADIUS as isize;
        for offset_y in -radius..=radius {
            for offset_x in -radius..=radius {
                if let Some(found) = self.get(x - offset_x, y - offset_y) {
                    visit(found, (offset_x, offset_y));
                }
            }
        }
    }

    /// The pixel at `offset` from a source patch centre; always inside the
    /// level, since source patches lie wholly inside it.
    fn copied_pixel(&self, source: u32, offset: (isize, isize)) -> usize {
        let (x, y) = source_position(source, self.level_width);
        (y + offset.1) as usize * self.level_width + (x + offset.0) as usize
    }

    fn distance_percentile(&self, fraction: f32) -> f32 {
        let mut distances = self
            .matches
            .iter()
            .zip(&self.active)
            .filter(|(found, active)| **active && found.distance.is_finite())
            .map(|(found, _)| found.distance)
            .collect::<Vec<_>>();
        if distances.is_empty() {
            return 0.0;
        }
        let index = ((distances.len() - 1) as f32 * fraction).round() as usize;
        *distances.select_nth_unstable_by(index, f32::total_cmp).1
    }
}

impl Level {
    /// Mean squared feature difference per pixel between the patch centred
    /// on `target` and the one centred on level pixel `source`, plus the
    /// source's locality cost. Target pixels beyond the level border are left
    /// out. Returns early, with some value above `cutoff`, once the patch
    /// cannot beat it.
    fn patch_distance(&self, target: (usize, usize), source: usize, cutoff: f32) -> f32 {
        let (width, height) = (self.width, self.height);
        let (target_x, target_y) = target;
        let (source_x, source_y) = (source % width, source / width);
        let (left, right) = (
            target_x.saturating_sub(RADIUS),
            (target_x + RADIUS).min(width - 1),
        );
        let (top, bottom) = (
            target_y.saturating_sub(RADIUS),
            (target_y + RADIUS).min(height - 1),
        );
        let columns = right - left + 1;
        let count = (columns * (bottom - top + 1)) as f32;
        let locality = self.sources.locality[source];
        let limit = (cutoff - locality) * count;
        // Source pixels at the same offsets; the source patch is inside.
        let source_left = source_x + left - target_x;
        let mut sum = 0.0f32;
        for y in top..=bottom {
            let source_row = (source_y + y - target_y) * width + source_left;
            let target_row = y * width + left;
            let target = &self.features[target_row..target_row + columns];
            let source = &self.features[source_row..source_row + columns];
            for (a, b) in target.iter().zip(source) {
                for channel in 0..FEATURES {
                    let difference = a[channel] - b[channel];
                    sum += difference * difference;
                }
            }
            if sum > limit {
                break;
            }
        }
        sum / count + locality
    }
}

fn source_position(source: u32, level_width: usize) -> (isize, isize) {
    let source = source as usize;
    (
        (source % level_width) as isize,
        (source / level_width) as isize,
    )
}

/// SplitMix64: small, fast and deterministic per band.
struct Rng(u64);

impl Rng {
    fn new(seed: u64, stream: u64) -> Self {
        Self(mix(seed ^ mix(stream.wrapping_add(0x5851_F42D_4C95_7F2D))))
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        mix(self.0)
    }

    /// Uniform in `0..bound`; the modulo bias is negligible for image sizes.
    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound.max(1) as u64) as usize
    }

    /// Uniform in `-radius..=radius`.
    fn offset(&mut self, radius: usize) -> isize {
        self.below(2 * radius + 1) as isize - radius as isize
    }
}

fn mix(mut value: u64) -> u64 {
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}
