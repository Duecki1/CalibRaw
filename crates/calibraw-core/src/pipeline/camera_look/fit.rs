//! Fitting a [`CameraLook`] to a photo's embedded preview.
//!
//! The fit compares the camera's preview with the canonical render of the
//! photo (`ExposureParams::source_reference`, as-shot white balance), never
//! the current edit. Both are reduced to a working size of at most
//! [`WORKING_EDGE`] pixels, blurred, and the preview is aligned to the
//! render by a search over scale and offset on luminance ranks, so a
//! different crop or aspect is tolerated and tone differences do not steer
//! the alignment. Pixels clipped in either image, near black in the preview
//! or on strong edges, where a residual misalignment would dominate, are
//! left out. A quarter of the remaining pixels, in a block pattern, are held
//! out and score the fit; the rest train it with a Cauchy-weighted
//! (robust) loss.
//!
//! The stages are fitted in order: the exposure offset matching the median
//! luminance (each candidate is rendered again), the tone curve and matrix
//! alternately, then the residual grid on what they leave. The guard keeps
//! the residual only if it improves the held-out CIEDE2000 enough and the
//! baked table has no folds, falls back to the curve and matrix under the
//! same test against the canonical render, and otherwise fits no look.

use super::bake::BakeCheck;
use super::delta_e::{ciede2000, linear_srgb_to_lab};
use super::image::{
    align, blur, decode_linear, resample_area, sample_bilinear, trim_preview_borders, RgbPlane,
};
use super::solve::SymmetricMatrix;
use super::{
    trilinear_corners, CameraLook, LookCurve, ResidualGrid, LOOK_CURVE_KNOTS, MAX_LOOK_EXPOSURE_EV,
    RESIDUAL_GRID_POINTS, RESIDUAL_GRID_SIZE,
};
use crate::color_math::{srgb_decode_signed, srgb_encode, srgb_encode_signed};
use crate::matrix::{self, Matrix3, IDENTITY3};
use crate::pipeline::RawThumbnail;
use std::fmt;

pub use super::image::FitAlignment;

/// Previews with a shorter long edge are too coarse to fit.
pub const MIN_PREVIEW_EDGE: u32 = 320;
/// The long edge of the images the fit compares, in pixels.
const WORKING_EDGE: usize = 256;
/// Blur applied to both working images, in working pixels.
const BLUR_SIGMA: f32 = 1.0;
/// Alignments whose rank correlation is lower do not show the same picture.
const MIN_ALIGNMENT_CORRELATION: f32 = 0.95;
/// Encoded values at or above this are treated as clipped.
const CLIP_HIGH: f32 = 0.97;
/// Preview pixels whose brightest channel is below this are near black.
const CLIP_LOW: f32 = 0.02;
/// Encoded luminance change per working pixel above which a pixel lies on a
/// strong edge. Half a pixel of misalignment there shifts a colour by about
/// one CIEDE2000 unit.
const EDGE_GRADIENT: f32 = 0.02;
/// The fit keeps at least this share of the aligned pixels, the flattest
/// ones, however strong the edges.
const MIN_FLAT_SHARE: f32 = 0.3;
/// Side of the blocks that alternate between training and held out, in
/// working pixels.
const HOLDOUT_BLOCK: usize = 16;
/// Fewer training samples than this cannot fit a look.
const MIN_TRAINING_SAMPLES: usize = 1500;
/// CIEDE2000 at which a sample's robust weight halves.
const ROBUST_SCALE: f32 = 5.0;
/// Median luminance difference, in EV, at which the exposure search stops.
const EXPOSURE_TOLERANCE: f32 = 0.03;
/// Renders the exposure search may make after the canonical one.
const EXPOSURE_RENDERS: usize = 3;
/// Rounds of the curve and matrix alternation.
const CURVE_MATRIX_ROUNDS: usize = 4;
/// Rounds of the residual grid fit.
const RESIDUAL_ROUNDS: usize = 2;
/// Weight of the curve's second differences, per unit of training weight.
const CURVE_SMOOTHNESS: f64 = 0.02;
/// Pull of the curve knots towards the identity, per unit of training
/// weight; it only decides knots no sample reaches.
const CURVE_IDENTITY_PULL: f64 = 1e-4;
/// Pull of the matrix towards the identity, relative to the mean diagonal
/// of its normal equations.
const MATRIX_IDENTITY_PULL: f64 = 1e-3;
/// Weight of the residual grid's Laplacian, per unit of training weight.
const RESIDUAL_SMOOTHNESS: f64 = 0.01;
/// Pull of the residual towards zero, per unit of training weight.
const RESIDUAL_ZERO_PULL: f64 = 1e-4;
/// Training weight near a lattice point, as a share of the total, at which
/// its residual is kept at half strength. Points with less support fade to
/// the curve and matrix alone.
const RESIDUAL_HALF_SUPPORT: f64 = 1e-3;

