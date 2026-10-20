//! Lateral chromatic aberration: a lens images red and blue at a slightly
//! different magnification than green, so their edges land a little inside or
//! outside green's, increasingly toward the corners, and show as red/cyan or
//! blue/yellow fringes.
//!
//! [`LateralChromaticAberration::estimate`] measures the photo once, when it is
//! decoded: it finds how far red and blue sit from green in blocks across the
//! frame and fits one radial model per channel. The GPU moves both channels
//! back onto green by the edit's Chromatic Aberration amount
//! (`chromatic_aberration.wgsl`, which also removes the axial fringes no
//! displacement explains).

use super::LoadedRaw;
use rayon::prelude::*;

/// Where red and blue sit relative to green over the whole photo. Positions
/// are normalized: the centre of the photo is the origin and its half
/// diagonal has length 1, so the model holds at every resolution, and crops,
/// proxies and tiles share their source's.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LateralChromaticAberration {
    /// `[k1, k2, tx, ty]` of red, then of blue: at normalized position `u`
    /// the channel shows what green shows at `u + (k1 + k2·|u|²)·u + t`.
    channels: [[f32; 4]; 2],
}

/// A colour channel the model moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LateralChannel {
    Red,
    Blue,
}

impl LateralChannel {
    const ALL: [Self; 2] = [Self::Red, Self::Blue];

    const fn index(self) -> usize {
        match self {
            Self::Red => 0,
            Self::Blue => 1,
        }
    }

    /// The channel's index in RGB.
    const fn rgb_index(self) -> usize {
        match self {
            Self::Red => 0,
            Self::Blue => 2,
        }
    }
}

/// Native pixels of a measurement block's side.
const BLOCK: usize = 64;
/// Gaussian sigma, in native pixels, of the filter that interpolates each
/// channel from its photosites. Applying the same blur to every channel keeps
/// their displacement and removes most of the noise that would otherwise pull
/// the estimates toward zero.
const PREFILTER_SIGMA: f32 = 1.2;
const PREFILTER_RADIUS: usize = 4;
/// Largest displacement, in native pixels, the model may predict anywhere in
/// the frame. Lenses stay well below it; an estimate beyond it is rejected.
/// The export halo reserves it (`CHROMATIC_ABERRATION_SUPPORT`).
pub const MAX_LATERAL_SHIFT: f32 = 8.0;
/// Pixels around a block that its displaced, filtered planes may read.
const MARGIN: usize = PREFILTER_RADIUS + MAX_LATERAL_SHIFT as usize + 4;
const REGION: usize = BLOCK + 2 * MARGIN;
/// Most block rows measured; columns follow the aspect ratio. A few hundred
/// blocks pin down the four parameters of each channel's model.
const MAX_BLOCK_ROWS: usize = 16;
/// Blocks whose green varies by less than this share of its level carry no
/// position and are skipped before filtering.
const MIN_BLOCK_CONTRAST: f32 = 0.01;
/// Fewest blocks that must agree with the model for it to be used.
const MIN_INLIER_BLOCKS: usize = 16;
/// Share of a block's channel variance its displaced green must explain.
const MIN_BLOCK_FIT: f64 = 0.85;
/// Spread, in native pixels, that one block's measurement differs from the
/// radial model by even when its edges are sharp: local lens defects and
/// edges whose colours really differ. It keeps the strongest blocks from
/// outweighing the rest.
const MODEL_ERROR_PX: f64 = 0.15;
/// Smallest displacement worth correcting, in native pixels at the corners.
const MIN_CORNER_SHIFT: f32 = 0.1;

impl LateralChromaticAberration {
    pub const NONE: Self = Self {
        channels: [[0.0; 4]; 2],
    };

    pub fn is_none(&self) -> bool {
        *self == Self::NONE
    }

    /// A model with `[k1, k2, tx, ty]` of red and of blue.
    #[cfg(test)]
    pub(crate) const fn from_coefficients(red: [f32; 4], blue: [f32; 4]) -> Self {
        Self {
            channels: [red, blue],
        }
    }

    /// `[k1, k2, tx, ty]` of `channel` (see the field documentation).
    pub fn coefficients(&self, channel: LateralChannel) -> [f32; 4] {
        self.channels[channel.index()]
    }

