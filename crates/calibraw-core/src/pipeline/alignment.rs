//! Registration of frames of one static scene, shared by HDR merge and focus stacking.
//!
//! Frames are compared as exposure-normalized log luminance in linear camera RGB, with
//! saturated and nearly black samples masked out. A brightness offset between frames
//! (exposure steps, flicker, imprecise metadata) is estimated together with the geometry,
//! so it cannot bias the alignment, and a robust (Huber) loss limits the influence of
//! noise, moving subjects and defocus differences.
//!
//! The geometry is a homography, which describes any camera rotation exactly: turning a
//! handheld wide-angle camera far enough to shift the image center moves the edges
//! further still, which no similarity transform can follow. Estimation runs coarse to
//! fine on a luminance pyramid: an exhaustive similarity search on the coarsest level,
//! then Levenberg–Marquardt refinement of the forward-additive Lucas–Kanade objective
//! (Baker & Matthews, "Lucas-Kanade 20 Years On"), restricted to a similarity on coarse
//! levels and free on finer ones.

use anyhow::{ensure, Result};
use rayon::prelude::*;

type Matrix = [[f64; 3]; 3];

const IDENTITY: Matrix = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

fn multiply(a: &Matrix, b: &Matrix) -> Matrix {
    std::array::from_fn(|r| std::array::from_fn(|c| (0..3).map(|k| a[r][k] * b[k][c]).sum()))
}

/// Maps reference pixels to source pixels, with pixel centers at integer coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Alignment {
    /// Homography acting on homogeneous pixel coordinates `(x, y, 1)`.
    matrix: Matrix,
}

impl Default for Alignment {
    fn default() -> Self {
        Self { matrix: IDENTITY }
    }
}

impl Alignment {
    #[cfg(test)]
    pub(crate) fn translation(x: f32, y: f32) -> Self {
        let mut matrix = IDENTITY;
        matrix[0][2] = f64::from(x);
        matrix[1][2] = f64::from(y);
        Self { matrix }
    }

    /// Scale and rotation (radians, turning +x towards +y) about the center of a
    /// `width × height` image, followed by a translation in source pixels.
    fn similarity(width: u32, height: u32, x: f32, y: f32, rotation: f32, scale: f32) -> Self {
        let (sin, cos) = f64::from(rotation).sin_cos();
        let (a, b) = (f64::from(scale) * cos, f64::from(scale) * sin);
        let cx = f64::from(width.max(1) - 1) * 0.5;
        let cy = f64::from(height.max(1) - 1) * 0.5;
        Self {
            matrix: [
                [a, -b, cx - a * cx + b * cy + f64::from(x)],
                [b, a, cy - b * cx - a * cy + f64::from(y)],
                [0.0, 0.0, 1.0],
            ],
        }
    }

    pub(super) fn is_finite(self) -> bool {
        self.matrix.iter().flatten().all(|v| v.is_finite())
    }

    /// The source position of reference pixel `(x, y)`; NaN where the homography maps the
    /// point to infinity or behind the camera.
    pub(super) fn map(&self, x: f32, y: f32) -> (f32, f32) {
        let (x, y) = (f64::from(x), f64::from(y));
        let m = &self.matrix;
        let w = m[2][0] * x + m[2][1] * y + m[2][2];
        if w <= 0.0 {
            return (f32::NAN, f32::NAN);
        }
        (
            ((m[0][0] * x + m[0][1] * y + m[0][2]) / w) as f32,
            ((m[1][0] * x + m[1][1] * y + m[1][2]) / w) as f32,
        )
    }

    /// Source area per reference area at `(x, y)`: det(H) / w³.
    fn area_scale(&self, x: f64, y: f64) -> f64 {
        let m = &self.matrix;
        let w = m[2][0] * x + m[2][1] * y + m[2][2];
        let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
        det / w.powi(3)
    }
}

/// A source frame registered to the reference.
#[derive(Clone, Copy, Debug)]
pub struct Registration {
    pub alignment: Alignment,
    /// Source brightness relative to the reference after exposure normalization.
    pub gain: f32,
}