/// Mean held-out CIEDE2000 improvement over the canonical render a look
/// must reach, absolute and as a share of the canonical render's mean.
/// Measured on the Demo photos (2026-11): 67 of 74 reach it; below it a
/// look's change is too small to see.
const MIN_LOOK_GAIN: f32 = 0.5;
const MIN_LOOK_GAIN_SHARE: f32 = 0.25;
/// Mean held-out CIEDE2000 improvement over the curve and matrix the
/// residual grid must reach, absolute and as a share of their mean. About
/// one Demo photo in seven keeps its residual.
const MIN_RESIDUAL_GAIN: f32 = 0.2;
const MIN_RESIDUAL_GAIN_SHARE: f32 = 0.15;
/// How much the residual grid may raise the held-out 90th percentile over
/// the curve and matrix.
const MAX_RESIDUAL_P90_RISE: f32 = 0.05;

#[derive(Debug)]
pub enum CameraLookFitError {
    /// The preview's long edge is below [`MIN_PREVIEW_EDGE`].
    PreviewTooSmall { edge: u32 },
    /// A render the fit asked for failed.
    Render(anyhow::Error),
    /// No scale and offset overlays the preview on the render, or the best
    /// one does not show the same picture.
    Alignment { correlation: Option<f32> },
    /// Too few pixels survived clipping and edge removal.
    TooFewSamples { samples: usize },
    /// The normal equations of a stage could not be solved.
    Solve,
}

impl fmt::Display for CameraLookFitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PreviewTooSmall { edge } => write!(
                formatter,
                "the embedded preview is {edge} pixels; at least {MIN_PREVIEW_EDGE} are needed"
            ),
            Self::Render(error) => write!(formatter, "the photo could not be rendered: {error:#}"),
            Self::Alignment {
                correlation: Some(correlation),
            } => write!(
                formatter,
                "the embedded preview does not match the photo (correlation {correlation:.3})"
            ),
            Self::Alignment { correlation: None } => {
                formatter.write_str("the embedded preview does not overlap the photo")
            }
            Self::TooFewSamples { samples } => write!(
                formatter,
                "only {samples} pixels of the preview can be compared; at least {MIN_TRAINING_SAMPLES} are needed"
            ),
            Self::Solve => formatter.write_str("the camera look could not be solved"),
        }
    }
}

impl std::error::Error for CameraLookFitError {}

/// A stage of the model, in the order the fit adds them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FitStage {
    /// The canonical render without a look.
    Canonical,
    /// With the exposure offset.
    Exposure,
    /// With the exposure offset and the tone curve.
    Curve,
    /// With the exposure offset, curve and matrix.
    Matrix,
    /// With the residual grid as well.
    Residual,
    /// Curve and matrix baked into the LUT that renders them.
    BakedMatrix,
    /// Curve, matrix and residual baked into the LUT that renders them.
    BakedResidual,
}

impl FitStage {
    pub fn label(self) -> &'static str {
        match self {
            Self::Canonical => "canonical",
            Self::Exposure => "+exposure",
            Self::Curve => "+curve",
            Self::Matrix => "+matrix",
            Self::Residual => "+residual",
            Self::BakedMatrix => "baked c+m",
            Self::BakedResidual => "baked full",
        }
    }
}

/// Held-out CIEDE2000 of one stage.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StageScore {
    pub stage: FitStage,
    pub mean: f32,
    pub p90: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FitVerdict {
    /// Curve, matrix and residual.
    Full,
    /// The residual was rejected; curve and matrix only.
    CurveAndMatrix,
    /// No look improves the match enough.
    Rejected,
}

/// The outcome of [`fit_camera_look`].
#[derive(Clone, Debug)]
pub struct CameraLookFit {
    /// The look the guard accepted; `None` when [`Self::verdict`] is
    /// [`FitVerdict::Rejected`].
    pub look: Option<CameraLook>,
    pub verdict: FitVerdict,
    pub alignment: FitAlignment,
    /// Every stage's held-out score, also of stages the guard rejected.
    pub scores: Vec<StageScore>,
    pub training_samples: usize,
    pub held_out_samples: usize,
    /// The bake of the curve and matrix, and of the full model.
    pub curve_matrix_bake: BakeCheck,
    pub full_bake: BakeCheck,
    /// The full model, whatever the verdict, for diagnostics.
    pub full_look: CameraLook,
}

