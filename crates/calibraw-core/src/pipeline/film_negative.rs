//! Scans of film negatives: the positive the rest of the pipeline develops
//! from a negative, and the estimate of its settings from the scan.
//!
//! A negative's density above the unexposed film, its base, grows with the
//! logarithm of the scene's exposure. The scene's exposure is therefore a
//! power of the light the film passes relative to the base, per channel:
//!
//! ```text
//! density  = -log2(camera / base)            stops above the base
//! positive = 2^print_exposure * (2^(exponent * density) - 1)
//! ```
//!
//! `camera` is linear camera RGB before white balance: normalized sensor
//! values (black 0, white level 1) of a RAW mosaic, or the linear values of a
//! raster. Dividing by the base removes the orange mask of colour film and
//! the camera's white balance. The power gives the exposure relative to the
//! least the film recorded, at the base; subtracting that, as a RAW's black
//! level is subtracted, makes unexposed film neutral black. The exponents
//! undo the film's contrast; red and blue differ from green so that the
//! densest parts of the scan, its highlights, come out neutral too.
//! `film_positive` in scene_source.wgsl applies this on the GPU and
//! [`FilmNegative::positive`] on the CPU, for pickers.

use super::sigmoid::MIDDLE_GREY;
use super::{CfaKind, ExposureParams, GeometryTransform, LoadedRaw};
use crate::pipeline::geometry::GeometryInverseMap;
use serde::{Deserialize, Serialize};

/// How the source is read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FilmSource {
    /// A photo, or a scan of a print or slide, developed as it is.
    #[default]
    Positive,
    /// A scan of a film negative, developed as its positive.
    Negative(FilmNegative),
    /// A scan of a film negative whose settings are estimated when its photo
    /// is next decoded ([`ExposureParams::settle_film`]): the Library marks
    /// photos so without reading them. Renders as it is until then.
    NegativeToEstimate(FilmStock),
}

impl FilmSource {
    pub fn is_positive(&self) -> bool {
        matches!(self, Self::Positive)
    }

    pub fn negative(&self) -> Option<&FilmNegative> {
        match self {
            Self::Positive | Self::NegativeToEstimate(_) => None,
            Self::Negative(negative) => Some(negative),
        }
    }
}

/// The kind of film a negative was made on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FilmStock {
    #[default]
    Color,
    /// Inverted from one density, the weighted mean of the channels'
    /// ([`MONOCHROME_DENSITY_WEIGHTS`]), so stains and the scanner's colour
    /// leave no tint.
    BlackAndWhite,
}

/// The settings that turn a negative into its positive (see the module
/// documentation).
#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
pub struct FilmNegative {
    pub stock: FilmStock,
    /// Light through unexposed film in red, green and blue, in linear camera
    /// units before white balance. The positive is black there.
    pub base: [f32; 3],
    /// The exponent of each channel's density. Green's sets the contrast of
    /// the positive; red's and blue's also balance the film's dye layers.
    pub exponents: [f32; 3],
    /// EV applied to the positive, which puts the scan's middle tones near
    /// scene middle grey.
    pub print_exposure: f32,
}

/// Exponent of green: colour negative films record about 0.6 density per
/// decade of exposure, so its reciprocal restores the scene's contrast.
pub const FILM_CONTRAST: f32 = 1.0 / 0.6;

/// Weights of red, green and blue in the density of black-and-white film and
/// in the brightness of scan samples. Matches `FILM_MONOCHROME_WEIGHTS` in
/// scene_source.wgsl.
pub const MONOCHROME_DENSITY_WEIGHTS: [f32; 3] = [0.25, 0.5, 0.25];

/// The least light, relative to the base, a sample is taken to pass: 14 stops
/// of density, beyond any film. Keeps opaque surroundings and noise below
/// black finite. Matches `FILM_MIN_TRANSMITTANCE` in scene_source.wgsl.
pub const FILM_MIN_TRANSMITTANCE: f32 = 1.0 / 16384.0;

