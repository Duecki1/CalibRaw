//! `GeometryInverseMap`: output pixel to source position through the geometry and lens maps.

use super::*;

#[derive(Clone, Copy, Debug)]
pub struct GeometryInverseMap<'a> {
    /// Output pixel to lens-corrected source pixel.
    source_from_output: Homography,
    projective: bool,
    lens_geometry: Option<&'a LensGeometryMap>,
    source_width: u32,
    source_height: u32,
    output_width: u32,
    output_height: u32,
}

impl<'a> GeometryInverseMap<'a> {
    pub fn new_with_lens(
        geometry: GeometryTransform,
        lens_geometry: Option<&'a LensGeometryMap>,
        source_width: u32,
        source_height: u32,
        output_width: u32,
        output_height: u32,
    ) -> Self {
        let geometry = geometry.sanitized();
        Self {
            source_from_output: GeometryMapping::new(geometry, source_width, source_height)
                .source_pixel_from_output_pixel(output_width, output_height),
            projective: !geometry.perspective.is_identity(),
            lens_geometry,
            source_width: source_width.max(1),
            source_height: source_height.max(1),
            output_width: output_width.max(1),
            output_height: output_height.max(1),
        }
    }

    /// Whether one Jacobian holds for every output pixel: without lens correction and
    /// perspective the map is affine.
    pub fn is_affine(self) -> bool {
        self.lens_geometry.is_none() && !self.projective
    }

    /// The source pixel that output pixel `(output_x, output_y)` shows. Export resamples
    /// through the same map on the GPU (`geometry_resample.wgsl` in calibraw-gpu), so a
    /// change here is made there too.
    pub fn source_position(self, output_x: f32, output_y: f32) -> [f32; 2] {
        let corrected = self.source_from_output.apply([output_x, output_y]);
        if corrected[0] < -0.5
            || corrected[1] < -0.5
            || corrected[0] > self.source_width as f32 - 0.5
            || corrected[1] > self.source_height as f32 - 0.5
        {
            return corrected;
        }
        self.lens_geometry.map_or(corrected, |lens| {
            lens.source_position_for_raster(
                corrected[0],
                corrected[1],
                self.source_width,
                self.source_height,
            )
        })
    }

    /// The map from output pixel to lens-corrected source pixel, before the lens map.
    pub fn source_from_output(self) -> Homography {
        self.source_from_output
    }

    /// The lens map applied after [`Self::source_from_output`], if any.
    pub fn lens_geometry(self) -> Option<&'a LensGeometryMap> {
        self.lens_geometry
    }

    /// Width and height of the source image, in pixels.
    pub fn source_size(self) -> [u32; 2] {
        [self.source_width, self.source_height]
    }

    /// Width and height of the output raster, in pixels.
    pub fn output_size(self) -> [u32; 2] {
        [self.output_width, self.output_height]
    }
}

pub(super) fn sanitized_crop(crop: [f32; 4]) -> [f32; 4] {
    let geometry = GeometryTransform {
        crop,
        ..Default::default()
    };
    geometry.sanitized().crop
}

pub(super) fn lerp_crop(start: [f32; 4], end: [f32; 4], t: f32) -> [f32; 4] {
    [
        start[0] + (end[0] - start[0]) * t,
        start[1] + (end[1] - start[1]) * t,
        start[2] + (end[2] - start[2]) * t,
        start[3] + (end[3] - start[3]) * t,
    ]
}