impl CameraLookFit {
    pub fn score(&self, stage: FitStage) -> Option<StageScore> {
        self.scores
            .iter()
            .copied()
            .find(|score| score.stage == stage)
    }
}

/// Fits the camera's look from its embedded `preview`. `render` renders the
/// canonical edit of the photo with a scene exposure offset in EV, as the
/// look's offset applies it, as an sRGB-encoded RGBA8 image of the whole
/// photo, oriented as the preview, at about the preview's size; the fit
/// calls it a few times.
pub fn fit_camera_look(
    preview: &RawThumbnail,
    mut render: impl FnMut(f32) -> anyhow::Result<RawThumbnail>,
) -> Result<CameraLookFit, CameraLookFitError> {
    let edge = preview.width.max(preview.height);
    if edge < MIN_PREVIEW_EDGE {
        return Err(CameraLookFitError::PreviewTooSmall { edge });
    }
    let canonical = render(0.0).map_err(CameraLookFitError::Render)?;
    let preview = &trim_preview_borders(
        preview,
        canonical.width as f32 / canonical.height.max(1) as f32,
    );
    let frame = WorkingFrame::new(&canonical, preview);
    let canonical_plane = frame.render_plane(&canonical);
    let preview_plane = frame.preview_plane(preview);
    let alignment = align(&canonical_plane, &preview_plane)
        .ok_or(CameraLookFitError::Alignment { correlation: None })?;
    if alignment.correlation < MIN_ALIGNMENT_CORRELATION {
        return Err(CameraLookFitError::Alignment {
            correlation: Some(alignment.correlation),
        });
    }
    let aligned = AlignedPreview::new(&canonical_plane, &preview_plane, &alignment);

    let exposure_ev = search_exposure(&frame, &aligned, &canonical_plane, &mut render)?;
    let exposed_plane = if exposure_ev == 0.0 {
        canonical_plane.clone()
    } else {
        frame.render_plane(&render(exposure_ev).map_err(CameraLookFitError::Render)?)
    };

    let (training, held_out) = aligned.samples(&canonical_plane, &exposed_plane);
    if training.len() < MIN_TRAINING_SAMPLES {
        return Err(CameraLookFitError::TooFewSamples {
            samples: training.len(),
        });
    }

    let mut weights = vec![1.0_f32; training.len()];
    let identity = CameraLook::new(exposure_ev, LookCurve::identity(), IDENTITY3, None);
    let mut curve_only = None;
    let mut curve_matrix = identity.clone();
    for round in 0..CURVE_MATRIX_ROUNDS {
        let curve = fit_curve(&training, &weights, &curve_matrix.matrix)?;
        if round == 0 {
            curve_only = Some(CameraLook::new(exposure_ev, curve.clone(), IDENTITY3, None));
        }
        let with_curve = CameraLook::new(exposure_ev, curve, curve_matrix.matrix, None);
        let matrix = fit_matrix(&training, &weights, &with_curve.curve)?;
        curve_matrix = CameraLook::new(exposure_ev, with_curve.curve, matrix, None);
        weights = robust_weights(&training, |sample| curve_matrix.map_display(sample.render));
    }
    let curve_only = curve_only.unwrap_or_else(|| identity.clone());

    let mut full = curve_matrix.clone();
    for _ in 0..RESIDUAL_ROUNDS {
        let grid = fit_residual(&training, &weights, &curve_matrix)?;
        full = CameraLook::new(
            exposure_ev,
            curve_matrix.curve.clone(),
            curve_matrix.matrix,
            Some(grid),
        );
        weights = robust_weights(&training, |sample| full.map_display(sample.render));
    }

    let curve_matrix_lut = curve_matrix.bake();
    let full_lut = full.bake();
    let curve_matrix_bake = curve_matrix.check_folds();
    let full_bake = full.check_folds();

    let scores = vec![
        score(FitStage::Canonical, &held_out, |sample| sample.canonical),
        score(FitStage::Exposure, &held_out, |sample| sample.render),
        score(FitStage::Curve, &held_out, |sample| {
            curve_only.map_display(sample.render)
        }),
        score(FitStage::Matrix, &held_out, |sample| {
            curve_matrix.map_display(sample.render)
        }),
        score(FitStage::Residual, &held_out, |sample| {
            full.map_display(sample.render)
        }),
        score(FitStage::BakedMatrix, &held_out, |sample| {
            curve_matrix_lut.sample_tetrahedral(sample.render)
        }),
        score(FitStage::BakedResidual, &held_out, |sample| {
            full_lut.sample_tetrahedral(sample.render)
        }),
    ];
    let stage = |stage| {
        scores
            .iter()
            .copied()
            .find(|score| score.stage == stage)
            .unwrap_or(StageScore {
                stage,
                mean: f32::INFINITY,
                p90: f32::INFINITY,
            })
    };
    let canonical = stage(FitStage::Canonical);
    let baked_curve_matrix = stage(FitStage::BakedMatrix);
    let baked_full = stage(FitStage::BakedResidual);
    // A look must clearly improve the match and never worsen its worst part.
    let improves = |look: StageScore| {
        let gain = canonical.mean - look.mean;
        gain >= MIN_LOOK_GAIN
            && gain >= MIN_LOOK_GAIN_SHARE * canonical.mean
            && look.p90 <= canonical.p90
    };
    let curve_matrix_accepted = improves(baked_curve_matrix) && !curve_matrix_bake.has_folds();
    let residual_gain = baked_curve_matrix.mean - baked_full.mean;
    let full_accepted = improves(baked_full)
        && residual_gain >= MIN_RESIDUAL_GAIN
        && residual_gain >= MIN_RESIDUAL_GAIN_SHARE * baked_curve_matrix.mean
        && baked_full.p90 <= baked_curve_matrix.p90 + MAX_RESIDUAL_P90_RISE
        && !full_bake.has_folds();
    let (verdict, look) = if full_accepted {
        (FitVerdict::Full, Some(full.clone()))
    } else if curve_matrix_accepted {
        (FitVerdict::CurveAndMatrix, Some(curve_matrix))
    } else {
        (FitVerdict::Rejected, None)
    };
    Ok(CameraLookFit {
        look,
        verdict,
        alignment,
        scores,
        training_samples: training.len(),
        held_out_samples: held_out.len(),
        curve_matrix_bake,
        full_bake,
        full_look: full,
    })
}

