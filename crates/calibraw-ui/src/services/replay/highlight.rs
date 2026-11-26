//! The area of a mask as a detailed replay highlights it: the mask's
//! coverage, rasterized on the CPU in source pixels and resampled onto the
//! video canvas through the output-to-source map export resamples the photo
//! with (`GeometryInverseMap`), so it lines up with the rendered still
//! through crop, quarter turns, flips, rotation, perspective and the lens map.

use crate::pipeline::geometry::GeometryInverseMap;
use crate::pipeline::{GeometryTransform, LoadedRaw, MaskStack};

/// Samples per output edge when finding the source region the output shows.
const BOUNDARY_SAMPLES: u32 = 64;
/// Coverage resolution relative to the photo on the canvas. Above 1, so that
/// rotation and perspective do not leave the highlight blockier than the
/// photo.
const COVERAGE_OVERSAMPLING: f64 = 1.5;

/// Coverage of mask `index` of `masks` on a canvas of `canvas_width` ×
/// `canvas_height`, one byte per pixel, where the photo, developed with
/// `geometry`, fills `photo_rect` (`[x, y, width, height]`,
/// [`super::frames::fitted_rect`]). 255 marks full coverage at the mask's
/// full opacity, so a faint mask still shows clearly; the canvas outside the
/// photo is 0.
pub(super) fn mask_highlight(
    masks: &MaskStack,
    index: usize,
    raw: &LoadedRaw,
    geometry: GeometryTransform,
    photo_rect: [u32; 4],
    canvas_width: u32,
    canvas_height: u32,
) -> Vec<u8> {
    let mut canvas = vec![0u8; canvas_width as usize * canvas_height as usize];
    let [photo_x, photo_y, photo_width, photo_height] = photo_rect;
    let lens = raw.lens_geometry.as_deref();
    let map = GeometryInverseMap::new_with_lens(
        geometry,
        lens,
        raw.width,
        raw.height,
        photo_width,
        photo_height,
    );
    let source_size = [raw.width, raw.height];
    let Some(region) = shown_source_region(map, source_size) else {
        return canvas;
    };
    let extent = coverage_extent(region, photo_width, photo_height);
    let coverage = masks.rasterize_layer_region(index, extent, region, source_size, lens);
    // The raster includes the mask's opacity; divide it out so the
    // highlight shows where the mask applies, not how strongly.
    let opacity = masks
        .masks
        .get(index)
        .map_or(1.0, |mask| mask.opacity.clamp(1e-6, 1.0));
    let raster = CoverageRaster {
        values: &coverage,
        extent,
        region,
    };

    for y in 0..photo_height {
        let row_start = (y + photo_y) as usize * canvas_width as usize + photo_x as usize;
        let row = &mut canvas[row_start..row_start + photo_width as usize];
        for (x, value) in row.iter_mut().enumerate() {
            let [source_x, source_y] = map.source_position(x as f32, y as f32);
            if source_x < -0.5
                || source_y < -0.5
                || source_x > raw.width as f32 - 0.5
                || source_y > raw.height as f32 - 0.5
            {
                continue;
            }
            let covered = raster.sample(source_x, source_y) / opacity;
            *value = (covered * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
    canvas
}

/// The source pixels `[x, y, width, height]` the output shows, or `None` when
/// it shows none of the source. The map is continuous and one to one, so the
/// output's border bounds what its interior shows.
fn shown_source_region(map: GeometryInverseMap<'_>, source_size: [u32; 2]) -> Option<[u32; 4]> {
    let [output_width, output_height] = map.output_size().map(|size| size as f32);
    let mut min = [f32::INFINITY; 2];
    let mut max = [f32::NEG_INFINITY; 2];
    for step in 0..=BOUNDARY_SAMPLES {
        let t = step as f32 / BOUNDARY_SAMPLES as f32;
        // Output pixel positions run from -0.5 to size - 0.5 across the
        // pixels' outer edges.
        let x = t * output_width - 0.5;
        let y = t * output_height - 0.5;
        for [output_x, output_y] in [
            [x, -0.5],
            [x, output_height - 0.5],
            [-0.5, y],
            [output_width - 0.5, y],
        ] {
            let position = map.source_position(output_x, output_y);
            for axis in 0..2 {
                if position[axis].is_finite() {
                    min[axis] = min[axis].min(position[axis]);
                    max[axis] = max[axis].max(position[axis]);
                }
            }
        }
    }
    // Source pixel centres sit at whole numbers; one pixel of margin keeps
    // the bilinear footprint of the outermost samples inside the region.
    let start = |axis: usize| {
        (min[axis] - 1.0)
            .floor()
            .clamp(0.0, source_size[axis] as f32) as u32
    };
    let end = |axis: usize| {
        (max[axis] + 2.0)
            .ceil()
            .clamp(0.0, source_size[axis] as f32) as u32
    };
    let [x0, y0] = [start(0), start(1)];
    let [x1, y1] = [end(0), end(1)];
    (x1 > x0 && y1 > y0).then_some([x0, y0, x1 - x0, y1 - y0])
}

/// Raster size for `region`: about the photo's size on the canvas, never
/// more than one sample per source pixel.
fn coverage_extent(region: [u32; 4], photo_width: u32, photo_height: u32) -> [u32; 2] {
    let region_edge = f64::from(region[2].max(region[3]));
    let photo_edge = f64::from(photo_width.max(photo_height));
    let scale = (COVERAGE_OVERSAMPLING * photo_edge / region_edge).min(1.0);
    [region[2], region[3]].map(|size| ((f64::from(size) * scale).ceil() as u32).max(1))
}

/// Mask coverage over a source region, one value from 0 to 1 per raster pixel.
struct CoverageRaster<'a> {
    values: &'a [u8],
    extent: [u32; 2],
    /// The source pixels `[x, y, width, height]` the raster spans.
    region: [u32; 4],
}

impl CoverageRaster<'_> {
    /// Bilinear coverage at a source pixel position (centres at whole numbers).
    fn sample(&self, source_x: f32, source_y: f32) -> f32 {
        let [extent_width, extent_height] = self.extent;
        let [region_x, region_y, region_width, region_height] = self.region;
        // Raster pixel centres sit at half steps across the region.
        let x =
            (source_x + 0.5 - region_x as f32) * extent_width as f32 / region_width as f32 - 0.5;
        let y =
            (source_y + 0.5 - region_y as f32) * extent_height as f32 / region_height as f32 - 0.5;
        let max_x = extent_width.saturating_sub(1) as f32;
        let max_y = extent_height.saturating_sub(1) as f32;
        let x = x.clamp(0.0, max_x);
        let y = y.clamp(0.0, max_y);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = (
            (x0 + 1).min(extent_width as usize - 1),
            (y0 + 1).min(extent_height as usize - 1),
        );
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let at =
            |px: usize, py: usize| f32::from(self.values[py * extent_width as usize + px]) / 255.0;
        let top = at(x0, y0) + (at(x1, y0) - at(x0, y0)) * fx;
        let bottom = at(x0, y1) + (at(x1, y1) - at(x0, y1)) * fx;
        top + (bottom - top) * fy
    }
}