/// Bounds that keep a hand-edited or damaged sidecar renderable. The largest
/// exponent at the 14 stops of [`FILM_MIN_TRANSMITTANCE`] plus the largest
/// print exposure stays below 2^128, within f32.
const MIN_BASE: f32 = 1e-6;
const EXPONENT_RANGE: std::ops::RangeInclusive<f32> = 0.2..=8.0;
const PRINT_EXPOSURE_RANGE: std::ops::RangeInclusive<f32> = -40.0..=8.0;
/// Red and blue exponents stay within this factor of green's.
const MAX_CHANNEL_RATIO: f32 = 2.0;
/// Highlight density, in stops, that a channel and green both need before
/// the channel is balanced against green; thinner film shows too little of
/// its dyes, and the channel keeps green's exponent.
const MIN_BALANCE_DENSITY: f32 = 0.25;

/// Sample values from this fraction of the white level up are clipped: light
/// around the film, which tells nothing about it.
const CLIPPED: f32 = 0.97;
/// Samples along each axis of the region an estimate reads.
const SAMPLE_GRID_EDGE: u32 = 256;
/// Source pixels along each axis a picked area covers at least, so a click
/// without a drag reads the few pixels around it.
const MIN_AREA_EDGE: f32 = 6.0;
/// Share of the brightest samples averaged into an estimated base.
const BASE_SHARE: f32 = 0.005;
/// Percentile of each channel's density taken as the scan's highlights.
const HIGHLIGHT_PERCENTILE: f32 = 0.995;
/// Fewer samples than this inside the picture give no estimate.
const MIN_PICTURE_SAMPLES: usize = 64;

impl FilmNegative {
    /// Estimates a negative of `stock` from `samples` of its scan (linear
    /// camera RGB, see [`LoadedRaw::film_samples_in_view`]). The base is
    /// `base`, or else the mean of the brightest samples: in a scan that
    /// shows some film border those are unexposed film, otherwise the
    /// thinnest shadows, which lie just above it. `None` when the samples do
    /// not show enough of a picture.
    pub fn estimate(
        stock: FilmStock,
        samples: &[[f32; 3]],
        base: Option<[f32; 3]>,
    ) -> Option<Self> {
        let base = match base {
            Some(base) => base,
            None => brightest_mean(samples)?,
        };
        if !base
            .iter()
            .all(|value| value.is_finite() && *value > MIN_BASE)
        {
            return None;
        }
        let densities: Vec<[f32; 3]> = samples
            .iter()
            .map(|sample| density(*sample, base).map(|value| value.max(0.0)))
            .collect();
        let weighted_densities: Vec<f32> =
            densities.iter().map(|density| weighted(*density)).collect();
        let surround = surround_density(&weighted_densities);
        let picture: Vec<[f32; 3]> = densities
            .iter()
            .zip(&weighted_densities)
            .filter(|(_, weighted_density)| **weighted_density < surround)
            .map(|(density, _)| *density)
            .collect();
        if picture.len() < MIN_PICTURE_SAMPLES {
            return None;
        }

        let exponents = match stock {
            FilmStock::BlackAndWhite => [FILM_CONTRAST; 3],
            FilmStock::Color => {
                let highlights: [f32; 3] = std::array::from_fn(|channel| {
                    percentile(
                        picture.iter().map(|density| density[channel]),
                        HIGHLIGHT_PERCENTILE,
                    )
                });
                std::array::from_fn(|channel| {
                    let ratio = if highlights[channel] > MIN_BALANCE_DENSITY
                        && highlights[1] > MIN_BALANCE_DENSITY
                    {
                        highlights[1] / highlights[channel]
                    } else {
                        1.0
                    };
                    FILM_CONTRAST * ratio.clamp(1.0 / MAX_CHANNEL_RATIO, MAX_CHANNEL_RATIO)
                })
            }
        };
        let mut negative = Self {
            stock,
            base,
            exponents,
            print_exposure: 0.0,
        };
        let median = percentile(
            picture
                .iter()
                .map(|density| weighted(negative.positive_from_density(*density))),
            0.5,
        );
        // A picture that is mostly unexposed film leaves no median to place.
        if median <= f32::EPSILON {
            return None;
        }
        // The scan's median goes to scene middle grey.
        negative.print_exposure = MIDDLE_GREY.log2() - median.log2();
        Some(negative.sanitized())
    }