/// The working sizes of the render and the preview. The preview is scaled
/// so that, where the two have different aspects, it spans the render
/// along the axis it covers fully, as a camera crops a wider or taller
/// preview from the sensor's centre.
struct WorkingFrame {
    render_size: (usize, usize),
    preview_size: (usize, usize),
}

impl WorkingFrame {
    fn new(render: &RawThumbnail, preview: &RawThumbnail) -> Self {
        let (render_width, render_height) = (render.width as f32, render.height as f32);
        let long = render_width.max(render_height);
        let scale = (WORKING_EDGE as f32 / long).min(1.0);
        let render_size = (
            ((render_width * scale).round() as usize).max(8),
            ((render_height * scale).round() as usize).max(8),
        );
        let coverage =
            (preview.width as f32 / render_width).max(preview.height as f32 / render_height);
        let preview_scale = scale / coverage;
        let preview_size = (
            ((preview.width as f32 * preview_scale).round() as usize).max(8),
            ((preview.height as f32 * preview_scale).round() as usize).max(8),
        );
        Self {
            render_size,
            preview_size,
        }
    }

    fn render_plane(&self, image: &RawThumbnail) -> RgbPlane {
        let plane = resample_area(
            &decode_linear(image),
            self.render_size.0,
            self.render_size.1,
        );
        blur(&plane, BLUR_SIGMA)
    }

    fn preview_plane(&self, image: &RawThumbnail) -> RgbPlane {
        let plane = resample_area(
            &decode_linear(image),
            self.preview_size.0,
            self.preview_size.1,
        );
        blur(&plane, BLUR_SIGMA)
    }
}

/// The preview resampled onto the working render's pixels, in linear sRGB;
/// `None` where it does not reach.
struct AlignedPreview {
    width: usize,
    height: usize,
    pixels: Vec<Option<[f32; 3]>>,
}

/// One compared pixel, sRGB-encoded.
struct Sample {
    /// The render with the look's exposure offset.
    render: [f32; 3],
    /// The canonical render, without it.
    canonical: [f32; 3],
    /// The preview's colour in CIELAB.
    preview_lab: [f32; 3],
    preview: [f32; 3],
}

