//! Rasterizing mask components into coverage at a target resolution.

use super::*;

pub(super) fn uses_corrected_uv(geometry: &MaskGeometry) -> bool {
    matches!(
        geometry,
        MaskGeometry::Radial {
            initialized: true,
            ..
        } | MaskGeometry::Linear {
            initialized: true,
            ..
        }
    )
}

/// Share the lens inverse across every analytic shape in a layer. UVs may lie
/// outside the crop: corrected geometry must not be clipped to native crop bounds.
pub(super) fn corrected_region_uv(
    extent: [u32; 2],
    region: [u32; 4],
    full_size: [u32; 2],
    lens: &LensGeometryMap,
) -> Vec<[f32; 2]> {
    let full = full_size.map(|value| value.max(1));
    let origin = [
        region[0] as f32 / full[0] as f32,
        region[1] as f32 / full[1] as f32,
    ];
    let scale = [
        region[2].max(1) as f32 / full[0] as f32,
        region[3].max(1) as f32 / full[1] as f32,
    ];
    let last = full.map(|value| value.saturating_sub(1) as f32);
    (0..extent[0] as usize * extent[1] as usize)
        .into_par_iter()
        .map(|index| {
            let x = index % extent[0] as usize;
            let y = index / extent[0] as usize;
            let native_uv = [
                (region[0] as f32 + (x as f32 + 0.5) * region[2].max(1) as f32 / extent[0] as f32)
                    / full[0] as f32,
                (region[1] as f32 + (y as f32 + 0.5) * region[3].max(1) as f32 / extent[1] as f32)
                    / full[1] as f32,
            ];
            // Match the UI's UV-to-lens convention, which uses the last pixel
            // coordinate rather than full extent or a half-pixel offset.
            let corrected = lens.corrected_position_for_raster(
                native_uv[0] * last[0],
                native_uv[1] * last[1],
                full[0],
                full[1],
            );
            std::array::from_fn(|axis| {
                let uv = if last[axis] > 0.0 {
                    corrected[axis] / last[axis]
                } else {
                    native_uv[axis]
                };
                (uv - origin[axis]) / scale[axis].max(f32::EPSILON)
            })
        })
        .collect()
}

pub(super) fn crop_mask_image(source: &MaskImage, u0: f32, v0: f32, du: f32, dv: f32) -> MaskImage {
    let mut cropped = source.clone();
    cropped.sampling_rect = crop_sampling_rect(source.sampling_rect, u0, v0, du, dv);
    cropped
}

pub(super) fn crop_rgb_image(
    source: &MaskRgbImage,
    u0: f32,
    v0: f32,
    du: f32,
    dv: f32,
) -> MaskRgbImage {
    let mut cropped = source.clone();
    cropped.sampling_rect = crop_sampling_rect(source.sampling_rect, u0, v0, du, dv);
    cropped
}

fn crop_sampling_rect(source: [f32; 4], u0: f32, v0: f32, du: f32, dv: f32) -> [f32; 4] {
    let source_width = source[2] - source[0];
    let source_height = source[3] - source[1];
    [
        source[0] + u0 * source_width,
        source[1] + v0 * source_height,
        source[0] + (u0 + du) * source_width,
        source[1] + (v0 + dv) * source_height,
    ]
}

pub(super) fn rasterize_component(
    component: &MaskComponent,
    width: u32,
    height: u32,
    image_width: u32,
    image_height: u32,
    subject_refinement: &SubjectRefinement,
    corrected_uv: Option<&[[f32; 2]]>,
) -> Vec<f32> {
    // Square preview atlases and aspect-preserving detail rasters must measure
    // grow/feather in the same image space. Shape on an image-aspect grid, then
    // map coverage to the requested atlas. Keep that grid even at zero sliders
    // so starting a drag does not change the generated matte or path sampling.
    let needs_isotropic_grid = match &component.geometry {
        MaskGeometry::Ai { mask: Some(_), .. } | MaskGeometry::Object { mask: Some(_), .. } => true,
        MaskGeometry::Path { points, .. } => points.len() >= 3,
        MaskGeometry::LuminanceRange {
            source: Some(_), ..
        }
        | MaskGeometry::ColorRange {
            source: Some(_),
            sampled: true,
            ..
        } => true,
        _ => false,
    };
    if needs_isotropic_grid {
        let space = MaskRasterSpace::new(width, height, image_width, image_height);
        if let Some([internal_width, internal_height]) = space.isotropic_extent() {
            let coverage = rasterize_component_internal(
                component,
                internal_width,
                internal_height,
                image_width,
                image_height,
                subject_refinement,
                None,
            );
            return resample_coverage(&coverage, internal_width, internal_height, width, height);
        }
    }
    rasterize_component_internal(
        component,
        width,
        height,
        image_width,
        image_height,
        subject_refinement,
        corrected_uv,
    )
}