    /// Where `channel` shows green's content at pixel `point` of the photo
    /// at `extent` pixels: the displacement, in those pixels, from `point`.
    pub fn displacement(
        &self,
        channel: LateralChannel,
        point: [f32; 2],
        extent: [u32; 2],
    ) -> [f32; 2] {
        let frame = Frame::new(extent);
        let [k1, k2, tx, ty] = self.coefficients(channel);
        let [ux, uy] = frame.normalize(point);
        let radial = k1 + k2 * (ux * ux + uy * uy);
        [
            (radial * ux + tx) * frame.half_diagonal,
            (radial * uy + ty) * frame.half_diagonal,
        ]
    }

    /// The largest displacement of `channel`, in pixels, at the corners of a
    /// photo at `extent` pixels.
    pub fn largest_corner_shift(&self, channel: LateralChannel, extent: [u32; 2]) -> f32 {
        let [right, bottom] = extent.map(|edge| edge.saturating_sub(1) as f32);
        [[0.0, 0.0], [right, 0.0], [0.0, bottom], [right, bottom]]
            .map(|corner| self.displacement(channel, corner, extent))
            .into_iter()
            .map(|[x, y]| x.hypot(y))
            .fold(0.0, f32::max)
    }

    /// Measures the photo's lateral chromatic aberration. Returns
    /// [`Self::NONE`] when the photo shows too few edges to measure it or
    /// when it is too small to matter.
    pub fn estimate(raw: &LoadedRaw) -> Self {
        let started = std::time::Instant::now();
        let extent = [raw.width, raw.height];
        let model = match raw.scene_linear_raster() {
            Some(rgb) if raw.is_pre_demosaiced_raster() => {
                estimate_from(&RasterSamples { rgb, extent })
            }
            _ if raw.raw_pixels.len() == (raw.width as usize) * (raw.height as usize) => {
                estimate_from(&MosaicSamples { raw })
            }
            _ => Self::NONE,
        };
        let message = format!(
            "Lateral chromatic aberration measured in {:.3}s: red {:.2} px, blue {:.2} px at the corners",
            started.elapsed().as_secs_f64(),
            model.largest_corner_shift(LateralChannel::Red, extent),
            model.largest_corner_shift(LateralChannel::Blue, extent),
        );
        log::debug!("{message}");
        crate::diagnostics::record(message);
        model
    }
}

/// Normalized coordinates of a frame: centre at the origin, half diagonal 1.
struct Frame {
    center: [f32; 2],
    half_diagonal: f32,
}

impl Frame {
    fn new([width, height]: [u32; 2]) -> Self {
        let center = [
            width.saturating_sub(1) as f32 * 0.5,
            height.saturating_sub(1) as f32 * 0.5,
        ];
        Self {
            center,
            half_diagonal: center[0].hypot(center[1]).max(1.0),
        }
    }

    fn normalize(&self, [x, y]: [f32; 2]) -> [f32; 2] {
        [
            (x - self.center[0]) / self.half_diagonal,
            (y - self.center[1]) / self.half_diagonal,
        ]
    }
}

/// The colour samples of a photo, read one block region at a time.
trait ChannelSamples: Sync {
    fn extent(&self) -> [u32; 2];

    /// Fills `planes` with the region of `REGION`² pixels whose top-left
    /// pixel is `origin`: per channel, the Gaussian-weighted mean of the
    /// channel's samples around each pixel. Returns false, before filtering,
    /// when the region holds clipped samples, whose colour is unknown, or when
    /// its block is flat.
    fn fill(&self, origin: [usize; 2], planes: &mut BlockPlanes) -> bool;
}

/// Running statistics of a block's green samples.
#[derive(Default)]
struct GreenSpread {
    count: f32,
    sum: f32,
    squares: f32,
}

impl GreenSpread {
    fn add(&mut self, value: f32) {
        self.count += 1.0;
        self.sum += value;
        self.squares += value * value;
    }

    fn is_flat(&self) -> bool {
        if self.count < 1.0 {
            return true;
        }
        let mean = self.sum / self.count;
        let variance = (self.squares / self.count - mean * mean).max(0.0);
        variance.sqrt() < MIN_BLOCK_CONTRAST * (mean.abs() + 0.01)
    }
}

