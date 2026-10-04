//! Opposed highlight reconstruction: chroma estimated from unclipped neighbours.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct OpposedChromaCacheKey {
    source_identity: usize,
    wb_bits: [u32; 4],
    black_point_bits: u32,
    clip_threshold_bits: u32,
    pub(super) use_ai_cfa: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct OpposedChromaPreparedKey {
    source_identity: usize,
    black_point_bits: u32,
    clip_threshold_bits: u32,
    pub(super) use_ai_cfa: bool,
}

/// Shared opposed-highlight estimator state.
///
/// `results` is keyed by every input that changes the final chroma reference, including WB.
/// `prepared` intentionally excludes WB: clipping/nearby classification is WB-independent for
/// positive RAW white-balance coefficients, so the expensive full-image scan can be reused while
/// temperature/tint is scrubbed and only the prepared candidate pixels need to be re-evaluated.
#[derive(Debug, Default)]
pub struct OpposedChromaCacheState {
    results: HashMap<OpposedChromaCacheKey, [f32; 3]>,
    pub(super) prepared: HashMap<OpposedChromaPreparedKey, Arc<Vec<usize>>>,
}

impl Deref for OpposedChromaCacheState {
    type Target = HashMap<OpposedChromaCacheKey, [f32; 3]>;

    fn deref(&self) -> &Self::Target {
        &self.results
    }
}

impl DerefMut for OpposedChromaCacheState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.results
    }
}

pub type OpposedChromaCache = Arc<RwLock<OpposedChromaCacheState>>;

impl LoadedRaw {
    fn opposed_sensor_value(&self, index: usize, black_point: f32, pixels: &[u16]) -> f32 {
        let channel = usize::from(self.color_indices[index].min(3));
        let raw = f32::from(pixels[index]);
        let metadata_black = self.black_levels_per_pixel[index];
        let white = self.white_levels[channel].max(metadata_black + 1.0);
        let sensor_range = (white - metadata_black).max(1.0);
        let black_offset = black_point.clamp(-0.25, 0.25) * sensor_range;
        let calibrated_black = (metadata_black + black_offset).clamp(0.0, white - 1.0);
        ((raw - calibrated_black) / (white - calibrated_black)).clamp(0.0, 4.0)
    }

    fn opposed_logical_color(&self, index: usize) -> usize {
        match self.color_indices[index].min(3) {
            0 => 0,
            2 => 2,
            _ => 1,
        }
    }

    fn opposed_refavg(
        &self,
        row: usize,
        col: usize,
        black_point: f32,
        pixels: &[u16],
        wb_coeffs: [f32; 4],
    ) -> f32 {
        let width = self.width as usize;
        let height = self.height as usize;
        let center = row * width + col;
        let center_color = self.opposed_logical_color(center);
        let mut means = [0.0f32; 3];
        let mut counts = [0u32; 3];

        let row_end = (row + 2).min(height.saturating_sub(1));
        let col_end = (col + 2).min(width.saturating_sub(1));
        for sample_row in row.saturating_sub(1)..row_end {
            for sample_col in col.saturating_sub(1)..col_end {
                let index = sample_row * width + sample_col;
                let physical = usize::from(self.color_indices[index].min(3));
                let color = self.opposed_logical_color(index);
                let value =
                    self.opposed_sensor_value(index, black_point, pixels) * wb_coeffs[physical];
                means[color] += value.max(0.0);
                counts[color] += 1;
            }
        }
        for color in 0..3 {
            means[color] = if counts[color] == 0 {
                0.0
            } else {
                (means[color] / counts[color] as f32).cbrt()
            };
        }
        let opposed_root = match center_color {
            0 => 0.5 * (means[1] + means[2]),
            1 => 0.5 * (means[0] + means[2]),
            _ => 0.5 * (means[0] + means[1]),
        };
        opposed_root * opposed_root * opposed_root
    }