impl AlignedPreview {
    fn new(render: &RgbPlane, preview: &RgbPlane, alignment: &FitAlignment) -> Self {
        let mut pixels = Vec::with_capacity(render.width * render.height);
        for y in 0..render.height {
            for x in 0..render.width {
                let (px, py) = alignment.map(
                    (render.width, render.height),
                    (preview.width, preview.height),
                    x as f32,
                    y as f32,
                );
                pixels.push(sample_bilinear(preview, px, py, |value| value));
            }
        }
        Self {
            width: render.width,
            height: render.height,
            pixels,
        }
    }

    /// The training and held-out samples where neither image is clipped and
    /// neither has a strong edge.
    fn samples(&self, canonical: &RgbPlane, exposed: &RgbPlane) -> (Vec<Sample>, Vec<Sample>) {
        let encode = |rgb: [f32; 3]| rgb.map(srgb_encode);
        let luminance = |rgb: [f32; 3]| 0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2];
        let gradient = |value: &dyn Fn(usize, usize) -> Option<f32>, x: usize, y: usize| {
            let horizontal = match (value(x.wrapping_sub(1), y), value(x + 1, y)) {
                (Some(left), Some(right)) => (right - left) * 0.5,
                _ => return None,
            };
            let vertical = match (value(x, y.wrapping_sub(1)), value(x, y + 1)) {
                (Some(top), Some(bottom)) => (bottom - top) * 0.5,
                _ => return None,
            };
            Some(horizontal.hypot(vertical))
        };
        let preview_luminance = |x: usize, y: usize| {
            (x < self.width && y < self.height)
                .then(|| self.pixels[y * self.width + x])
                .flatten()
                .map(|rgb| luminance(encode(rgb)))
        };
        let render_luminance = |x: usize, y: usize| {
            (x < self.width && y < self.height).then(|| luminance(encode(exposed.at(x, y))))
        };

        let mut candidates = Vec::new();
        for y in 0..self.height {
            for x in 0..self.width {
                let Some(preview) = self.pixels[y * self.width + x].map(encode) else {
                    continue;
                };
                let render = encode(exposed.at(x, y));
                let canonical_rgb = encode(canonical.at(x, y));
                let preview_max = preview.iter().copied().fold(0.0_f32, f32::max);
                let render_max = render.iter().copied().fold(0.0_f32, f32::max);
                let canonical_max = canonical_rgb.iter().copied().fold(0.0_f32, f32::max);
                if !(CLIP_LOW..CLIP_HIGH).contains(&preview_max)
                    || render_max >= CLIP_HIGH
                    || canonical_max >= CLIP_HIGH
                {
                    continue;
                }
                let (Some(preview_edge), Some(render_edge)) = (
                    gradient(&preview_luminance, x, y),
                    gradient(&render_luminance, x, y),
                ) else {
                    continue;
                };
                candidates.push((
                    x,
                    y,
                    preview_edge.max(render_edge),
                    render,
                    canonical_rgb,
                    preview,
                ));
            }
        }
        // Strong edges are dropped, but never so many that fewer than
        // MIN_FLAT_SHARE of the candidates remain.
        let mut strengths: Vec<f32> = candidates.iter().map(|candidate| candidate.2).collect();
        strengths.sort_by(f32::total_cmp);
        let keep_index = ((strengths.len() as f32 * MIN_FLAT_SHARE) as usize)
            .min(strengths.len().saturating_sub(1));
        let limit = strengths
            .get(keep_index)
            .map_or(EDGE_GRADIENT, |share_limit| share_limit.max(EDGE_GRADIENT));

        let mut training = Vec::new();
        let mut held_out = Vec::new();
        for (x, y, edge, render, canonical, preview) in candidates {
            if edge > limit {
                continue;
            }
            let sample = Sample {
                render,
                canonical,
                preview_lab: linear_srgb_to_lab(preview.map(srgb_decode_signed)),
                preview,
            };
            let (bx, by) = (x / HOLDOUT_BLOCK, y / HOLDOUT_BLOCK);
            if (bx + 3 * by) % 4 == 0 {
                held_out.push(sample);
            } else {
                training.push(sample);
            }
        }
        (training, held_out)
    }
}

