// SPDX-License-Identifier: GPL-3.0-or-later
// Hot pixel detection follows darktable's hot pixels module
// (src/iop/hotpixels.c). Copyright (C) 2011-2026 darktable developers.
// The repair follows RawTherapee's bad pixel interpolation
// (rtengine/badpixels.cc, `interpolateBadPixelsBayer`).
// Copyright (C) 2004-2019 Gabor Horvath and the RawTherapee developers.
// Copyright (C) 2026 CalibRaw contributors (Rust adaptation).

//! Defective photosites of a sensor mosaic, and their repair.
//!
//! A photosite is defective when its sample does not measure the light that
//! reached it: one the RAW declares (LibRaw's zero samples and DNG bad pixel
//! lists, repaired as the RAW decodes), or a hot pixel found in the mosaic
//! ([`remove_hot_pixels`], an edit's source correction). [`repair`] replaces
//! each from the photosites of its colour around it and never from another
//! defect, so the order in which defects are repaired does not matter.
//!
//! Both read samples as `raw_sensor_at` (raw_sampling.wgsl) does at black
//! point 0: a sample's distance above its black level as a share of its
//! channel's range to white, before white balance.

use super::{CfaKind, CompactPixelMap, LoadedRaw, PixelLayout};
use rayon::prelude::*;
use std::time::Instant;

/// Samples at or below this are never hot: darktable's default threshold,
/// which leaves the noise of the shadows alone. darktable compares after its
/// white balance; here the threshold applies before it, so what is found does
/// not depend on the edit.
const HOT_PIXEL_THRESHOLD: f32 = 0.05;
/// A photosite is hot when this share of its sample still exceeds the sample
/// of each of its four nearest photosites of the same CFA channel, that is
/// when it is more than eight times as bright as all four: darktable's
/// multiplier at its default strength, 0.25 / 2.
const HOT_PIXEL_SHARE: f32 = 0.125;
/// The offsets `(dx, dy)` darktable searches, nearest first, for a
/// photosite's four nearest photosites of its CFA channel. Searching by
/// channel, which tells the two Bayer greens apart, finds the photosites two
/// away along the row and column of a Bayer mosaic, as darktable's Bayer
/// variant compares, and the nearest of the same colour in X-Trans.
const NEIGHBOUR_SEARCH: [(isize, isize); 20] = [
    (-1, 0),
    (1, 0),
    (0, -1),
    (0, 1),
    (-1, -1),
    (-1, 1),
    (1, -1),
    (1, 1),
    (-2, 0),
    (2, 0),
    (0, -2),
    (0, 2),
    (-2, -1),
    (-2, 1),
    (2, -1),
    (2, 1),
    (-1, -2),
    (1, -2),
    (-1, 2),
    (1, 2),
];
/// Rows one worker scans for hot pixels at a time.
const SCAN_ROWS: usize = 64;
/// One side of each pair of photosites on opposite sides of a defect within
/// two photosites of it; the other side is at `(-dx, -dy)`. RawTherapee's
/// Bayer pairs are those whose photosites have the defect's colour; taking
/// every such pair serves X-Trans too.
const REPAIR_PAIRS: [(isize, isize); 12] = [
    (1, 0),
    (2, 0),
    (-2, 1),
    (-1, 1),
    (0, 1),
    (1, 1),
    (2, 1),
    (-2, 2),
    (-1, 2),
    (0, 2),
    (1, 2),
    (2, 2),
];
/// Keeps a pair whose samples are equal from weighing infinitely:
/// RawTherapee's 1 on its 16-bit scale, in normalized units.
const REPAIR_EPSILON: f32 = 1.0 / 65_535.0;

/// A set of photosites of a mosaic by row-major index, one bit each, so a
/// lookup takes constant time and the set's size does not grow with what it
/// holds.
#[derive(Clone, Debug)]
pub(crate) struct SensorDefects {
    words: Vec<u64>,
}

impl SensorDefects {
    /// No defects among `len` photosites.
    pub(crate) fn none(len: usize) -> Self {
        Self {
            words: vec![0; len.div_ceil(64)],
        }
    }

    /// The photosites whose sample is `value`.
    // Only the LibRaw decode, which builds without LibRaw leave out, uses
    // this and `add`.
    #[cfg_attr(not(libraw_available), allow(dead_code))]
    pub(crate) fn where_sample_is(samples: &[u16], value: u16) -> Self {
        let words = samples
            .par_chunks(64)
            .map(|chunk| {
                chunk.iter().enumerate().fold(0, |word, (bit, &sample)| {
                    word | u64::from(sample == value) << bit
                })
            })
            .collect();
        Self { words }
    }