    pub(super) fn prepare_opposed_chroma_candidates(
        &self,
        black_point: f32,
        clip_threshold: f32,
        pixels: &[u16],
    ) -> Vec<usize> {
        let width = self.width as usize;
        let height = self.height as usize;
        if width == 0 || height == 0 || pixels.len() != width.saturating_mul(height) {
            return Vec::new();
        }

        let mask_width = width / 3;
        let mask_height = height / 3;
        if mask_width == 0 || mask_height == 0 {
            return Vec::new();
        }
        let aligned_mask_width = mask_width.div_ceil(8) * 8;
        let aligned_mask_height = mask_height.div_ceil(8) * 8;
        let aligned_mask_area = aligned_mask_width.saturating_mul(aligned_mask_height);
        let last_raw_mask_index = ((height - 1) / 3) * mask_width + (width - 1) / 3;
        let required_mask_size = (last_raw_mask_index + 1).div_ceil(8) * 8;
        let mask_size = aligned_mask_area.max(required_mask_size);
        // Store RGB clipping flags together so rows can be processed independently.
        let mut clipped_mask = vec![0u8; mask_size];
        let clip = 0.987 * clip_threshold.max(0.01);
        clipped_mask[..mask_width * mask_height]
            .par_chunks_mut(mask_width)
            .enumerate()
            .for_each(|(mask_row, cells)| {
                if mask_row >= mask_height.saturating_sub(1) {
                    return;
                }
                for (mask_col, cell) in cells
                    .iter_mut()
                    .enumerate()
                    .take(mask_width.saturating_sub(1))
                {
                    for offset_y in 0..3 {
                        let row = mask_row * 3 + offset_y;
                        for offset_x in 0..3 {
                            let index = row * width + mask_col * 3 + offset_x;
                            if self.opposed_sensor_value(index, black_point, pixels) >= clip {
                                *cell |= 1 << self.opposed_logical_color(index);
                            }
                        }
                    }
                }
            });
        if clipped_mask.iter().all(|&cell| cell == 0) {
            return Vec::new();
        }

        // Mark cells near clipping by scattering a corner-less 7x7 footprint from
        // each clipped cell. Destinations within 3 cells of the left/top or 4 cells
        // of the right/bottom border are never marked.
        let mut nearby_mask = clipped_mask.clone();
        for source_row in 0..mask_height {
            for source_col in 0..mask_width {
                let flags = clipped_mask[source_row * mask_width + source_col];
                if flags == 0 {
                    continue;
                }
                for offset_y in -3isize..=3 {
                    for offset_x in -3isize..=3 {
                        if offset_x.abs() == 3 && offset_y.abs() == 3 {
                            continue;
                        }
                        let row = source_row as isize - offset_y;
                        let col = source_col as isize - offset_x;
                        if row >= 3
                            && col >= 3
                            && row < mask_height.saturating_sub(4) as isize
                            && col < mask_width.saturating_sub(4) as isize
                        {
                            nearby_mask[row as usize * mask_width + col as usize] |= flags;
                        }
                    }
                }
            }
        }

        // Only normalize pixels near clipping. Collect indexed row bands in order
        // so the subsequent floating-point accumulation remains bit-for-bit stable.
        let bands: Vec<Vec<usize>> = (0..height.div_ceil(24))
            .into_par_iter()
            .map(|band| {
                let mut candidates = Vec::new();
                for row in band * 24..((band + 1) * 24).min(height) {
                    for col in 0..width {
                        let flags = nearby_mask[(row / 3) * mask_width + col / 3];
                        if flags == 0 {
                            continue;
                        }
                        let index = row * width + col;
                        if flags & (1 << self.opposed_logical_color(index)) == 0 {
                            continue;
                        }
                        let value = self.opposed_sensor_value(index, black_point, pixels);
                        if value > 0.2 * clip && value < clip {
                            candidates.push(index);
                        }
                    }
                }
                candidates
            })
            .collect();
        bands.into_iter().flatten().collect()
    }

