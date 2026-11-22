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
                    }
                    | MaskGeometry::Person {
                        mask,
                        grow,
                        feather,
                        ..
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

    /// The margin of a mask at the far end of the Grow and Feather sliders, so
    /// a region reserved with it survives any drag of them.
    pub fn shape_slider_margin_pixels(image_width: u32, image_height: u32) -> u32 {
        let edge = image_width.min(image_height).max(1) as f32;
        soft_shape_margin_pixels(edge, 1.0, 1.0).ceil() as u32
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
            let opacity = component.opacity.clamp(0.0, 1.0);

            let Some(existing) = combined.as_mut() else {
                combined = Some(if component.combine == MaskCombineMode::Add {
                    // Moving an empty mask toward the coverage scales it.
                    if opacity < 1.0 {
                        coverage.par_iter_mut().for_each(|value| *value *= opacity);
                    }
                    coverage
                } else {
                    raster_buffers::recycle(coverage);
                    vec![0.0; len]
                });
                continue;
            };
            let combine: fn(f32, f32) -> f32 = match component.combine {
                MaskCombineMode::Add => |dst, src| dst.max(src),
                MaskCombineMode::Subtract => |dst, src| dst * (1.0 - src),
                MaskCombineMode::Intersect => |dst, src| dst * src,
            };
            // Full opacity takes the combined value itself rather than a
            // blend, so masks without component opacity stay bit-identical.
            existing
                .par_iter_mut()
                .zip(coverage.par_iter())
                .for_each(|(dst, &src)| {
                    let combined = combine(*dst, src);
                    *dst = if opacity < 1.0 {
                        *dst + (combined - *dst) * opacity
                    } else {
                        combined
                    };
                });
            raster_buffers::recycle(coverage);
        }

        let Some(mut combined) = combined else {
            return vec![0.0; len];
        };
        let opacity = mask.opacity.clamp(0.0, 1.0);
        // In place: a second buffer of this size would cost its page faults.
        combined.par_iter_mut().for_each(|value| {
            let inverted = if mask.invert { 1.0 - *value } else { *value };
            *value = inverted.clamp(0.0, 1.0) * opacity;
        });
        combined
    }

    pub fn rasterize_layer(
        &self,
        layer: usize,
        atlas_width: u32,
        atlas_height: u32,
        image_width: u32,
        image_height: u32,
    ) -> Vec<u8> {
        let coverage = self.rasterize_layer_coverage(
            layer,
            atlas_width,
            atlas_height,
            image_width,
            image_height,
        );
        let bytes = coverage
            .par_iter()
            .map(|&value| (value * 255.0 + 0.5) as u8)
            .collect();
        raster_buffers::recycle(coverage);
        bytes
    }

    pub fn rasterize_layer_f16(
        &self,
        layer: usize,
        atlas_width: u32,
        atlas_height: u32,
        image_width: u32,
        image_height: u32,
    ) -> Vec<u16> {
        let coverage = self.rasterize_layer_coverage(
            layer,
            atlas_width,
            atlas_height,
            image_width,
            image_height,
        );
        let bits = coverage
            .par_iter()
            .map(|&value| f16::from_f32(value).to_bits())
            .collect();
        raster_buffers::recycle(coverage);
        bits
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
        let coverage = self.rasterize_layer_region_coverage(layer, extent, region, full_size, lens);
        let bytes = coverage
            .par_iter()
            .map(|&value| (value * 255.0 + 0.5) as u8)
            .collect();
        raster_buffers::recycle(coverage);
        bytes
    }

    /// Like `rasterize_layer_region`, returning IEEE binary16 coverage bits.
    /// For several layers of one region, crop once with [`Self::region`].
    pub fn rasterize_layer_region_f16(
        &self,
        layer: usize,
        extent: [u32; 2],
        region: [u32; 4],
        full_size: [u32; 2],
        lens: Option<&LensGeometryMap>,
    ) -> Vec<u16> {
        self.region(region, full_size)
            .rasterize_layer_f16(layer, extent, lens)
    }

    fn rasterize_layer_region_coverage(
        &self,
        layer: usize,
        extent: [u32; 2],
        region: [u32; 4],
        full_size: [u32; 2],
        lens: Option<&LensGeometryMap>,
    ) -> Vec<f32> {
        self.region(region, full_size)
            .rasterize_layer_coverage(layer, extent, lens)
    }

    /// The masks cropped to the native region `[x, y, width, height]` of an
    /// image of `full_size` pixels, to rasterize its layers from. Cropping
    /// copies every mask, so callers that rasterize several layers of one
    /// region keep the result.
    pub fn region(&self, region: [u32; 4], full_size: [u32; 2]) -> MaskRegion<'_> {
        let masks = if region == [0, 0, full_size[0], full_size[1]] {
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
        MaskRegion {
            masks,
            region,
            full_size,
            corrected_uv: std::sync::Mutex::new(None),
        }
    }

    /// Rasterize one component of a native region, including the component's
    /// inversion and opacity. Like `rasterize_component_layer`, this excludes
    /// mask opacity and composition.
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
        // The component's own coverage shows its opacity as strength.
        let opacity = component.opacity.clamp(0.0, 1.0);
        coverage
            .into_iter()
            .map(|value| (value.clamp(0.0, 1.0) * opacity * 255.0 + 0.5) as u8)
            .collect()
    }
}