    /// Adds photosite `index`, which must lie in the mosaic the set was made
    /// for.
    pub(crate) fn insert(&mut self, index: usize) {
        self.words[index / 64] |= 1 << (index % 64);
    }

    /// Adds every photosite of `other`, a set of the same mosaic.
    #[cfg_attr(not(libraw_available), allow(dead_code))]
    pub(crate) fn add(&mut self, other: &Self) {
        for (word, other) in self.words.iter_mut().zip(&other.words) {
            *word |= other;
        }
    }

    pub(crate) fn contains(&self, index: usize) -> bool {
        self.words
            .get(index / 64)
            .is_some_and(|word| word & (1 << (index % 64)) != 0)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.words.iter().all(|&word| word == 0)
    }

    pub(crate) fn count(&self) -> usize {
        self.words
            .iter()
            .map(|word| word.count_ones() as usize)
            .sum()
    }

    /// The photosites in the set, in increasing order.
    fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.words
            .iter()
            .enumerate()
            .flat_map(|(word_index, &word)| {
                let mut bits = word;
                std::iter::from_fn(move || {
                    (bits != 0).then(|| {
                        let bit = bits.trailing_zeros() as usize;
                        bits &= bits - 1;
                        word_index * 64 + bit
                    })
                })
            })
    }
}

/// How to read the samples of a mosaic: its size, each photosite's CFA
/// channel and black level and each channel's white level.
pub(crate) struct MosaicLayout<'a> {
    width: usize,
    height: usize,
    /// Red 0, green 1, blue 2 and the second Bayer green 3, as
    /// [`LoadedRaw::color_indices`].
    channels: &'a CompactPixelMap<u8>,
    blacks: &'a CompactPixelMap<f32>,
    whites: [f32; 4],
}

impl<'a> MosaicLayout<'a> {
    pub(crate) fn new(
        width: u32,
        height: u32,
        channels: &'a CompactPixelMap<u8>,
        blacks: &'a CompactPixelMap<f32>,
        whites: [f32; 4],
    ) -> Self {
        Self {
            width: width as usize,
            height: height as usize,
            channels,
            blacks,
            whites,
        }
    }

    fn of(raw: &'a LoadedRaw) -> Self {
        Self::new(
            raw.width,
            raw.height,
            &raw.color_indices,
            &raw.black_levels_per_pixel,
            raw.white_levels,
        )
    }

    fn len(&self) -> usize {
        self.width * self.height
    }

    fn channel(&self, index: usize) -> u8 {
        self.channels[index].min(3)
    }

    /// The colour photosite `index` records: its channel, both Bayer greens
    /// as green.
    fn color(&self, index: usize) -> u8 {
        match self.channel(index) {
            3 => 1,
            channel => channel,
        }
    }

    /// Black level and range to white of photosite `index`.
    fn levels(&self, index: usize) -> (f32, f32) {
        levels(
            self.blacks[index],
            self.whites[usize::from(self.channel(index))],
        )
    }

    fn normalized(&self, samples: &[u16], index: usize) -> f32 {
        let (black, range) = self.levels(index);
        (f32::from(samples[index]) - black) / range
    }

    /// The sample of photosite `index` for normalized `value`, rounded to the
    /// 16-bit codes.
    fn sample(&self, index: usize, value: f32) -> u16 {
        let (black, range) = self.levels(index);
        (black + value * range)
            .round()
            .clamp(0.0, f32::from(u16::MAX)) as u16
    }

    /// Appends row `y` of `samples`, normalized, to `values`; `rows` holds the
    /// row's channels and black levels meanwhile.
    fn append_normalized_row(
        &self,
        samples: &[u16],
        y: usize,
        rows: &mut RowScratch,
        values: &mut Vec<f32>,
    ) {
        rows.channels.clear();
        rows.blacks.clear();
        self.channels.append_row_to(y as u32, &mut rows.channels);
        self.blacks.append_row_to(y as u32, &mut rows.blacks);
        let start = y * self.width;
        values.extend(
            samples[start..start + self.width]
                .iter()
                .zip(&rows.channels)
                .zip(&rows.blacks)
                .map(|((&sample, &channel), &black)| {
                    let (black, range) = levels(black, self.whites[usize::from(channel.min(3))]);
                    (f32::from(sample) - black) / range
                }),
        );
    }