/// The exposure offset in EV at which the render's median luminance over
/// the compared pixels matches the preview's, by the secant method on
/// renders of candidate offsets.
fn search_exposure(
    frame: &WorkingFrame,
    aligned: &AlignedPreview,
    canonical: &RgbPlane,
    render: &mut impl FnMut(f32) -> anyhow::Result<RawThumbnail>,
) -> Result<f32, CameraLookFitError> {
    let luminance = |rgb: [f32; 3]| 0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2];
    // The pixels compared stay those unclipped in the canonical render.
    let mut indices = Vec::new();
    let mut preview_luminance = Vec::new();
    for (index, preview) in aligned.pixels.iter().enumerate() {
        let Some(preview) = preview else { continue };
        let render_max = canonical.pixels[index]
            .iter()
            .copied()
            .fold(0.0_f32, f32::max);
        let preview_max = preview.iter().copied().fold(0.0_f32, f32::max);
        let high = crate::color_math::srgb_decode(CLIP_HIGH);
        let low = crate::color_math::srgb_decode(CLIP_LOW);
        if render_max < high && preview_max < high && preview_max >= low {
            indices.push(index);
            preview_luminance.push(luminance(*preview));
        }
    }
    if indices.len() < MIN_TRAINING_SAMPLES {
        return Err(CameraLookFitError::TooFewSamples {
            samples: indices.len(),
        });
    }
    let preview_median = median(&mut preview_luminance).max(1e-5);
    let difference = |plane: &RgbPlane| {
        let mut values: Vec<f32> = indices
            .iter()
            .map(|&index| luminance(plane.pixels[index]))
            .collect();
        (preview_median / median(&mut values).max(1e-5)).log2()
    };

    let mut previous = (0.0_f32, difference(canonical));
    let mut best = previous;
    if previous.1.abs() <= EXPOSURE_TOLERANCE {
        return Ok(0.0);
    }
    // The tone mapper's mid-tone slope is near 1.3 in log display luminance
    // per EV; the secant steps refine that first guess.
    let mut current_ev = (previous.1 / 1.3).clamp(-MAX_LOOK_EXPOSURE_EV, MAX_LOOK_EXPOSURE_EV);
    for _ in 0..EXPOSURE_RENDERS {
        let plane = frame.render_plane(&render(current_ev).map_err(CameraLookFitError::Render)?);
        let current = (current_ev, difference(&plane));
        if current.1.abs() < best.1.abs() {
            best = current;
        }
        if current.1.abs() <= EXPOSURE_TOLERANCE {
            break;
        }
        let slope = (current.1 - previous.1) / (current.0 - previous.0);
        if !slope.is_finite() || slope.abs() < 1e-3 {
            break;
        }
        let next =
            (current.0 - current.1 / slope).clamp(-MAX_LOOK_EXPOSURE_EV, MAX_LOOK_EXPOSURE_EV);
        previous = current;
        current_ev = next;
    }
    Ok(best.0)
}

fn median(values: &mut [f32]) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let middle = values.len() / 2;
    let (_, value, _) = values.select_nth_unstable_by(middle, f32::total_cmp);
    *value
}

/// Cauchy weights of the samples' CIEDE2000 under `map`.
fn robust_weights(samples: &[Sample], map: impl Fn(&Sample) -> [f32; 3]) -> Vec<f32> {
    samples
        .iter()
        .map(|sample| {
            let error = sample_delta_e(sample, map(sample)) / ROBUST_SCALE;
            1.0 / (1.0 + error * error)
        })
        .collect()
}

fn sample_delta_e(sample: &Sample, encoded: [f32; 3]) -> f32 {
    ciede2000(
        linear_srgb_to_lab(encoded.map(srgb_decode_signed)),
        sample.preview_lab,
    )
}

fn score(stage: FitStage, samples: &[Sample], map: impl Fn(&Sample) -> [f32; 3]) -> StageScore {
    let mut errors: Vec<f32> = samples
        .iter()
        .map(|sample| sample_delta_e(sample, map(sample)))
        .collect();
    let mean = errors.iter().sum::<f32>() / errors.len().max(1) as f32;
    errors.sort_by(f32::total_cmp);
    let p90 = errors
        .get((errors.len() as f32 * 0.9) as usize)
        .or(errors.last())
        .copied()
        .unwrap_or(0.0);
    StageScore { stage, mean, p90 }
}