/// Bilinear taps for `(x, y)`, or `None` outside the pixel centers of the image.
pub(super) fn sample_coordinates(
    width: usize,
    height: usize,
    x: f32,
    y: f32,
) -> Option<([usize; 4], [f32; 4])> {
    if !(x >= 0.0 && y >= 0.0 && x <= (width - 1) as f32 && y <= (height - 1) as f32) {
        return None;
    }
    let ix = x.floor() as usize;
    let iy = y.floor() as usize;
    let jx = (ix + 1).min(width - 1);
    let jy = (iy + 1).min(height - 1);
    let dx = x - ix as f32;
    let dy = y - iy as f32;
    Some((
        [
            iy * width + ix,
            iy * width + jx,
            jy * width + ix,
            jy * width + jx,
        ],
        [
            (1.0 - dx) * (1.0 - dy),
            dx * (1.0 - dy),
            (1.0 - dx) * dy,
            dx * dy,
        ],
    ))
}

/// The finest pyramid level is binned down to at most this edge length.
const FINEST_EDGE: usize = 4096;
/// Levels are halved while the shorter edge stays at least twice this.
const COARSEST_EDGE: usize = 32;
/// Levels with a longer edge than this refine the full homography; coarser levels have
/// too few pixels to constrain perspective and refine a similarity.
const HOMOGRAPHY_EDGE: usize = 128;
/// Samples whose brightest camera channel reaches this are treated as clipped.
const CLIPPED: f32 = 0.95;
/// Samples darker than this (white = 1) are too noisy to align.
const BLACK: f32 = 0.004;
/// Reference samples used per level; more add time, not accuracy.
const MAX_SAMPLES: usize = 1 << 17;
/// Coarsest-level search window in coarsest-level pixels (±1/8 of a 64 px edge or more).
const SEARCH_RADIUS: i32 = 8;
const SEARCH_ROTATIONS_DEGREES: [f32; 9] = [-2.0, -1.5, -1.0, -0.5, 0.0, 0.5, 1.0, 1.5, 2.0];
const SEARCH_SCALES: [f32; 3] = [0.96, 1.0, 1.04];
/// Weak prior pulling the perspective terms towards zero (per sample, in normalized
/// coordinates), so a frame with detail in only one region keeps a plausible geometry.
const PERSPECTIVE_PRIOR: f64 = 1e-2;
/// Minimum log-luminance correlation of the aligned frames on the verification level.
/// Any related view of one scene correlates far above it, unrelated frames do not.
const MIN_CORRELATION: f64 = 0.6;
/// The verification level is the finest one no longer than this, where defocus and
/// sensor noise no longer dominate the comparison.
const VERIFICATION_EDGE: usize = 512;

/// One pyramid level: log luminance, NaN where masked.
struct Level {
    width: usize,
    height: usize,
    values: Vec<f32>,
}

/// A reference sample in level pixel coordinates.
#[derive(Clone, Copy)]
struct Sample {
    x: f32,
    y: f32,
    value: f32,
}

/// The reference frame's pyramid and its sparse samples per level, built once per bracket
/// or stack and reused for every source frame.
pub struct AlignmentReference {
    width: usize,
    height: usize,
    levels: Vec<Level>,
    samples: Vec<Vec<Sample>>,
}

impl AlignmentReference {
    /// `rgb` is black-subtracted, white-normalized camera RGB; `exposure` is relative to
    /// the frame the others are normalized to.
    pub fn new(width: u32, height: u32, rgb: &[f32], exposure: f32) -> Result<Self> {
        let levels = pyramid(width as usize, height as usize, rgb, exposure)?;
        let samples = levels.iter().map(Level::samples).collect();
        Ok(Self {
            width: width as usize,
            height: height as usize,
            levels,
            samples,
        })
    }

