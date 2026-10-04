//! Rasterizing whole mask layers, regions and crops of a `MaskStack`.

use super::*;

impl MaskStack {
    pub fn cropped_for_region(
        &self,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        full_width: u32,
        full_height: u32,
    ) -> Self {
        // Scene depth stays in full-image coordinates for GPU full-image UV sampling.
        let mut cropped = self.clone();
        let full_width = full_width.max(1);
        let full_height = full_height.max(1);
        let width = width.max(1);
        let height = height.max(1);
        let u0 = x as f32 / full_width as f32;
        let v0 = y as f32 / full_height as f32;
        let du = width as f32 / full_width as f32;
        let dv = height as f32 / full_height as f32;
        let image_scale = full_width.min(full_height) as f32 / width.min(height) as f32;

        let remap_point = |point: &mut [f32; 2]| {
            point[0] = (point[0] - u0) / du.max(f32::EPSILON);
            point[1] = (point[1] - v0) / dv.max(f32::EPSILON);
        };

        for mask in &mut cropped.masks {
            for component in &mut mask.components {
                match &mut component.geometry {
                    MaskGeometry::Fullscreen => {}
                    MaskGeometry::Brush { size, dabs, .. } => {
                        *size *= image_scale;
                        for dab in dabs {
                            remap_point(&mut dab.center);
                            dab.size *= image_scale;
                        }
                    }
                    MaskGeometry::Radial { center, radius, .. } => {
                        remap_point(center);
                        radius[0] /= du.max(f32::EPSILON);
                        radius[1] /= dv.max(f32::EPSILON);
                    }
                    MaskGeometry::Linear { start, end, .. } => {
                        remap_point(start);
                        remap_point(end);
                    }
                    MaskGeometry::Path {
                        points,
                        grow,
                        feather,
                    } => {
                        for point in points {
                            remap_point(&mut point.position);
                            point.handle_in[0] /= du.max(f32::EPSILON);
                            point.handle_in[1] /= dv.max(f32::EPSILON);
                            point.handle_out[0] /= du.max(f32::EPSILON);
                            point.handle_out[1] /= dv.max(f32::EPSILON);
                        }
                        *grow *= image_scale;
                        *feather *= image_scale.powf(1.0 / 1.30);
                    }
                    MaskGeometry::Ai {
                        mask,
                        grow,
                        feather,
                    } => {
                        *mask = mask
                            .as_ref()
                            .map(|source| crop_mask_image(source, u0, v0, du, dv));
                        *grow *= image_scale;
                        *feather *= image_scale.powf(1.0 / 1.30);
                    }
                    MaskGeometry::Object {
                        mask,
                        grow,
                        feather,
                        brush_size,
                        strokes,
                        ..
                    } => {
                        *mask = mask
                            .as_ref()
                            .map(|source| crop_mask_image(source, u0, v0, du, dv));
                        *grow *= image_scale;
                        *feather *= image_scale.powf(1.0 / 1.30);
                        *brush_size *= image_scale;
                        for stroke in strokes {
                            if stroke.brush_size > 0.0 {
                                stroke.brush_size *= image_scale;
                            }
                            for point in &mut stroke.points {
                                remap_point(point);
                            }
                        }
                    }
                    MaskGeometry::LuminanceRange { source, grow, .. }
                    | MaskGeometry::ColorRange { source, grow, .. } => {
                        *source = source
                            .as_ref()
                            .map(|source| crop_rgb_image(source, u0, v0, du, dv));
                        *grow *= image_scale;
                    }
                    MaskGeometry::Placeholder => {}
                    MaskGeometry::DepthRange { depth, .. } => {
                        *depth = depth
                            .as_ref()
                            .map(|image| crop_mask_image(image, u0, v0, du, dv));
                    }
                }
            }
        }
        cropped.subject_refinement.size *= image_scale;
        for dab in &mut cropped.subject_refinement.dabs {
            remap_point(&mut dab.center);
            dab.size *= image_scale;
        }
        cropped
    }

    pub fn raster_margin_pixels_for_layer(
        &self,
        mask_index: usize,
        component_index: Option<usize>,
        image_width: u32,
        image_height: u32,
    ) -> u32 {
        let Some(mask) = self.masks.get(mask_index) else {
            return 2;
        };
        let edge = image_width.min(image_height).max(1) as f32;
        mask.components
            .iter()
            .enumerate()
            .filter(|(index, component)| {
                component.enabled && component_index.is_none_or(|selected| selected == *index)
            })
            .map(|(_, component)| component_shape_margin_pixels(component, edge))
            .fold(2.0_f32, f32::max)
            .ceil() as u32
    }