    #[cfg(test)]
    pub(super) fn prepare_opposed_chroma_candidates_serial(
        &self,
        black_point: f32,
        clip_threshold: f32,
        pixels: &[u16],
    ) -> Vec<usize> {
        let width = self.width as usize;
        let height = self.height as usize;
        if width == 0 || height == 0 || pixels.len() != width.saturating_mul(height) {
            return Vec::new();
        }

        let mask_width = width / 3;
        let mask_height = height / 3;
        if mask_width == 0 || mask_height == 0 {
            return Vec::new();
        }
        let aligned_mask_width = mask_width.div_ceil(8) * 8;
        let aligned_mask_height = mask_height.div_ceil(8) * 8;
        let aligned_mask_area = aligned_mask_width.saturating_mul(aligned_mask_height);
        let last_raw_mask_index = ((height - 1) / 3) * mask_width + (width - 1) / 3;
        let required_mask_size = (last_raw_mask_index + 1).div_ceil(8) * 8;
        let mask_size = aligned_mask_area.max(required_mask_size);
        let mut clipped_mask = vec![false; 3 * mask_size];
        let clip = 0.987 * clip_threshold.max(0.01);

        for mask_row in 0..mask_height.saturating_sub(1) {
            for mask_col in 0..mask_width.saturating_sub(1) {
                let mask_index = mask_row * mask_width + mask_col;
                for offset_y in 0..3 {
                    let row = mask_row * 3 + offset_y;
                    for offset_x in 0..3 {
                        let col = mask_col * 3 + offset_x;
                        let index = row * width + col;
                        let color = self.opposed_logical_color(index);
                        let value = self.opposed_sensor_value(index, black_point, pixels);
                        clipped_mask[color * mask_size + mask_index] |= value >= clip;
                    }
                }
            }
        }

        // Dilate from clipped cells instead of probing a ~7x7 neighbourhood around every mask
        // cell. Clipped highlights are normally sparse, so this turns the dominant preparation
        // cost from O(mask_area * kernel_area) into O(mask_area + clipped_cells * kernel_area).
        // As above, destinations within 3 cells of the left/top or 4 cells of the
        // right/bottom border are never marked.
        let mut nearby_mask = clipped_mask.clone();
        for color in 0..3 {
            let plane = color * mask_size;
            for source_row in 0..mask_height {
                for source_col in 0..mask_width {
                    let source_index = source_row * mask_width + source_col;
                    if !clipped_mask[plane + source_index] {
                        continue;
                    }
                    for offset_y in -3isize..=3 {
                        for offset_x in -3isize..=3 {
                            if offset_x.abs() == 3 && offset_y.abs() == 3 {
                                continue;
                            }
                            let destination_row = source_row as isize - offset_y;
                            let destination_col = source_col as isize - offset_x;
                            if destination_row < 0
                                || destination_col < 0
                                || destination_row >= mask_height as isize
                                || destination_col >= mask_width as isize
                            {
                                continue;
                            }
                            let row = destination_row as usize;
                            let col = destination_col as usize;
                            let safe = col >= 3
                                && row >= 3
                                && col < mask_width.saturating_sub(4)
                                && row < mask_height.saturating_sub(4);
                            if safe {
                                nearby_mask[plane + row * mask_width + col] = true;
                            }
                        }
                    }
                }
            }
        }

        let mut candidates = Vec::new();
        for row in 0..height {
            for col in 0..width {
                let index = row * width + col;
                let color = self.opposed_logical_color(index);
                let value = self.opposed_sensor_value(index, black_point, pixels);
                let mask_index = (row / 3) * mask_width + col / 3;
                if nearby_mask[color * mask_size + mask_index] && value > 0.2 * clip && value < clip
                {
                    candidates.push(index);
                }
            }
        }
        candidates
    }

    fn calculate_opposed_chroma_from_candidates(
        &self,
        black_point: f32,
        pixels: &[u16],
        wb_coeffs: [f32; 4],
        candidates: &[usize],
    ) -> [f32; 3] {
        let width = self.width as usize;
        if width == 0 {
            return [0.0; 3];
        }
        let mut sums = [0.0f32; 3];
        let mut counts = [0.0f32; 3];
        for &index in candidates {
            let row = index / width;
            let col = index % width;
            let physical = usize::from(self.color_indices[index].min(3));
            let color = self.opposed_logical_color(index);
            let value = self.opposed_sensor_value(index, black_point, pixels) * wb_coeffs[physical];
            sums[color] += value - self.opposed_refavg(row, col, black_point, pixels, wb_coeffs);
            counts[color] += 1.0;
        }

        std::array::from_fn(|color| {
            if counts[color] > 100.0 {
                sums[color] / counts[color]
            } else {
                0.0
            }
        })
    }