    /// The positive of linear camera RGB `camera` before white balance, as
    /// `film_positive` in scene_source.wgsl renders it without the white
    /// balance it applies afterwards.
    pub fn positive(&self, camera: [f32; 3]) -> [f32; 3] {
        self.positive_from_density(density(camera, self.base))
    }

    fn positive_from_density(&self, density: [f32; 3]) -> [f32; 3] {
        let density = match self.stock {
            FilmStock::Color => density,
            FilmStock::BlackAndWhite => [weighted(density); 3],
        };
        std::array::from_fn(|channel| {
            self.print_exposure.exp2() * ((self.exponents[channel] * density[channel]).exp2() - 1.0)
        })
    }

    /// This negative within the bounds the renderer supports.
    pub fn sanitized(self) -> Self {
        let finite_or =
            |value: f32, fallback: f32| if value.is_finite() { value } else { fallback };
        Self {
            stock: self.stock,
            base: self.base.map(|value| finite_or(value, 1.0).max(MIN_BASE)),
            exponents: self.exponents.map(|value| {
                finite_or(value, FILM_CONTRAST)
                    .clamp(*EXPONENT_RANGE.start(), *EXPONENT_RANGE.end())
            }),
            print_exposure: finite_or(self.print_exposure, 0.0)
                .clamp(*PRINT_EXPOSURE_RANGE.start(), *PRINT_EXPOSURE_RANGE.end()),
        }
    }

    /// The base of an area chosen as unexposed film: the mean of its
    /// `samples` ([`LoadedRaw::film_samples_in_area`]), or `None` without any.
    pub fn base_of_area(samples: &[[f32; 3]]) -> Option<[f32; 3]> {
        mean(samples)
    }
}

/// What [`ExposureParams::settle_film`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilmSettlement {
    /// There was no negative to estimate.
    Unchanged,
    Estimated,
    /// The photo showed no negative, so it is developed as it is.
    NotFound,
}

impl ExposureParams {
    /// Estimates a [`FilmSource::NegativeToEstimate`] from `raw`, the photo's
    /// decoded source, inside the crop of `geometry`, as the Film negative
    /// card's Auto does. Every other source stays as it is.
    pub fn settle_film(&mut self, raw: &LoadedRaw, geometry: GeometryTransform) -> FilmSettlement {
        let FilmSource::NegativeToEstimate(stock) = self.film else {
            return FilmSettlement::Unchanged;
        };
        let samples = raw.film_samples_in_view(geometry, self.black_point);
        match FilmNegative::estimate(stock, &samples, None) {
            Some(negative) => {
                self.film = FilmSource::Negative(negative);
                FilmSettlement::Estimated
            }
            None => {
                self.film = FilmSource::Positive;
                FilmSettlement::NotFound
            }
        }
    }
}

/// Stops of density of `camera` above `base`, per channel; negative where the
/// scan is brighter than the base.
fn density(camera: [f32; 3], base: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|channel| {
        -(camera[channel] / base[channel].max(MIN_BASE))
            .max(FILM_MIN_TRANSMITTANCE)
            .log2()
    })
}

fn weighted(values: [f32; 3]) -> f32 {
    values
        .iter()
        .zip(MONOCHROME_DENSITY_WEIGHTS)
        .map(|(value, weight)| value * weight)
        .sum()
}

fn mean(samples: &[[f32; 3]]) -> Option<[f32; 3]> {
    if samples.is_empty() {
        return None;
    }
    let mut sum = [0.0f64; 3];
    for sample in samples {
        for (total, value) in sum.iter_mut().zip(sample) {
            *total += f64::from(*value);
        }
    }
    Some(sum.map(|total| (total / samples.len() as f64) as f32))
}