    pub fn raster_margin_pixels(&self, image_width: u32, image_height: u32) -> u32 {
        self.masks
            .iter()
            .enumerate()
            .map(|(index, _)| {
                self.raster_margin_pixels_for_layer(index, None, image_width, image_height)
            })
            .max()
            .unwrap_or(2)
    }

    pub(super) fn rasterize_layer_coverage(
        &self,
        layer: usize,
        atlas_width: u32,
        atlas_height: u32,
        image_width: u32,
        image_height: u32,
    ) -> Vec<f32> {
        self.rasterize_layer_coverage_with_sampling(
            layer,
            atlas_width,
            atlas_height,
            image_width,
            image_height,
            None,
        )
    }

    fn rasterize_layer_coverage_with_sampling(
        &self,
        layer: usize,
        atlas_width: u32,
        atlas_height: u32,
        image_width: u32,
        image_height: u32,
        corrected_uv: Option<&[[f32; 2]]>,
    ) -> Vec<f32> {
        let len = atlas_width as usize * atlas_height as usize;
        let Some(mask) = self.masks.get(layer) else {
            return vec![0.0; len];
        };
        if mask.components.is_empty() {
            return vec![0.0; len];
        }

        let mut combined: Option<Vec<f32>> = None;
        for component in &mask.components {
            if !component.enabled || !component.geometry.is_initialized() {
                continue;
            }
            // A mask begins empty. A leading subtraction or intersection cannot
            // affect it, so avoid rasterizing an otherwise discarded component.
            if combined.is_none() && component.combine != MaskCombineMode::Add {
                combined = Some(vec![0.0; len]);
                continue;
            }
            let mut coverage = rasterize_component(
                component,
                atlas_width,
                atlas_height,
                image_width,
                image_height,
                &self.subject_refinement,
                corrected_uv,
            );
            if component.invert {
                coverage
                    .par_iter_mut()
                    .for_each(|value| *value = 1.0 - *value);
            }

            let Some(existing) = combined.as_mut() else {
                combined = Some(if component.combine == MaskCombineMode::Add {
                    coverage
                } else {
                    vec![0.0; len]
                });
                continue;
            };
            match component.combine {
                MaskCombineMode::Add => {
                    existing
                        .par_iter_mut()
                        .zip(coverage.into_par_iter())
                        .for_each(|(dst, src)| *dst = dst.max(src));
                }
                MaskCombineMode::Subtract => {
                    existing
                        .par_iter_mut()
                        .zip(coverage.into_par_iter())
                        .for_each(|(dst, src)| *dst *= 1.0 - src);
                }
                MaskCombineMode::Intersect => {
                    existing
                        .par_iter_mut()
                        .zip(coverage.into_par_iter())
                        .for_each(|(dst, src)| *dst *= src);
                }
            }
        }

        let Some(combined) = combined else {
            return vec![0.0; len];
        };
        let opacity = mask.opacity.clamp(0.0, 1.0);
        combined
            .into_par_iter()
            .map(|value| {
                let value = if mask.invert { 1.0 - value } else { value };
                value.clamp(0.0, 1.0) * opacity
            })
            .collect()
    }

    pub fn rasterize_layer(
        &self,
        layer: usize,
        atlas_width: u32,
        atlas_height: u32,
        image_width: u32,
        image_height: u32,
    ) -> Vec<u8> {
        self.rasterize_layer_coverage(layer, atlas_width, atlas_height, image_width, image_height)
            .into_par_iter()
            .map(|value| (value * 255.0 + 0.5) as u8)
            .collect()
    }

    pub fn rasterize_layer_f16(
        &self,
        layer: usize,
        atlas_width: u32,
        atlas_height: u32,
        image_width: u32,
        image_height: u32,
    ) -> Vec<u16> {
        self.rasterize_layer_coverage(layer, atlas_width, atlas_height, image_width, image_height)
            .into_par_iter()
            .map(|value| f16::from_f32(value).to_bits())
            .collect()
    }

    /// Rasterize a native region `[x, y, width, height]` into `extent` pixels.
    /// Radial and linear shapes use corrected full-image UVs when a lens is supplied;
    /// all other geometries retain native coordinates. Composition, inversion and
    /// mask opacity match `rasterize_layer`.
    pub fn rasterize_layer_region(
        &self,
        layer: usize,
        extent: [u32; 2],
        region: [u32; 4],
        full_size: [u32; 2],
        lens: Option<&LensGeometryMap>,
    ) -> Vec<u8> {
        self.rasterize_layer_region_coverage(layer, extent, region, full_size, lens)
            .into_par_iter()
            .map(|value| (value * 255.0 + 0.5) as u8)
            .collect()
    }