fn in_block(x: usize, y: usize) -> bool {
    (MARGIN..MARGIN + BLOCK).contains(&x) && (MARGIN..MARGIN + BLOCK).contains(&y)
}

/// A sensor mosaic, Bayer or X-Trans, in normalized sensor units.
struct MosaicSamples<'a> {
    raw: &'a LoadedRaw,
}

/// Sensor values at or above this share of white are taken as clipped.
const CLIP_SHARE: f32 = 0.97;

impl ChannelSamples for MosaicSamples<'_> {
    fn extent(&self) -> [u32; 2] {
        [self.raw.width, self.raw.height]
    }

    fn fill(&self, [x0, y0]: [usize; 2], planes: &mut BlockPlanes) -> bool {
        let raw = self.raw;
        let width = raw.width as usize;
        planes.clear();
        let mut green = GreenSpread::default();
        for y in 0..REGION {
            for x in 0..REGION {
                let index = (y0 + y) * width + x0 + x;
                let cfa = raw.color_indices.get(index).copied().unwrap_or(1).min(3) as usize;
                let black = raw
                    .black_levels_per_pixel
                    .get(index)
                    .copied()
                    .unwrap_or(0.0);
                let white = raw.white_levels[cfa].max(black + 1.0);
                let sample = f32::from(raw.raw_pixels[index]);
                if sample >= black + (white - black) * CLIP_SHARE {
                    return false;
                }
                let channel = if cfa == 3 { 1 } else { cfa };
                let value = (sample - black) / (white - black);
                if channel == 1 && in_block(x, y) {
                    green.add(value);
                }
                planes.add_sample(channel, x, y, value);
            }
        }
        if green.is_flat() {
            return false;
        }
        planes.interpolate();
        true
    }
}

/// A demosaiced raster's scene-linear RGB.
struct RasterSamples<'a> {
    rgb: &'a [f32],
    extent: [u32; 2],
}

/// Raster values at or above this are taken as clipped.
const RASTER_CLIP: f32 = 0.98;

impl ChannelSamples for RasterSamples<'_> {
    fn extent(&self) -> [u32; 2] {
        self.extent
    }

    fn fill(&self, [x0, y0]: [usize; 2], planes: &mut BlockPlanes) -> bool {
        let width = self.extent[0] as usize;
        planes.clear();
        let mut green = GreenSpread::default();
        for y in 0..REGION {
            for x in 0..REGION {
                let index = ((y0 + y) * width + x0 + x) * 3;
                let pixel = &self.rgb[index..index + 3];
                if pixel.iter().any(|value| *value >= RASTER_CLIP) {
                    return false;
                }
                if in_block(x, y) {
                    green.add(pixel[1]);
                }
                for (channel, value) in pixel.iter().enumerate() {
                    planes.add_sample(channel, x, y, *value);
                }
            }
        }
        if green.is_flat() {
            return false;
        }
        planes.interpolate();
        true
    }
}

/// The three channel planes of one block region, `REGION`² pixels each.
struct BlockPlanes {
    /// Weighted sample sums, then the interpolated channel values.
    values: [Vec<f32>; 3],
    /// Sample weights per pixel.
    weights: [Vec<f32>; 3],
    /// Central differences of interpolated green along x and y.
    green_gradient: [Vec<f32>; 2],
    scratch: Vec<f32>,
    kernel: [f32; 2 * PREFILTER_RADIUS + 1],
}

impl BlockPlanes {
    fn new() -> Self {
        let mut kernel = [0.0; 2 * PREFILTER_RADIUS + 1];
        for (index, weight) in kernel.iter_mut().enumerate() {
            let offset = index as f32 - PREFILTER_RADIUS as f32;
            *weight = (-0.5 * (offset / PREFILTER_SIGMA).powi(2)).exp();
        }
        Self {
            values: std::array::from_fn(|_| vec![0.0; REGION * REGION]),
            weights: std::array::from_fn(|_| vec![0.0; REGION * REGION]),
            green_gradient: std::array::from_fn(|_| vec![0.0; REGION * REGION]),
            scratch: vec![0.0; REGION * REGION],
            kernel,
        }
    }

    fn clear(&mut self) {
        for plane in self.values.iter_mut().chain(self.weights.iter_mut()) {
            plane.fill(0.0);
        }
    }