/// The mean of the brightest [`BASE_SHARE`] of `samples`, at least one.
fn brightest_mean(samples: &[[f32; 3]]) -> Option<[f32; 3]> {
    let mut sorted = samples.to_vec();
    sorted.sort_by(|a, b| weighted(*b).total_cmp(&weighted(*a)));
    let count = ((sorted.len() as f32 * BASE_SHARE).ceil() as usize).min(sorted.len());
    mean(&sorted[..count])
}

/// The `fraction` (0–1) percentile of `values`; 0 without any.
fn percentile(values: impl Iterator<Item = f32>, fraction: f32) -> f32 {
    let mut values: Vec<f32> = values.filter(|value| value.is_finite()).collect();
    if values.is_empty() {
        return 0.0;
    }
    let index = ((values.len() - 1) as f32 * fraction.clamp(0.0, 1.0)).round() as usize;
    let (_, value, _) = values.select_nth_unstable_by(index, f32::total_cmp);
    *value
}

/// The density, in stops above the base, from which samples are taken to show
/// the film holder or other opaque surroundings rather than the picture: the
/// start of the first stop-wide, nearly empty stretch of the density
/// histogram above its median. Opaque surroundings are far denser than any
/// exposed film, with few densities in between. Infinite when there is no
/// such stretch.
fn surround_density(densities: &[f32]) -> f32 {
    const BIN_STOPS: f32 = 0.25;
    const BINS: usize = 64;
    const EMPTY_RUN: usize = 4;
    // Blurred edges of the surroundings and printing on the film rim leave a
    // few samples in the gap.
    const EMPTY_SHARE: f32 = 0.01;
    let mut histogram = [0usize; BINS];
    for density in densities {
        let bin = ((density / BIN_STOPS) as usize).min(BINS - 1);
        histogram[bin] += 1;
    }
    let empty = (densities.len() as f32 * EMPTY_SHARE) as usize;
    let mut below = 0;
    let mut run = 0;
    for (bin, count) in histogram.iter().enumerate() {
        if below * 2 < densities.len() {
            below += count;
            continue;
        }
        run = if *count <= empty { run + 1 } else { 0 };
        if run == EMPTY_RUN {
            return (bin + 1 - EMPTY_RUN) as f32 * BIN_STOPS;
        }
    }
    f32::INFINITY
}

impl LoadedRaw {
    /// Samples of the scan inside the photo's crop under `geometry`, on a
    /// grid of at most [`SAMPLE_GRID_EDGE`] per axis, for
    /// [`FilmNegative::estimate`]. Clipped samples are left out.
    pub fn film_samples_in_view(
        &self,
        geometry: GeometryTransform,
        black_point: f32,
    ) -> Vec<[f32; 3]> {
        let (output_width, output_height) = geometry.crop_pixel_dimensions(self.width, self.height);
        let inverse_map = GeometryInverseMap::new_with_lens(
            geometry,
            self.lens_geometry.as_deref(),
            self.width,
            self.height,
            output_width,
            output_height,
        );
        let columns = output_width.min(SAMPLE_GRID_EDGE);
        let rows = output_height.min(SAMPLE_GRID_EDGE);
        let mut samples = Vec::with_capacity((columns * rows) as usize);
        for row in 0..rows {
            for column in 0..columns {
                let x = (column as f32 + 0.5) * output_width as f32 / columns as f32 - 0.5;
                let y = (row as f32 + 0.5) * output_height as f32 / rows as f32 - 0.5;
                let [source_x, source_y] = inverse_map.source_position(x, y);
                if let Some(sample) = self.film_sample_near(source_x, source_y, black_point) {
                    samples.push(sample);
                }
            }
        }
        samples
    }