    /// Like `rasterize_layer_region`, returning IEEE binary16 coverage bits.
    pub fn rasterize_layer_region_f16(
        &self,
        layer: usize,
        extent: [u32; 2],
        region: [u32; 4],
        full_size: [u32; 2],
        lens: Option<&LensGeometryMap>,
    ) -> Vec<u16> {
        self.rasterize_layer_region_coverage(layer, extent, region, full_size, lens)
            .into_par_iter()
            .map(|value| f16::from_f32(value).to_bits())
            .collect()
    }

    fn rasterize_layer_region_coverage(
        &self,
        layer: usize,
        extent: [u32; 2],
        region: [u32; 4],
        full_size: [u32; 2],
        lens: Option<&LensGeometryMap>,
    ) -> Vec<f32> {
        if extent.contains(&0) {
            return Vec::new();
        }
        let cropped = if region == [0, 0, full_size[0], full_size[1]] {
            std::borrow::Cow::Borrowed(self)
        } else {
            std::borrow::Cow::Owned(self.cropped_for_region(
                region[0],
                region[1],
                region[2],
                region[3],
                full_size[0],
                full_size[1],
            ))
        };
        let needs_corrected_uv = cropped.masks.get(layer).is_some_and(|mask| {
            mask.components
                .iter()
                .any(|component| component.enabled && uses_corrected_uv(&component.geometry))
        });
        let corrected_uv = lens.filter(|_| needs_corrected_uv).map(|lens| {
            raster_cache::corrected_uv(extent, region, full_size, lens, || {
                corrected_region_uv(extent, region, full_size, lens)
            })
        });
        cropped.rasterize_layer_coverage_with_sampling(
            layer,
            extent[0],
            extent[1],
            region[2].max(1),
            region[3].max(1),
            corrected_uv.as_deref().map(Vec::as_slice),
        )
    }

    /// Rasterize one component of a native region, including component inversion.
    /// Like `rasterize_component_layer`, this excludes mask opacity and composition.
    /// Only radial and linear shapes sample corrected full-image coordinates.
    pub fn rasterize_component_region(
        &self,
        mask_index: usize,
        component_index: usize,
        extent: [u32; 2],
        region: [u32; 4],
        full_size: [u32; 2],
        lens: Option<&LensGeometryMap>,
    ) -> Vec<u8> {
        if extent.contains(&0) {
            return Vec::new();
        }
        let cropped = if region == [0, 0, full_size[0], full_size[1]] {
            std::borrow::Cow::Borrowed(self)
        } else {
            std::borrow::Cow::Owned(self.cropped_for_region(
                region[0],
                region[1],
                region[2],
                region[3],
                full_size[0],
                full_size[1],
            ))
        };
        let needs_corrected_uv = cropped
            .masks
            .get(mask_index)
            .and_then(|mask| mask.components.get(component_index))
            .is_some_and(|component| uses_corrected_uv(&component.geometry));
        let corrected_uv = lens.filter(|_| needs_corrected_uv).map(|lens| {
            raster_cache::corrected_uv(extent, region, full_size, lens, || {
                corrected_region_uv(extent, region, full_size, lens)
            })
        });
        cropped.rasterize_component_layer_with_sampling(
            mask_index,
            component_index,
            extent,
            [region[2].max(1), region[3].max(1)],
            corrected_uv.as_deref().map(Vec::as_slice),
        )
    }

    pub fn rasterize_component_layer(
        &self,
        mask_index: usize,
        component_index: usize,
        width: u32,
        height: u32,
        image_width: u32,
        image_height: u32,
    ) -> Vec<u8> {
        self.rasterize_component_layer_with_sampling(
            mask_index,
            component_index,
            [width, height],
            [image_width, image_height],
            None,
        )
    }

    fn rasterize_component_layer_with_sampling(
        &self,
        mask_index: usize,
        component_index: usize,
        extent: [u32; 2],
        image_size: [u32; 2],
        corrected_uv: Option<&[[f32; 2]]>,
    ) -> Vec<u8> {
        let [width, height] = extent;
        let [image_width, image_height] = image_size;
        let Some(component) = self
            .masks
            .get(mask_index)
            .and_then(|mask| mask.components.get(component_index))
        else {
            return vec![0; width as usize * height as usize];
        };
        let mut coverage = rasterize_component(
            component,
            width,
            height,
            image_width,
            image_height,
            &self.subject_refinement,
            corrected_uv,
        );
        if component.invert {
            for value in &mut coverage {
                *value = 1.0 - *value;
            }
        }
        coverage
            .into_iter()
            .map(|value| (value.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
            .collect()
    }
}