    /// The four nearest photosites of the CFA channel of each cell of the
    /// mosaic's `period`-wide pattern, by cell `(x % period) + (y % period) *
    /// period`; `None` when a cell lacks four within the search, or the
    /// mosaic is too small to find a cell's photosite with the whole search
    /// around it.
    fn neighbour_table(&self, period: usize) -> Option<Vec<[(isize, isize); 4]>> {
        if self.width < 2 * period + 2 || self.height < 2 * period + 2 {
            return None;
        }
        (0..period * period)
            .map(|cell| {
                // The cell's photosite one period in, which has the whole
                // search inside the mosaic.
                let x = cell % period + period;
                let y = cell / period + period;
                let channel = self.channel(y * self.width + x);
                let mut found = NEIGHBOUR_SEARCH.into_iter().filter(|&(dx, dy)| {
                    let neighbour_x = x.wrapping_add_signed(dx);
                    let neighbour_y = y.wrapping_add_signed(dy);
                    self.channel(neighbour_y * self.width + neighbour_x) == channel
                });
                Some([found.next()?, found.next()?, found.next()?, found.next()?])
            })
            .collect()
    }

    /// The estimate of defect `index` from the photosites of its colour
    /// around it that are not defects, normalized; `None` without any.
    ///
    /// After RawTherapee's `interpolateBadPixelsBayer`: the mean of each
    /// [`REPAIR_PAIRS`] pair, weighted by the inverse of its distance and of
    /// how much its two samples differ, so the estimate follows an edge rather
    /// than crossing it. Without a whole pair, the plain mean of the colour in
    /// the 5×5 window, as LibRaw's `remove_zeroes` takes it.
    fn estimate(&self, samples: &[u16], defects: &SensorDefects, index: usize) -> Option<f32> {
        let (x, y) = (index % self.width, index / self.width);
        let color = self.color(index);
        let sound = |dx: isize, dy: isize| -> Option<f32> {
            let neighbour_x = x.checked_add_signed(dx).filter(|&nx| nx < self.width)?;
            let neighbour_y = y.checked_add_signed(dy).filter(|&ny| ny < self.height)?;
            let neighbour = neighbour_y * self.width + neighbour_x;
            (self.color(neighbour) == color && !defects.contains(neighbour))
                .then(|| self.normalized(samples, neighbour))
        };
        let mut weighted = 0.0;
        let mut weights = 0.0;
        for (dx, dy) in REPAIR_PAIRS {
            let (Some(first), Some(second)) = (sound(dx, dy), sound(-dx, -dy)) else {
                continue;
            };
            let distance = ((dx * dx + dy * dy) as f32).sqrt();
            let weight = 1.0 / (distance * ((first - second).abs() + REPAIR_EPSILON));
            weighted += weight * (first + second);
            weights += weight;
        }
        if weights > 0.0 {
            return Some(weighted / (2.0 * weights));
        }
        let (sum, count) = (-2..=2)
            .flat_map(|dy| (-2..=2).map(move |dx| (dx, dy)))
            .filter_map(|(dx, dy)| sound(dx, dy))
            .fold((0.0, 0u32), |(sum, count), value| (sum + value, count + 1));
        (count > 0).then(|| sum / count as f32)
    }
}

/// Scratch rows for [`MosaicLayout::append_normalized_row`].
#[derive(Default)]
struct RowScratch {
    channels: Vec<u8>,
    blacks: Vec<f32>,
}

/// Black level and range to white of a photosite, as `raw_sensor_at` reads
/// them: white at least one code above black.
fn levels(black: f32, white: f32) -> (f32, f32) {
    let white = white.max(black + 1.0);
    (black, (white - black).max(1.0))
}

/// Replaces the sample of every defect of `samples`, laid out as `layout`,
/// with its estimate from the photosites of its colour around it; returns
/// how many it replaced. A defect with no sound photosite of its colour within
/// two photosites keeps its sample.
pub(crate) fn repair(
    layout: &MosaicLayout<'_>,
    samples: &mut [u16],
    defects: &SensorDefects,
) -> usize {
    let mut repaired = 0;
    // An estimate reads only sound photosites, which no repair changes.
    for index in defects.iter() {
        if let Some(value) = layout.estimate(samples, defects, index) {
            samples[index] = layout.sample(index, value);
            repaired += 1;
        }
    }
    repaired
}

