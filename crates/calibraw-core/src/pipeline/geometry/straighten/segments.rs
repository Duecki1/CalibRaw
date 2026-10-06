//! Line-segment detection following the region-growing stage of LSD (R. Grompone von Gioi et
//! al., "LSD: a Line Segment Detector", IPOL 2012), written from the paper. The a-contrario
//! NFA validation is replaced by length and straightness tests, which are enough to find the
//! long, clean edges that straightening relies on.

use std::f32::consts::{FRAC_PI_8, PI, TAU};

/// Largest angle between a pixel's level line and its region's mean orientation.
const ANGLE_TOLERANCE: f32 = FRAC_PI_8;
/// LSD's bound for gradients explained by 8-bit quantization alone: q / sin(tolerance), q = 2.
const MIN_GRADIENT: f32 = 2.0 / 0.382_683_43;
/// Largest magnitude-weighted RMS distance of region pixels from the fitted axis. A clean edge
/// concentrates its gradient within about a pixel; curved or blob-like regions spread further.
const MAX_ACROSS_RMS: f64 = 1.0;
/// Smallest number of region pixels per unit length; 8-connected edges give about two.
const MIN_PIXELS_PER_LENGTH: f32 = 1.0;
/// Radius reduction applied to a region that is not straight, as in LSD.
const RADIUS_REDUCTION: f64 = 0.75;
const MAGNITUDE_BINS: usize = 1024;

/// A straight edge in raster pixel coordinates (x right, y down, pixel centres on integers).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct LineSegment {
    pub(super) start: [f32; 2],
    pub(super) end: [f32; 2],
}

impl LineSegment {
    pub(super) fn delta(self) -> [f32; 2] {
        [self.end[0] - self.start[0], self.end[1] - self.start[1]]
    }

    pub(super) fn midpoint(self) -> [f32; 2] {
        [
            (self.start[0] + self.end[0]) * 0.5,
            (self.start[1] + self.end[1]) * 0.5,
        ]
    }

    pub(super) fn length(self) -> f32 {
        let [dx, dy] = self.delta();
        dx.hypot(dy)
    }
}

const UNUSED: u8 = 0;
const USED: u8 = 1;
/// Gradient too weak to give a reliable orientation; never part of a region.
const UNDEFINED: u8 = 2;

struct Gradients {
    width: usize,
    height: usize,
    magnitude: Vec<f32>,
    /// Level-line angle in radians, (-π, π]; the edge direction with its contrast polarity.
    angle: Vec<f32>,
}

/// Pyramid levels searched, each half the size of the previous one.
const PYRAMID_LEVELS: usize = 3;
/// Shortest edge of a pyramid level worth searching.
const MIN_LEVEL_EDGE: usize = 160;

/// Detects segments at least `min_length` pixels long at full size and on a pyramid of 2×
/// box-downsampled rasters, returned in full-size coordinates.
///
/// Defocused and soft edges (shallow depth of field, shadowed masonry) spread their gradient
/// over several pixels, so the straightness test rejects them at full size, while their
/// downsampled gradient is as tight as a sharp edge's. Sharp edges are found at every level;
/// callers merge those duplicates.
pub(super) fn detect_line_segments_multiscale(
    width: usize,
    height: usize,
    values: &[f32],
    min_length: f32,
) -> Vec<LineSegment> {
    let mut segments = detect_line_segments(width, height, values, min_length);
    let mut level_width = width;
    let mut level_height = height;
    let mut level_values = values.to_vec();
    let mut scale = 1.0f32;
    for _ in 1..PYRAMID_LEVELS {
        if level_width.min(level_height) / 2 < MIN_LEVEL_EDGE {
            break;
        }
        (level_width, level_height, level_values) = halve(level_width, level_height, &level_values);
        scale *= 2.0;
        // A coarse pixel centre i lies at fine position `scale * i + (scale - 1) / 2`.
        let offset = (scale - 1.0) * 0.5;
        let map = |[x, y]: [f32; 2]| [x * scale + offset, y * scale + offset];
        segments.extend(
            detect_line_segments(level_width, level_height, &level_values, min_length / scale)
                .into_iter()
                .map(|segment| LineSegment {
                    start: map(segment.start),
                    end: map(segment.end),
                }),
        );
    }
    segments
}