/// The tone curve that best maps each channel of the render onto the
/// preview's channel with `matrix`'s effect removed, smoothed, then made
/// monotone.
fn fit_curve(
    samples: &[Sample],
    weights: &[f32],
    matrix: &Matrix3,
) -> Result<LookCurve, CameraLookFitError> {
    let inverse = matrix::invert(*matrix).ok_or(CameraLookFitError::Solve)?;
    let knots = LOOK_CURVE_KNOTS;
    let last = (knots - 1) as f32;
    let mut normal = SymmetricMatrix::zeros(knots);
    let mut rhs = vec![0.0_f64; knots];
    let mut total = 0.0_f64;
    for (sample, &weight) in samples.iter().zip(weights) {
        let target = matrix::transform(inverse, sample.preview.map(srgb_decode_signed))
            .map(|value| srgb_encode_signed(value).clamp(-0.1, 1.2));
        for (input, target) in sample.render.into_iter().zip(target) {
            let position = input.clamp(0.0, 1.0) * last;
            let cell = position.floor().min(last - 1.0);
            let index = cell as usize;
            let fraction = f64::from(position - cell);
            let weight = f64::from(weight);
            let basis = [(index, 1.0 - fraction), (index + 1, fraction)];
            for &(row, a) in &basis {
                for &(column, b) in &basis {
                    normal.add(row, column, weight * a * b);
                }
                rhs[row] += weight * a * f64::from(target);
            }
            total += weight;
        }
    }
    let smoothness = CURVE_SMOOTHNESS * total;
    for centre in 1..knots - 1 {
        let taps = [(centre - 1, 1.0), (centre, -2.0), (centre + 1, 1.0)];
        for &(row, a) in &taps {
            for &(column, b) in &taps {
                normal.add(row, column, smoothness * a * b);
            }
        }
    }
    let pull = CURVE_IDENTITY_PULL * total;
    for (knot, value) in rhs.iter_mut().enumerate() {
        normal.add(knot, knot, pull);
        *value += pull * knot as f64 / last as f64;
    }
    let support: Vec<f64> = (0..knots).map(|knot| normal.diagonal(knot)).collect();
    normal.solve(&mut rhs, 1).ok_or(CameraLookFitError::Solve)?;
    let monotone = pool_adjacent_violators(&rhs, &support);
    let knots: [f32; LOOK_CURVE_KNOTS] = std::array::from_fn(|index| monotone[index] as f32);
    LookCurve::from_knots(knots).ok_or(CameraLookFitError::Solve)
}

/// The weighted least-squares non-decreasing sequence closest to `values`.
fn pool_adjacent_violators(values: &[f64], weights: &[f64]) -> Vec<f64> {
    // Blocks of (mean, weight, length).
    let mut blocks: Vec<(f64, f64, usize)> = Vec::with_capacity(values.len());
    for (&value, &weight) in values.iter().zip(weights) {
        blocks.push((value, weight.max(1e-12), 1));
        while blocks.len() > 1 && blocks[blocks.len() - 2].0 > blocks[blocks.len() - 1].0 {
            let (mean_b, weight_b, length_b) = blocks.pop().unwrap_or((0.0, 0.0, 0));
            let (mean_a, weight_a, length_a) = blocks.pop().unwrap_or((0.0, 0.0, 0));
            let weight = weight_a + weight_b;
            blocks.push((
                (mean_a * weight_a + mean_b * weight_b) / weight,
                weight,
                length_a + length_b,
            ));
        }
    }
    blocks
        .into_iter()
        .flat_map(|(mean, _, length)| std::iter::repeat_n(mean, length))
        .collect()
}

/// The 3×3 matrix in linear sRGB that best maps the curved render onto the
/// preview. Each sample's error is weighted by the slope of the sRGB
/// encoding at the preview's value, so it counts as it would in encoded
/// values.
fn fit_matrix(
    samples: &[Sample],
    weights: &[f32],
    curve: &LookCurve,
) -> Result<Matrix3, CameraLookFitError> {
    let mut rows = IDENTITY3;
    for (row_index, row) in rows.iter_mut().enumerate() {
        let mut normal = SymmetricMatrix::zeros(3);
        let mut rhs = vec![0.0_f64; 3];
        for (sample, &weight) in samples.iter().zip(weights) {
            let input = sample
                .render
                .map(|value| f64::from(srgb_decode_signed(curve.evaluate(value))));
            let target = f64::from(srgb_decode_signed(sample.preview[row_index]));
            let slope = encoding_slope(target);
            let weight = f64::from(weight) * slope * slope;
            for a in 0..3 {
                for b in 0..3 {
                    normal.add(a, b, weight * input[a] * input[b]);
                }
                rhs[a] += weight * input[a] * target;
            }
        }
        let pull =
            MATRIX_IDENTITY_PULL * (0..3).map(|index| normal.diagonal(index)).sum::<f64>() / 3.0;
        for (index, value) in rhs.iter_mut().enumerate() {
            normal.add(index, index, pull);
            *value += pull * f64::from(u8::from(index == row_index));
        }
        normal.solve(&mut rhs, 1).ok_or(CameraLookFitError::Solve)?;
        *row = [rhs[0] as f32, rhs[1] as f32, rhs[2] as f32];
    }
    matrix::invert(rows).ok_or(CameraLookFitError::Solve)?;
    Ok(rows)
}