    pub fn align(&self, rgb: &[f32], exposure: f32) -> Result<Registration> {
        let source: Vec<_> = pyramid(self.width, self.height, rgb, exposure)?
            .iter()
            .map(GradientLevel::new)
            .collect();
        let samples = self.measurable_samples(exposure);
        let coarsest = self.levels.len() - 1;
        let mut model = search(&samples[coarsest], &source[coarsest]);
        for level in (0..self.levels.len()).rev() {
            if level < coarsest {
                model = model.on_grid(2.0);
            }
            let stage =
                if self.levels[level].width.max(self.levels[level].height) >= HOMOGRAPHY_EDGE {
                    Stage::Homography
                } else {
                    Stage::Similarity
                };
            model = refine(&samples[level], &source[level], model, stage);
        }
        self.verify(&samples, &source, model)?;
        let stride = self.width.max(self.height).div_ceil(FINEST_EDGE);
        let model = model.on_grid(stride as f64);
        let alignment = Alignment {
            matrix: model.matrix,
        };
        self.ensure_plausible(alignment)?;
        Ok(Registration {
            alignment,
            gain: model.bias.exp() as f32,
        })
    }

    /// The reference samples whose radiance a source at `exposure` measures too, with a
    /// margin. Masks then depend on scene radiance, not on the candidate geometry, so
    /// every candidate is compared on the same samples, even when one frame of an HDR
    /// bracket clips or crushes most of what the other shows.
    fn measurable_samples(&self, exposure: f32) -> Vec<Vec<Sample>> {
        const MARGIN: f32 = 1.5;
        let low = (MARGIN * BLACK / exposure).ln();
        let high = (CLIPPED / (MARGIN * exposure)).ln();
        self.samples
            .iter()
            .map(|level| {
                level
                    .iter()
                    .filter(|sample| (low..high).contains(&sample.value))
                    .copied()
                    .collect()
            })
            .collect()
    }

    /// Rejects alignments that do not bring the frames into agreement: too little overlap
    /// or detail, or a correlation no view of the same scene would have.
    fn verify(
        &self,
        samples: &[Vec<Sample>],
        source: &[GradientLevel],
        model: Model,
    ) -> Result<()> {
        let level = (0..self.levels.len())
            .find(|&l| self.levels[l].width.max(self.levels[l].height) <= VERIFICATION_EDGE)
            .unwrap_or(self.levels.len() - 1);
        let alignment = Alignment {
            matrix: model.on_grid(0.5f64.powi(level as i32)).matrix,
        };
        let samples = &samples[level];
        let source = &source[level];
        let mut inside = 0;
        let mut pairs = Vec::with_capacity(samples.len());
        for sample in samples {
            let (x, y) = alignment.map(sample.x, sample.y);
            if sample_coordinates(source.width, source.height, x, y).is_some() {
                inside += 1;
            }
            if let Some([value, _, _]) = source.sample(x, y) {
                pairs.push((f64::from(sample.value), f64::from(value)));
            }
        }
        ensure!(
            inside * 2 >= samples.len() && pairs.len() >= 64,
            "Too little of the frames overlaps with usable detail to align them; use overlapping shots of the same static scene"
        );
        let n = pairs.len() as f64;
        let (mean_a, mean_b) = pairs
            .iter()
            .fold((0.0, 0.0), |(a, b), (x, y)| (a + x / n, b + y / n));
        let (mut covariance, mut var_a, mut var_b) = (0.0, 0.0, 0.0);
        for (a, b) in &pairs {
            covariance += (a - mean_a) * (b - mean_b);
            var_a += (a - mean_a).powi(2);
            var_b += (b - mean_b).powi(2);
        }
        ensure!(
            var_a > 1e-6 * n && var_b > 1e-6 * n,
            "The frames have too little detail to align"
        );
        ensure!(
            covariance / (var_a * var_b).sqrt() >= MIN_CORRELATION,
            "Could not reliably auto-align the frames; use overlapping shots of the same static scene"
        );
        Ok(())
    }

