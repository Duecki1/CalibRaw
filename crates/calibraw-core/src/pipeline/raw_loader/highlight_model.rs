//! Highlight reconstruction model: the colour relations clipped photosites are rebuilt from.
//!
//! A clipped photosite only knows that its colour is at least the clip level. The GPU pass
//! (`highlights.wgsl`) rebuilds it from the colours around it that did not clip, and needs to
//! know how the photo's colours relate where they are still measured. [`HighlightModel`] holds
//! that on a coarse grid over the whole photo, so previews, crops and export tiles of one photo
//! reconstruct alike.
//!
//! All colours are white-balanced camera RGB with the sensor's white level at the white-balance
//! multiplier, the units of `RawSampling::raw_camera_at`. A colour is "known" where it did not
//! clip. Three cases, by how many colours of a pixel are known:
//!
//! - Two known, `a` and `b`: the clipped colour is `(∛a / 2 + ∛b / 2 + chroma)³`, darktable's
//!   opposed estimate. The chroma is a colour's cube root less the mean cube root of the other
//!   two where it did not clip, chosen as darktable's segmentation method chooses it: per clipped
//!   segment of the colour, at the one candidate beside it that is smooth and close to the clip
//!   level, else 0 (`segments` says where this differs from darktable). A candidate rather than
//!   a mean of the border keeps a sky between clouds from taking the clouds' noisy fringes.
//! - One known: the colour runs from the entry colour, the reconstructed colour where the
//!   second colour starts to clip, to blown white as the known colour nears its own clip level.
//! - None known: blown white, every colour at the highest clip level.

use super::*;

mod segments;

use segments::ColourSegments;

/// Target edge of a model cell in photosites: a multiple of the Bayer and X-Trans periods, so
/// every cell holds each colour.
const CELL_EDGE: u32 = 12;
/// The share of the clip level a clipped sample must reach, as in `RawSampling::is_raw_clipped`.
const CLIP_DETECTION: f32 = 0.987;
/// A cell without clipping tells the colours of a highlight from this share of the clip level
/// up, and with full weight from [`DONOR_FULL`].
const DONOR_START: f32 = 0.2;
const DONOR_FULL: f32 = 0.8;
/// Weight a level of the spread keeps of the level below it, so near cells count for more.
const SPREAD_DECAY: f32 = 0.7;
/// Summed weight below which a spread value is trusted less than fully.
const SUPPORT_FLOOR: f32 = 1e-4;
/// A colour counts as known in a cell while at most this share of its samples clipped.
const KNOWN_CLIPPED_SHARE: f32 = 0.25;
const STATISTICS_CACHE_ENTRIES: usize = 3;
const MODEL_CACHE_ENTRIES: usize = 4;

/// What highlight reconstruction reads beside a clipped photosite's own neighbourhood: one
/// cell per about 12×12 photosites, row-major from the top left. Cell `(i, j)` of `cells`
/// `[nx, ny]` is centred at `((i + 0.5) / nx, (j + 0.5) / ny)` of the whole photo, so crops,
/// proxies and tiles share one model; the GPU places each with its tile origin and full-image
/// extent.
#[derive(Debug)]
pub struct HighlightModel {
    cells: [u32; 2],
    chroma: Box<[[f32; 4]]>,
    entry: Box<[[f32; 4]]>,
}

impl HighlightModel {
    /// Most cells along an axis.
    pub const MAX_CELLS: u32 = 512;

    /// Cells across and down the model of a photo `[width, height]` photosites in size.
    pub fn grid_for([width, height]: [u32; 2]) -> [u32; 2] {
        let edge = CELL_EDGE.max(width.max(height).div_ceil(Self::MAX_CELLS));
        [width.div_ceil(edge).max(1), height.div_ceil(edge).max(1)]
    }

    pub fn cells(&self) -> [u32; 2] {
        self.cells
    }

    /// Per cell, the chroma of red, green and blue: a colour's cube root less the mean cube root
    /// of the other two where it did not clip; the fourth value is unused.
    pub fn chroma(&self) -> &[[f32; 4]] {
        &self.chroma
    }

