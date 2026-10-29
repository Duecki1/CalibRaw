//! Binning of a sensor mosaic to co-sited RGB ([`BinnedSensor`]) for reduced
//! proxies.

use crate::pipeline::{AiDenoisedImage, BinnedSensor, CfaKind, LoadedRaw};
use rayon::prelude::*;
use std::ops::{Range, RangeInclusive};
use std::sync::atomic::{AtomicBool, Ordering};

/// The colour a CFA channel records: the second Bayer green is green.
const CFA_CHANNEL_COLOR: [u8; 4] = [0, 1, 2, 1];
/// The colour index of green.
const GREEN: u8 = 1;

/// Output rows binned together, so that the source rows they share are
/// decoded once ([`Binning::contribution_rows`]).
const BAND_ROWS: usize = 8;

/// One axis of the filter for one output pixel: the source positions it
/// reads and their weights.
type Taps = Vec<(u32, f32)>;

/// A binned region of a mosaic.
pub(super) struct BinnedMosaic {
    pub(super) sensor: BinnedSensor,
    /// The factor binning scales the sensor's noise variance by, per CFA
    /// channel (R, G1, B, G2), for `NoiseProfile::scaled_variance`.
    pub(super) variance_scale: [f32; 4],
}

/// Bins `region` of `raw`'s mosaic into `output` pixels of co-sited RGB.
/// Each colour is the triangle-filtered mean of its photosites around a
/// pixel's centre, with a half-width of the reduction or
/// [`min_binning_radius`], whichever is larger: every colour is estimated at
/// the same place and filtered for the output's own pixel spacing, so detail
/// finer than the output averages out instead of turning into false colour.
/// On a Bayer mosaic, red and blue are the binned green, in their units,
/// plus the filtered mean of their photosites' difference from the green at
/// them ([`green_guidance`], [`guide_by_green`]): they follow green's detail
/// as a demosaic does, so detail finer than their own photosite spacing does
/// not alias either. An AI-denoised Bayer mosaic is binned alongside with the
/// same weights. `None` once `cancel` is set, which is checked every
/// [`BAND_ROWS`] rows.
pub(super) fn bin_mosaic(
    raw: &LoadedRaw,
    [x, y, region_width, region_height]: [u32; 4],
    [width, height]: [u32; 2],
    cancel: &AtomicBool,
) -> Option<BinnedMosaic> {
    let scale_x = f64::from(region_width) / f64::from(width);
    let scale_y = f64::from(region_height) / f64::from(height);
    let radius_x = scale_x.max(min_binning_radius(raw.cfa_kind));
    let radius_y = scale_y.max(min_binning_radius(raw.cfa_kind));
    // The filter's columns depend only on the output column, so they are
    // worked out once instead of for every row.
    let mut column_taps: Vec<Taps> = (0..width)
        .map(|px| triangle_taps(x, px, scale_x, radius_x, raw.width))
        .collect();
    // The source columns the taps reach, and one more on each side for the
    // greens beside each photosite.
    let span_start = column_taps
        .iter()
        .find_map(|taps| taps.first())
        .map_or(0, |&(sx, _)| sx);
    let span_end = column_taps
        .iter()
        .rev()
        .find_map(|taps| taps.last())
        .map_or(0, |&(sx, _)| sx + 1);
    let decoded = span_start.saturating_sub(1)..(span_end + 1).min(raw.width);
    for (sx, _) in column_taps.iter_mut().flatten() {
        *sx -= decoded.start;
    }
    let ai_image = raw.ai_denoised_image();
    let binning = Binning {
        raw,
        ai_mosaic: ai_image.as_ref().and_then(AiDenoisedImage::bayer_cfa),
        span: (span_start - decoded.start) as usize..(span_end - decoded.start) as usize,
        decoded,
        column_taps,
        origin_y: y,
        scale_y,
        radius_y,
        guidance: green_guidance(raw),
    };

    // One plane of interleaved RGB rows for the sensor and one for the
    // AI-denoised mosaic, handed out to bands of rows that bin in parallel.
    let plane_len = width as usize * height as usize * 3;
    let band_len = width as usize * 3 * BAND_ROWS;
    let mut planes: Vec<Vec<f32>> = (0..binning.plane_count())
        .map(|_| vec![0.0; plane_len])
        .collect();
    let mut bands: Vec<Vec<&mut [f32]>> = (0..(height as usize).div_ceil(BAND_ROWS))
        .map(|_| Vec::with_capacity(planes.len()))
        .collect();
    for plane in &mut planes {
        for (band, rows) in bands.iter_mut().zip(plane.chunks_mut(band_len)) {
            band.push(rows);
        }
    }
    let inverse_counts = bands
        .into_par_iter()
        .enumerate()
        .map(|(band, mut rows)| {
            if cancel.load(Ordering::Relaxed) {
                return [0.0; 3];
            }
            binning.bin_band((band * BAND_ROWS) as u32, &mut rows)
        })
        .reduce(
            || [0.0f64; 3],
            |sum, band| std::array::from_fn(|color| sum[color] + band[color]),
        );
    if cancel.load(Ordering::Relaxed) {
        return None;
    }

    // A binned colour is a weighted mean of independent photosites, so its
    // variance is the sensor's times Σw²/(Σw)², averaged over the pixels. A
    // guided red is that mean plus the binned green less the mean green at
    // its photosites; both greens average nearly the same photosites, so it
    // keeps about the variance of the mean, as does blue.
    let pixels = f64::from(width) * f64::from(height);
    let [red, green, blue] = inverse_counts.map(|sum| (sum / pixels) as f32);
    let mut planes = planes.into_iter();
    let rgb = planes.next()?;
    Some(BinnedMosaic {
        sensor: BinnedSensor::new(rgb, planes.next()),
        variance_scale: [red, green, blue, green],
    })
}