    fn add_sample(&mut self, channel: usize, x: usize, y: usize, value: f32) {
        self.values[channel][y * REGION + x] += value;
        self.weights[channel][y * REGION + x] += 1.0;
    }

    /// Normalized convolution: the blurred samples over the blurred weights.
    /// Also takes green's gradient.
    fn interpolate(&mut self) {
        for channel in 0..3 {
            blur(&mut self.values[channel], &mut self.scratch, &self.kernel);
            blur(&mut self.weights[channel], &mut self.scratch, &self.kernel);
            for (value, weight) in self.values[channel].iter_mut().zip(&self.weights[channel]) {
                *value = if *weight > 1e-4 { *value / weight } else { 0.0 };
            }
        }
        let green = &self.values[1];
        let [along_x, along_y] = &mut self.green_gradient;
        for y in 1..REGION - 1 {
            for x in 1..REGION - 1 {
                let index = y * REGION + x;
                along_x[index] = 0.5 * (green[index + 1] - green[index - 1]);
                along_y[index] = 0.5 * (green[index + REGION] - green[index - REGION]);
            }
        }
    }

    fn plane(&self, channel: usize) -> &[f32] {
        &self.values[channel]
    }
}

/// Separable blur of a `REGION`² plane in place. Pixels within the kernel's
/// radius of the region's edge are left unblurred: no block reads them.
fn blur(plane: &mut [f32], scratch: &mut [f32], kernel: &[f32]) {
    const _: () = assert!(PREFILTER_RADIUS + 4 <= MARGIN);
    let radius = kernel.len() / 2;
    scratch.copy_from_slice(plane);
    for y in 0..REGION {
        let row = &plane[y * REGION..(y + 1) * REGION];
        for x in radius..REGION - radius {
            scratch[y * REGION + x] = kernel
                .iter()
                .zip(&row[x - radius..=x + radius])
                .map(|(weight, value)| weight * value)
                .sum();
        }
    }
    plane.copy_from_slice(scratch);
    for y in radius..REGION - radius {
        for x in 0..REGION {
            plane[y * REGION + x] = kernel
                .iter()
                .enumerate()
                .map(|(index, weight)| weight * scratch[(y + index - radius) * REGION + x])
                .sum();
        }
    }
}

/// One block's measurement of a channel against green.
#[derive(Clone, Copy, Debug)]
struct BlockShift {
    /// The block centre, in pixels.
    center: [f64; 2],
    /// The channel shows green's content this many pixels away.
    shift: [f64; 2],
    /// Inverse covariance of `shift`, including the model-error floor.
    information: [[f64; 2]; 2],
}

fn estimate_from(samples: &dyn ChannelSamples) -> LateralChromaticAberration {
    let [width, height] = samples.extent();
    let (width, height) = (width as usize, height as usize);
    if width < REGION * 3 || height < REGION * 3 {
        return LateralChromaticAberration::NONE;
    }
    let rows = MAX_BLOCK_ROWS.min((height - 2 * MARGIN) / BLOCK).max(2);
    let columns = (rows * width / height).clamp(2, (width - 2 * MARGIN) / BLOCK);
    let origins: Vec<[usize; 2]> = (0..rows)
        .flat_map(|row| {
            (0..columns).map(move |column| {
                [
                    column * (width - REGION) / (columns - 1),
                    row * (height - REGION) / (rows - 1),
                ]
            })
        })
        .collect();
    let measurements: Vec<[Option<BlockShift>; 2]> = origins
        .par_iter()
        .map_init(BlockPlanes::new, |planes, &origin| {
            if !samples.fill(origin, planes) {
                return [None, None];
            }
            LateralChannel::ALL.map(|channel| measure_block(planes, origin, channel))
        })
        .collect();

    let frame = Frame::new([width as u32, height as u32]);
    let mut channels = [[0.0f32; 4]; 2];
    for channel in LateralChannel::ALL {
        let blocks: Vec<BlockShift> = measurements
            .iter()
            .filter_map(|measured| measured[channel.index()])
            .collect();
        let Some(model) = fit_radial_model(&blocks, &frame) else {
            return LateralChromaticAberration::NONE;
        };
        channels[channel.index()] = model;
    }
    let model = LateralChromaticAberration { channels };
    let extent = [width as u32, height as u32];
    let largest = LateralChannel::ALL
        .into_iter()
        .map(|channel| model.largest_corner_shift(channel, extent))
        .fold(0.0f32, f32::max);
    if !largest.is_finite() || !(MIN_CORNER_SHIFT..=MAX_LATERAL_SHIFT).contains(&largest) {
        return LateralChromaticAberration::NONE;
    }
    model
}

