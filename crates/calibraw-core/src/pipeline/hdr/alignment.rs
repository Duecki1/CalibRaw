use super::{sample_coordinates, validate_rgb};
use anyhow::{ensure, Result};

/// Maps reference pixels to source pixels, rotating about the image center.
#[derive(Clone, Copy, Debug, Default)]
pub struct Alignment {
    pub x: f32,
    pub y: f32,
    pub rotation: f32,
}

impl Alignment {
    pub(super) fn is_finite(self) -> bool {
        [self.x, self.y, self.rotation]
            .iter()
            .all(|v| v.is_finite())
    }

    pub(super) fn transform(self, width: usize, height: usize) -> Transform {
        let (sin, cos) = self.rotation.sin_cos();
        let cx = (width - 1) as f32 * 0.5;
        let cy = (height - 1) as f32 * 0.5;
        Transform {
            sin,
            cos,
            x: cx - cos * cx + sin * cy + self.x,
            y: cy - sin * cx - cos * cy + self.y,
        }
    }
}

pub(super) struct Transform {
    sin: f32,
    cos: f32,
    x: f32,
    y: f32,
}
impl Transform {
    pub(super) fn map(&self, x: f32, y: f32) -> (f32, f32) {
        (
            self.cos * x - self.sin * y + self.x,
            self.sin * x + self.cos * y + self.y,
        )
    }
}

struct Level {
    width: usize,
    height: usize,
    log_signal: Vec<f32>,
}

/// Bounded image pyramid of exposure-normalized log luminance. Saturated and nearly black
/// samples are masked out, making alignment insensitive to the bracket's exposure steps.
pub struct AlignmentReference {
    levels: Vec<Level>,
    width: u32,
    height: u32,
    scale: f32,
}

impl AlignmentReference {
    pub fn new(width: u32, height: u32, rgb: &[f32], exposure: f32) -> Result<Self> {
        validate_rgb(width, height, rgb, exposure)?;
        let stride = width.max(height).div_ceil(2048) as usize;
        let w = (width as usize).div_ceil(stride);
        let h = (height as usize).div_ceil(stride);
        let mut sums = vec![0.0; w * h];
        let mut counts = vec![0u32; w * h];
        for (i, pixel) in rgb.chunks_exact(3).enumerate() {
            let peak = pixel.iter().copied().fold(0.0f32, f32::max);
            let signal = (pixel[0] + 2.0 * pixel[1] + pixel[2]) * 0.25;
            if peak < 0.95 && signal > 0.004 {
                let dest = (i / width as usize / stride) * w + (i % width as usize / stride);
                sums[dest] += signal / exposure;
                counts[dest] += 1;
            }
        }
        let signal = sums
            .into_iter()
            .zip(counts)
            .map(|(v, n)| if n > 0 { (v / n as f32).ln() } else { f32::NAN })
            .collect();
        let mut levels = vec![Level {
            width: w,
            height: h,
            log_signal: signal,
        }];
        while levels
            .last()
            .unwrap()
            .width
            .min(levels.last().unwrap().height)
            >= 64
        {
            let prev = levels.last().unwrap();
            let (w, h) = (prev.width.div_ceil(2), prev.height.div_ceil(2));
            let mut signal = vec![f32::NAN; w * h];
            for y in 0..h {
                for x in 0..w {
                    let mut sum = 0.0;
                    let mut count = 0;
                    for sy in y * 2..(y * 2 + 2).min(prev.height) {
                        for sx in x * 2..(x * 2 + 2).min(prev.width) {
                            let v = prev.log_signal[sy * prev.width + sx];
                            if v.is_finite() {
                                sum += v;
                                count += 1;
                            }
                        }
                    }
                    if count > 0 {
                        signal[y * w + x] = sum / count as f32;
                    }
                }
            }
            levels.push(Level {
                width: w,
                height: h,
                log_signal: signal,
            });
        }
        Ok(Self {
            levels,
            width,
            height,
            scale: stride as f32,
        })
    }

