//! Frames warped onto the coarse seam grid from their thumbnails. Exposure balancing,
//! seam finding and cropping compare frames here, at a fraction of a megapixel.

use super::camera::Camera;
use super::preview::Reduced;
use super::projection::{Extent, Grid};
use rayon::prelude::*;

/// One frame on the seam grid, over the rectangle of grid pixels its footprint meets.
pub(super) struct Overview {
    /// `[x0, y0, x1, y1)` on the grid.
    pub(super) rect: [usize; 4],
    /// Linear camera RGB, exposure-normalized to the reference.
    pub(super) rgb: Vec<[f32; 3]>,
    /// The frame sees the pixel.
    pub(super) covered: Vec<bool>,
    /// The thumbnail measured it: inside the sensor, not clipped, above black.
    pub(super) measured: Vec<bool>,
    /// Some sample it mixes clipped.
    pub(super) clipped: Vec<bool>,
    /// Distance from the frame's nearest edge, relative to its long edge.
    pub(super) depth: Vec<f32>,
}

impl Overview {
    pub(super) fn width(&self) -> usize {
        self.rect[2] - self.rect[0]
    }

    /// The index of grid pixel `(x, y)` in this overview, when inside its rectangle.
    pub(super) fn index(&self, x: usize, y: usize) -> Option<usize> {
        let [x0, y0, x1, y1] = self.rect;
        (x >= x0 && x < x1 && y >= y0 && y < y1).then(|| (y - y0) * self.width() + x - x0)
    }

    /// Whether the frame covers grid pixel `(x, y)`.
    pub(super) fn covers(&self, x: usize, y: usize) -> bool {
        self.index(x, y).is_some_and(|i| self.covered[i])
    }
}

/// Warps `thumbnail` of the frame `camera` describes onto `grid` over `extent`.
pub(super) fn warp(grid: &Grid, camera: &Camera, thumbnail: &Reduced, extent: Extent) -> Overview {
    let rect = grid.pixels(extent).unwrap_or([0, 0, 0, 0]);
    let [x0, y0, x1, y1] = rect;
    let width = x1 - x0;
    let (frame_width, frame_height) = camera.pinhole.size();
    let long = frame_width.max(frame_height);
    let pixels: Vec<([f32; 3], bool, bool, bool, f32)> = (0..width * (y1 - y0))
        .into_par_iter()
        .map(|i| {
            let (x, y) = ((x0 + i % width) as f64, (y0 + i / width) as f64);
            let outside = ([0.0; 3], false, false, false, 0.0);
            let Some(p) = grid
                .ray(x, y)
                .and_then(|ray| camera.pixel(ray))
                .filter(|&p| camera.pinhole.contains(p))
            else {
                return outside;
            };
            let depth = (camera.pinhole.depth(p) / long) as f32;
            match thumbnail.sample(p[0], p[1]) {
                Some(sample) => (sample.rgb, true, sample.measured, sample.clipped, depth),
                None => ([0.0; 3], true, false, false, depth),
            }
        })
        .collect();
    let mut overview = Overview {
        rect,
        rgb: Vec::with_capacity(pixels.len()),
        covered: Vec::with_capacity(pixels.len()),
        measured: Vec::with_capacity(pixels.len()),
        clipped: Vec::with_capacity(pixels.len()),
        depth: Vec::with_capacity(pixels.len()),
    };
    for (rgb, covered, measured, clipped, depth) in pixels {
        overview.rgb.push(rgb);
        overview.covered.push(covered);
        overview.measured.push(measured);
        overview.clipped.push(clipped);
        overview.depth.push(depth);
    }
    overview
}
