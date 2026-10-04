//! `LensGeometryMap`: sampled lens-correction displacement between source and corrected images.

use super::*;

#[derive(Clone, Debug)]
pub struct LensGeometryMap {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) grid_width: u32,
    pub(super) grid_height: u32,
    coordinates: Arc<[[f32; 2]]>,
}

impl LensGeometryMap {
    pub fn new(
        width: u32,
        height: u32,
        grid_width: u32,
        grid_height: u32,
        coordinates: Vec<[f32; 2]>,
    ) -> Option<Self> {
        let width = width.max(1);
        let height = height.max(1);
        let grid_width = grid_width.max(2);
        let grid_height = grid_height.max(2);
        if coordinates.len() != grid_width as usize * grid_height as usize
            || coordinates
                .iter()
                .any(|point| !point[0].is_finite() || !point[1].is_finite())
        {
            return None;
        }
        Some(Self {
            width,
            height,
            grid_width,
            grid_height,
            coordinates: coordinates.into(),
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub(crate) fn shares_mapping(&self, other: &Self) -> bool {
        self.width == other.width
            && self.height == other.height
            && self.grid_width == other.grid_width
            && self.grid_height == other.grid_height
            && Arc::ptr_eq(&self.coordinates, &other.coordinates)
    }

    pub fn source_position_for_raster(
        &self,
        corrected_x: f32,
        corrected_y: f32,
        raster_width: u32,
        raster_height: u32,
    ) -> [f32; 2] {
        let raster_width = raster_width.max(1);
        let raster_height = raster_height.max(1);
        let map_x = scale_pixel_coordinate(corrected_x, raster_width, self.width);
        let map_y = scale_pixel_coordinate(corrected_y, raster_height, self.height);
        let mapped = self.source_position(map_x, map_y);
        [
            scale_pixel_coordinate(mapped[0], self.width, raster_width),
            scale_pixel_coordinate(mapped[1], self.height, raster_height),
        ]
    }

    pub fn corrected_position_for_raster(
        &self,
        source_x: f32,
        source_y: f32,
        raster_width: u32,
        raster_height: u32,
    ) -> [f32; 2] {
        if !source_x.is_finite() || !source_y.is_finite() {
            return [source_x, source_y];
        }
        let max_x = raster_width.saturating_sub(1) as f32;
        let max_y = raster_height.saturating_sub(1) as f32;
        let target = [source_x.clamp(0.0, max_x), source_y.clamp(0.0, max_y)];
        let mut estimate = target;
        let mut best = estimate;
        let mut best_error = f32::INFINITY;

        for _ in 0..10 {
            let mapped = self.source_position_for_raster(
                estimate[0],
                estimate[1],
                raster_width,
                raster_height,
            );
            if !mapped[0].is_finite() || !mapped[1].is_finite() {
                break;
            }
            let error = [mapped[0] - target[0], mapped[1] - target[1]];
            let residual = error[0].abs().max(error[1].abs());
            if residual < best_error {
                best_error = residual;
                best = estimate;
            }
            if residual < 1e-3 {
                return estimate;
            }

            let step = 0.5_f32;
            let sample_x = if estimate[0] + step <= max_x {
                estimate[0] + step
            } else {
                (estimate[0] - step).max(0.0)
            };
            let sample_y = if estimate[1] + step <= max_y {
                estimate[1] + step
            } else {
                (estimate[1] - step).max(0.0)
            };
            let dx = sample_x - estimate[0];
            let dy = sample_y - estimate[1];
            if dx.abs() <= 1e-6 || dy.abs() <= 1e-6 {
                break;
            }
            let mapped_x =
                self.source_position_for_raster(sample_x, estimate[1], raster_width, raster_height);
            let mapped_y =
                self.source_position_for_raster(estimate[0], sample_y, raster_width, raster_height);
            if [mapped_x[0], mapped_x[1], mapped_y[0], mapped_y[1]]
                .iter()
                .any(|value| !value.is_finite())
            {
                break;
            }
            let j00 = (mapped_x[0] - mapped[0]) / dx;
            let j10 = (mapped_x[1] - mapped[1]) / dx;
            let j01 = (mapped_y[0] - mapped[0]) / dy;
            let j11 = (mapped_y[1] - mapped[1]) / dy;
            let determinant = j00 * j11 - j01 * j10;
            let correction = if determinant.is_finite() && determinant.abs() > 1e-5 {
                [
                    (j11 * error[0] - j01 * error[1]) / determinant,
                    (-j10 * error[0] + j00 * error[1]) / determinant,
                ]
            } else {
                error
            };
            if !correction[0].is_finite() || !correction[1].is_finite() {
                break;
            }
            let next = [
                (estimate[0] - correction[0].clamp(-64.0, 64.0)).clamp(0.0, max_x),
                (estimate[1] - correction[1].clamp(-64.0, 64.0)).clamp(0.0, max_y),
            ];
            if (next[0] - estimate[0])
                .abs()
                .max((next[1] - estimate[1]).abs())
                < 1e-4
            {
                break;
            }
            estimate = next;
        }
        best
    }

    fn source_position(&self, x: f32, y: f32) -> [f32; 2] {
        if !x.is_finite() || !y.is_finite() {
            return [x, y];
        }
        let gx = if self.width <= 1 {
            0.0
        } else {
            x / (self.width - 1) as f32 * (self.grid_width - 1) as f32
        };
        let gy = if self.height <= 1 {
            0.0
        } else {
            y / (self.height - 1) as f32 * (self.grid_height - 1) as f32
        };
        let gx = gx.clamp(0.0, (self.grid_width - 1) as f32);
        let gy = gy.clamp(0.0, (self.grid_height - 1) as f32);
        let x0 = gx.floor() as u32;
        let y0 = gy.floor() as u32;
        let x1 = (x0 + 1).min(self.grid_width - 1);
        let y1 = (y0 + 1).min(self.grid_height - 1);
        let tx = gx - x0 as f32;
        let ty = gy - y0 as f32;
        let load = |px: u32, py: u32| self.coordinates[(py * self.grid_width + px) as usize];
        let a = load(x0, y0);
        let b = load(x1, y0);
        let c = load(x0, y1);
        let d = load(x1, y1);
        [
            lerp(lerp(a[0], b[0], tx), lerp(c[0], d[0], tx), ty),
            lerp(lerp(a[1], b[1], tx), lerp(c[1], d[1], tx), ty),
        ]
    }
}

fn scale_pixel_coordinate(value: f32, from_extent: u32, to_extent: u32) -> f32 {
    if from_extent <= 1 || to_extent <= 1 {
        0.0
    } else {
        value * (to_extent - 1) as f32 / (from_extent - 1) as f32
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