/// 2×2 box downsample; an odd last row or column is dropped.
fn halve(width: usize, height: usize, values: &[f32]) -> (usize, usize, Vec<f32>) {
    let (out_width, out_height) = (width / 2, height / 2);
    let mut out = Vec::with_capacity(out_width * out_height);
    for y in 0..out_height {
        let top = &values[2 * y * width..(2 * y + 1) * width];
        let bottom = &values[(2 * y + 1) * width..(2 * y + 2) * width];
        out.extend(
            (0..out_width)
                .map(|x| (top[2 * x] + top[2 * x + 1] + bottom[2 * x] + bottom[2 * x + 1]) * 0.25),
        );
    }
    (out_width, out_height, out)
}

/// Detects segments at least `min_length` pixels long in a 0–255 luminance raster.
pub(super) fn detect_line_segments(
    width: usize,
    height: usize,
    values: &[f32],
    min_length: f32,
) -> Vec<LineSegment> {
    if width < 3 || height < 3 || values.len() != width * height {
        return Vec::new();
    }
    let smoothed = smooth(width, height, values);
    let gradients = gradients(width, height, &smoothed);
    let mut status: Vec<u8> = gradients
        .magnitude
        .iter()
        .map(|&magnitude| {
            if magnitude > MIN_GRADIENT {
                UNUSED
            } else {
                UNDEFINED
            }
        })
        .collect();

    let mut segments = Vec::new();
    let mut region = Vec::new();
    for seed in seeds_by_magnitude(&gradients.magnitude, &status) {
        if status[seed] != UNUSED {
            continue;
        }
        let mut fit = grow_and_fit(
            &gradients,
            &mut status,
            &mut region,
            seed,
            ANGLE_TOLERANCE,
            min_length,
        );
        if fit == Fit::Curved {
            // Two edges meeting at a shallow angle merge into one region; a tighter tolerance
            // from the same seed usually keeps the stronger edge.
            for &index in &region {
                status[index] = UNUSED;
            }
            fit = grow_and_fit(
                &gradients,
                &mut status,
                &mut region,
                seed,
                ANGLE_TOLERANCE * 0.5,
                min_length,
            );
        }
        if fit == Fit::Curved {
            fit = reduce_radius(&gradients, &mut status, &mut region, seed, min_length);
        }
        if let Fit::Line(segment) = fit {
            if segment.length() >= min_length {
                segments.push(segment);
            }
        }
    }
    segments
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Fit {
    Line(LineSegment),
    /// Too small or not elongated along its level lines.
    Rejected,
    /// Elongated, but its pixels spread too far from a straight axis.
    Curved,
}

/// Separable [1 2 1] / 4 blur, edge-clamped, which suppresses aliasing before 2×2 gradients.
fn smooth(width: usize, height: usize, values: &[f32]) -> Vec<f32> {
    let mut horizontal = vec![0.0; values.len()];
    for y in 0..height {
        let row = &values[y * width..(y + 1) * width];
        for x in 0..width {
            let left = row[x.saturating_sub(1)];
            let right = row[(x + 1).min(width - 1)];
            horizontal[y * width + x] = (left + 2.0 * row[x] + right) * 0.25;
        }
    }
    let mut out = vec![0.0; values.len()];
    for y in 0..height {
        let above = y.saturating_sub(1) * width;
        let below = (y + 1).min(height - 1) * width;
        for x in 0..width {
            out[y * width + x] =
                (horizontal[above + x] + 2.0 * horizontal[y * width + x] + horizontal[below + x])
                    * 0.25;
        }
    }
    out
}

/// LSD's 2×2 gradient, located at pixel corners; the last row and column stay undefined.
fn gradients(width: usize, height: usize, values: &[f32]) -> Gradients {
    let mut magnitude = vec![0.0; values.len()];
    let mut angle = vec![0.0; values.len()];
    for y in 0..height - 1 {
        for x in 0..width - 1 {
            let index = y * width + x;
            let top_left = values[index];
            let top_right = values[index + 1];
            let bottom_left = values[index + width];
            let bottom_right = values[index + width + 1];
            let diagonal = bottom_right - top_left;
            let anti_diagonal = top_right - bottom_left;
            let gx = diagonal + anti_diagonal;
            let gy = diagonal - anti_diagonal;
            magnitude[index] = gx.hypot(gy) * 0.5;
            angle[index] = gx.atan2(-gy);
        }
    }
    Gradients {
        width,
        height,
        magnitude,
        angle,
    }
}

/// Defined pixels, strongest gradient first (bucket order, as in LSD).
fn seeds_by_magnitude(magnitude: &[f32], status: &[u8]) -> Vec<usize> {
    let max = magnitude.iter().copied().fold(0.0f32, f32::max);
    if max <= 0.0 {
        return Vec::new();
    }
    let bin_of = |value: f32| {
        (((value / max) * (MAGNITUDE_BINS - 1) as f32) as usize).min(MAGNITUDE_BINS - 1)
    };
    let mut counts = vec![0usize; MAGNITUDE_BINS + 1];
    for (index, &value) in magnitude.iter().enumerate() {
        if status[index] == UNUSED {
            counts[MAGNITUDE_BINS - 1 - bin_of(value) + 1] += 1;
        }
    }
    for bin in 1..counts.len() {
        counts[bin] += counts[bin - 1];
    }
    let mut order = vec![0usize; counts[MAGNITUDE_BINS]];
    for (index, &value) in magnitude.iter().enumerate() {
        if status[index] == UNUSED {
            let slot = &mut counts[MAGNITUDE_BINS - 1 - bin_of(value)];
            order[*slot] = index;
            *slot += 1;
        }
    }
    order
}

fn angle_difference(a: f32, b: f32) -> f32 {
    let difference = (a - b).abs() % TAU;
    if difference > PI {
        TAU - difference
    } else {
        difference
    }
}

/// Grows the 8-connected region of pixels whose level lines agree with the region's running
/// mean orientation, marks it used and fits a rectangle to it.
fn grow_and_fit(
    gradients: &Gradients,
    status: &mut [u8],
    region: &mut Vec<usize>,
    seed: usize,
    tolerance: f32,
    min_length: f32,
) -> Fit {
    let width = gradients.width;
    let height = gradients.height;
    region.clear();
    region.push(seed);
    status[seed] = USED;
    let mut sum_cos = gradients.angle[seed].cos();
    let mut sum_sin = gradients.angle[seed].sin();
    let mut region_angle = gradients.angle[seed];
    let mut next = 0;
    while next < region.len() {
        let index = region[next];
        next += 1;
        let x = index % width;
        let y = index / width;
        for ny in y.saturating_sub(1)..=(y + 1).min(height - 1) {
            for nx in x.saturating_sub(1)..=(x + 1).min(width - 1) {
                let neighbour = ny * width + nx;
                if status[neighbour] != UNUSED
                    || angle_difference(gradients.angle[neighbour], region_angle) > tolerance
                {
                    continue;
                }
                status[neighbour] = USED;
                region.push(neighbour);
                sum_cos += gradients.angle[neighbour].cos();
                sum_sin += gradients.angle[neighbour].sin();
                region_angle = sum_sin.atan2(sum_cos);
            }
        }
    }
    // A segment has at least `MIN_PIXELS_PER_LENGTH` pixels per unit length.
    if (region.len() as f32) < MIN_PIXELS_PER_LENGTH * min_length {
        return Fit::Rejected;
    }
    fit_line(gradients, region, Some((region_angle, tolerance)))
}

/// Shrinks a curved region towards its seed until the remainder is straight or too short, as
/// LSD does when a region fails its density test. Removed pixels become available as seeds.
fn reduce_radius(
    gradients: &Gradients,
    status: &mut [u8],
    region: &mut Vec<usize>,
    seed: usize,
    min_length: f32,
) -> Fit {
    let width = gradients.width;
    let position = |index: usize| ((index % width) as f64, (index / width) as f64);
    let (seed_x, seed_y) = position(seed);
    let distance_squared = |index: usize| {
        let (x, y) = position(index);
        (x - seed_x).powi(2) + (y - seed_y).powi(2)
    };
    let mut radius_squared = region
        .iter()
        .map(|&index| distance_squared(index))
        .fold(0.0, f64::max);
    loop {
        radius_squared *= RADIUS_REDUCTION * RADIUS_REDUCTION;
        // The region spans at most twice the radius.
        if 2.0 * radius_squared.sqrt() < f64::from(min_length) {
            return Fit::Rejected;
        }
        region.retain(|&index| {
            let keep = distance_squared(index) <= radius_squared;
            if !keep {
                status[index] = UNUSED;
            }
            keep
        });
        match fit_line(gradients, region, None) {
            Fit::Curved => continue,
            fit => return fit,
        }
    }
}

/// Fits the magnitude-weighted principal axis of the region and measures its extent along and
/// across that axis. With `orientation`, the axis must agree with the region's mean level-line
/// angle within the tolerance.
fn fit_line(gradients: &Gradients, region: &[usize], orientation: Option<(f32, f32)>) -> Fit {
    if region.len() < 2 {
        return Fit::Rejected;
    }
    let width = gradients.width;
    let position = |index: usize| ((index % width) as f64, (index / width) as f64);
    let mut weight_sum = 0.0f64;
    let (mut mean_x, mut mean_y) = (0.0f64, 0.0f64);
    for &index in region {
        let weight = f64::from(gradients.magnitude[index]);
        let (x, y) = position(index);
        weight_sum += weight;
        mean_x += weight * x;
        mean_y += weight * y;
    }
    if weight_sum <= 0.0 {
        return Fit::Rejected;
    }
    mean_x /= weight_sum;
    mean_y /= weight_sum;
    let (mut sxx, mut syy, mut sxy) = (0.0f64, 0.0f64, 0.0f64);
    for &index in region {
        let weight = f64::from(gradients.magnitude[index]);
        let (x, y) = position(index);
        let (dx, dy) = (x - mean_x, y - mean_y);
        sxx += weight * dx * dx;
        syy += weight * dy * dy;
        sxy += weight * dx * dy;
    }
    let axis = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    if let Some((region_angle, tolerance)) = orientation {
        // The principal axis is unoriented; it must agree with the level lines up to polarity.
        let difference = angle_difference(axis as f32, region_angle);
        if difference.min(PI - difference) > tolerance {
            return Fit::Rejected;
        }
    }
    let (sin, cos) = axis.sin_cos();
    let (mut along_min, mut along_max) = (f64::MAX, f64::MIN);
    let mut across_squared = 0.0f64;
    for &index in region {
        let (x, y) = position(index);
        let (dx, dy) = (x - mean_x, y - mean_y);
        let along = dx * cos + dy * sin;
        let across = -dx * sin + dy * cos;
        along_min = along_min.min(along);
        along_max = along_max.max(along);
        across_squared += f64::from(gradients.magnitude[index]) * across * across;
    }
    let length = (along_max - along_min + 1.0) as f32;
    if (region.len() as f32) < MIN_PIXELS_PER_LENGTH * length {
        return Fit::Rejected;
    }
    if (across_squared / weight_sum).sqrt() > MAX_ACROSS_RMS {
        return Fit::Curved;
    }
    // Gradients sit at pixel corners, half a pixel right of and below the pixel index.
    let centre_x = mean_x + 0.5;
    let centre_y = mean_y + 0.5;
    Fit::Line(LineSegment {
        start: [
            (centre_x + along_min * cos) as f32,
            (centre_y + along_min * sin) as f32,
        ],
        end: [
            (centre_x + along_max * cos) as f32,
            (centre_y + along_max * sin) as f32,
        ],
    })
}
