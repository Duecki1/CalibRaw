//! Re-synthesizing sensor grain over inpainted areas from nearby source texture.

use super::*;

/// The photo's grain around a pass, ready to be copied into its fill.
pub(super) struct Grain {
    bounds: NativeRect,
    /// Canonical-scene high-pass residual of the surroundings.
    residual: Vec<[f32; 3]>,
    /// Low-pass luminance, used to scale grain to the fill's brightness.
    base_luma: Vec<f32>,
    /// Top-left corners (local) of blocks that hold pure grain.
    pub(super) sources: Vec<(u32, u32)>,
    /// Origin of the block grid in the fill, so every fill block maps to one source.
    grid_origin: (u32, u32),
    seed: u64,
}

impl Grain {
    /// `None` when the surroundings hold no block of pure grain to copy.
    pub(super) fn sample(
        surroundings: &NativeSurroundings,
        unfilled: &RemoveMask,
        raw: &LoadedRaw,
        exposure: &ExposureParams,
        fill: NativeRect,
    ) -> Option<Self> {
        let bounds = surroundings.bounds;
        let (width, height) = (bounds.width as usize, bounds.height as usize);
        if surroundings.scene.len() != width * height * 3 || width == 0 || height == 0 {
            return None;
        }
        let lowpass = gaussian_blur_rgb(&surroundings.scene, width, height, GRAIN_LOWPASS_SIGMA);
        let mut residual = Vec::with_capacity(width * height);
        let mut base_luma = Vec::with_capacity(width * height);
        for (scene, low) in surroundings
            .scene
            .chunks_exact(3)
            .zip(lowpass.chunks_exact(3))
        {
            let high = [scene[0] - low[0], scene[1] - low[1], scene[2] - low[2]];
            residual.push(pipeline_scene_to_canonical_remove_scene(
                raw, exposure, high,
            ));
            base_luma.push(luma(pipeline_scene_to_canonical_remove_scene(
                raw,
                exposure,
                [low[0], low[1], low[2]],
            )));
        }

        // Pixels of the object, and a margin around it, are not grain.
        let mut excluded = vec![false; width * height];
        if let Some(overlap) = unfilled.bounds.intersect(bounds) {
            for y in overlap.y..overlap.bottom() {
                for x in overlap.x..overlap.right() {
                    if unfilled.contains_global(x, y) {
                        excluded[(y - bounds.y) as usize * width + (x - bounds.x) as usize] = true;
                    }
                }
            }
        }
        let excluded = dilate_square(&excluded, width, height, GRAIN_EXCLUSION);

        let block = GRAIN_BLOCK as usize;
        let mut candidates = Vec::new();
        for top in (0..height.saturating_sub(block - 1)).step_by(block) {
            for left in (0..width.saturating_sub(block - 1)).step_by(block) {
                let rows = top..top + block;
                if rows
                    .clone()
                    .any(|y| excluded[y * width + left..y * width + left + block].contains(&true))
                {
                    continue;
                }
                let corner = (left as u32, top as u32);
                let global = NativeRect {
                    x: bounds.x + corner.0,
                    y: bounds.y + corner.1,
                    width: GRAIN_BLOCK,
                    height: GRAIN_BLOCK,
                };
                let pixels = rows.flat_map(|y| y * width + left..y * width + left + block);
                let (energy, peak, luma_sum) =
                    pixels.fold((0.0f32, 0.0f32, 0.0f32), |acc, index| {
                        (
                            acc.0 + luma(residual[index]).abs(),
                            acc.1.max(channel_magnitude(residual[index])),
                            acc.2 + base_luma[index],
                        )
                    });
                candidates.push(GrainCandidate {
                    corner,
                    energy,
                    peak,
                    luma: luma_sum / (block * block) as f32,
                    coherence: orientation_coherence(&residual, width, left, top),
                    distance: rect_distance(global, fill),
                });
            }
        }
        let sources = select_grain_sources(candidates, &residual, width);
        (!sources.is_empty()).then(|| Self {
            bounds,
            residual,
            base_luma,
            sources,
            grid_origin: (fill.x, fill.y),
            seed: u64::from(fill.x) << 32 | u64::from(fill.y),
        })
    }

    /// Adds grain to the generated canonical-scene pixel at native `(x, y)`.
    pub(super) fn add(&self, x: u32, y: u32, generated: &mut [f32; 3]) {
        let local_x = x.wrapping_sub(self.grid_origin.0);
        let local_y = y.wrapping_sub(self.grid_origin.1);
        let block = (local_x / GRAIN_BLOCK, local_y / GRAIN_BLOCK);
        let pick = mix64(self.seed ^ (u64::from(block.0) << 32 | u64::from(block.1)));
        let (source_x, source_y) = self.sources[(pick % self.sources.len() as u64) as usize];
        let index = (source_y + local_y % GRAIN_BLOCK) as usize * self.bounds.width as usize
            + (source_x + local_x % GRAIN_BLOCK) as usize;
        // Shot noise grows with the square root of the signal.
        let scale = ((luma(*generated).max(0.0) + 1e-3) / (self.base_luma[index].max(0.0) + 1e-3))
            .sqrt()
            .clamp(0.5, 2.0);
        for (value, grain) in generated.iter_mut().zip(self.residual[index]) {
            *value += grain * scale;
        }
    }
}