    /// Bounds the motion to what overlapping shots of one scene can have: the center moves
    /// less than 15% of the frame, corners less than a quarter of the long edge, and no
    /// part of the frame changes area by more than 1.5× (a few degrees of turn at the
    /// edge of an ultra-wide view).
    fn ensure_plausible(&self, alignment: Alignment) -> Result<()> {
        let (w, h) = ((self.width - 1) as f32, (self.height - 1) as f32);
        let (cx, cy) = (w * 0.5, h * 0.5);
        let (x, y) = alignment.map(cx, cy);
        let long = self.width.max(self.height) as f32;
        let plausible = (x - cx).abs() < 0.15 * self.width as f32
            && (y - cy).abs() < 0.15 * self.height as f32
            && [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h), (cx, cy)]
                .into_iter()
                .all(|(px, py)| {
                    let (x, y) = alignment.map(px, py);
                    let area = alignment.area_scale(f64::from(px), f64::from(py));
                    (x - px).hypot(y - py) < 0.25 * long && (1.0 / 1.5..1.5).contains(&area)
                });
        ensure!(plausible, "Frame movement is too large to align");
        Ok(())
    }
}

/// Builds the masked log-luminance pyramid, finest level first.
fn pyramid(width: usize, height: usize, rgb: &[f32], exposure: f32) -> Result<Vec<Level>> {
    ensure!(
        width > 0 && height > 0 && rgb.len() == width * height * 3,
        "Alignment frame dimensions do not match the buffer"
    );
    ensure!(
        exposure.is_finite() && exposure > 0.0,
        "Invalid relative exposure for alignment"
    );
    let stride = width.max(height).div_ceil(FINEST_EDGE);
    let (w, h) = (width.div_ceil(stride), height.div_ceil(stride));
    let mut values = vec![f32::NAN; w * h];
    values.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        // Sum of luminance, pixel count and whether any pixel clipped.
        let mut bins = vec![(0.0f32, 0usize, false); w];
        for sy in y * stride..((y + 1) * stride).min(height) {
            for (sx, pixel) in rgb[sy * width * 3..(sy + 1) * width * 3]
                .chunks_exact(3)
                .enumerate()
            {
                let bin = &mut bins[sx / stride];
                let peak = pixel[0].max(pixel[1]).max(pixel[2]);
                bin.0 += (pixel[0] + 2.0 * pixel[1] + pixel[2]) * 0.25;
                bin.1 += 1;
                bin.2 |= peak.is_nan() || peak >= CLIPPED;
            }
        }
        // A clipped pixel biases its bin, so the bin is dropped; shadow noise does not,
        // so black is judged on the bin mean.
        for (value, (sum, count, clipped)) in row.iter_mut().zip(bins) {
            let mean = sum / count as f32;
            if !clipped && mean > BLACK {
                *value = (mean / exposure).ln();
            }
        }
    });
    let mut levels = vec![Level {
        width: w,
        height: h,
        values,
    }];
    while let Some(last) = levels
        .last()
        .filter(|l| l.width.min(l.height) >= 2 * COARSEST_EDGE)
    {
        let next = last.downsample();
        levels.push(next);
    }
    Ok(levels)
}

impl Level {
    /// 2×2 mean of the valid samples; a cell needs at least two. Requiring all four would
    /// compound across levels until a coarse cell needs thousands of valid pixels.
    fn downsample(&self) -> Self {
        let (w, h) = (self.width.div_ceil(2), self.height.div_ceil(2));
        let mut values = vec![f32::NAN; w * h];
        values.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            for (x, value) in row.iter_mut().enumerate() {
                let mut sum = 0.0;
                let mut count = 0;
                for sy in y * 2..(y * 2 + 2).min(self.height) {
                    for sx in x * 2..(x * 2 + 2).min(self.width) {
                        let v = self.values[sy * self.width + sx];
                        if v.is_finite() {
                            sum += v;
                            count += 1;
                        }
                    }
                }
                if count >= 2 {
                    *value = sum / count as f32;
                }
            }
        });
        Self {
            width: w,
            height: h,
            values,
        }
    }

    /// Valid samples on a regular grid, at most about `MAX_SAMPLES`.
    fn samples(&self) -> Vec<Sample> {
        let valid = self.values.iter().filter(|v| v.is_finite()).count();
        let step = ((valid as f64 / MAX_SAMPLES as f64).sqrt().ceil() as usize).max(1);
        let mut samples = Vec::with_capacity(valid / (step * step) + 1);
        for y in (0..self.height).step_by(step) {
            for x in (0..self.width).step_by(step) {
                let value = self.values[y * self.width + x];
                if value.is_finite() {
                    samples.push(Sample {
                        x: x as f32,
                        y: y as f32,
                        value,
                    });
                }
            }
        }
        samples
    }
}

