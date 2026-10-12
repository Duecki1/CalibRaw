//! Priority-order initialisation of the coarsest level (Criminisi, Pérez and
//! Toyama, "Region Filling and Object Removal by Exemplar-Based Image
//! Inpainting", 2004).
//!
//! The hole is filled patch by patch from its edge. The next patch is the one
//! on the edge with the most known pixels around it and the strongest
//! structure (an isophote, such as a shoreline) running into the hole, so
//! lines continue across the hole before texture fills in around them. Each
//! copies the source patch that best matches its known pixels, so the first
//! estimate carries real texture: a smooth start would instead draw every
//! match towards the flattest region of the photo.

use super::pyramid::{Level, FEATURES};
use super::PATCH_RADIUS;
use rayon::prelude::*;

const RADIUS: isize = PATCH_RADIUS as isize;
/// Most (patch, source) comparisons; beyond it each patch compares a fixed
/// sample of sources, and PatchMatch sweeps refine the result afterwards.
const COMPARISON_BUDGET: usize = 10_000_000;
const MIN_SAMPLED_SOURCES: usize = 512;
/// Keeps edges without structure in the queue, ordered by confidence alone.
const DATA_FLOOR: f32 = 0.01;

pub(super) fn initialise(level: &mut Level, seed: u64) {
    let mut fill = Filling::new(level);
    // Each copy fills a third of a patch on average.
    let patches = (fill.unknown * 3 / (2 * PATCH_RADIUS + 1).pow(2)).max(1);
    let sources = level
        .sources
        .sample((COMPARISON_BUDGET / patches).max(MIN_SAMPLED_SOURCES), seed);
    while let Some(target) = fill.next(level) {
        let best = sources
            .par_iter()
            .map(|&source| (source, fill.masked_distance(level, target, source)))
            .min_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)))
            .map(|(source, _)| source);
        let Some(source) = best else {
            break;
        };
        fill.copy(level, target, source);
    }
}

struct Filling {
    width: usize,
    height: usize,
    known: Vec<bool>,
    /// Criminisi's confidence: 1 for original pixels, inherited by copies.
    confidence: Vec<f32>,
    /// Unknown pixels next to a known one, possibly stale (checked on use).
    front: Vec<usize>,
    /// Cached priority per pixel; NaN once a nearby copy changed it.
    priority: Vec<f32>,
    unknown: usize,
}

impl Filling {
    fn new(level: &Level) -> Self {
        let known = level.hole.iter().map(|hole| !hole).collect::<Vec<_>>();
        let confidence = known
            .iter()
            .map(|known| f32::from(u8::from(*known)))
            .collect();
        let mut filling = Self {
            width: level.width,
            height: level.height,
            known,
            confidence,
            front: Vec::new(),
            priority: vec![f32::NAN; level.width * level.height],
            unknown: level.hole_pixels.len(),
        };
        filling.front = level
            .hole_pixels
            .iter()
            .map(|index| *index as usize)
            .filter(|index| filling.on_front(*index))
            .collect();
        filling
    }

    fn position(&self, index: usize) -> (isize, isize) {
        ((index % self.width) as isize, (index / self.width) as isize)
    }

    fn index(&self, x: isize, y: isize) -> Option<usize> {
        (x >= 0 && y >= 0 && (x as usize) < self.width && (y as usize) < self.height)
            .then(|| y as usize * self.width + x as usize)
    }

    fn on_front(&self, index: usize) -> bool {
        if self.known[index] {
            return false;
        }
        let (x, y) = self.position(index);
        (-1..=1).any(|dy| {
            (-1..=1).any(|dx| {
                self.index(x + dx, y + dy)
                    .is_some_and(|neighbor| self.known[neighbor])
            })
        })
    }