/// A block of the surroundings that might be copied as grain.
#[derive(Clone, Copy, Debug)]
struct GrainCandidate {
    /// Top-left corner in surroundings-local coordinates.
    corner: (u32, u32),
    /// Sum of absolute residual luminance.
    energy: f32,
    /// Largest absolute residual on any channel.
    peak: f32,
    /// Mean low-pass luminance.
    pub(super) luma: f32,
    /// How strongly the residual is oriented one way (0 = grain, 1 = a line).
    coherence: f32,
    /// Native distance to the fill.
    pub(super) distance: u32,
}

/// Picks the blocks that hold only the photo's grain, from material like the
/// area around the hole, nearest first.
fn select_grain_sources(
    candidates: Vec<GrainCandidate>,
    residual: &[[f32; 3]],
    width: usize,
) -> Vec<(u32, u32)> {
    let near = candidates
        .iter()
        .filter(|candidate| candidate.distance <= GRAIN_NEAR_DISTANCE)
        .map(|candidate| candidate.luma)
        .collect::<Vec<_>>();
    let reference = median(if near.is_empty() {
        candidates.iter().map(|candidate| candidate.luma).collect()
    } else {
        near
    });
    let Some(reference) = reference else {
        return Vec::new();
    };
    let similar = candidates
        .into_iter()
        .filter(|candidate| {
            let ratio = candidate.luma.max(1e-6) / reference.max(1e-6);
            (1.0 / GRAIN_MAX_BRIGHTNESS_RATIO..=GRAIN_MAX_BRIGHTNESS_RATIO).contains(&ratio)
        })
        .collect::<Vec<_>>();
    let Some(energy) = median(similar.iter().map(|candidate| candidate.energy).collect()) else {
        return Vec::new();
    };
    let peak_limit = noise_deviation(&similar, residual, width) * GRAIN_MAX_PEAK_DEVIATIONS;
    let mut sources = similar
        .into_iter()
        .filter(|candidate| {
            candidate.energy <= energy * GRAIN_MAX_ENERGY_RATIO
                && candidate.peak <= peak_limit
                && candidate.coherence <= GRAIN_MAX_COHERENCE
        })
        .collect::<Vec<_>>();
    sources.sort_by_key(|candidate| candidate.distance);
    sources.truncate(GRAIN_MAX_SOURCES);
    sources
        .into_iter()
        .map(|candidate| candidate.corner)
        .collect()
}

/// Robust noise deviation of the candidate blocks: the median absolute
/// residual over their pixels, which features too sparse to move the median
/// cannot inflate.
fn noise_deviation(candidates: &[GrainCandidate], residual: &[[f32; 3]], width: usize) -> f32 {
    let block = GRAIN_BLOCK as usize;
    let magnitudes = candidates
        .iter()
        .flat_map(|candidate| {
            let (left, top) = (candidate.corner.0 as usize, candidate.corner.1 as usize);
            (top..top + block)
                .flat_map(move |y| &residual[y * width + left..y * width + left + block])
        })
        .map(|value| channel_magnitude(*value))
        .collect();
    // 1.4826 turns a median absolute deviation into a standard deviation.
    median(magnitudes).unwrap_or(0.0) * 1.4826
}

/// Structure-tensor coherence of the residual luminance in one block:
/// `((Jxx - Jyy)^2 + 4 Jxy^2) / (Jxx + Jyy)^2`.
pub(super) fn orientation_coherence(
    residual: &[[f32; 3]],
    width: usize,
    left: usize,
    top: usize,
) -> f32 {
    let block = GRAIN_BLOCK as usize;
    let at = |x: usize, y: usize| luma(residual[y * width + x]);
    let (mut xx, mut yy, mut xy) = (0.0f32, 0.0f32, 0.0f32);
    for y in top + 1..top + block - 1 {
        for x in left + 1..left + block - 1 {
            let gradient_x = at(x + 1, y) - at(x - 1, y);
            let gradient_y = at(x, y + 1) - at(x, y - 1);
            xx += gradient_x * gradient_x;
            yy += gradient_y * gradient_y;
            xy += gradient_x * gradient_y;
        }
    }
    let trace = xx + yy;
    if trace <= f32::EPSILON {
        return 0.0;
    }
    ((xx - yy).powi(2) + 4.0 * xy * xy) / (trace * trace)
}

fn median(mut values: Vec<f32>) -> Option<f32> {
    if values.is_empty() {
        return None;
    }
    let middle = values.len() / 2;
    Some(*values.select_nth_unstable_by(middle, f32::total_cmp).1)
}

fn channel_magnitude(value: [f32; 3]) -> f32 {
    value
        .iter()
        .fold(0.0f32, |max, channel| max.max(channel.abs()))
}

/// Gap in native pixels between two rectangles; zero when they touch.
fn rect_distance(a: NativeRect, b: NativeRect) -> u32 {
    let gap = |start_a: u32, end_a: u32, start_b: u32, end_b: u32| {
        start_b
            .saturating_sub(end_a)
            .max(start_a.saturating_sub(end_b))
    };
    gap(a.x, a.right(), b.x, b.right()).max(gap(a.y, a.bottom(), b.y, b.bottom()))
}

pub(super) fn luma(rgb: [f32; 3]) -> f32 {
    rgb[0] * 0.2627 + rgb[1] * 0.6780 + rgb[2] * 0.0593
}

/// SplitMix64 finalizer: a well-mixed, deterministic hash.
pub(super) fn mix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}