/// The half-width, in photosites, of the binning filter at least: the
/// spacing of same-coloured photosites along an axis, so that every colour
/// has samples on both sides of a pixel's centre. Every 2×2 Bayer and 3×3
/// X-Trans window holds all three colours.
fn min_binning_radius(cfa_kind: CfaKind) -> f64 {
    match cfa_kind {
        CfaKind::Bayer => 2.0,
        CfaKind::XTrans => 3.0,
    }
}

/// One axis of the triangle filter for output pixel `index`: the source
/// positions in the window, clamped to `0..limit`, and their weights.
fn triangle_taps(origin: u32, index: u32, scale: f64, radius: f64, limit: u32) -> Taps {
    let center = f64::from(origin) + (f64::from(index) + 0.5) * scale;
    let first = (center - radius).floor().max(0.0) as u32;
    let last = ((center + radius).ceil().max(0.0) as u32).min(limit.saturating_sub(1));
    (first..=last)
        .filter_map(|position| {
            let weight = 1.0 - ((f64::from(position) + 0.5) - center).abs() / radius;
            (weight > 0.0).then_some((position, weight as f32))
        })
        .collect()
}

/// Whether red and blue are binned as differences from green, and if so
/// the scale of green in each colour's units: the as-shot white balance of
/// green relative to that colour's, so green times it is what a neutral
/// photosite of the colour records. The differences are taken in these
/// units, in which neutral detail has none, while the binned values stay in
/// sensor units, so a later white balance still applies to them.
///
/// Only Bayer mosaics are guided. An X-Trans red or blue photosite has green
/// on both sides along one axis only, so the green at it cannot follow detail
/// along the other and guided colours fringe more than independently binned
/// ones.
fn green_guidance(raw: &LoadedRaw) -> Option<[f32; 3]> {
    if raw.cfa_kind != CfaKind::Bayer {
        return None;
    }
    let usable = |gain: f32| gain.is_finite() && gain > 0.0;
    let green = raw.wb_coeffs[usize::from(GREEN)];
    Some(std::array::from_fn(|color| {
        let gain = raw.wb_coeffs[color];
        if usable(gain) && usable(green) {
            green / gain
        } else {
            1.0
        }
    }))
}

/// What every band of one [`bin_mosaic`] shares.
struct Binning<'a> {
    raw: &'a LoadedRaw,
    ai_mosaic: Option<&'a [u16]>,
    /// The source columns decoded for every row. Decoded rows and column
    /// taps count their columns from its start.
    decoded: Range<u32>,
    /// The decoded columns the column taps reach.
    span: Range<usize>,
    /// The taps of every output column.
    column_taps: Vec<Taps>,
    origin_y: u32,
    scale_y: f64,
    radius_y: f64,
    guidance: Option<[f32; 3]>,
}