/// Lucas-Kanade with an affine intensity model: finds the shift `s` and gain
/// and offset with `channel(x) ≈ a·green(x + s) + b` over the block, which
/// holds across an edge whatever the colours on its two sides.
fn measure_block(
    planes: &BlockPlanes,
    origin: [usize; 2],
    channel: LateralChannel,
) -> Option<BlockShift> {
    let green = planes.plane(1);
    let target = planes.plane(channel.rgb_index());
    let green = GreenPlanes {
        value: green,
        along_x: &planes.green_gradient[0],
        along_y: &planes.green_gradient[1],
    };
    let mut shift = [0.0f64; 2];
    let mut solved = None;
    for _ in 0..8 {
        let system = normal_equations(&green, target, shift);
        let theta = solve4(system.matrix, system.vector)?;
        let gain = theta[0];
        if gain.abs() < 1e-3 {
            return None;
        }
        let delta = [theta[2] / gain, theta[3] / gain];
        shift[0] += delta[0];
        shift[1] += delta[1];
        if shift[0].hypot(shift[1]) > f64::from(MAX_LATERAL_SHIFT) {
            return None;
        }
        solved = Some(system);
        if delta[0].hypot(delta[1]) < 0.01 {
            break;
        }
    }
    let system = normal_equations(&green, target, shift);
    let theta = solve4(system.matrix, system.vector)?;
    let explained: f64 = (0..4).map(|row| theta[row] * system.vector[row]).sum();
    let residual = (system.target_squares - explained).max(0.0) / system.count;
    let variance = system.target_variance();
    if solved.is_none() || variance <= 0.0 || 1.0 - residual / variance < MIN_BLOCK_FIT {
        return None;
    }

    // Information on the shift: the Schur complement of gain and offset in
    // the normal equations, scaled by gain² over the residual variance.
    let m = system.matrix;
    let ab = [[m[0][0], m[0][1]], [m[1][0], m[1][1]]];
    let ab_inverse = invert2(ab)?;
    let mut schur = [[m[2][2], m[2][3]], [m[3][2], m[3][3]]];
    for row in 0..2 {
        for column in 0..2 {
            let mut correction = 0.0;
            for i in 0..2 {
                for j in 0..2 {
                    correction += m[i][2 + row] * ab_inverse[i][j] * m[j][2 + column];
                }
            }
            schur[row][column] -= correction;
        }
    }
    let scale = theta[0] * theta[0] / residual.max(1e-12);
    let information = schur.map(|row| row.map(|value| value * scale));
    // Add the model-error floor to the covariance.
    let mut covariance = invert2(information)?;
    covariance[0][0] += MODEL_ERROR_PX * MODEL_ERROR_PX;
    covariance[1][1] += MODEL_ERROR_PX * MODEL_ERROR_PX;
    let information = invert2(covariance)?;
    Some(BlockShift {
        center: [
            (origin[0] + MARGIN) as f64 + BLOCK as f64 * 0.5 - 0.5,
            (origin[1] + MARGIN) as f64 + BLOCK as f64 * 0.5 - 0.5,
        ],
        shift,
        information,
    })
}

fn block_pixels() -> impl Iterator<Item = (usize, usize)> {
    (MARGIN..MARGIN + BLOCK).flat_map(|y| (MARGIN..MARGIN + BLOCK).map(move |x| (x, y)))
}

/// Least-squares normal equations of `target ≈ a·g + b + c·gx + d·gy`, with
/// green and its gradient taken at each block pixel displaced by `shift`.
struct NormalEquations {
    matrix: [[f64; 4]; 4],
    vector: [f64; 4],
    target_squares: f64,
    target_sum: f64,
    count: f64,
}

impl NormalEquations {
    fn target_variance(&self) -> f64 {
        let mean = self.target_sum / self.count;
        self.target_squares / self.count - mean * mean
    }
}