/// The derivative of the sRGB encoding at linear `value`, floored where the
/// encoding's slope stops rising.
fn encoding_slope(value: f64) -> f64 {
    let value = value.max(0.003_130_8);
    1.055 / 2.4 * value.powf(1.0 / 2.4 - 1.0)
}

/// The smooth residual grid over what `base` leaves, with lattice points
/// faded by the training weight around them.
fn fit_residual(
    samples: &[Sample],
    weights: &[f32],
    base: &CameraLook,
) -> Result<ResidualGrid, CameraLookFitError> {
    let points = RESIDUAL_GRID_POINTS;
    let mut normal = SymmetricMatrix::zeros(points);
    let mut rhs = vec![0.0_f64; points * 3];
    let mut support = vec![0.0_f64; points];
    let mut total = 0.0_f64;
    for (sample, &weight) in samples.iter().zip(weights) {
        let mapped = base.map_display(sample.render);
        let residual: [f64; 3] =
            std::array::from_fn(|channel| f64::from(sample.preview[channel] - mapped[channel]));
        let (corners, corner_weights) = trilinear_corners(sample.render);
        let weight = f64::from(weight);
        for (a, &corner_a) in corners.iter().enumerate() {
            let weight_a = f64::from(corner_weights[a]);
            if weight_a == 0.0 {
                continue;
            }
            for (b, &corner_b) in corners.iter().enumerate() {
                normal.add(
                    corner_a,
                    corner_b,
                    weight * weight_a * f64::from(corner_weights[b]),
                );
            }
            for channel in 0..3 {
                rhs[corner_a * 3 + channel] += weight * weight_a * residual[channel];
            }
            support[corner_a] += weight * weight_a;
        }
        total += weight;
    }

    let size = RESIDUAL_GRID_SIZE;
    let index = |r: usize, g: usize, b: usize| r + size * (g + size * b);
    let smoothness = RESIDUAL_SMOOTHNESS * total;
    let mut taps = Vec::with_capacity(7);
    for b in 0..size {
        for g in 0..size {
            for r in 0..size {
                // Laplacian with the grid's faces as reflecting boundaries.
                taps.clear();
                let centre = index(r, g, b);
                let coordinates = [r, g, b];
                for axis in 0..3 {
                    for direction in [-1_isize, 1] {
                        let neighbour = coordinates[axis] as isize + direction;
                        if (0..size as isize).contains(&neighbour) {
                            let mut moved = coordinates;
                            moved[axis] = neighbour as usize;
                            taps.push((index(moved[0], moved[1], moved[2]), 1.0));
                        }
                    }
                }
                let degree = taps.len() as f64;
                taps.push((centre, -degree));
                for &(row, a) in &taps {
                    for &(column, c) in &taps {
                        normal.add(row, column, smoothness * a * c);
                    }
                }
            }
        }
    }
    let pull = RESIDUAL_ZERO_PULL * total;
    for point in 0..points {
        normal.add_symmetric(point, point, pull);
    }
    normal.solve(&mut rhs, 3).ok_or(CameraLookFitError::Solve)?;

    // Support is averaged over each point's 3×3×3 neighbourhood, so the
    // fade does not follow the noise of single cells.
    let half_support = RESIDUAL_HALF_SUPPORT * total;
    let mut values = Vec::with_capacity(points);
    for b in 0..size {
        for g in 0..size {
            for r in 0..size {
                let mut sum = 0.0;
                let mut count = 0.0;
                for db in -1_isize..=1 {
                    for dg in -1_isize..=1 {
                        for dr in -1_isize..=1 {
                            let (nr, ng, nb) = (r as isize + dr, g as isize + dg, b as isize + db);
                            let range = 0..size as isize;
                            if range.contains(&nr) && range.contains(&ng) && range.contains(&nb) {
                                sum += support[index(nr as usize, ng as usize, nb as usize)];
                                count += 1.0;
                            }
                        }
                    }
                }
                let neighbourhood = sum / count;
                let confidence = neighbourhood / (neighbourhood + half_support);
                let point = index(r, g, b);
                values.push(std::array::from_fn(|channel| {
                    (rhs[point * 3 + channel] * confidence) as f32
                }));
            }
        }
    }
    ResidualGrid::new(values).ok_or(CameraLookFitError::Solve)
}
