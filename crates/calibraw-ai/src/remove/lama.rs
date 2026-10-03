//! Big-LaMa inference for one Remove pass.
//!
//! The ONNX export takes a fixed 512x512 image in [0, 1] with a binary mask,
//! masks the image itself, and returns `mask * prediction + (1 - mask) *
//! image` scaled to [0, 255]. A pass maps its native crop onto that input, so
//! a 512px crop runs without any resampling.
//!
//! Sensor noise is handled outside the model: given a noisy context, LaMa's
//! Fourier convolutions turn the grain into a periodic pattern. The model
//! therefore sees a denoised context and fills structure only, and the
//! photo's own grain is copied back in from the surroundings at native
//! resolution.

use super::*;

/// Native pixels feathered outside the filled target, where the model also
/// generated content, so the patch blends into the photo without a seam.
const MIN_FEATHER: f32 = 2.0;
const FEATHER_PER_MODEL_PIXEL: f32 = 1.5;
/// The view gain is metered on context pixels only; with fewer than this many
/// the whole crop is used.
const MIN_METERED_PIXELS: usize = 64;
/// Native ring around a pass whose grain is copied into the fill.
const GRAIN_RING: u32 = 96;
/// Grain is copied in blocks of this edge; sensor noise decorrelates within a
/// few pixels, so block seams are invisible.
const GRAIN_BLOCK: u32 = 16;
/// Gaussian that separates grain (the residual) from image structure.
const GRAIN_LOWPASS_SIGMA: f32 = 2.5;
/// Native pixels kept between grain sources and the object being removed.
const GRAIN_EXCLUSION: usize = 4;
/// Blocks with more high-frequency energy than this multiple of the median
/// contain edges or texture rather than grain and are not copied.
const GRAIN_MAX_ENERGY_RATIO: f32 = 1.5;
/// Blocks with any pixel further than this many noise deviations from the
/// local mean hold a feature (a hair, a crease, an edge) rather than grain.
/// Thin or curved features barely raise a block's average energy or its
/// orientation, so this catches what the other limits miss; pure Gaussian
/// grain exceeds it in about one block in ten, which leaves plenty to copy.
const GRAIN_MAX_PEAK_DEVIATIONS: f32 = 3.5;
/// Blocks whose gradients point this consistently one way hold a line (a
/// crease, a hair, an edge fragment); grain has no preferred direction.
/// Measured as structure-tensor coherence in [0, 1]: pure grain blocks score
/// 0.007 at the median and 0.04 at the 95th percentile, so this keeps most
/// grain while rejecting any line visible above it.
const GRAIN_MAX_COHERENCE: f32 = 0.05;
/// Grain is only taken from blocks this much brighter or darker than the
/// area right around the hole at most. Other materials (a dark strap next to
/// skin) carry different texture, and grain scaled up from a much darker
/// block turns faint structure into bright scratches.
const GRAIN_MAX_BRIGHTNESS_RATIO: f32 = 1.5;
/// Native distance from the hole that defines "right around the hole".
const GRAIN_NEAR_DISTANCE: u32 = 48;
/// Only the nearest qualifying blocks are used: they share the fill's focus,
/// lighting and material.
const GRAIN_MAX_SOURCES: usize = 64;
/// Edge-preserving denoise of the model input: spatial radius and sigma in
/// model pixels, range sigma as a multiple of the estimated noise.
const DENOISE_RADIUS: usize = 3;
const DENOISE_SPATIAL_SIGMA: f32 = 1.5;
const DENOISE_RANGE_PER_NOISE: f32 = 2.0;

/// Native pixels around a pass, rendered like the model's context, that the
/// photo's grain is sampled from.
pub(super) struct NativeSurroundings {
    pub bounds: NativeRect,
    /// Interleaved pipeline-scene RGB.
    pub scene: Vec<f32>,
}

/// The native region to render as [`NativeSurroundings`] for `pass`.
pub(super) fn surroundings_region(
    pass: &RemovePass,
    image_width: u32,
    image_height: u32,
) -> NativeRect {
    let margin = PassGeometry::new(pass.crop).feather().ceil() as u32 + 1 + GRAIN_RING;
    let bounds = pass.target.bounds;
    let x = bounds.x.saturating_sub(margin);
    let y = bounds.y.saturating_sub(margin);
    NativeRect {
        x,
        y,
        width: (bounds.right() + margin).min(image_width) - x,
        height: (bounds.bottom() + margin).min(image_height) - y,
    }
}