/// A mask stack cropped to a region of the image ([`MaskStack::region`]).
pub struct MaskRegion<'a> {
    masks: std::borrow::Cow<'a, MaskStack>,
    /// The native region `[x, y, width, height]`.
    region: [u32; 4],
    full_size: [u32; 2],
    /// The lens-corrected UVs the last layer sampled, with the extent and lens
    /// they are for. The layers of one region share them even when the global
    /// cache (`raster_cache::corrected_uv`) cannot keep a table this large.
    corrected_uv: std::sync::Mutex<Option<KeptCorrectedUv>>,
}

/// Lens-corrected UVs of a [`MaskRegion`] at one extent, for one lens.
struct KeptCorrectedUv {
    extent: [u32; 2],
    lens: LensGeometryMap,
    uv: Arc<Vec<[f32; 2]>>,
}

impl MaskRegion<'_> {
    /// Coverage of `layer` over the region in `extent` pixels, as IEEE
    /// binary16 bits. Radial and linear shapes use corrected full-image UVs
    /// when a lens is supplied; composition, inversion and mask opacity match
    /// `MaskStack::rasterize_layer`.
    pub fn rasterize_layer_f16(
        &self,
        layer: usize,
        extent: [u32; 2],
        lens: Option<&LensGeometryMap>,
    ) -> Vec<u16> {
        let mut bits = Vec::new();
        self.rasterize_layer_f16_into(layer, extent, lens, &mut bits);
        bits
    }

    /// As [`Self::rasterize_layer_f16`], into `bits`, whose allocation a
    /// caller rasterizing many layers keeps from one to the next.
    pub fn rasterize_layer_f16_into(
        &self,
        layer: usize,
        extent: [u32; 2],
        lens: Option<&LensGeometryMap>,
        bits: &mut Vec<u16>,
    ) {
        let coverage = self.rasterize_layer_coverage(layer, extent, lens);
        bits.clear();
        bits.par_extend(
            coverage
                .par_iter()
                .map(|&value| f16::from_f32(value).to_bits()),
        );
        raster_buffers::recycle(coverage);
    }

    fn rasterize_layer_coverage(
        &self,
        layer: usize,
        extent: [u32; 2],
        lens: Option<&LensGeometryMap>,
    ) -> Vec<f32> {
        if extent.contains(&0) {
            return Vec::new();
        }
        let region = self.region;
        let needs_corrected_uv = self.masks.masks.get(layer).is_some_and(|mask| {
            mask.components
                .iter()
                .any(|component| component.enabled && uses_corrected_uv(&component.geometry))
        });
        let corrected_uv = lens
            .filter(|_| needs_corrected_uv)
            .map(|lens| self.corrected_uv(extent, lens));
        self.masks.rasterize_layer_coverage_with_sampling(
            layer,
            extent[0],
            extent[1],
            region[2].max(1),
            region[3].max(1),
            corrected_uv.as_deref().map(Vec::as_slice),
        )
    }

    /// The lens-corrected UVs of this region at `extent`, computed once for
    /// every layer that samples them.
    fn corrected_uv(&self, extent: [u32; 2], lens: &LensGeometryMap) -> Arc<Vec<[f32; 2]>> {
        let kept = |slot: &Option<KeptCorrectedUv>| {
            slot.as_ref()
                .filter(|kept| kept.extent == extent && kept.lens.shares_mapping(lens))
                .map(|kept| Arc::clone(&kept.uv))
        };
        let lock = || {
            self.corrected_uv
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        };
        if let Some(uv) = kept(&lock()) {
            return uv;
        }
        let (region, full_size) = (self.region, self.full_size);
        // Computed without the lock held, as the global cache does.
        let uv = raster_cache::corrected_uv(extent, region, full_size, lens, || {
            corrected_region_uv(extent, region, full_size, lens)
        });
        *lock() = Some(KeptCorrectedUv {
            extent,
            lens: lens.clone(),
            uv: Arc::clone(&uv),
        });
        uv
    }
}