    /// The front pixel with the highest priority, or `None` once filled.
    fn next(&mut self, level: &Level) -> Option<usize> {
        let mut front = std::mem::take(&mut self.front);
        front.retain(|index| !self.known[*index]);
        front.sort_unstable();
        front.dedup();
        for &index in &front {
            if self.priority[index].is_nan() {
                self.priority[index] = self.priority(level, index);
            }
        }
        let best = front
            .iter()
            .map(|&index| (index, self.priority[index]))
            .max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0)))
            .map(|(index, _)| index);
        self.front = front;
        best
    }

    /// Confidence (share of trusted pixels in the patch) times the data term
    /// (structure flowing into the hole across the front).
    fn priority(&self, level: &Level, index: usize) -> f32 {
        let (x, y) = self.position(index);
        let mut confidence = 0.0f32;
        let mut isophote = [0.0f32; 2];
        let mut strongest = 0.0f32;
        let mut normal = [0.0f32; 2];
        for dy in -RADIUS..=RADIUS {
            for dx in -RADIUS..=RADIUS {
                let Some(pixel) = self.index(x + dx, y + dy) else {
                    continue;
                };
                if !self.known[pixel] {
                    continue;
                }
                confidence += self.confidence[pixel];
                // The known region's outward gradient is the front's normal.
                normal[0] -= dx as f32;
                normal[1] -= dy as f32;
                if let Some(gradient) = self.gradient(level, x + dx, y + dy) {
                    let magnitude = gradient[0].hypot(gradient[1]);
                    if magnitude > strongest {
                        strongest = magnitude;
                        isophote = [-gradient[1], gradient[0]];
                    }
                }
            }
        }
        let area = ((2 * RADIUS + 1) * (2 * RADIUS + 1)) as f32;
        let length = normal[0].hypot(normal[1]);
        let data = if length > 0.0 {
            (isophote[0] * normal[0] + isophote[1] * normal[1]).abs() / length
        } else {
            0.0
        };
        confidence / area * (data + DATA_FLOOR)
    }

    /// Luma gradient by central differences, where all four neighbours are known.
    fn gradient(&self, level: &Level, x: isize, y: isize) -> Option<[f32; 2]> {
        let luma = |x: isize, y: isize| {
            let index = self.index(x, y).filter(|index| self.known[*index])?;
            let rgb = level.guide[index];
            Some(0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2])
        };
        Some([
            (luma(x + 1, y)? - luma(x - 1, y)?) * 0.5,
            (luma(x, y + 1)? - luma(x, y - 1)?) * 0.5,
        ])
    }

    /// Mean squared feature difference over the known pixels of the patch
    /// centred on `target`, against the same offsets around `source`, plus the
    /// source's locality cost.
    fn masked_distance(&self, level: &Level, target: usize, source: usize) -> f32 {
        let (x, y) = self.position(target);
        let width = self.width as isize;
        let mut sum = 0.0f32;
        let mut count = 0usize;
        for dy in -RADIUS..=RADIUS {
            for dx in -RADIUS..=RADIUS {
                let Some(pixel) = self.index(x + dx, y + dy) else {
                    continue;
                };
                if !self.known[pixel] {
                    continue;
                }
                // Source patches lie wholly inside the level.
                let copied = (source as isize + dy * width + dx) as usize;
                let (a, b) = (level.features[pixel], level.features[copied]);
                for channel in 0..FEATURES {
                    let difference = a[channel] - b[channel];
                    sum += difference * difference;
                }
                count += 1;
            }
        }
        if count == 0 {
            f32::INFINITY
        } else {
            sum / count as f32 + level.sources.locality(source)
        }
    }

    /// Fills the unknown pixels of the patch at `target` from `source`.
    fn copy(&mut self, level: &mut Level, target: usize, source: usize) {
        let (x, y) = self.position(target);
        let width = self.width as isize;
        let area = ((2 * RADIUS + 1) * (2 * RADIUS + 1)) as f32;
        let confidence = (-RADIUS..=RADIUS)
            .flat_map(|dy| (-RADIUS..=RADIUS).map(move |dx| (dx, dy)))
            .filter_map(|(dx, dy)| self.index(x + dx, y + dy))
            .map(|pixel| self.confidence[pixel])
            .sum::<f32>()
            / area;
        for dy in -RADIUS..=RADIUS {
            for dx in -RADIUS..=RADIUS {
                let Some(pixel) = self.index(x + dx, y + dy) else {
                    continue;
                };
                if self.known[pixel] {
                    continue;
                }
                let copied = (source as isize + dy * width + dx) as usize;
                let feature = level.features[copied];
                level.features[pixel] = feature;
                level.guide[pixel] = [feature[0], feature[1], feature[2]];
                self.known[pixel] = true;
                self.confidence[pixel] = confidence;
                self.unknown -= 1;
            }
        }
        // Pixels just outside the copied patch may have joined the front, and
        // every patch overlapping it has a new priority.
        let reach = 2 * RADIUS + 2;
        for dy in -reach..=reach {
            for dx in -reach..=reach {
                if let Some(pixel) = self.index(x + dx, y + dy) {
                    self.priority[pixel] = f32::NAN;
                    if dx.abs() <= RADIUS + 1 && dy.abs() <= RADIUS + 1 && self.on_front(pixel) {
                        self.front.push(pixel);
                    }
                }
            }
        }
    }
}