/// Fills `pass.target` from `scene`, a render of `pass.crop`. `unfilled` holds
/// every stroke pixel not filled yet; all of them inside the crop stay masked,
/// so the model never sees the object it removes.
pub(super) fn infer_pass(
    model_path: &Path,
    pass: &RemovePass,
    unfilled: &RemoveMask,
    raw: &LoadedRaw,
    exposure: &ExposureParams,
    scene: &ResizedRemoveSceneCrop,
    surroundings: &NativeSurroundings,
) -> Result<RemovePatch> {
    let geometry = PassGeometry::new(pass.crop);
    let model_mask = geometry.model_mask(unfilled);
    anyhow::ensure!(
        model_mask.iter().any(|masked| *masked),
        "Remove mask vanished at the model resolution"
    );
    let scene = scene_at_model_resolution(scene)?;
    let view_gain = metered_view_gain(raw, &scene, &model_mask);

    let edge = BIG_LAMA_INPUT_EDGE as usize;
    let plane = edge * edge;
    let mut image_values = vec![0.0f32; plane * 3];
    for (index, pixel) in scene.pixels().enumerate() {
        let srgb = remove_scene_to_model_srgb(raw, [pixel[0], pixel[1], pixel[2]], view_gain);
        for channel in 0..3 {
            image_values[channel * plane + index] = srgb[channel].clamp(0.0, 1.0);
        }
    }
    denoise_model_input(&mut image_values, &model_mask);
    let mask_values = model_mask
        .iter()
        .map(|masked| f32::from(u8::from(*masked)))
        .collect::<Vec<_>>();
    let output = run_big_lama(model_path, image_values, mask_values)?;

    let alpha = geometry.feather_alpha(&pass.target, &model_mask);
    let bounds = alpha.bounds;
    let grain = Grain::sample(surroundings, unfilled, raw, exposure, bounds);
    let mut rgb16f = Vec::with_capacity(bounds.width as usize * bounds.height as usize * 3);
    for y in bounds.y..bounds.bottom() {
        for x in bounds.x..bounds.right() {
            let [model_x, model_y] = geometry.native_to_model(x, y);
            let srgb = sample_catmull_rom(&output, model_x, model_y).map(|v| v.clamp(0.0, 1.0));
            let mut generated =
                remove_model_srgb_to_canonical_scene(raw, exposure, srgb, view_gain);
            if let Some(grain) = &grain {
                grain.add(x, y, &mut generated);
            }
            for value in generated {
                let finite = if value.is_finite() { value } else { 0.0 };
                rgb16f.push(half::f16::from_f32(finite.clamp(-65_504.0, 65_504.0)).to_bits());
            }
        }
    }
    RemovePatch::new_scene(bounds, rgb16f, alpha.pixels).map_err(anyhow::Error::msg)
}

/// Maps a native crop onto the square model input.
#[derive(Clone, Copy, Debug)]
struct PassGeometry {
    crop: NativeRect,
    /// Native pixels per model pixel along each axis.
    scale: [f32; 2],
}

impl PassGeometry {
    fn new(crop: NativeRect) -> Self {
        let edge = BIG_LAMA_INPUT_EDGE as f32;
        Self {
            crop,
            scale: [
                crop.width.max(1) as f32 / edge,
                crop.height.max(1) as f32 / edge,
            ],
        }
    }

    /// Feather width in native pixels: wider when the model ran downscaled,
    /// so the softer upsampled fill fades in over a few of its own pixels.
    fn feather(self) -> f32 {
        (FEATHER_PER_MODEL_PIXEL * self.scale[0].max(self.scale[1])).max(MIN_FEATHER)
    }

    /// Continuous model coordinates of a native pixel centre.
    fn native_to_model(self, x: u32, y: u32) -> [f32; 2] {
        [
            ((x - self.crop.x) as f32 + 0.5) / self.scale[0] - 0.5,
            ((y - self.crop.y) as f32 + 0.5) / self.scale[1] - 0.5,
        ]
    }

    fn model_index(self, x: u32, y: u32) -> usize {
        let edge = BIG_LAMA_INPUT_EDGE as usize;
        let model_x = (((x - self.crop.x) as f32 / self.scale[0]) as usize).min(edge - 1);
        let model_y = (((y - self.crop.y) as f32 / self.scale[1]) as usize).min(edge - 1);
        model_y * edge + model_x
    }

    /// Model-resolution mask: a model pixel is masked when any native pixel
    /// it covers is unfilled, so no fragment of the object stays visible.
    /// It is then grown by the feather width, which the model fills too.
    fn model_mask(self, unfilled: &RemoveMask) -> Vec<bool> {
        let edge = BIG_LAMA_INPUT_EDGE as usize;
        let mut mask = vec![false; edge * edge];
        if let Some(overlap) = unfilled.bounds.intersect(self.crop) {
            for y in overlap.y..overlap.bottom() {
                for x in overlap.x..overlap.right() {
                    if unfilled.contains_global(x, y) {
                        mask[self.model_index(x, y)] = true;
                    }
                }
            }
        }
        let radius = (self.feather() / self.scale[0].min(self.scale[1]))
            .ceil()
            .max(1.0) as usize;
        dilate_square(&mask, edge, edge, radius)
    }