/// A source level with central-difference gradients: `[value, d/dx, d/dy]`, NaN where any
/// contributing sample is masked or at the border.
struct GradientLevel {
    width: usize,
    height: usize,
    values: Vec<[f32; 3]>,
}

impl GradientLevel {
    fn new(level: &Level) -> Self {
        let (w, h) = (level.width, level.height);
        let v = |x: usize, y: usize| level.values[y * w + x];
        let mut values = vec![[f32::NAN; 3]; w * h];
        values.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            if y == 0 || y + 1 == h {
                return;
            }
            for (x, out) in row.iter_mut().enumerate().take(w - 1).skip(1) {
                *out = [
                    v(x, y),
                    0.5 * (v(x + 1, y) - v(x - 1, y)),
                    0.5 * (v(x, y + 1) - v(x, y - 1)),
                ];
            }
        });
        Self {
            width: w,
            height: h,
            values,
        }
    }

    /// Bilinear sample. All four taps must be valid, even those with zero weight, so
    /// validity does not jump when a position crosses an integer coordinate.
    fn sample(&self, x: f32, y: f32) -> Option<[f32; 3]> {
        let (indices, weights) = sample_coordinates(self.width, self.height, x, y)?;
        let mut result = [0.0; 3];
        for (index, weight) in indices.into_iter().zip(weights) {
            for (out, value) in result.iter_mut().zip(self.values[index]) {
                *out += weight * value;
            }
        }
        result.iter().all(|v| v.is_finite()).then_some(result)
    }
}

/// Exhaustive search over translation, rotation and scale on the coarsest level,
/// comparing brightness-invariant (mean-removed) robust residuals.
fn search(samples: &[Sample], source: &GradientLevel) -> Model {
    let (width, height) = (source.width as u32, source.height as u32);
    let mut candidates = Vec::new();
    for scale in SEARCH_SCALES {
        for rotation in SEARCH_ROTATIONS_DEGREES {
            for y in -SEARCH_RADIUS..=SEARCH_RADIUS {
                for x in -SEARCH_RADIUS..=SEARCH_RADIUS {
                    candidates.push((x as f32, y as f32, rotation.to_radians(), scale));
                }
            }
        }
    }
    candidates
        .into_par_iter()
        .filter_map(|(x, y, rotation, scale)| {
            let alignment = Alignment::similarity(width, height, x, y, rotation, scale);
            let (cost, bias) = search_cost(samples, source, alignment)?;
            // Prefer the smallest motion among equally good candidates.
            let motion = x.abs() + y.abs() + rotation.abs() + (scale - 1.0).abs();
            Some((cost, motion, alignment, bias))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)))
        .map_or(
            Model {
                matrix: IDENTITY,
                bias: 0.0,
            },
            |(_, _, alignment, bias)| Model {
                matrix: alignment.matrix,
                bias: f64::from(bias),
            },
        )
}

/// Mean-removed squared residual (clamped against outliers) and the mean residual, or
/// `None` when the frames overlap by less than half.
fn search_cost(
    samples: &[Sample],
    source: &GradientLevel,
    alignment: Alignment,
) -> Option<(f32, f32)> {
    let mut inside = 0;
    let mut residuals = Vec::with_capacity(samples.len());
    for sample in samples {
        let (x, y) = alignment.map(sample.x, sample.y);
        if sample_coordinates(source.width, source.height, x, y).is_some() {
            inside += 1;
        }
        if let Some([value, _, _]) = source.sample(x, y) {
            residuals.push(value - sample.value);
        }
    }
    if residuals.len() < 32 || inside * 2 < samples.len() {
        return None;
    }
    let mean = residuals.iter().sum::<f32>() / residuals.len() as f32;
    let cost = residuals
        .iter()
        .map(|r| (r - mean).powi(2).min(0.25))
        .sum::<f32>()
        / residuals.len() as f32;
    Some((cost, mean))
}