impl Binning<'_> {
    /// The planes binned: the sensor and, when there is one, the AI-denoised
    /// mosaic.
    fn plane_count(&self) -> usize {
        1 + usize::from(self.ai_mosaic.is_some())
    }

    /// Bins the band of output rows from `first_py` into `planes`, each
    /// plane's interleaved RGB rows of the band. Returns, per colour, the sum
    /// over the band's pixels of Σw²/(Σw)².
    fn bin_band(&self, first_py: u32, planes: &mut [&mut [f32]]) -> [f64; 3] {
        let row_len = self.column_taps.len() * 3;
        let row_count = planes.first().map_or(0, |rows| rows.len() / row_len);
        let row_taps: Vec<Taps> = (first_py..first_py + row_count as u32)
            .map(|py| {
                triangle_taps(
                    self.origin_y,
                    py,
                    self.scale_y,
                    self.radius_y,
                    self.raw.height,
                )
            })
            .collect();
        let tapped = row_taps.iter().flatten().map(|&(sy, _)| sy);
        let (Some(first_row), Some(last_row)) = (tapped.clone().min(), tapped.max()) else {
            return [0.0; 3];
        };
        let source = self.contribution_rows(first_row..=last_row);
        let mut sums = RowSums::default();
        let mut inverse_counts = [0.0f64; 3];
        for (row, taps) in row_taps.iter().enumerate() {
            sums.accumulate(&source, first_row, &self.column_taps, taps);
            sums.write_row(self.guidance, planes, row * row_len);
            for (count, row_count) in inverse_counts.iter_mut().zip(sums.inverse_counts()) {
                *count += row_count;
            }
        }
        inverse_counts
    }

    /// Source rows `rows`, as each photosite contributes to its colour's
    /// mean: its normalized value, or with guidance, as [`guide_by_green`]
    /// turns it. Guided rows decode the rows beside them too, for the greens
    /// above and below each photosite.
    fn contribution_rows(&self, rows: RangeInclusive<u32>) -> Vec<SensorRow> {
        let mut scratch = RowScratch::default();
        let mut decode = |y| SensorRow::decode(self, y, &mut scratch);
        let Some(green_scale) = self.guidance else {
            return rows.map(decode).collect();
        };
        let first_decoded = rows.start().saturating_sub(1);
        let last_decoded = (*rows.end() + 1).min(self.raw.height - 1);
        let mut sensor: Vec<SensorRow> = (first_decoded..=last_decoded).map(&mut decode).collect();
        let first = (rows.start() - first_decoded) as usize;
        let last = (rows.end() - first_decoded) as usize;
        for index in first..=last {
            let (before, rest) = sensor.split_at_mut(index);
            if let Some((row, after)) = rest.split_first_mut() {
                guide_by_green(
                    before.last(),
                    row,
                    after.first(),
                    self.span.clone(),
                    green_scale,
                );
            }
        }
        sensor.truncate(last + 1);
        sensor.drain(..first);
        sensor
    }
}

/// A source row of a mosaic over the decoded columns ([`Binning::decoded`]):
/// each photosite's colour, then a plane of values for each binned plane.
struct SensorRow {
    colors: Vec<u8>,
    planes: Vec<Vec<f32>>,
}

/// Buffers [`SensorRow::decode`] reuses from row to row.
#[derive(Default)]
struct RowScratch {
    channels: Vec<u8>,
    blacks: Vec<f32>,
}

impl SensorRow {
    /// Row `y`, normalized like `raw_sensor_at` (raw_sampling.wgsl) at black
    /// point 0: each photosite with its own black level and its channel's
    /// white level.
    fn decode(binning: &Binning<'_>, y: u32, scratch: &mut RowScratch) -> Self {
        let raw = binning.raw;
        let RowScratch { channels, blacks } = scratch;
        channels.clear();
        raw.color_indices.append_row_to(y, channels);
        blacks.clear();
        raw.black_levels_per_pixel.append_row_to(y, blacks);
        let columns = binning.decoded.start as usize..binning.decoded.end as usize;
        let row_start = y as usize * raw.width as usize;
        let channel_at = |x: usize| usize::from(channels[x].min(3));
        let normalize = |samples: &[u16]| -> Vec<f32> {
            columns
                .clone()
                .map(|x| {
                    let black = blacks[x];
                    let white = raw.white_levels[channel_at(x)].max(black + 1.0);
                    (f32::from(samples[row_start + x]) - black) / (white - black)
                })
                .collect()
        };
        let sensor: &[u16] = &raw.raw_pixels;
        Self {
            colors: columns
                .clone()
                .map(|x| CFA_CHANNEL_COLOR[channel_at(x)])
                .collect(),
            planes: std::iter::once(sensor)
                .chain(binning.ai_mosaic)
                .map(normalize)
                .collect(),
        }
    }
}