    /// Alpha for the patch: opaque on the target and fading to zero over the
    /// feather width outside it, never beyond what the model generated.
    fn feather_alpha(self, target: &RemoveMask, model_mask: &[bool]) -> FeatherAlpha {
        let feather = self.feather();
        let margin = feather.ceil() as u32 + 1;
        let region = NativeRect {
            x: target.bounds.x.saturating_sub(margin),
            y: target.bounds.y.saturating_sub(margin),
            width: target.bounds.width + 2 * margin,
            height: target.bounds.height + 2 * margin,
        }
        .intersect(self.crop)
        .unwrap_or(target.bounds);
        let distance = chamfer_distance(region, target);
        let mut alpha = vec![0u8; distance.len()];
        for (index, (value, distance)) in alpha.iter_mut().zip(&distance).enumerate() {
            let x = region.x + (index % region.width as usize) as u32;
            let y = region.y + (index / region.width as usize) as u32;
            if !model_mask[self.model_index(x, y)] {
                continue;
            }
            let coverage = (1.0 - distance / (feather + 1.0)).clamp(0.0, 1.0);
            *value = (coverage * 255.0).round() as u8;
        }
        FeatherAlpha::shrunk(region, alpha)
    }
}

struct FeatherAlpha {
    bounds: NativeRect,
    pixels: Vec<u8>,
}

impl FeatherAlpha {
    fn shrunk(region: NativeRect, alpha: Vec<u8>) -> Self {
        let width = region.width as usize;
        let (mut left, mut top, mut right, mut bottom) = (usize::MAX, usize::MAX, 0, 0);
        for (index, value) in alpha.iter().enumerate() {
            if *value != 0 {
                let (x, y) = (index % width, index / width);
                left = left.min(x);
                top = top.min(y);
                right = right.max(x + 1);
                bottom = bottom.max(y + 1);
            }
        }
        if right <= left {
            return Self {
                bounds: NativeRect::default(),
                pixels: Vec::new(),
            };
        }
        let mut pixels = Vec::with_capacity((right - left) * (bottom - top));
        for y in top..bottom {
            pixels.extend_from_slice(&alpha[y * width + left..y * width + right]);
        }
        Self {
            bounds: NativeRect {
                x: region.x + left as u32,
                y: region.y + top as u32,
                width: (right - left) as u32,
                height: (bottom - top) as u32,
            },
            pixels,
        }
    }
}

/// Approximate Euclidean distance (3-4 chamfer) in native pixels from each
/// pixel of `region` to the nearest target pixel.
fn chamfer_distance(region: NativeRect, target: &RemoveMask) -> Vec<f32> {
    const ORTHOGONAL: u32 = 3;
    const DIAGONAL: u32 = 4;
    let width = region.width as usize;
    let height = region.height as usize;
    let mut distance = vec![u32::MAX / 2; width * height];
    for y in 0..height {
        for x in 0..width {
            if target.contains_global(region.x + x as u32, region.y + y as u32) {
                distance[y * width + x] = 0;
            }
        }
    }
    let relax = |distance: &mut [u32], index: usize, neighbor: usize, step: u32| {
        let candidate = distance[neighbor] + step;
        if candidate < distance[index] {
            distance[index] = candidate;
        }
    };
    for y in 0..height {
        for x in 0..width {
            let index = y * width + x;
            if x > 0 {
                relax(&mut distance, index, index - 1, ORTHOGONAL);
            }
            if y > 0 {
                relax(&mut distance, index, index - width, ORTHOGONAL);
                if x > 0 {
                    relax(&mut distance, index, index - width - 1, DIAGONAL);
                }
                if x + 1 < width {
                    relax(&mut distance, index, index - width + 1, DIAGONAL);
                }
            }
        }
    }
    for y in (0..height).rev() {
        for x in (0..width).rev() {
            let index = y * width + x;
            if x + 1 < width {
                relax(&mut distance, index, index + 1, ORTHOGONAL);
            }
            if y + 1 < height {
                relax(&mut distance, index, index + width, ORTHOGONAL);
                if x + 1 < width {
                    relax(&mut distance, index, index + width + 1, DIAGONAL);
                }
                if x > 0 {
                    relax(&mut distance, index, index + width - 1, DIAGONAL);
                }
            }
        }
    }
    distance
        .into_iter()
        .map(|value| value as f32 / ORTHOGONAL as f32)
        .collect()
}