/// The estimate on one level: the homography in that level's pixels and the
/// log-luminance bias of the source.
#[derive(Clone, Copy, Debug)]
struct Model {
    matrix: Matrix,
    bias: f64,
}

impl Model {
    /// The same mapping on a grid `factor` times finer, whose pixels bin into this one's
    /// from the top-left: `B·H·B⁻¹` with `B` mapping this grid's pixels to the finer one's
    /// (`x ↦ factor·x + (factor − 1)/2`). A factor below one moves to a coarser grid.
    fn on_grid(self, factor: f64) -> Self {
        let offset = 0.5 * (factor - 1.0);
        let to_fine = [
            [factor, 0.0, offset],
            [0.0, factor, offset],
            [0.0, 0.0, 1.0],
        ];
        let to_coarse = [
            [1.0 / factor, 0.0, -offset / factor],
            [0.0, 1.0 / factor, -offset / factor],
            [0.0, 0.0, 1.0],
        ];
        Self {
            matrix: multiply(&multiply(&to_fine, &self.matrix), &to_coarse),
            bias: self.bias,
        }
    }
}

/// Parameters refined on a level.
#[derive(Clone, Copy)]
enum Stage {
    /// Translation, rotation and scale (4) plus bias.
    Similarity,
    /// All eight homography entries plus bias.
    Homography,
}

impl Stage {
    /// Columns map the stage's parameters to the full parameter vector
    /// `[h00, h01, h02, h10, h11, h12, h20, h21, bias]`. A similarity step changes
    /// `(h00, h01, h10, h11)` by `(a, −b, b, a)`.
    fn basis(self) -> Vec<[f64; PARAMETERS]> {
        let unit = |i: usize| std::array::from_fn(|k| f64::from(u8::from(k == i)));
        match self {
            Self::Similarity => vec![
                unit(2),
                unit(5),
                std::array::from_fn(|k| f64::from(u8::from(k == 0 || k == 4))),
                std::array::from_fn(|k| match k {
                    1 => -1.0,
                    3 => 1.0,
                    _ => 0.0,
                }),
                unit(8),
            ],
            Self::Homography => (0..PARAMETERS).map(unit).collect(),
        }
    }
}

const PARAMETERS: usize = 9;

/// Pixel coordinates centered on the level and scaled by its half diagonal, which keeps
/// the homography entries comparable in magnitude for the solver.
struct Normalization {
    cx: f64,
    cy: f64,
    radius: f64,
}

impl Normalization {
    fn new(width: usize, height: usize) -> Self {
        Self {
            cx: (width - 1) as f64 * 0.5,
            cy: (height - 1) as f64 * 0.5,
            radius: 0.5 * (width as f64).hypot(height as f64),
        }
    }

    /// `N` (pixels to normalized coordinates) and `N⁻¹`.
    fn matrices(&self) -> (Matrix, Matrix) {
        let (r, cx, cy) = (self.radius, self.cx, self.cy);
        (
            [
                [1.0 / r, 0.0, -cx / r],
                [0.0, 1.0 / r, -cy / r],
                [0.0, 0.0, 1.0],
            ],
            [[r, 0.0, cx], [0.0, r, cy], [0.0, 0.0, 1.0]],
        )
    }

    /// `N·H·N⁻¹`, scaled so its last entry is one, followed by the bias.
    fn parameters(&self, model: Model) -> [f64; PARAMETERS] {
        let (to_normalized, to_pixels) = self.matrices();
        let h = multiply(&multiply(&to_normalized, &model.matrix), &to_pixels);
        let scale = 1.0 / h[2][2];
        [
            h[0][0] * scale,
            h[0][1] * scale,
            h[0][2] * scale,
            h[1][0] * scale,
            h[1][1] * scale,
            h[1][2] * scale,
            h[2][0] * scale,
            h[2][1] * scale,
            model.bias,
        ]
    }

    fn model(&self, p: &[f64; PARAMETERS]) -> Model {
        let (to_normalized, to_pixels) = self.matrices();
        let h = [[p[0], p[1], p[2]], [p[3], p[4], p[5]], [p[6], p[7], 1.0]];
        Model {
            matrix: multiply(&multiply(&to_pixels, &h), &to_normalized),
            bias: p[8],
        }
    }
}