fn resample_coverage(
    source: &[f32],
    source_width: u32,
    source_height: u32,
    width: u32,
    height: u32,
) -> Vec<f32> {
    let sample_axis = |index: u32, target_extent: u32, source_extent: u32| {
        // Match raster sampling at pixel centers and extend the edge pixels.
        let position = ((index as f64 + 0.5) * source_extent as f64 / target_extent as f64 - 0.5)
            .clamp(0.0, (source_extent - 1) as f64);
        let lower = position.floor() as usize;
        let upper = (lower + 1).min(source_extent as usize - 1);
        (lower, upper, (position - lower as f64) as f32)
    };
    let columns: Vec<_> = (0..width)
        .map(|x| sample_axis(x, width, source_width))
        .collect();
    let mut output = vec![0.0; width as usize * height as usize];
    output
        .par_chunks_mut(width as usize)
        .enumerate()
        .for_each(|(y, row)| {
            let (y0, y1, wy) = sample_axis(y as u32, height, source_height);
            let row0 = &source[y0 * source_width as usize..][..source_width as usize];
            let row1 = &source[y1 * source_width as usize..][..source_width as usize];
            for (value, &(x0, x1, wx)) in row.iter_mut().zip(&columns) {
                let top = row0[x0] + (row0[x1] - row0[x0]) * wx;
                let bottom = row1[x0] + (row1[x1] - row1[x0]) * wx;
                *value = (top + (bottom - top) * wy).clamp(0.0, 1.0);
            }
        });
    output
}

fn rasterize_component_internal(
    component: &MaskComponent,
    width: u32,
    height: u32,
    image_width: u32,
    image_height: u32,
    subject_refinement: &SubjectRefinement,
    corrected_uv: Option<&[[f32; 2]]>,
) -> Vec<f32> {
    let mut space = MaskRasterSpace::new(width, height, image_width, image_height);
    space.corrected_uv = corrected_uv;
    match &component.geometry {
        MaskGeometry::Fullscreen => vec![1.0; width as usize * height as usize],
        MaskGeometry::Brush {
            overlap_enabled,
            stroke_starts,
            dabs,
            ..
        } => rasterize_recorded_brush(space, dabs, *overlap_enabled, stroke_starts),
        MaskGeometry::Radial {
            center,
            radius,
            rotation,
            feather,
            initialized: true,
        } => rasterize_radial(space, *center, *radius, *rotation, *feather),
        MaskGeometry::Linear {
            start,
            end,
            feather,
            initialized: true,
        } => rasterize_linear(space, *start, *end, *feather),
        MaskGeometry::Path {
            points,
            grow,
            feather,
        } if points.len() >= 3 => rasterize_path(space, points, *grow, *feather),
        MaskGeometry::Ai {
            mask: Some(mask),
            grow,
            feather,
        } => {
            let mut coverage = rasterize_mask_image(width, height, mask);
            if matches!(component.kind, MaskKind::Subject | MaskKind::Background)
                && !subject_refinement.is_empty()
            {
                let delta = rasterize_subject_refinement_delta(space, subject_refinement);
                coverage.par_iter_mut().zip(delta.into_par_iter()).for_each(
                    |(probability, delta)| {
                        *probability = (*probability + delta).clamp(0.0, 1.0);
                    },
                );
            }
            let grow = if component.kind == MaskKind::Background {
                -*grow
            } else {
                *grow
            };
            shape_probability_mask_from_source(&mut coverage, width, height, grow, *feather, mask);
            if component.kind == MaskKind::Background {
                coverage
                    .par_iter_mut()
                    .for_each(|value| *value = 1.0 - *value);
            }
            coverage
        }
        MaskGeometry::Object {
            mask: Some(mask),
            grow,
            feather,
            ..
        } => {
            let mut coverage = rasterize_mask_image(width, height, mask);
            shape_probability_mask_from_source(&mut coverage, width, height, *grow, *feather, mask);
            coverage
        }
        MaskGeometry::Object {
            mask: None,
            brush_size,
            strokes,
            ..
        } => {
            let dabs = object_prompt_dabs(strokes, *brush_size);
            rasterize_brush(space, &dabs)
        }
        MaskGeometry::LuminanceRange {
            source: Some(source),
            low,
            high,
            grow,
            feather,
        } => {
            let mut coverage =
                rasterize_luminance_range(width, height, source, *low, *high, *feather);
            if grow.abs() > 1e-5 {
                shape_probability_mask(&mut coverage, width, height, *grow, 0.0);
            }
            coverage
        }
        MaskGeometry::ColorRange {
            source: Some(source),
            sample,
            tolerance,
            grow,
            feather,
            sampled: true,
        } => {
            let mut coverage =
                rasterize_color_range(width, height, source, *sample, *tolerance, *feather);
            if grow.abs() > 1e-5 {
                shape_probability_mask(&mut coverage, width, height, *grow, 0.0);
            }
            coverage
        }
        MaskGeometry::DepthRange {
            depth: Some(depth),
            range,
        } => {
            let mut coverage = rasterize_mask_image(width, height, depth);
            coverage
                .par_iter_mut()
                .for_each(|value| *value = range.weight(*value));
            coverage
        }
        _ => vec![0.0; width as usize * height as usize],
    }
}