/// The photo's grain around a pass, ready to be copied into its fill.
struct Grain {
    bounds: NativeRect,
    /// Canonical-scene high-pass residual of the surroundings.
    residual: Vec<[f32; 3]>,
    /// Low-pass luminance, used to scale grain to the fill's brightness.
    base_luma: Vec<f32>,
    /// Top-left corners (local) of blocks that hold pure grain.
    sources: Vec<(u32, u32)>,
    /// Origin of the block grid in the fill, so every fill block maps to one source.
    grid_origin: (u32, u32),
    seed: u64,
}

impl Grain {
    /// `None` when the surroundings hold no block of pure grain to copy.
    fn sample(
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
    fn add(&self, x: u32, y: u32, generated: &mut [f32; 3]) {
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
    luma: f32,
    /// How strongly the residual is oriented one way (0 = grain, 1 = a line).
    coherence: f32,
    /// Native distance to the fill.
    distance: u32,
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
fn orientation_coherence(residual: &[[f32; 3]], width: usize, left: usize, top: usize) -> f32 {
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

fn luma(rgb: [f32; 3]) -> f32 {
    rgb[0] * 0.2627 + rgb[1] * 0.6780 + rgb[2] * 0.0593
}

/// SplitMix64 finalizer: a well-mixed, deterministic hash.
fn mix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

/// Separable Gaussian blur of interleaved RGB with clamped edges.
fn gaussian_blur_rgb(rgb: &[f32], width: usize, height: usize, sigma: f32) -> Vec<f32> {
    let radius = (sigma * 3.0).ceil() as isize;
    let kernel = (-radius..=radius)
        .map(|offset| (-(offset * offset) as f32 / (2.0 * sigma * sigma)).exp())
        .collect::<Vec<_>>();
    let total = kernel.iter().sum::<f32>();
    let kernel = kernel
        .iter()
        .map(|weight| weight / total)
        .collect::<Vec<_>>();
    let pass = |source: &[f32], horizontal: bool| {
        let mut out = vec![0.0f32; source.len()];
        for y in 0..height {
            for x in 0..width {
                let mut sum = [0.0f32; 3];
                for (tap, weight) in kernel.iter().enumerate() {
                    let offset = tap as isize - radius;
                    let (sample_x, sample_y) = if horizontal {
                        (
                            (x as isize + offset).clamp(0, width as isize - 1) as usize,
                            y,
                        )
                    } else {
                        (
                            x,
                            (y as isize + offset).clamp(0, height as isize - 1) as usize,
                        )
                    };
                    let index = (sample_y * width + sample_x) * 3;
                    for channel in 0..3 {
                        sum[channel] += source[index + channel] * weight;
                    }
                }
                out[(y * width + x) * 3..(y * width + x) * 3 + 3].copy_from_slice(&sum);
            }
        }
        out
    };
    pass(&pass(rgb, true), false)
}

/// Edge-preserving bilateral denoise of the planar model input, over context
/// pixels only. The range sigma follows the measured noise, so clean images
/// are left nearly untouched while grain is removed without softening edges.
fn denoise_model_input(planar: &mut [f32], model_mask: &[bool]) {
    let edge = BIG_LAMA_INPUT_EDGE as usize;
    let plane = edge * edge;
    let luma_at = |values: &[f32], index: usize| {
        luma([
            values[index],
            values[plane + index],
            values[2 * plane + index],
        ])
    };
    // Robust noise estimate: median absolute deviation from the 3x3 mean.
    let mut deviations = Vec::new();
    for y in 1..edge - 1 {
        for x in 1..edge - 1 {
            let index = y * edge + x;
            if model_mask[index] {
                continue;
            }
            let mean = (0..9)
                .map(|tap| luma_at(planar, (y + tap / 3 - 1) * edge + x + tap % 3 - 1))
                .sum::<f32>()
                / 9.0;
            deviations.push((luma_at(planar, index) - mean).abs());
        }
    }
    if deviations.is_empty() {
        return;
    }
    let middle = deviations.len() / 2;
    let median = *deviations.select_nth_unstable_by(middle, f32::total_cmp).1;
    // 1.4826 turns a MAD into a standard deviation; 1.06 undoes the 3x3 mean.
    let noise = median * 1.4826 * 1.06;
    let range_sigma = (noise * DENOISE_RANGE_PER_NOISE).max(1e-3);

    let source = planar.to_vec();
    let spatial = (0..=2 * DENOISE_RADIUS)
        .map(|offset| {
            let distance = offset as f32 - DENOISE_RADIUS as f32;
            (-(distance * distance) / (2.0 * DENOISE_SPATIAL_SIGMA * DENOISE_SPATIAL_SIGMA)).exp()
        })
        .collect::<Vec<_>>();
    for y in 0..edge {
        for x in 0..edge {
            let index = y * edge + x;
            if model_mask[index] {
                continue;
            }
            let center = [
                source[index],
                source[plane + index],
                source[2 * plane + index],
            ];
            let mut sum = [0.0f32; 3];
            let mut total = 0.0f32;
            for neighbor_y in y.saturating_sub(DENOISE_RADIUS)..(y + DENOISE_RADIUS + 1).min(edge) {
                for neighbor_x in
                    x.saturating_sub(DENOISE_RADIUS)..(x + DENOISE_RADIUS + 1).min(edge)
                {
                    let neighbor = neighbor_y * edge + neighbor_x;
                    if model_mask[neighbor] {
                        continue;
                    }
                    let value = [
                        source[neighbor],
                        source[plane + neighbor],
                        source[2 * plane + neighbor],
                    ];
                    // Mean over channels, comparable to the per-channel noise.
                    let difference = (0..3)
                        .map(|channel| (value[channel] - center[channel]).powi(2))
                        .sum::<f32>()
                        / 3.0;
                    let weight = spatial[neighbor_x + DENOISE_RADIUS - x]
                        * spatial[neighbor_y + DENOISE_RADIUS - y]
                        * (-difference / (2.0 * range_sigma * range_sigma)).exp();
                    for channel in 0..3 {
                        sum[channel] += value[channel] * weight;
                    }
                    total += weight;
                }
            }
            for channel in 0..3 {
                planar[channel * plane + index] = sum[channel] / total;
            }
        }
    }
}

/// Grows a binary mask by `radius` pixels in every direction (square
/// structuring element, applied separably).
fn dilate_square(mask: &[bool], width: usize, height: usize, radius: usize) -> Vec<bool> {
    let mut rows = vec![false; mask.len()];
    for y in 0..height {
        let row = &mask[y * width..(y + 1) * width];
        for x in 0..width {
            let start = x.saturating_sub(radius);
            let end = (x + radius + 1).min(width);
            rows[y * width + x] = row[start..end].iter().any(|value| *value);
        }
    }
    let mut out = vec![false; mask.len()];
    for x in 0..width {
        for y in 0..height {
            let start = y.saturating_sub(radius);
            let end = (y + radius + 1).min(height);
            out[y * width + x] = (start..end).any(|row| rows[row * width + x]);
        }
    }
    out
}

/// The rendered crop as a 512x512 linear scene image.
fn scene_at_model_resolution(scene: &ResizedRemoveSceneCrop) -> Result<Rgb32FImage> {
    anyhow::ensure!(
        scene.width <= BIG_LAMA_INPUT_EDGE
            && scene.height <= BIG_LAMA_INPUT_EDGE
            && scene.pixels.len() == scene.width as usize * scene.height as usize * 3,
        "Remove working scene {}x{} is invalid",
        scene.width,
        scene.height,
    );
    let image: Rgb32FImage = ImageBuffer::from_raw(scene.width, scene.height, scene.pixels.clone())
        .context("construct Remove working scene")?;
    if scene.width == BIG_LAMA_INPUT_EDGE && scene.height == BIG_LAMA_INPUT_EDGE {
        return Ok(image);
    }
    Ok(image::imageops::resize(
        &image,
        BIG_LAMA_INPUT_EDGE,
        BIG_LAMA_INPUT_EDGE,
        FilterType::Lanczos3,
    ))
}

/// Exposure for the model's view, metered on the context the model keeps so
/// the brightness of the removed object does not shift it.
fn metered_view_gain(raw: &LoadedRaw, scene: &Rgb32FImage, model_mask: &[bool]) -> f32 {
    let context = scene
        .pixels()
        .zip(model_mask)
        .filter(|(_, masked)| !**masked)
        .flat_map(|(pixel, _)| pixel.0)
        .collect::<Vec<_>>();
    if context.len() / 3 >= MIN_METERED_PIXELS {
        remove_model_view_gain(raw, &context)
    } else {
        remove_model_view_gain(raw, scene.as_raw())
    }
}

fn run_big_lama(
    model_path: &Path,
    image_values: Vec<f32>,
    mask_values: Vec<f32>,
) -> Result<Rgb32FImage> {
    let edge = BIG_LAMA_INPUT_EDGE as usize;
    let plane = edge * edge;
    let image_tensor = Tensor::from_array(([1usize, 3, edge, edge], image_values))
        .context("create Big-LaMa image tensor")?;
    let mask_tensor = Tensor::from_array(([1usize, 1, edge, edge], mask_values))
        .context("create Big-LaMa mask tensor")?;
    let values = with_model_session(
        AiModel::BigLama,
        model_path,
        SessionOptions::new("Big-LaMa Remove"),
        ModelRetention::WhileWarm,
        |session| {
            session.run_with_fallback(
                "Big-LaMa Remove ONNX inference",
                |ort_session, _accelerated| {
                    let outputs = ort_session
                        .run(ort::inputs![&image_tensor, &mask_tensor])
                        .context("run Big-LaMa ONNX inference")?;
                    let output = outputs
                        .values()
                        .next()
                        .context("Big-LaMa returned no output tensor")?;
                    let (shape, values) = output
                        .try_extract_tensor::<f32>()
                        .context("read Big-LaMa output tensor")?;
                    anyhow::ensure!(
                        shape.as_ref() == [1, 3, edge as i64, edge as i64],
                        "unexpected Big-LaMa output shape {shape:?}"
                    );
                    anyhow::ensure!(
                        values.len() == plane * 3 && values.iter().all(|value| value.is_finite()),
                        "Big-LaMa output tensor is invalid"
                    );
                    Ok(values.to_vec())
                },
            )
        },
    )?;
    let mut interleaved = vec![0.0f32; plane * 3];
    for index in 0..plane {
        for channel in 0..3 {
            interleaved[index * 3 + channel] =
                (values[channel * plane + index] / 255.0).clamp(0.0, 1.0);
        }
    }
    ImageBuffer::from_raw(BIG_LAMA_INPUT_EDGE, BIG_LAMA_INPUT_EDGE, interleaved)
        .context("construct Big-LaMa output image")
}

/// Bicubic (Catmull-Rom) sample at continuous pixel coordinates, where pixel
/// `i` is centred on `i`. Sharper than bilinear when the model ran downscaled.
fn sample_catmull_rom(image: &Rgb32FImage, x: f32, y: f32) -> [f32; 3] {
    fn weights(t: f32) -> [f32; 4] {
        let t2 = t * t;
        let t3 = t2 * t;
        [
            0.5 * (-t3 + 2.0 * t2 - t),
            0.5 * (3.0 * t3 - 5.0 * t2 + 2.0),
            0.5 * (-3.0 * t3 + 4.0 * t2 + t),
            0.5 * (t3 - t2),
        ]
    }
    let (width, height) = (image.width() as i64, image.height() as i64);
    let (base_x, base_y) = (x.floor(), y.floor());
    let weights_x = weights(x - base_x);
    let weights_y = weights(y - base_y);
    let mut sum = [0.0f32; 3];
    for (row, weight_y) in weights_y.iter().enumerate() {
        let sample_y = (base_y as i64 + row as i64 - 1).clamp(0, height - 1) as u32;
        for (column, weight_x) in weights_x.iter().enumerate() {
            let sample_x = (base_x as i64 + column as i64 - 1).clamp(0, width - 1) as u32;
            let pixel = image.get_pixel(sample_x, sample_y);
            for channel in 0..3 {
                sum[channel] += pixel[channel] * weight_x * weight_y;
            }
        }
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    fn square_mask(x: u32, y: u32, edge: u32) -> RemoveMask {
        RemoveMask {
            bounds: NativeRect {
                x,
                y,
                width: edge,
                height: edge,
            },
            pixels: vec![255; (edge * edge) as usize],
        }
    }

    fn crop(x: u32, y: u32, edge: u32) -> NativeRect {
        NativeRect {
            x,
            y,
            width: edge,
            height: edge,
        }
    }

    #[test]
    fn native_pass_maps_pixels_one_to_one() {
        let geometry = PassGeometry::new(crop(1000, 2000, BIG_LAMA_INPUT_EDGE));
        assert_eq!(geometry.native_to_model(1000, 2000), [0.0, 0.0]);
        assert_eq!(geometry.native_to_model(1511, 2511), [511.0, 511.0]);
        assert_eq!(geometry.feather(), MIN_FEATHER);
    }

    #[test]
    fn downscaled_mask_keeps_every_object_pixel_masked() {
        // One native pixel per 4x4 block would vanish under nearest sampling.
        let geometry = PassGeometry::new(crop(0, 0, BIG_LAMA_INPUT_EDGE * 4));
        let speck = square_mask(1001, 1003, 1);
        let mask = geometry.model_mask(&speck);
        assert!(mask[geometry.model_index(1001, 1003)]);
        // Grown by the feather (6 native = 2 model pixels with margin).
        assert!(mask[geometry.model_index(1001 + 8, 1003)]);
        assert!(!mask[geometry.model_index(1001 + 40, 1003)]);
    }

    #[test]
    fn alpha_is_opaque_on_the_target_and_feathers_outside_it() {
        let geometry = PassGeometry::new(crop(0, 0, BIG_LAMA_INPUT_EDGE));
        let target = square_mask(100, 100, 20);
        let model_mask = geometry.model_mask(&target);
        let alpha = geometry.feather_alpha(&target, &model_mask);
        let at = |x: u32, y: u32| {
            alpha.pixels
                [((y - alpha.bounds.y) * alpha.bounds.width + (x - alpha.bounds.x)) as usize]
        };
        for y in 100..120 {
            for x in 100..120 {
                assert_eq!(at(x, y), 255, "target pixel {x},{y} is not fully replaced");
            }
        }
        assert!(at(99, 110) > 0 && at(99, 110) < 255);
        assert!(at(98, 110) < at(99, 110));
        assert!(alpha.bounds.x >= 100 - 3 && alpha.bounds.right() <= 120 + 3);
    }

    #[test]
    fn alpha_never_extends_beyond_generated_pixels() {
        let geometry = PassGeometry::new(crop(0, 0, BIG_LAMA_INPUT_EDGE));
        let target = square_mask(100, 100, 20);
        let model_mask = vec![false; (BIG_LAMA_INPUT_EDGE * BIG_LAMA_INPUT_EDGE) as usize];
        let alpha = geometry.feather_alpha(&target, &model_mask);
        assert!(alpha.pixels.is_empty());
    }

    #[test]
    fn catmull_rom_reproduces_pixels_and_linear_ramps() {
        let mut image = Rgb32FImage::new(8, 1);
        for x in 0..8 {
            image.put_pixel(x, 0, Rgb([x as f32 / 8.0; 3]));
        }
        assert!((sample_catmull_rom(&image, 3.0, 0.0)[0] - 3.0 / 8.0).abs() < 1e-6);
        assert!((sample_catmull_rom(&image, 3.5, 0.0)[0] - 3.5 / 8.0).abs() < 1e-6);
    }

    /// Deterministic pseudo-random noise in [-amplitude, amplitude].
    fn noise(index: usize, amplitude: f32) -> f32 {
        (mix64(index as u64) as f64 / u64::MAX as f64 * 2.0 - 1.0) as f32 * amplitude
    }

    #[test]
    fn denoise_flattens_grain_but_keeps_edges() {
        let edge = BIG_LAMA_INPUT_EDGE as usize;
        let plane = edge * edge;
        let mut planar = vec![0.0f32; plane * 3];
        for index in 0..plane {
            let base = if index % edge < edge / 2 { 0.2 } else { 0.8 };
            for channel in 0..3 {
                planar[channel * plane + index] = base + noise(index * 3 + channel, 0.04);
            }
        }
        let mask = vec![false; plane];
        denoise_model_input(&mut planar, &mask);
        let row = edge * 100;
        let flat = (row + 10..row + 200)
            .map(|index| planar[index])
            .collect::<Vec<_>>();
        let mean = flat.iter().sum::<f32>() / flat.len() as f32;
        let deviation = (flat.iter().map(|value| (value - mean).powi(2)).sum::<f32>()
            / flat.len() as f32)
            .sqrt();
        // Uniform noise of ±0.04 has a deviation of 0.023.
        assert!(
            deviation < 0.023 * 0.4,
            "grain left after denoise: {deviation}"
        );
        assert!(planar[row + edge / 2 - 2] < 0.3 && planar[row + edge / 2 + 1] > 0.7);
    }

    #[test]
    fn grain_is_copied_from_the_surroundings_but_never_from_the_object() {
        let raw = LoadedRaw::from_scene_linear_rec2020(1, 1, vec![0.2; 3]).unwrap();
        let bounds = NativeRect {
            x: 0,
            y: 0,
            width: 128,
            height: 128,
        };
        let mut scene = vec![0.0f32; 128 * 128 * 3];
        for (index, value) in scene.iter_mut().enumerate() {
            *value = 0.2 + noise(index, 0.05);
        }
        // The object is bright and flat: copying it would brighten the fill.
        let object = square_mask(48, 48, 32);
        for y in 48..80 {
            for x in 48..80 {
                scene[(y * 128 + x) * 3..(y * 128 + x) * 3 + 3].fill(0.9);
            }
        }
        let surroundings = NativeSurroundings { bounds, scene };
        let grain = Grain::sample(
            &surroundings,
            &object,
            &raw,
            &ExposureParams::default(),
            object.bounds,
        )
        .unwrap();
        for (x, y) in &grain.sources {
            let block = NativeRect {
                x: *x,
                y: *y,
                width: GRAIN_BLOCK,
                height: GRAIN_BLOCK,
            };
            assert!(block
                .intersect(NativeRect {
                    x: 44,
                    y: 44,
                    width: 40,
                    height: 40
                })
                .is_none());
        }
        let mut sum = 0.0;
        let mut squares = 0.0;
        for y in 48..80 {
            for x in 48..80 {
                let mut pixel = [0.2f32; 3];
                grain.add(x, y, &mut pixel);
                sum += pixel[1] - 0.2;
                squares += (pixel[1] - 0.2).powi(2);
                let mut again = [0.2f32; 3];
                grain.add(x, y, &mut again);
                assert_eq!(pixel, again);
            }
        }
        let count = 32.0 * 32.0;
        let deviation = (squares / count - (sum / count).powi(2)).sqrt();
        // Uniform noise of ±0.05 minus its blur keeps most of its 0.029 deviation.
        assert!(
            deviation > 0.015 && deviation < 0.035,
            "grain deviation {deviation}"
        );
        assert!((sum / count).abs() < 0.01);
    }

    #[test]
    fn grain_never_copies_thin_features_from_clean_surroundings() {
        let raw = LoadedRaw::from_scene_linear_rec2020(1, 1, vec![0.4; 3]).unwrap();
        let size = 192u32;
        let bounds = NativeRect {
            x: 0,
            y: 0,
            width: size,
            height: size,
        };
        // Textured, noise-free surroundings (like skin) with a few faint
        // single-pixel hairs: each barely raises its block's average energy.
        let mut scene = vec![0.0f32; (size * size * 3) as usize];
        for (index, value) in scene.iter_mut().enumerate() {
            *value = 0.4 + noise(index / 3, 0.02);
        }
        let mut hair = vec![false; (size * size) as usize];
        for step in 0..40u32 {
            for (x, y) in [
                (10 + step, 20 + step / 2),
                (150, 120 + step),
                (30 + step, 170),
            ] {
                hair[(y * size + x) as usize] = true;
                let pixel = ((y * size + x) * 3) as usize;
                for value in &mut scene[pixel..pixel + 3] {
                    *value -= 0.1;
                }
            }
        }
        let object = square_mask(80, 80, 32);
        let surroundings = NativeSurroundings { bounds, scene };
        let grain = Grain::sample(
            &surroundings,
            &object,
            &raw,
            &ExposureParams::default(),
            object.bounds,
        )
        .unwrap();
        for (x, y) in &grain.sources {
            for row in *y..*y + GRAIN_BLOCK {
                for column in *x..*x + GRAIN_BLOCK {
                    assert!(
                        !hair[(row * size + column) as usize],
                        "a block containing a hair is used as grain"
                    );
                }
            }
        }
        assert!(grain.sources.len() > 20, "too few clean blocks kept");
    }

    #[test]
    fn coherence_separates_lines_from_grain() {
        let width = GRAIN_BLOCK as usize;
        let grain = (0..width * width)
            .map(|index| [noise(index, 0.02); 3])
            .collect::<Vec<_>>();
        assert!(orientation_coherence(&grain, width, 0, 0) < 0.15);
        let mut line = grain.clone();
        for step in 0..width {
            // A visible line: about seven times the grain's deviation.
            line[step * width + step / 2 + 2] = [line[step * width + step / 2 + 2][0] - 0.08; 3];
        }
        assert!(orientation_coherence(&line, width, 0, 0) > GRAIN_MAX_COHERENCE);
    }

    #[test]
    fn grain_comes_only_from_similar_material_around_the_hole() {
        let raw = LoadedRaw::from_scene_linear_rec2020(1, 1, vec![0.4; 3]).unwrap();
        let size = 192u32;
        let bounds = NativeRect {
            x: 0,
            y: 0,
            width: size,
            height: size,
        };
        // Bright skin with fine texture, next to a dark strap whose stitching is
        // faint in absolute terms but would turn into bright scratches.
        let mut scene = vec![0.0f32; (size * size * 3) as usize];
        for y in 0..size {
            for x in 0..size {
                let index = (y * size + x) as usize;
                let value = if x < 48 {
                    0.05 + if (x + y) % 9 == 0 { 0.02 } else { 0.0 }
                } else {
                    0.45 + noise(index, 0.01)
                };
                scene[index * 3..index * 3 + 3].fill(value);
            }
        }
        let object = square_mask(100, 80, 32);
        let surroundings = NativeSurroundings { bounds, scene };
        let grain = Grain::sample(
            &surroundings,
            &object,
            &raw,
            &ExposureParams::default(),
            object.bounds,
        )
        .unwrap();
        assert!(!grain.sources.is_empty());
        assert!(
            grain.sources.iter().all(|(x, _)| *x >= 48),
            "grain was taken from the dark strap"
        );
        assert!(grain.sources.len() <= GRAIN_MAX_SOURCES);
    }
}