/// Normal equations of the robust Gauss–Newton step and the robust cost.
#[derive(Clone, Copy)]
struct Normal {
    hessian: [[f64; PARAMETERS]; PARAMETERS],
    gradient: [f64; PARAMETERS],
    cost: f64,
    absolute: f64,
    count: usize,
}

impl Default for Normal {
    fn default() -> Self {
        Self {
            hessian: [[0.0; PARAMETERS]; PARAMETERS],
            gradient: [0.0; PARAMETERS],
            cost: 0.0,
            absolute: 0.0,
            count: 0,
        }
    }
}

impl Normal {
    fn merge(mut self, other: Self) -> Self {
        for (row, other) in self.hessian.iter_mut().zip(other.hessian) {
            for (value, other) in row.iter_mut().zip(other) {
                *value += other;
            }
        }
        for (value, other) in self.gradient.iter_mut().zip(other.gradient) {
            *value += other;
        }
        self.cost += other.cost;
        self.absolute += other.absolute;
        self.count += other.count;
        self
    }

    fn mean_cost(&self) -> f64 {
        if self.count == 0 {
            f64::INFINITY
        } else {
            self.cost / self.count as f64
        }
    }

    /// Solves `(Pᵀ·H·P + λ·diag) δ = −Pᵀ·g` for the stage parameters by Gaussian
    /// elimination with partial pivoting and returns the full-parameter step `P·δ`.
    fn step(&self, stage: Stage, lambda: f64) -> Option<[f64; PARAMETERS]> {
        let basis = stage.basis();
        let n = basis.len();
        let project = |u: &[f64; PARAMETERS], v: &[f64; PARAMETERS]| -> f64 {
            (0..PARAMETERS)
                .map(|k| {
                    u[k] * (0..PARAMETERS)
                        .map(|l| self.hessian[k][l] * v[l])
                        .sum::<f64>()
                })
                .sum()
        };
        let mut m: Vec<Vec<f64>> = basis
            .iter()
            .map(|u| {
                let mut row: Vec<f64> = basis.iter().map(|v| project(u, v)).collect();
                row.push(
                    -(0..PARAMETERS)
                        .map(|k| u[k] * self.gradient[k])
                        .sum::<f64>(),
                );
                row
            })
            .collect();
        for (i, row) in m.iter_mut().enumerate() {
            row[i] += lambda * row[i] + 1e-12;
        }
        for column in 0..n {
            let pivot =
                (column..n).max_by(|&a, &b| m[a][column].abs().total_cmp(&m[b][column].abs()))?;
            m.swap(column, pivot);
            if m[column][column].abs() < 1e-18 {
                return None;
            }
            let (upper, lower) = m.split_at_mut(column + 1);
            let pivot = &upper[column];
            for row in lower {
                let factor = row[column] / pivot[column];
                for (value, pivot) in row[column..].iter_mut().zip(&pivot[column..]) {
                    *value -= factor * pivot;
                }
            }
        }
        let mut reduced = vec![0.0; n];
        for row in (0..n).rev() {
            let known: f64 = (row + 1..n).map(|k| m[row][k] * reduced[k]).sum();
            reduced[row] = (m[row][n] - known) / m[row][row];
        }
        let step: [f64; PARAMETERS] =
            std::array::from_fn(|k| basis.iter().zip(&reduced).map(|(u, d)| u[k] * d).sum());
        step.iter().all(|v| v.is_finite()).then_some(step)
    }
}