/// Interpolated green of a block region and its gradient.
struct GreenPlanes<'a> {
    value: &'a [f32],
    along_x: &'a [f32],
    along_y: &'a [f32],
}

fn normal_equations(green: &GreenPlanes<'_>, target: &[f32], shift: [f64; 2]) -> NormalEquations {
    let mut matrix = [[0.0f64; 4]; 4];
    let mut vector = [0.0f64; 4];
    let mut target_squares = 0.0;
    let mut target_sum = 0.0;
    // The shift is the same for every pixel, so are the bilinear weights. The
    // shift stays below MAX_LATERAL_SHIFT, so the samples stay in the region.
    let whole = [shift[0].floor(), shift[1].floor()];
    let [fx, fy] = [(shift[0] - whole[0]) as f32, (shift[1] - whole[1]) as f32];
    let weights = [
        (1.0 - fx) * (1.0 - fy),
        fx * (1.0 - fy),
        (1.0 - fx) * fy,
        fx * fy,
    ];
    let offset = whole[1] as isize * REGION as isize + whole[0] as isize;
    let sample = |plane: &[f32], index: usize| {
        let base = (index as isize + offset) as usize;
        f64::from(
            weights[0] * plane[base]
                + weights[1] * plane[base + 1]
                + weights[2] * plane[base + REGION]
                + weights[3] * plane[base + REGION + 1],
        )
    };
    for (x, y) in block_pixels() {
        let index = y * REGION + x;
        let features = [
            sample(green.value, index),
            1.0,
            sample(green.along_x, index),
            sample(green.along_y, index),
        ];
        let t = f64::from(target[index]);
        for row in 0..4 {
            for column in 0..4 {
                matrix[row][column] += features[row] * features[column];
            }
            vector[row] += features[row] * t;
        }
        target_squares += t * t;
        target_sum += t;
    }
    NormalEquations {
        matrix,
        vector,
        target_squares,
        target_sum,
        count: (BLOCK * BLOCK) as f64,
    }
}

/// Fits `[k1, k2, tx, ty]` (in normalized units) to block shifts by weighted
/// least squares, discounting blocks the model cannot explain (Tukey's
/// biweight at a fixed chi-square, since most blocks see little aberration
/// and would otherwise set the scale).
fn fit_radial_model(blocks: &[BlockShift], frame: &Frame) -> Option<[f32; 4]> {
    if blocks.len() < MIN_INLIER_BLOCKS {
        return None;
    }
    let half_diagonal = f64::from(frame.half_diagonal);
    // The shift, in pixels, that a unit of each parameter adds at `center`.
    let design = |center: [f64; 2]| -> [[f64; 2]; 4] {
        let [ux, uy] = frame.normalize([center[0] as f32, center[1] as f32]);
        let (ux, uy) = (f64::from(ux), f64::from(uy));
        let r2 = ux * ux + uy * uy;
        [[ux, uy], [r2 * ux, r2 * uy], [1.0, 0.0], [0.0, 1.0]]
            .map(|column| column.map(|value| value * half_diagonal))
    };
    let dot = |a: [f64; 2], b: [f64; 2]| a[0] * b[0] + a[1] * b[1];
    let weigh = |information: [[f64; 2]; 2], shift: [f64; 2]| {
        [dot(information[0], shift), dot(information[1], shift)]
    };
    // A zero-shift prior worth two blocks of model error at the corners keeps
    // a few strong blocks from extrapolating a large shift.
    let prior = 2.0 * half_diagonal * half_diagonal / (MODEL_ERROR_PX * MODEL_ERROR_PX);
    const REJECT_CHI2: f64 = 2.0 * 16.0;
    let mut weights = vec![1.0f64; blocks.len()];
    let mut theta = [0.0f64; 4];
    for _ in 0..12 {
        let mut matrix = [[0.0f64; 4]; 4];
        let mut vector = [0.0f64; 4];
        for (row, item) in matrix.iter_mut().enumerate() {
            item[row] = prior;
        }
        for (block, weight) in blocks.iter().zip(&weights) {
            if *weight <= 0.0 {
                continue;
            }
            // matrix += w · Mᵀ I M, vector += w · Mᵀ I s
            let columns = design(block.center);
            for (row, column) in matrix.iter_mut().zip(&columns) {
                let weighted = weigh(block.information, *column);
                for (entry, other) in row.iter_mut().zip(&columns) {
                    *entry += weight * dot(weighted, *other);
                }
            }
            for (entry, column) in vector.iter_mut().zip(&columns) {
                *entry += weight * dot(weigh(block.information, *column), block.shift);
            }
        }
        theta = solve4(matrix, vector)?;
        for (block, weight) in blocks.iter().zip(weights.iter_mut()) {
            let predicted = design(block.center)
                .iter()
                .zip(theta)
                .fold([0.0, 0.0], |sum, (column, value)| {
                    [sum[0] + column[0] * value, sum[1] + column[1] * value]
                });
            let residual = [block.shift[0] - predicted[0], block.shift[1] - predicted[1]];
            let chi2 = dot(residual, weigh(block.information, residual));
            let u = chi2 / REJECT_CHI2;
            *weight = if u < 1.0 { (1.0 - u) * (1.0 - u) } else { 0.0 };
        }
    }
    let inliers = weights.iter().filter(|weight| **weight > 0.0).count();
    (inliers >= MIN_INLIER_BLOCKS && theta.iter().all(|value| value.is_finite()))
        .then(|| theta.map(|value| value as f32))
}