pub(super) fn rasterize_mask_image(width: u32, height: u32, mask: &MaskImage) -> Vec<f32> {
    if width == 0 || height == 0 || mask.width == 0 || mask.height == 0 {
        return vec![0.0; width as usize * height as usize];
    }
    let row_stride = width as usize;
    let mut out = vec![0.0; row_stride * height as usize];
    out.par_chunks_mut(row_stride)
        .enumerate()
        .for_each(|(y, row)| {
            let sample_v = mask.sampling_rect[1]
                + (y as f32 + 0.5) / height as f32
                    * (mask.sampling_rect[3] - mask.sampling_rect[1]);
            let source_y = (sample_v * mask.height as f32 - 0.5)
                .clamp(0.0, mask.height.saturating_sub(1) as f32);
            let y0 = source_y.floor() as usize;
            let y1 = (y0 + 1).min(mask.height as usize - 1);
            let fy = source_y - y0 as f32;
            for (x, value) in row.iter_mut().enumerate() {
                let sample_u = mask.sampling_rect[0]
                    + (x as f32 + 0.5) / width as f32
                        * (mask.sampling_rect[2] - mask.sampling_rect[0]);
                let source_x = (sample_u * mask.width as f32 - 0.5)
                    .clamp(0.0, mask.width.saturating_sub(1) as f32);
                let x0 = source_x.floor() as usize;
                let x1 = (x0 + 1).min(mask.width as usize - 1);
                let fx = source_x - x0 as f32;
                let top_left = mask.pixels[y0 * mask.width as usize + x0] as f32 / 255.0;
                let top_right = mask.pixels[y0 * mask.width as usize + x1] as f32 / 255.0;
                let bottom_left = mask.pixels[y1 * mask.width as usize + x0] as f32 / 255.0;
                let bottom_right = mask.pixels[y1 * mask.width as usize + x1] as f32 / 255.0;
                let top = top_left + (top_right - top_left) * fx;
                let bottom = bottom_left + (bottom_right - bottom_left) * fx;
                *value = top + (bottom - top) * fy;
            }
        });
    out
}

pub(super) fn component_shape_margin_pixels(component: &MaskComponent, image_edge: f32) -> f32 {
    let shape_margin = |grow: f32, feather: f32| {
        grow.abs() * image_edge * 0.05 + mask_feather_radius(image_edge, feather) + 2.0
    };
    match &component.geometry {
        MaskGeometry::Ai { grow, feather, .. } | MaskGeometry::Object { grow, feather, .. } => {
            shape_margin(*grow, *feather)
        }
        MaskGeometry::LuminanceRange { grow, .. } | MaskGeometry::ColorRange { grow, .. } => {
            shape_margin(*grow, 0.0)
        }
        MaskGeometry::Path { grow, feather, .. } => shape_margin(*grow, *feather),
        _ => 2.0,
    }
}

pub(super) fn rasterize_luminance_range(
    width: u32,
    height: u32,
    source: &MaskRgbImage,
    low: f32,
    high: f32,
    feather: f32,
) -> Vec<f32> {
    sample_rgb_mask(width, height, source, |rgb| {
        let linear = rgb.map(srgb_decode_signed);
        let luminance = 0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2];
        luminance_range_weight(luminance, low, high, feather)
    })
}

/// Selection weight (0–1) of linear `luminance` in a luminance-range mask:
/// full between `low` and `high`, with a smooth ramp of `feather × 0.35`
/// outside each bound. The UI draws its range curve with the same function.
pub fn luminance_range_weight(luminance: f32, low: f32, high: f32, feather: f32) -> f32 {
    let low = low.min(high).clamp(0.0, 1.0);
    let high = high.max(low).clamp(0.0, 1.0);
    let transition = feather.clamp(0.0, 1.0) * 0.35;
    let enter = smoothstep(low - transition, low, luminance);
    let leave = 1.0 - smoothstep(high, high + transition, luminance);
    enter * leave
}

