//! Brush, object-prompt and subject-refinement stroke rasterization.

use super::*;

pub(super) fn object_prompt_dabs(strokes: &[ObjectStroke], size: f32) -> Vec<BrushDab> {
    let dab_count = strokes.iter().map(|stroke| stroke.points.len()).sum();
    let mut dabs = Vec::with_capacity(dab_count);
    for stroke in strokes {
        let opacity = if stroke.positive { 1.0 } else { -1.0 };
        let captured_size = if stroke.brush_size > 0.0 {
            stroke.brush_size
        } else {
            size
        };
        dabs.extend(stroke.points.iter().copied().map(|center| BrushDab {
            center,
            opacity,
            size: captured_size,
            feather: 0.0,
        }));
    }
    dabs
}

#[derive(Clone, Copy)]
struct BrushRasterSpec {
    opacity: f32,
    center_x: f32,
    center_y: f32,
    radius_x: f32,
    radius_y: f32,
    min_x: i32,
    max_x: i32,
    min_y: i32,
    max_y: i32,
    antialias: f32,
    inner: f32,
}

pub fn rasterize_brush_dabs(
    width: u32,
    height: u32,
    image_width: u32,
    image_height: u32,
    dabs: &[BrushDab],
) -> Vec<u8> {
    rasterize_brush(
        MaskRasterSpace::new(width, height, image_width, image_height),
        dabs,
    )
    .into_iter()
    .map(|value| (value.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
    .collect()
}

pub(super) fn rasterize_brush(space: MaskRasterSpace, dabs: &[BrushDab]) -> Vec<f32> {
    let [width, height] = space.raster;
    if width == 0 || height == 0 || dabs.is_empty() {
        return vec![0.0; width as usize * height as usize];
    }

    let specs = brush_raster_specs(space, dabs);

    let row_stride = width as usize;
    let mut out = vec![0.0f32; row_stride * height as usize];
    out.par_chunks_mut(row_stride * ROW_BAND_HEIGHT)
        .enumerate()
        .for_each(|(band_index, band)| {
            let rows = RowBand::new(band_index, band.len(), row_stride);
            for spec in &specs {
                for_each_dab_coverage(spec, rows, |index, coverage| {
                    if spec.opacity >= 0.0 {
                        band[index] = band[index].max(coverage * spec.opacity.clamp(0.0, 1.0));
                    } else {
                        band[index] *= 1.0 - coverage * (-spec.opacity).clamp(0.0, 1.0);
                    }
                });
            }
        });
    out
}

/// Brush rasters are processed in parallel bands of this many rows.
const ROW_BAND_HEIGHT: usize = 64;

/// The raster rows held by one parallel band slice (row-major, `row_stride` pixels per row).
#[derive(Clone, Copy)]
struct RowBand {
    start_y: usize,
    end_y: usize,
    row_stride: usize,
}

impl RowBand {
    fn new(band_index: usize, band_len: usize, row_stride: usize) -> Self {
        let start_y = band_index * ROW_BAND_HEIGHT;
        let band_height = band_len / row_stride;
        Self {
            start_y,
            end_y: start_y + band_height - 1,
            row_stride,
        }
    }
}

/// Calls `apply(index, coverage)` for every pixel of `rows` inside the dab footprint, in
/// row-major order. `index` addresses the band slice; `coverage` is the anti-aliased
/// `1 - smoothstep(inner, 1 + antialias, distance)`. Hot path: keep the float ops in this order.
#[inline]
fn for_each_dab_coverage(spec: &BrushRasterSpec, rows: RowBand, mut apply: impl FnMut(usize, f32)) {
    if spec.max_y < rows.start_y as i32 || spec.min_y > rows.end_y as i32 {
        return;
    }
    let min_y = spec.min_y.max(rows.start_y as i32);
    let max_y = spec.max_y.min(rows.end_y as i32);
    for y in min_y..=max_y {
        let dy = (y as f32 + 0.5 - spec.center_y) / spec.radius_y.max(0.5);
        let row_offset = (y as usize - rows.start_y) * rows.row_stride;
        for x in spec.min_x..=spec.max_x {
            let dx = (x as f32 + 0.5 - spec.center_x) / spec.radius_x.max(0.5);
            let distance = (dx * dx + dy * dy).sqrt();
            if distance >= 1.0 + spec.antialias {
                continue;
            }
            let coverage = 1.0 - smoothstep(spec.inner, 1.0 + spec.antialias, distance);
            apply(row_offset + x as usize, coverage);
        }
    }
}

/// Per-band scratch for one recorded stroke group: overlapping dabs within a group keep their
/// strongest alpha instead of compounding, and the group is applied once when flushed.
struct StrokeGroupCoverage {
    alpha: Vec<f32>,
    touched: Vec<usize>,
}

impl StrokeGroupCoverage {
    fn new(band_len: usize) -> Self {
        Self {
            alpha: vec![0.0f32; band_len],
            touched: Vec::new(),
        }
    }

    /// Raises every covered pixel to `coverage * |opacity|` (opacity clamped to 1).
    #[inline]
    fn accumulate(&mut self, specs: &[BrushRasterSpec], rows: RowBand) {
        for spec in specs {
            for_each_dab_coverage(spec, rows, |index, coverage| {
                let alpha = coverage * spec.opacity.abs().clamp(0.0, 1.0);
                if alpha > self.alpha[index] {
                    if self.alpha[index] == 0.0 {
                        self.touched.push(index);
                    }
                    self.alpha[index] = alpha;
                }
            });
        }
    }

    /// Hands each touched pixel's group alpha to `apply(index, alpha)` and clears the scratch
    /// for the next group.
    #[inline]
    fn flush(&mut self, mut apply: impl FnMut(usize, f32)) {
        for index in self.touched.drain(..) {
            apply(index, self.alpha[index]);
            self.alpha[index] = 0.0;
        }
    }
}

fn brush_raster_specs(space: MaskRasterSpace, dabs: &[BrushDab]) -> Vec<BrushRasterSpec> {
    let [width, height] = space.raster;
    let [image_width, image_height] = space.image;
    let image_min = image_width.min(image_height).max(1) as f32;
    let mut specs = Vec::with_capacity(dabs.len());
    for dab in dabs {
        let radius_image = dab.size.clamp(f32::EPSILON, 0.5) * image_min;
        let radius_x = radius_image * width as f32 / image_width.max(1) as f32;
        let radius_y = radius_image * height as f32 / image_height.max(1) as f32;
        let bbox_x = radius_x.ceil().max(1.0) as i32 + 1;
        let bbox_y = radius_y.ceil().max(1.0) as i32 + 1;
        let feather = dab.feather.clamp(0.0, 1.0);
        let center_x = dab.center[0] * width as f32;
        let center_y = dab.center[1] * height as f32;
        let min_x = (center_x.floor() as i32 - bbox_x).max(0);
        let max_x = (center_x.ceil() as i32 + bbox_x).min(width as i32 - 1);
        let min_y = (center_y.floor() as i32 - bbox_y).max(0);
        let max_y = (center_y.ceil() as i32 + bbox_y).min(height as i32 - 1);
        let antialias = (1.0 / radius_x.max(radius_y).max(1.0)).clamp(0.002, 0.25);
        let inner = (1.0 - feather).clamp(0.0, 1.0 - antialias);
        specs.push(BrushRasterSpec {
            opacity: dab.opacity,
            center_x,
            center_y,
            radius_x,
            radius_y,
            min_x,
            max_x,
            min_y,
            max_y,
            antialias,
            inner,
        });
    }
    specs
}

#[derive(Clone, Copy)]
struct BrushStrokeGroup {
    start: usize,
    end: usize,
    pub(super) positive: bool,
}

fn recorded_brush_groups(
    dabs: &[BrushDab],
    stroke_starts: &[usize],
) -> (usize, Vec<BrushStrokeGroup>) {
    let mut starts = stroke_starts
        .iter()
        .copied()
        .filter(|&start| start < dabs.len())
        .collect::<Vec<_>>();
    starts.sort_unstable();
    starts.dedup();
    let Some(&ungrouped_end) = starts.first() else {
        return (dabs.len(), Vec::new());
    };

    let mut groups = Vec::with_capacity(starts.len());
    for (stroke_index, &stroke_start) in starts.iter().enumerate() {
        let stroke_end = starts.get(stroke_index + 1).copied().unwrap_or(dabs.len());
        let mut group_start = stroke_start;
        let mut positive = dabs[group_start].opacity >= 0.0;
        for (offset, dab) in dabs[stroke_start + 1..stroke_end].iter().enumerate() {
            let dab_index = stroke_start + 1 + offset;
            let next_positive = dab.opacity >= 0.0;
            if next_positive != positive {
                groups.push(BrushStrokeGroup {
                    start: group_start - ungrouped_end,
                    end: dab_index - ungrouped_end,
                    positive,
                });
                group_start = dab_index;
                positive = next_positive;
            }
        }
        groups.push(BrushStrokeGroup {
            start: group_start - ungrouped_end,
            end: stroke_end - ungrouped_end,
            positive,
        });
    }
    (ungrouped_end, groups)
}

pub(super) fn rasterize_recorded_brush(
    space: MaskRasterSpace,
    dabs: &[BrushDab],
    overlap_enabled: bool,
    stroke_starts: &[usize],
) -> Vec<f32> {
    let [width, height] = space.raster;
    if width == 0 || height == 0 || dabs.is_empty() {
        return vec![0.0; width as usize * height as usize];
    }

    let (ungrouped_end, groups) = recorded_brush_groups(dabs, stroke_starts);
    if groups.is_empty() {
        return rasterize_brush(space, dabs);
    }

    let mut out = rasterize_brush(space, &dabs[..ungrouped_end]);
    let specs = brush_raster_specs(space, &dabs[ungrouped_end..]);

    let row_stride = width as usize;
    out.par_chunks_mut(row_stride * ROW_BAND_HEIGHT)
        .enumerate()
        .for_each(|(band_index, band)| {
            let rows = RowBand::new(band_index, band.len(), row_stride);
            let mut stroke = StrokeGroupCoverage::new(band.len());
            for group in &groups {
                stroke.accumulate(&specs[group.start..group.end], rows);
                stroke.flush(|index, alpha| {
                    if group.positive {
                        band[index] = if overlap_enabled {
                            band[index] + alpha * (1.0 - band[index])
                        } else {
                            band[index].max(alpha)
                        };
                    } else {
                        band[index] *= 1.0 - alpha;
                    }
                });
            }
        });
    out
}

pub(super) fn rasterize_subject_refinement_delta(
    space: MaskRasterSpace,
    refinement: &SubjectRefinement,
) -> Vec<f32> {
    let [width, height] = space.raster;
    if width == 0 || height == 0 || refinement.dabs.is_empty() {
        return vec![0.0; width as usize * height as usize];
    }

    let (ungrouped_end, groups) =
        recorded_brush_groups(&refinement.dabs, &refinement.stroke_starts);
    let specs = brush_raster_specs(space, &refinement.dabs);
    let row_stride = width as usize;
    let mut out = vec![0.0f32; row_stride * height as usize];

    out.par_chunks_mut(row_stride * ROW_BAND_HEIGHT)
        .enumerate()
        .for_each(|(band_index, band)| {
            let rows = RowBand::new(band_index, band.len(), row_stride);
            for spec in &specs[..ungrouped_end] {
                for_each_dab_coverage(spec, rows, |index, coverage| {
                    band[index] =
                        (band[index] + coverage * spec.opacity.clamp(-1.0, 1.0)).clamp(-1.0, 1.0);
                });
            }

            let grouped_specs = &specs[ungrouped_end..];
            let mut stroke = StrokeGroupCoverage::new(band.len());
            for group in &groups {
                stroke.accumulate(&grouped_specs[group.start..group.end], rows);
                let sign = if group.positive { 1.0 } else { -1.0 };
                stroke.flush(|index, alpha| {
                    band[index] = (band[index] + sign * alpha).clamp(-1.0, 1.0);
                });
            }
        });
    out
}