    /// Per cell, the entry colour multiplied by its support, then the support in `[0, 1]`:
    /// 0 where no reconstructed or bright unclipped colour is near.
    pub fn entry(&self) -> &[[f32; 4]] {
        &self.entry
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct StatisticsKey {
    source_identity: usize,
    black_point_bits: u32,
    clip_threshold_bits: u32,
    use_ai_cfa: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ModelKey {
    statistics: StatisticsKey,
    wb_bits: [u32; 3],
}

/// Shared highlight-model state of one photo.
///
/// `statistics` are measured from every photosite and do not depend on white balance, so a
/// white-balance change only refits the model from them. Both lists keep their most recent
/// entries. An entry of `None` records a photo without clipped photosites.
#[derive(Debug, Default)]
pub struct HighlightModelCacheState {
    statistics: Vec<(StatisticsKey, Option<Arc<HighlightStatistics>>)>,
    models: Vec<(ModelKey, Option<Arc<HighlightModel>>)>,
}

impl HighlightModelCacheState {
    /// Drops what was measured from the AI-denoised mosaic.
    pub(super) fn forget_ai_denoised(&mut self) {
        self.statistics.retain(|(key, _)| !key.use_ai_cfa);
        self.models.retain(|(key, _)| !key.statistics.use_ai_cfa);
    }
}

pub type HighlightModelCache = Arc<RwLock<HighlightModelCacheState>>;

fn remember<K: PartialEq, V>(entries: &mut Vec<(K, V)>, key: K, value: V, capacity: usize) {
    entries.retain(|(existing, _)| *existing != key);
    if entries.len() >= capacity {
        entries.remove(0);
    }
    entries.push((key, value));
}

/// Red, green and blue multipliers of `wb_coeffs` (R, G1, B, G2): green is the mean of both
/// greens when the sensor has a second one.
fn logical_white_balance(wb_coeffs: [f32; 4]) -> [f32; 3] {
    let green = if wb_coeffs[3] > 0.0 {
        0.5 * (wb_coeffs[1] + wb_coeffs[3])
    } else {
        wb_coeffs[1]
    };
    [wb_coeffs[0], green, wb_coeffs[2]]
}

/// The two colours a clipped `channel` is rebuilt from, in the order its chroma reads them.
/// `highlights.wgsl` uses the same order.
const fn other_channels(channel: usize) -> [usize; 2] {
    match channel {
        0 => [1, 2],
        1 => [0, 2],
        _ => [0, 1],
    }
}

/// Per-cell measurements of a mosaic in normalized sensor units, before white balance.
#[derive(Debug)]
struct HighlightStatistics {
    cells: [u32; 2],
    /// Mean recorded level of each colour; clipped samples count at their recorded level, so
    /// it is a lower bound of the true mean.
    mean: Vec<[f32; 3]>,
    /// Share of each colour's samples that clipped.
    clipped: Vec<[f32; 3]>,
    /// Weight of a cell without clipping as evidence of a highlight's colour.
    donor: Vec<f32>,
    /// Per colour, its segments and their candidates (`segments`).
    segments: [ColourSegments; 3],
}

#[derive(Clone, Copy, Default)]
struct CellSamples {
    sum: [f32; 3],
    count: [u32; 3],
    clipped: [u32; 3],
}

struct SpreadLevel {
    width: usize,
    height: usize,
    data: Vec<f32>,
}

impl SpreadLevel {
    /// The level at half the size: each cell the sum of the up to four cells it covers.
    fn halved(&self, stride: usize) -> Self {
        let width = self.width.div_ceil(2);
        let height = self.height.div_ceil(2);
        let mut data = vec![0.0f32; width * height * stride];
        for y in 0..self.height {
            for x in 0..self.width {
                let source = (y * self.width + x) * stride;
                let target = ((y / 2) * width + x / 2) * stride;
                for channel in 0..stride {
                    data[target + channel] += self.data[source + channel];
                }
            }
        }
        Self {
            width,
            height,
            data,
        }
    }

    /// Bilinear sample of `channel` at cell-centre position `(x, y)` of this level's grid.
    fn sample(&self, stride: usize, x: f32, y: f32, channel: usize) -> f32 {
        let x = x.clamp(0.0, (self.width - 1) as f32);
        let y = y.clamp(0.0, (self.height - 1) as f32);
        let x0 = x.floor() as usize;
        let y0 = y.floor() as usize;
        let x1 = (x0 + 1).min(self.width - 1);
        let y1 = (y0 + 1).min(self.height - 1);
        let fx = x - x0 as f32;
        let fy = y - y0 as f32;
        let at =
            |column: usize, row: usize| self.data[(row * self.width + column) * stride + channel];
        let top = at(x0, y0) * (1.0 - fx) + at(x1, y0) * fx;
        let bottom = at(x0, y1) * (1.0 - fx) + at(x1, y1) * fx;
        top * (1.0 - fy) + bottom * fy
    }
}

/// Spreads weighted values from the cells that hold them into the others (push-pull).
///
/// `base` holds `stride` values per cell of a `cells` grid: values already multiplied by the
/// cell's weight, then the weight. A cell with a weight of 1 or more keeps its own values; the
/// others add those of ever coarser levels, each level counting [`SPREAD_DECAY`] of the one
/// below. Dividing a result's values by its weight gives the mean of the cells around it,
/// nearer ones weighing more.
fn spread([width, height]: [u32; 2], stride: usize, base: Vec<f32>) -> Vec<f32> {
    let mut levels = vec![SpreadLevel {
        width: width as usize,
        height: height as usize,
        data: base,
    }];
    loop {
        let last = &levels[levels.len() - 1];
        if last.width.min(last.height) <= 2 {
            break;
        }
        let next = last.halved(stride);
        levels.push(next);
    }
    let mut levels = levels.into_iter().rev();
    let Some(mut fill) = levels.next() else {
        return Vec::new();
    };
    for mut level in levels {
        for y in 0..level.height {
            for x in 0..level.width {
                let cell = (y * level.width + x) * stride;
                let confidence = level.data[cell + stride - 1].clamp(0.0, 1.0);
                if confidence >= 1.0 {
                    continue;
                }
                // A coarser cell sums four of this level's.
                let gain = (1.0 - confidence) * SPREAD_DECAY * 0.25;
                let coarse_x = (x as f32 + 0.5) * 0.5 - 0.5;
                let coarse_y = (y as f32 + 0.5) * 0.5 - 0.5;
                for channel in 0..stride {
                    level.data[cell + channel] +=
                        gain * fill.sample(stride, coarse_x, coarse_y, channel);
                }
            }
        }
        fill = level;
    }
    fill.data
}

impl HighlightStatistics {
    /// The chroma of every cell under white balance `wb` ([`HighlightModel::chroma`]).
    fn chroma(&self, wb: [f32; 3]) -> Vec<[f32; 3]> {
        let wb_roots = wb.map(f32::cbrt);
        let colours: [Vec<f32>; 3] =
            std::array::from_fn(|channel| self.segments[channel].cell_chroma(channel, wb_roots));
        (0..self.mean.len())
            .map(|cell| std::array::from_fn(|channel| colours[channel][cell]))
            .collect()
    }

    fn model(&self, wb: [f32; 3]) -> HighlightModel {
        let count = self.mean.len();
        let cell_chromas = self.chroma(wb);
        let mut chroma = Vec::with_capacity(count);
        let mut entry_base = Vec::with_capacity(count * 4);
        for (index, cell_chroma) in cell_chromas.iter().enumerate() {
            let colour: [f32; 3] =
                std::array::from_fn(|channel| self.mean[index][channel] * wb[channel]);
            let clipped = self.clipped[index];
            // The colour of a cell with two known colours: its clipped one rebuilt from them.
            let rebuilt: [f32; 3] = std::array::from_fn(|channel| {
                if clipped[channel] <= 0.0 {
                    return colour[channel];
                }
                let [a, b] = other_channels(channel);
                let root = 0.5 * (colour[a].max(0.0).cbrt() + colour[b].max(0.0).cbrt())
                    + cell_chroma[channel];
                colour[channel].max(root * root * root)
            });
            let known = clipped
                .iter()
                .filter(|share| **share <= KNOWN_CLIPPED_SHARE)
                .count();
            let any_clipped = clipped.iter().any(|share| *share > 0.0);
            // Entry colours come from rebuilt cells and, where two colours clip at once, from
            // bright cells without clipping. `donor` is 0 for the other cells with clipping.
            let (entry, weight) = if any_clipped && known >= 2 {
                (rebuilt, 1.0)
            } else {
                (colour, self.donor[index])
            };
            entry_base.extend([
                entry[0] * weight,
                entry[1] * weight,
                entry[2] * weight,
                weight,
            ]);
            chroma.push([cell_chroma[0], cell_chroma[1], cell_chroma[2], 0.0]);
        }
        let entry = spread(self.cells, 4, entry_base)
            .chunks_exact(4)
            .map(|cell| {
                let support = (cell[3] / SUPPORT_FLOOR).clamp(0.0, 1.0);
                let scale = support / cell[3].max(1e-9);
                [cell[0] * scale, cell[1] * scale, cell[2] * scale, support]
            })
            .collect();
        HighlightModel {
            cells: self.cells,
            chroma: chroma.into_boxed_slice(),
            entry,
        }
    }
}

impl LoadedRaw {
    fn highlight_sensor_value(&self, index: usize, black_point: f32, pixels: &[u16]) -> f32 {
        let channel = usize::from(self.color_indices[index].min(3));
        let raw = f32::from(pixels[index]);
        let metadata_black = self.black_levels_per_pixel[index];
        let white = self.white_levels[channel].max(metadata_black + 1.0);
        let sensor_range = (white - metadata_black).max(1.0);
        let black_offset = black_point.clamp(-0.25, 0.25) * sensor_range;
        let calibrated_black = (metadata_black + black_offset).clamp(0.0, white - 1.0);
        ((raw - calibrated_black) / (white - calibrated_black)).clamp(0.0, 4.0)
    }

    fn logical_color(&self, index: usize) -> usize {
        match self.color_indices[index].min(3) {
            0 => 0,
            2 => 2,
            _ => 1,
        }
    }

    /// Measures the mosaic `pixels`; `None` when no photosite clipped.
    fn highlight_statistics(
        &self,
        black_point: f32,
        clip_threshold: f32,
        pixels: &[u16],
    ) -> Option<HighlightStatistics> {
        let width = self.width as usize;
        let height = self.height as usize;
        if width == 0 || height == 0 || pixels.len() != width.saturating_mul(height) {
            return None;
        }
        let cells = HighlightModel::grid_for([self.width, self.height]);
        let [cells_x, cells_y] = cells.map(|count| count as usize);
        let detection = CLIP_DETECTION * clip_threshold;
        let column_cell: Vec<usize> = (0..width).map(|x| x * cells_x / width).collect();
        let samples: Vec<CellSamples> = (0..cells_y)
            .into_par_iter()
            .flat_map_iter(|cell_row| {
                let mut row = vec![CellSamples::default(); cells_x];
                // The pixel rows y with y * cells_y / height == cell_row.
                let start = (cell_row * height).div_ceil(cells_y);
                let end = ((cell_row + 1) * height).div_ceil(cells_y);
                for y in start..end {
                    for (x, cell) in column_cell.iter().enumerate() {
                        let index = y * width + x;
                        let color = self.logical_color(index);
                        let value = self.highlight_sensor_value(index, black_point, pixels);
                        let cell = &mut row[*cell];
                        cell.sum[color] += value;
                        cell.count[color] += 1;
                        cell.clipped[color] += u32::from(value >= detection);
                    }
                }
                row
            })
            .collect();
        if samples
            .iter()
            .all(|cell| cell.clipped.iter().all(|count| *count == 0))
        {
            return None;
        }

        let mut mean = Vec::with_capacity(samples.len());
        let mut clipped = Vec::with_capacity(samples.len());
        let mut donor = Vec::with_capacity(samples.len());
        for cell in &samples {
            let complete = cell.count.iter().all(|count| *count > 0);
            let cell_mean: [f32; 3] =
                std::array::from_fn(|color| cell.sum[color] / cell.count[color].max(1) as f32);
            let cell_clipped: [f32; 3] = std::array::from_fn(|color| {
                cell.clipped[color] as f32 / cell.count[color].max(1) as f32
            });
            let brightness = cell_mean.iter().copied().fold(0.0f32, f32::max) / clip_threshold;
            let weight = if complete && cell.clipped.iter().all(|count| *count == 0) {
                ((brightness - DONOR_START) / (DONOR_FULL - DONOR_START))
                    .clamp(0.0, 1.0)
                    .powi(2)
            } else {
                0.0
            };
            mean.push(cell_mean);
            clipped.push(cell_clipped);
            donor.push(weight);
        }
        let clipped_cells: Vec<bool> = samples
            .iter()
            .map(|cell| cell.clipped.iter().any(|count| *count > 0))
            .collect();
        let segments =
            self.highlight_segments(cells, &clipped_cells, black_point, clip_threshold, pixels);
        Some(HighlightStatistics {
            cells,
            mean,
            clipped,
            donor,
            segments,
        })
    }

    /// The statistics for `key`: cached, or measured when this is the photo's full mosaic.
    /// The outer `None` is a crop, proxy or tile whose source was not measured yet.
    fn cached_highlight_statistics(
        &self,
        key: StatisticsKey,
        black_point: f32,
        clip_threshold: f32,
    ) -> Option<Option<Arc<HighlightStatistics>>> {
        if let Ok(cache) = self.highlight_model_cache.read() {
            if let Some((_, statistics)) = cache
                .statistics
                .iter()
                .find(|(existing, _)| *existing == key)
            {
                return Some(statistics.clone());
            }
        }
        // Only the full source may measure: a derived view would describe its own part of the
        // photo under the whole photo's key.
        if !self.highlight_reference_source || self.pixel_layout() != PixelLayout::Mosaic {
            return None;
        }
        let ai_image = key.use_ai_cfa.then(|| self.ai_denoised_image()).flatten();
        let pixels = ai_image
            .as_ref()
            .and_then(AiDenoisedImage::bayer_cfa)
            .unwrap_or(&self.raw_pixels);
        let statistics = self
            .highlight_statistics(black_point, clip_threshold, pixels)
            .map(Arc::new);
        if let Ok(mut cache) = self.highlight_model_cache.write() {
            remember(
                &mut cache.statistics,
                key,
                statistics.clone(),
                STATISTICS_CACHE_ENTRIES,
            );
        }
        Some(statistics)
    }

    /// The model highlight reconstruction rebuilds this photo's clipped photosites with, under
    /// the white-balance multipliers `wb_coeffs` (R, G1, B, G2). `highlight_clip` is the
    /// Develop setting, a fraction of [`LoadedRaw::sensor_saturation`].
    ///
    /// `None` when nothing clipped, and for a crop, proxy or tile whose full source was not
    /// asked first ([`Self::prime_highlight_model`]); reconstruction then rebuilds clipped
    /// photosites as neutral.
    pub fn highlight_model(
        &self,
        black_point: f32,
        highlight_clip: f32,
        use_ai_cfa: bool,
        wb_coeffs: [f32; 4],
    ) -> Option<Arc<HighlightModel>> {
        let black_point = black_point.clamp(-0.25, 0.25);
        let clip_threshold = self.sensor_saturation.clip_level(highlight_clip).max(0.01);
        let statistics_key = StatisticsKey {
            source_identity: Arc::as_ptr(&self.highlight_source_identity) as usize,
            black_point_bits: black_point.to_bits(),
            clip_threshold_bits: clip_threshold.to_bits(),
            use_ai_cfa,
        };
        let wb = logical_white_balance(wb_coeffs);
        let key = ModelKey {
            statistics: statistics_key,
            wb_bits: wb.map(f32::to_bits),
        };
        if let Ok(cache) = self.highlight_model_cache.read() {
            if let Some((_, model)) = cache.models.iter().find(|(existing, _)| *existing == key) {
                return model.clone();
            }
        }
        let statistics =
            self.cached_highlight_statistics(statistics_key, black_point, clip_threshold)?;
        let model = statistics.map(|statistics| Arc::new(statistics.model(wb)));
        if let Ok(mut cache) = self.highlight_model_cache.write() {
            remember(&mut cache.models, key, model.clone(), MODEL_CACHE_ENTRIES);
        }
        model
    }

    /// Whether `exposure` reconstructs highlights with a [`HighlightModel`].
    pub fn uses_highlight_model(&self, exposure: &ExposureParams) -> bool {
        exposure.highlight_method == HighlightReconstructionMethod::InpaintOpposed
            || (self.cfa_kind == CfaKind::XTrans
                && exposure.highlight_method == HighlightReconstructionMethod::Lch)
    }

    /// Measures this full source for `exposure`, so its crops, proxies and tiles find the
    /// model in the cache they share with it.
    pub fn prime_highlight_model(&self, exposure: &ExposureParams) -> Option<Arc<HighlightModel>> {
        let (temperature, tint) = self.white_balance_offsets(exposure.white_balance);
        let wb = self
            .adjusted_white_balance_and_camera_transform(temperature, tint)
            .0;
        self.highlight_model(
            exposure.black_point,
            exposure.highlight_clip,
            exposure.ai_denoise_enabled,
            wb,
        )
    }
}