/// Linearizes the Huber-weighted residual `source(H(p)) − reference(p) − bias` around
/// `p` (normalized parameters). With `huber = ∞` this is plain least squares.
fn linearize(
    samples: &[Sample],
    source: &GradientLevel,
    frame: &Normalization,
    p: &[f64; PARAMETERS],
    huber: f64,
) -> Normal {
    let (r, cx, cy) = (frame.radius, frame.cx, frame.cy);
    let mut normal = samples
        .par_chunks(4096)
        .map(|chunk| {
            let mut normal = Normal::default();
            for sample in chunk {
                let u = (f64::from(sample.x) - cx) / r;
                let v = (f64::from(sample.y) - cy) / r;
                let w = p[6] * u + p[7] * v + 1.0;
                if w <= 0.0 {
                    continue;
                }
                let su = (p[0] * u + p[1] * v + p[2]) / w;
                let sv = (p[3] * u + p[4] * v + p[5]) / w;
                let Some([value, gx, gy]) =
                    source.sample((cx + r * su) as f32, (cy + r * sv) as f32)
                else {
                    continue;
                };
                let residual = f64::from(value - sample.value) - p[8];
                // d residual / d(normalized source position) = gradient · radius.
                let (gu, gv) = (f64::from(gx) * r / w, f64::from(gy) * r / w);
                let perspective = -(gu * su + gv * sv);
                let jacobian = [
                    gu * u,
                    gu * v,
                    gu,
                    gv * u,
                    gv * v,
                    gv,
                    perspective * u,
                    perspective * v,
                    -1.0,
                ];
                let magnitude = residual.abs();
                let (weight, cost) = if magnitude <= huber {
                    (1.0, 0.5 * residual * residual)
                } else {
                    (huber / magnitude, huber * (magnitude - 0.5 * huber))
                };
                for i in 0..PARAMETERS {
                    let wj = weight * jacobian[i];
                    for (value, j) in normal.hessian[i][i..].iter_mut().zip(&jacobian[i..]) {
                        *value += wj * j;
                    }
                    normal.gradient[i] += wj * residual;
                }
                normal.cost += cost;
                normal.absolute += magnitude;
                normal.count += 1;
            }
            normal
        })
        .reduce(Normal::default, Normal::merge);
    for i in 0..PARAMETERS {
        for j in 0..i {
            normal.hessian[i][j] = normal.hessian[j][i];
        }
    }
    let prior = PERSPECTIVE_PRIOR * normal.count as f64;
    for i in [6, 7] {
        normal.hessian[i][i] += prior;
        normal.gradient[i] += prior * p[i];
        normal.cost += 0.5 * prior * p[i] * p[i];
    }
    normal
}

/// Levenberg–Marquardt refinement on one level.
fn refine(samples: &[Sample], source: &GradientLevel, model: Model, stage: Stage) -> Model {
    const ITERATIONS: usize = 40;
    let frame = Normalization::new(source.width, source.height);
    let mut p = frame.parameters(model);
    // Huber threshold from the residual spread at the start of the level
    // (mean absolute deviation × 1.25 ≈ σ for Gaussian noise).
    let initial = linearize(samples, source, &frame, &p, f64::INFINITY);
    if initial.count < 32 {
        return model;
    }
    let sigma = 1.25 * initial.absolute / initial.count as f64;
    let huber = 1.345 * sigma.max(0.01);
    let mut current = linearize(samples, source, &frame, &p, huber);
    let mut lambda = 1e-3;
    for _ in 0..ITERATIONS {
        let Some(step) = current.step(stage, lambda) else {
            break;
        };
        let candidate: [f64; PARAMETERS] = std::array::from_fn(|k| p[k] + step[k]);
        let next = linearize(samples, source, &frame, &candidate, huber);
        // A step must not buy a lower cost by leaving most of the overlap.
        if next.count * 2 >= current.count && next.mean_cost() <= current.mean_cost() {
            p = candidate;
            current = next;
            lambda = (lambda * 0.25).max(1e-7);
            // Largest resulting motion anywhere in the level, in level pixels.
            let motion = frame.radius
                * [
                    step[2].abs(),
                    step[5].abs(),
                    step[0].abs() + step[1].abs(),
                    step[3].abs() + step[4].abs(),
                    step[6].abs() + step[7].abs(),
                ]
                .into_iter()
                .fold(0.0, f64::max);
            if motion < 1e-3 {
                break;
            }
        } else {
            lambda *= 8.0;
            if lambda > 1e6 {
                break;
            }
        }
    }
    frame.model(&p)
}

#[cfg(test)]
mod tests;