pub(super) fn rasterize_color_range(
    width: u32,
    height: u32,
    source: &MaskRgbImage,
    sample: [f32; 3],
    tolerance: f32,
    feather: f32,
) -> Vec<f32> {
    let target = linear_srgb_to_oklab(sample.map(srgb_decode_signed));
    let tolerance = tolerance.clamp(0.005, 1.0) * 0.42;
    let softness = feather.clamp(0.0, 1.0) * tolerance.max(0.01);
    sample_rgb_mask(width, height, source, |rgb| {
        let color = linear_srgb_to_oklab(rgb.map(srgb_decode_signed));
        let distance = ((color[0] - target[0]).powi(2)
            + (color[1] - target[1]).powi(2)
            + (color[2] - target[2]).powi(2))
        .sqrt();
        1.0 - smoothstep(tolerance, tolerance + softness, distance)
    })
}

fn sample_rgb_mask(
    width: u32,
    height: u32,
    source: &MaskRgbImage,
    coverage: impl Fn([f32; 3]) -> f32 + Sync,
) -> Vec<f32> {
    if width == 0 || height == 0 || source.width == 0 || source.height == 0 {
        return vec![0.0; width as usize * height as usize];
    }
    let row_stride = width as usize;
    let mut out = vec![0.0; row_stride * height as usize];
    out.par_chunks_mut(row_stride)
        .enumerate()
        .for_each(|(y, row)| {
            let sample_v = source.sampling_rect[1]
                + (y as f32 + 0.5) / height.max(1) as f32
                    * (source.sampling_rect[3] - source.sampling_rect[1]);
            let source_y = (sample_v * source.height as f32 - 0.5)
                .clamp(0.0, source.height.saturating_sub(1) as f32);
            let y0 = source_y.floor() as usize;
            let y1 = (y0 + 1).min(source.height as usize - 1);
            let fy = source_y - y0 as f32;
            for (x, value) in row.iter_mut().enumerate() {
                let sample_u = source.sampling_rect[0]
                    + (x as f32 + 0.5) / width.max(1) as f32
                        * (source.sampling_rect[2] - source.sampling_rect[0]);
                let source_x = (sample_u * source.width as f32 - 0.5)
                    .clamp(0.0, source.width.saturating_sub(1) as f32);
                let x0 = source_x.floor() as usize;
                let x1 = (x0 + 1).min(source.width as usize - 1);
                let fx = source_x - x0 as f32;
                let top_left_index = (y0 * source.width as usize + x0) * 4;
                let top_right_index = (y0 * source.width as usize + x1) * 4;
                let bottom_left_index = (y1 * source.width as usize + x0) * 4;
                let bottom_right_index = (y1 * source.width as usize + x1) * 4;
                let rgb = std::array::from_fn(|channel| {
                    let top_left = source.rgba[top_left_index + channel] as f32 / 255.0;
                    let top_right = source.rgba[top_right_index + channel] as f32 / 255.0;
                    let bottom_left = source.rgba[bottom_left_index + channel] as f32 / 255.0;
                    let bottom_right = source.rgba[bottom_right_index + channel] as f32 / 255.0;
                    let top = top_left + (top_right - top_left) * fx;
                    let bottom = bottom_left + (bottom_right - bottom_left) * fx;
                    top + (bottom - top) * fy
                });
                *value = coverage(rgb).clamp(0.0, 1.0);
            }
        });
    out
}

#[derive(Clone, Copy)]
pub(super) struct MaskRasterSpace<'a> {
    pub(super) raster: [u32; 2],
    pub(super) image: [u32; 2],
    corrected_uv: Option<&'a [[f32; 2]]>,
}

impl MaskRasterSpace<'_> {
    pub(super) const fn new(width: u32, height: u32, image_width: u32, image_height: u32) -> Self {
        Self {
            raster: [width, height],
            image: [image_width, image_height],
            corrected_uv: None,
        }
    }

    pub(super) fn shape_uv(self, x: usize, y: usize) -> [f32; 2] {
        self.corrected_uv.map_or_else(
            || {
                [
                    (x as f32 + 0.5) / self.raster[0] as f32,
                    (y as f32 + 0.5) / self.raster[1] as f32,
                ]
            },
            |uv| uv[y * self.raster[0] as usize + x],
        )
    }

    fn isotropic_extent(self) -> Option<[u32; 2]> {
        let [width, height] = self.raster;
        let [image_width, image_height] = self.image;
        if width == 0 || height == 0 || image_width == 0 || image_height == 0 {
            return None;
        }
        // Fit the image aspect into the requested bounds, rounding once.
        let extent = if width as u64 * image_height as u64 <= height as u64 * image_width as u64 {
            let fitted_height =
                (width as u64 * image_height as u64 + image_width as u64 / 2) / image_width as u64;
            [width, fitted_height.clamp(1, height as u64) as u32]
        } else {
            let fitted_width = (height as u64 * image_width as u64 + image_height as u64 / 2)
                / image_height as u64;
            [fitted_width.clamp(1, width as u64) as u32, height]
        };
        (extent != self.raster).then_some(extent)
    }
}