fn invert2(m: [[f64; 2]; 2]) -> Option<[[f64; 2]; 2]> {
    let determinant = m[0][0] * m[1][1] - m[0][1] * m[1][0];
    (determinant.abs() > 1e-300 && determinant.is_finite()).then(|| {
        [
            [m[1][1] / determinant, -m[0][1] / determinant],
            [-m[1][0] / determinant, m[0][0] / determinant],
        ]
    })
}

/// Solves a 4×4 system by Gaussian elimination with partial pivoting.
fn solve4(mut matrix: [[f64; 4]; 4], mut vector: [f64; 4]) -> Option<[f64; 4]> {
    let scale = matrix
        .iter()
        .flatten()
        .fold(0.0f64, |largest, value| largest.max(value.abs()));
    if scale <= 0.0 || !scale.is_finite() {
        return None;
    }
    for column in 0..4 {
        let pivot = (column..4).max_by(|a, b| {
            matrix[*a][column]
                .abs()
                .total_cmp(&matrix[*b][column].abs())
        })?;
        if matrix[pivot][column].abs() <= scale * 1e-14 {
            return None;
        }
        matrix.swap(column, pivot);
        vector.swap(column, pivot);
        let pivot_row = matrix[column];
        for row in column + 1..4 {
            let factor = matrix[row][column] / pivot_row[column];
            for (entry, pivot) in matrix[row].iter_mut().zip(pivot_row).skip(column) {
                *entry -= factor * pivot;
            }
            vector[row] -= factor * vector[column];
        }
    }
    let mut solution = [0.0f64; 4];
    for row in (0..4).rev() {
        let sum: f64 = (row + 1..4).map(|k| matrix[row][k] * solution[k]).sum();
        solution[row] = (vector[row] - sum) / matrix[row][row];
    }
    solution
        .iter()
        .all(|value| value.is_finite())
        .then_some(solution)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{CfaKind, CompactPixelMap};

    /// A rotated checkerboard with soft edges, in [0.1, 0.8].
    fn pattern(x: f32, y: f32) -> f32 {
        let (s, c) = (0.5f32, 0.866f32);
        let (u, v) = (c * x + s * y, -s * x + c * y);
        let soft = |t: f32, period: f32| (std::f32::consts::TAU * t / period).sin().tanh() * 4.0;
        let checker = (soft(u, 37.0) * soft(v, 29.0)).tanh();
        0.45 + 0.35 * checker
    }

    /// A Bayer mosaic of `pattern` whose red is magnified by `1 + red` and
    /// blue by `1 + blue` about the centre, plus a fixed blue offset.
    fn mosaic(width: u32, height: u32, red: f32, blue: f32, blue_offset: [f32; 2]) -> LoadedRaw {
        let center = [(width - 1) as f32 * 0.5, (height - 1) as f32 * 0.5];
        let mut pixels = Vec::with_capacity((width * height) as usize);
        for y in 0..height {
            for x in 0..width {
                let (dx, dy) = (x as f32 - center[0], y as f32 - center[1]);
                let value = match (x % 2, y % 2) {
                    (0, 0) => pattern(center[0] + dx * (1.0 + red), center[1] + dy * (1.0 + red)),
                    (1, 1) => pattern(
                        center[0] + dx * (1.0 + blue) + blue_offset[0],
                        center[1] + dy * (1.0 + blue) + blue_offset[1],
                    ),
                    _ => pattern(x as f32, y as f32),
                };
                pixels.push((value * 4000.0) as u16);
            }
        }
        let raster = LoadedRaw::from_scene_linear_rec2020(1, 1, vec![0.5; 3]).unwrap();
        LoadedRaw {
            cfa_kind: CfaKind::Bayer,
            white_levels: [4095.0; 4],
            ..raster.derive_with(
                width,
                height,
                pixels,
                CompactPixelMap::repeating(width, height, 2, 2, vec![0, 1, 3, 2]),
                CompactPixelMap::repeating(width, height, 1, 1, vec![0.0]),
            )
        }
    }

    #[test]
    fn recovers_magnification_of_red_and_blue() {
        let (width, height) = (1200u32, 800u32);
        let raw = mosaic(width, height, 0.002, -0.0015, [0.0, 0.0]);
        let model = LateralChromaticAberration::estimate(&raw);
        assert!(!model.is_none());
        // Red at pixel p shows green's content at c + (p - c)(1 + m), which
        // is m(p - c) away.
        let extent = [width, height];
        for (corner, red, blue) in [
            ([0.0, 0.0], 0.002, -0.0015),
            ([1199.0, 799.0], 0.002, -0.0015),
            ([1199.0, 0.0], 0.002, -0.0015),
        ] {
            let offset = [corner[0] - 599.5, corner[1] - 399.5];
            for (channel, magnification) in
                [(LateralChannel::Red, red), (LateralChannel::Blue, blue)]
            {
                let measured = model.displacement(channel, corner, extent);
                let expected = [offset[0] * magnification, offset[1] * magnification];
                let error = (measured[0] - expected[0]).hypot(measured[1] - expected[1]);
                assert!(
                    error < 0.15,
                    "{channel:?} at {corner:?}: measured {measured:?}, expected {expected:?}"
                );
            }
        }
    }

    #[test]
    fn recovers_a_decentred_offset() {
        let raw = mosaic(1200, 800, 0.0, 0.0015, [0.6, -0.4]);
        let model = LateralChromaticAberration::estimate(&raw);
        let centre = model.displacement(LateralChannel::Blue, [599.5, 399.5], [1200, 800]);
        assert!(
            (centre[0] - 0.6).abs() < 0.12 && (centre[1] + 0.4).abs() < 0.12,
            "{centre:?}"
        );
        let red = model.displacement(LateralChannel::Red, [0.0, 0.0], [1200, 800]);
        assert!(red[0].hypot(red[1]) < 0.15, "{red:?}");
    }

    #[test]
    fn aligned_channels_need_no_correction() {
        let raw = mosaic(1200, 800, 0.0, 0.0, [0.0, 0.0]);
        assert!(LateralChromaticAberration::estimate(&raw).is_none());
    }

    #[test]
    fn flat_and_tiny_photos_are_left_alone() {
        let flat =
            LoadedRaw::from_scene_linear_rec2020(800, 600, vec![0.3; 800 * 600 * 3]).unwrap();
        assert!(LateralChromaticAberration::estimate(&flat).is_none());
        let tiny = LoadedRaw::from_scene_linear_rec2020(64, 48, vec![0.3; 64 * 48 * 3]).unwrap();
        assert!(LateralChromaticAberration::estimate(&tiny).is_none());
    }

    #[test]
    fn displacement_scales_with_resolution() {
        let model = LateralChromaticAberration {
            channels: [[0.001, 0.0005, 0.0, 0.0], [0.0; 4]],
        };
        let full = model.displacement(LateralChannel::Red, [0.0, 0.0], [4001, 3001]);
        let half = model.displacement(LateralChannel::Red, [0.0, 0.0], [2001, 1501]);
        assert!((full[0] - 2.0 * half[0]).abs() < 1e-3 && (full[1] - 2.0 * half[1]).abs() < 1e-3);
    }
}