    pub fn align(&self, rgb: &[f32], exposure: f32) -> Result<Alignment> {
        let source = Self::new(self.width, self.height, rgb, exposure)?;
        let mut alignment = Alignment::default();
        for level in (0..self.levels.len()).rev() {
            let reference = &self.levels[level];
            let source = &source.levels[level];
            if level == self.levels.len() - 1 {
                let mut best = reference.error(source, alignment);
                // Broad coarse search handles small handheld shifts and rotations.
                for angle in -4..=4 {
                    for y in -6..=6 {
                        for x in -6..=6 {
                            let candidate = Alignment {
                                x: x as f32,
                                y: y as f32,
                                rotation: (angle as f32 * 0.5).to_radians(),
                            };
                            let score = reference.error(source, candidate);
                            if score < best {
                                best = score;
                                alignment = candidate;
                            }
                        }
                    }
                }
            } else {
                alignment.x *= 2.0;
                alignment.y *= 2.0;
            }
            for step in [1.0, 0.5, 0.25, 0.125] {
                let angle_step = step / reference.width.max(reference.height) as f32;
                for _ in 0..20 {
                    let mut best = reference.error(source, alignment);
                    let mut next = alignment;
                    for axis in 0..3 {
                        for sign in [-1.0, 1.0] {
                            let mut candidate = alignment;
                            match axis {
                                0 => candidate.x += sign * step,
                                1 => candidate.y += sign * step,
                                _ => candidate.rotation += sign * angle_step,
                            }
                            let score = reference.error(source, candidate);
                            if score < best {
                                best = score;
                                next = candidate;
                            }
                        }
                    }
                    if next.x == alignment.x
                        && next.y == alignment.y
                        && next.rotation == alignment.rotation
                    {
                        break;
                    }
                    alignment = next;
                }
            }
        }
        let error = self.levels[0].error(&source.levels[0], alignment);
        ensure!(error < 0.08, "Could not reliably auto-align the HDR bracket; use overlapping exposures of the same static scene");
        alignment.x *= self.scale;
        alignment.y *= self.scale;
        ensure!(
            alignment.x.abs() < self.width as f32 * 0.15
                && alignment.y.abs() < self.height as f32 * 0.15
                && alignment.rotation.abs() < 5.0f32.to_radians(),
            "HDR bracket movement is too large to align"
        );
        Ok(alignment)
    }
}

impl Level {
    fn error(&self, source: &Self, alignment: Alignment) -> f32 {
        let transform = alignment.transform(self.width, self.height);
        let stride = ((self.width * self.height / 6000) as f32)
            .sqrt()
            .ceil()
            .max(1.0) as usize;
        let mut error = 0.0;
        let mut count = 0;
        let mut valid_reference = 0;
        for y in (0..self.height).step_by(stride) {
            for x in (0..self.width).step_by(stride) {
                let value = self.log_signal[y * self.width + x];
                if !value.is_finite() {
                    continue;
                }
                valid_reference += 1;
                let (sx, sy) = transform.map(x as f32, y as f32);
                let Some((indices, fractions)) =
                    sample_coordinates(source.width, source.height, sx, sy)
                else {
                    continue;
                };
                let mut sample = 0.0;
                let mut valid = true;
                for (index, fraction) in indices.into_iter().zip(fractions) {
                    if fraction <= 0.0 {
                        continue;
                    }
                    let v = source.log_signal[index];
                    if !v.is_finite() {
                        valid = false;
                        break;
                    }
                    sample += fraction * v;
                }
                if valid {
                    // Robust loss limits influence of noise and modest subject motion.
                    error += (sample - value).powi(2).min(0.25);
                    count += 1;
                }
            }
        }
        if count < 32 || count * 5 < valid_reference {
            f32::INFINITY
        } else {
            error / count as f32
        }
    }
}