/// The hot pixels of a mosaic as darktable's hot pixels module finds them at
/// its defaults: above [`HOT_PIXEL_THRESHOLD`], and with [`HOT_PIXEL_SHARE`]
/// of their sample above the sample of each of their four nearest photosites
/// of the same CFA channel. Two hot photosites that are among each other's
/// four hide each other, and the two photosites along each edge, which lack
/// some of those neighbours, are left alone, as darktable leaves them.
fn find_hot_pixels(layout: &MosaicLayout<'_>, samples: &[u16], period: usize) -> SensorDefects {
    let mut defects = SensorDefects::none(layout.len());
    // A mosaic with a table is at least 2 * period + 2 photosites high.
    let Some(neighbours) = layout.neighbour_table(period) else {
        return defects;
    };
    let bands = (2..layout.height - 2)
        .step_by(SCAN_ROWS)
        .collect::<Vec<_>>()
        .into_par_iter()
        .map(|first| {
            let last = (first + SCAN_ROWS).min(layout.height - 2);
            scan_band(layout, samples, &neighbours, period, first..last)
        })
        .collect::<Vec<_>>();
    for index in bands.into_iter().flatten() {
        defects.insert(index);
    }
    defects
}

/// The hot pixels of rows `rows` ([`find_hot_pixels`]).
fn scan_band(
    layout: &MosaicLayout<'_>,
    samples: &[u16],
    neighbours: &[[(isize, isize); 4]],
    period: usize,
    rows: std::ops::Range<usize>,
) -> Vec<usize> {
    let width = layout.width;
    // The band's rows normalized, with the two rows above and below it that
    // the neighbours reach.
    let top = rows.start - 2;
    let mut values = Vec::with_capacity((rows.len() + 4) * width);
    let mut scratch = RowScratch::default();
    for y in top..rows.end + 2 {
        layout.append_normalized_row(samples, y, &mut scratch, &mut values);
    }
    let mut hot = Vec::new();
    for y in rows {
        let row_start = (y - top) * width;
        for x in 2..width - 2 {
            let value = values[row_start + x];
            if value <= HOT_PIXEL_THRESHOLD {
                continue;
            }
            let share = value * HOT_PIXEL_SHARE;
            let cell = (y % period) * period + x % period;
            let brighter_than_all = neighbours[cell].iter().all(|&(dx, dy)| {
                let neighbour = (row_start + x)
                    .wrapping_add_signed(dy * width as isize)
                    .wrapping_add_signed(dx);
                share > values[neighbour]
            });
            if brighter_than_all {
                hot.push(y * width + x);
            }
        }
    }
    hot
}

/// `raw` with its hot pixels repaired, or `None` when it has none: a source
/// other than a sensor mosaic, or a mosaic whose photosites are all sound.
/// The repaired mosaic keeps everything else of `raw`. Like a lens-corrected
/// mosaic it replaces `raw` as the full-sensor reference, so it measures its
/// own highlight model.
pub fn remove_hot_pixels(raw: &LoadedRaw) -> Option<LoadedRaw> {
    if raw.pixel_layout() != PixelLayout::Mosaic {
        return None;
    }
    let started = Instant::now();
    let layout = MosaicLayout::of(raw);
    let period = match raw.cfa_kind {
        CfaKind::Bayer => 2,
        CfaKind::XTrans => 6,
    };
    let hot = find_hot_pixels(&layout, &raw.raw_pixels, period);
    if hot.is_empty() {
        crate::diagnostics::record(format!(
            "Hot pixel removal found none in {:.3}s",
            started.elapsed().as_secs_f64()
        ));
        return None;
    }
    let mut samples = raw.raw_pixels.to_vec();
    let repaired = repair(&layout, &mut samples, &hot);
    crate::diagnostics::record(format!(
        "Hot pixel removal repaired {repaired} of {} hot photosites in {:.3}s",
        hot.count(),
        started.elapsed().as_secs_f64()
    ));
    Some(LoadedRaw {
        lens_geometry: raw.lens_geometry.clone(),
        highlight_model_cache: Default::default(),
        highlight_source_identity: Default::default(),
        highlight_reference_source: true,
        ..raw.derive_with(
            raw.width,
            raw.height,
            samples,
            raw.color_indices.clone(),
            raw.black_levels_per_pixel.clone(),
        )
    })
}