    fn calculate_opposed_chroma(
        &self,
        black_point: f32,
        clip_threshold: f32,
        pixels: &[u16],
        wb_coeffs: [f32; 4],
    ) -> [f32; 3] {
        let candidates =
            self.prepare_opposed_chroma_candidates(black_point, clip_threshold, pixels);
        self.calculate_opposed_chroma_from_candidates(black_point, pixels, wb_coeffs, &candidates)
    }

    pub fn inpaint_opposed_chroma(
        &self,
        black_point: f32,
        clip_threshold: f32,
        use_ai_cfa: bool,
        wb_coeffs: [f32; 4],
    ) -> [f32; 3] {
        let key = OpposedChromaCacheKey {
            source_identity: Arc::as_ptr(&self.opposed_chroma_source_identity) as usize,
            wb_bits: wb_coeffs.map(f32::to_bits),
            black_point_bits: black_point.clamp(-0.25, 0.25).to_bits(),
            clip_threshold_bits: clip_threshold.max(0.01).to_bits(),
            use_ai_cfa,
        };
        if let Ok(cache) = self.opposed_chroma_cache.read() {
            if let Some(chroma) = cache.get(&key) {
                return *chroma;
            }
        }
        let prepared_key = OpposedChromaPreparedKey {
            source_identity: key.source_identity,
            black_point_bits: key.black_point_bits,
            clip_threshold_bits: key.clip_threshold_bits,
            use_ai_cfa: key.use_ai_cfa,
        };
        let ai_image = use_ai_cfa.then(|| self.ai_denoised_image()).flatten();
        let pixels = ai_image
            .as_ref()
            .and_then(AiDenoisedImage::bayer_cfa)
            .unwrap_or(self.raw_pixels.as_slice());
        let chroma = if self.opposed_chroma_reference_source {
            let prepared = self
                .opposed_chroma_cache
                .read()
                .ok()
                .and_then(|cache| cache.prepared.get(&prepared_key).cloned())
                .unwrap_or_else(|| {
                    let candidates = Arc::new(self.prepare_opposed_chroma_candidates(
                        black_point,
                        clip_threshold,
                        pixels,
                    ));
                    if let Ok(mut cache) = self.opposed_chroma_cache.write() {
                        cache
                            .prepared
                            .entry(prepared_key)
                            .or_insert_with(|| Arc::clone(&candidates));
                    }
                    candidates
                });
            self.calculate_opposed_chroma_from_candidates(black_point, pixels, wb_coeffs, &prepared)
        } else {
            // A derived crop/proxy must never populate full-source prepared state. This fallback
            // preserves standalone behavior if the caller forgot to prime the full source first.
            self.calculate_opposed_chroma(black_point, clip_threshold, pixels, wb_coeffs)
        };
        if self.opposed_chroma_reference_source {
            if let Ok(mut cache) = self.opposed_chroma_cache.write() {
                cache.insert(key, chroma);
            }
        }
        chroma
    }

    pub fn uses_opposed_chroma(&self, exposure: &ExposureParams) -> bool {
        exposure.highlight_method == HighlightReconstructionMethod::InpaintOpposed
            || (self.cfa_kind == CfaKind::XTrans
                && exposure.highlight_method == HighlightReconstructionMethod::Lch)
    }

    pub fn inpaint_opposed_chroma_for_exposure(&self, exposure: &ExposureParams) -> [f32; 3] {
        let wb = self
            .adjusted_white_balance_and_camera_transform(exposure.temperature, exposure.tint)
            .0;
        self.inpaint_opposed_chroma(
            exposure.black_point,
            exposure.highlight_clip,
            exposure.ai_denoise_enabled,
            wb,
        )
    }
}