    /// Samples of the scan inside the source rectangle from `first` to
    /// `second`, in coordinates normalized to the source, as an area picker
    /// reports it. Clipped samples are left out.
    pub fn film_samples_in_area(
        &self,
        first: [f32; 2],
        second: [f32; 2],
        black_point: f32,
    ) -> Vec<[f32; 3]> {
        let to_pixels = |u: f32, extent: u32| u.clamp(0.0, 1.0) * extent as f32;
        let x0 = to_pixels(first[0].min(second[0]), self.width);
        let x1 = to_pixels(first[0].max(second[0]), self.width);
        let y0 = to_pixels(first[1].min(second[1]), self.height);
        let y1 = to_pixels(first[1].max(second[1]), self.height);
        let (x0, x1) = widen(x0, x1, MIN_AREA_EDGE, self.width);
        let (y0, y1) = widen(y0, y1, MIN_AREA_EDGE, self.height);
        let edge = SAMPLE_GRID_EDGE / 2;
        let columns = ((x1 - x0).ceil() as u32).clamp(1, edge);
        let rows = ((y1 - y0).ceil() as u32).clamp(1, edge);
        let mut samples = Vec::with_capacity((columns * rows) as usize);
        for row in 0..rows {
            for column in 0..columns {
                let x = x0 + (column as f32 + 0.5) * (x1 - x0) / columns as f32;
                let y = y0 + (row as f32 + 0.5) * (y1 - y0) / rows as f32;
                if let Some(sample) = self.film_sample_near(x - 0.5, y - 0.5, black_point) {
                    samples.push(sample);
                }
            }
        }
        samples
    }

    /// Linear camera RGB before white balance near source pixel position
    /// `(x, y)`: the pixel of a raster, or the mean of each channel over the
    /// CFA period of a mosaic that holds it. `None` outside the source or
    /// where a channel is clipped.
    fn film_sample_near(&self, x: f32, y: f32, black_point: f32) -> Option<[f32; 3]> {
        if !(x.is_finite() && y.is_finite()) {
            return None;
        }
        let (x, y) = (x.round(), y.round());
        if x < 0.0 || y < 0.0 || x >= self.width as f32 || y >= self.height as f32 {
            return None;
        }
        let (x, y) = (x as u32, y as u32);
        let width = self.width as usize;
        let sample = if let Some(raster) = self
            .scene_linear_raster()
            .filter(|_| self.is_pre_demosaiced_raster())
        {
            let start = (y as usize * width + x as usize) * 3;
            [raster[start], raster[start + 1], raster[start + 2]]
        } else if !self.raw_pixels.is_empty() {
            let period = match self.cfa_kind {
                CfaKind::Bayer => 2,
                CfaKind::XTrans => 6,
            };
            let (left, top) = (x - x % period, y - y % period);
            let mut sums = [0.0f32; 3];
            let mut counts = [0u32; 3];
            for row in top..(top + period).min(self.height) {
                for column in left..(left + period).min(self.width) {
                    let index = row as usize * width + column as usize;
                    // The mosaic's second green (3) counts as green.
                    let channel = [0, 1, 2, 1][usize::from(self.color_indices[index].min(3))];
                    sums[channel] += self.normalized_sensor_value(index, black_point);
                    counts[channel] += 1;
                }
            }
            if counts.contains(&0) {
                return None;
            }
            std::array::from_fn(|channel| sums[channel] / counts[channel] as f32)
        } else {
            return None;
        };
        (sample
            .iter()
            .all(|value| value.is_finite() && *value < CLIPPED))
        .then_some(sample)
    }
}

/// `start..end` widened about its centre to at least `minimum`, within
/// `0..extent`.
fn widen(start: f32, end: f32, minimum: f32, extent: u32) -> (f32, f32) {
    if end - start >= minimum {
        return (start, end);
    }
    let centre = 0.5 * (start + end);
    let extent = extent as f32;
    let start = (centre - 0.5 * minimum).clamp(0.0, (extent - minimum).max(0.0));
    (start, (start + minimum).min(extent))
}