/// Turns the values of `row` in `columns` into what each photosite adds to
/// its colour's mean when red and blue are guided by green
/// ([`green_guidance`]): a green its value, a red or blue its difference
/// from the green at it, times its colour's `green_scale`. That green is
/// interpolated from the green neighbours along the axis where green changes
/// less, so that colour stays smooth across fine detail; they come from
/// `above`, `below` and the row's other decoded columns. Only red and blue
/// change and only greens are read, so the rows beside may already be
/// turned.
fn guide_by_green(
    above: Option<&SensorRow>,
    row: &mut SensorRow,
    below: Option<&SensorRow>,
    columns: Range<usize>,
    green_scale: [f32; 3],
) {
    let SensorRow { colors, planes } = row;
    for (plane, values) in planes.iter_mut().enumerate() {
        let green_beside = |row: Option<&SensorRow>, x: usize| {
            row.filter(|row| row.colors[x] == GREEN)
                .map(|row| row.planes[plane][x])
        };
        // A neighbouring pair's mean green and how much green changes across
        // it; a lone green has no known change.
        let pair = |a: Option<f32>, b: Option<f32>| match (a, b) {
            (Some(a), Some(b)) => Some((0.5 * (a + b), (a - b).abs())),
            (Some(green), None) | (None, Some(green)) => Some((green, f32::INFINITY)),
            (None, None) => None,
        };
        for x in columns.clone() {
            if colors[x] == GREEN {
                continue;
            }
            let green_at = |x: usize| (colors[x] == GREEN).then(|| values[x]);
            let left = x.checked_sub(1).and_then(green_at);
            let right = (x + 1 < colors.len()).then(|| green_at(x + 1)).flatten();
            let green = match (
                pair(left, right),
                pair(green_beside(above, x), green_beside(below, x)),
            ) {
                (Some((horizontal, dh)), Some((vertical, dv))) => {
                    if dh < dv {
                        horizontal
                    } else if dv < dh {
                        vertical
                    } else {
                        0.5 * (horizontal + vertical)
                    }
                }
                (Some((green, _)), None) | (None, Some((green, _))) => green,
                // No green beside it in a malformed CFA map: no difference.
                (None, None) => values[x],
            };
            values[x] -= green_scale[usize::from(colors[x])] * green;
        }
    }
}

/// The filter sums of one output row, per output pixel and colour, reused
/// from row to row.
#[derive(Default)]
struct RowSums {
    /// Σw·contribution of each plane, plane after plane.
    values: Vec<[f32; 3]>,
    /// Σw.
    weights: Vec<[f32; 3]>,
    /// Σw².
    squares: Vec<[f32; 3]>,
}

impl RowSums {
    /// Filters the contributions in `source`, which starts at source row
    /// `first_row`, with `column_taps` and `row_taps`.
    fn accumulate(
        &mut self,
        source: &[SensorRow],
        first_row: u32,
        column_taps: &[Taps],
        row_taps: &[(u32, f32)],
    ) {
        let pixels = column_taps.len();
        let planes = source.first().map_or(0, |row| row.planes.len());
        for sums in [&mut self.values, &mut self.weights, &mut self.squares] {
            sums.clear();
        }
        self.values.resize(pixels * planes, [0.0; 3]);
        self.weights.resize(pixels, [0.0; 3]);
        self.squares.resize(pixels, [0.0; 3]);
        for &(y, weight_y) in row_taps {
            let row = &source[(y - first_row) as usize];
            for (px, taps) in column_taps.iter().enumerate() {
                for &(x, weight_x) in taps {
                    let x = x as usize;
                    let color = usize::from(row.colors[x]);
                    let weight = weight_x * weight_y;
                    for (plane, values) in row.planes.iter().enumerate() {
                        self.values[plane * pixels + px][color] += weight * values[x];
                    }
                    self.weights[px][color] += weight;
                    self.squares[px][color] += weight * weight;
                }
            }
        }
    }

    /// Writes each plane's means as interleaved RGB from `offset` of its
    /// output in `planes`. Guided red and blue are differences from green,
    /// so green is added back to them. The window holds every colour of a
    /// Bayer or X-Trans mosaic ([`min_binning_radius`]); one a malformed CFA
    /// map leaves out adds nothing.
    fn write_row(&self, guidance: Option<[f32; 3]>, planes: &mut [&mut [f32]], offset: usize) {
        let pixels = self.weights.len();
        let green_scale = guidance.unwrap_or_default();
        let green = usize::from(GREEN);
        for (plane, output) in planes.iter_mut().enumerate() {
            let values = &self.values[plane * pixels..(plane + 1) * pixels];
            for (px, (sums, weights)) in values.iter().zip(&self.weights).enumerate() {
                let mean = |color: usize| {
                    if weights[color] > 0.0 {
                        sums[color] / weights[color]
                    } else {
                        0.0
                    }
                };
                for color in 0..3 {
                    let scale = if color == green {
                        0.0
                    } else {
                        green_scale[color]
                    };
                    output[offset + px * 3 + color] = scale * mean(green) + mean(color);
                }
            }
        }
    }

    /// Per colour, the sum over the row's pixels of Σw²/(Σw)².
    fn inverse_counts(&self) -> [f64; 3] {
        let mut inverse_counts = [0.0f64; 3];
        for (weights, squares) in self.weights.iter().zip(&self.squares) {
            for color in 0..3 {
                if weights[color] > 0.0 {
                    inverse_counts[color] +=
                        f64::from(squares[color] / (weights[color] * weights[color]));
                }
            }
        }
        inverse_counts
    }
}
