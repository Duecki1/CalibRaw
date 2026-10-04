use super::raw_loader::RawThumbnail;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

mod crop_fit;
mod inverse_map;
mod lens_map;
mod thumbnail;
use crop_fit::*;
pub use inverse_map::*;
pub use lens_map::*;
pub use thumbnail::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CropAspectRatio {
    Free,
    #[default]
    Original,
    Square,
    FourThree,
    ThreeFour,
    ThreeTwo,
    TwoThree,
    SixteenNine,
    NineSixteen,
}

impl CropAspectRatio {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Free => "Free",
            Self::Original => "Original",
            Self::Square => "1 × 1",
            Self::FourThree => "4 × 3",
            Self::ThreeFour => "3 × 4",
            Self::ThreeTwo => "3 × 2",
            Self::TwoThree => "2 × 3",
            Self::SixteenNine => "16 × 9",
            Self::NineSixteen => "9 × 16",
        }
    }

    pub fn value(self, source_width: u32, source_height: u32) -> Option<f32> {
        match self {
            Self::Free => None,
            Self::Original => Some(source_width.max(1) as f32 / source_height.max(1) as f32),
            Self::Square => Some(1.0),
            Self::FourThree => Some(4.0 / 3.0),
            Self::ThreeFour => Some(3.0 / 4.0),
            Self::ThreeTwo => Some(3.0 / 2.0),
            Self::TwoThree => Some(2.0 / 3.0),
            Self::SixteenNine => Some(16.0 / 9.0),
            Self::NineSixteen => Some(9.0 / 16.0),
        }
    }

    /// The same ratio in the other orientation; `Free`, `Original` and
    /// `Square` are their own transpose.
    pub const fn transposed(self) -> Self {
        match self {
            Self::FourThree => Self::ThreeFour,
            Self::ThreeFour => Self::FourThree,
            Self::ThreeTwo => Self::TwoThree,
            Self::TwoThree => Self::ThreeTwo,
            Self::SixteenNine => Self::NineSixteen,
            Self::NineSixteen => Self::SixteenNine,
            other => other,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
pub struct GeometryTransform {
    #[serde(default = "default_crop_rect")]
    pub crop: [f32; 4],
    #[serde(default)]
    pub aspect_ratio: CropAspectRatio,
    #[serde(default)]
    pub quarter_turns: u8,
    #[serde(default)]
    pub rotation_degrees: f32,
    #[serde(default)]
    pub flip_horizontal: bool,
    #[serde(default)]
    pub flip_vertical: bool,
    #[serde(default)]
    pub horizontal_transform: f32,
    #[serde(default)]
    pub vertical_transform: f32,
}

const fn default_crop_rect() -> [f32; 4] {
    [0.0, 0.0, 1.0, 1.0]
}

impl Default for GeometryTransform {
    fn default() -> Self {
        Self {
            crop: default_crop_rect(),
            aspect_ratio: CropAspectRatio::Original,
            quarter_turns: 0,
            rotation_degrees: 0.0,
            flip_horizontal: false,
            flip_vertical: false,
            horizontal_transform: 0.0,
            vertical_transform: 0.0,
        }
    }
}

impl GeometryTransform {
    pub const MIN_CROP_EXTENT: f32 = 0.01;

    pub fn sanitized(mut self) -> Self {
        for value in &mut self.crop {
            if !value.is_finite() {
                *value = 0.0;
            }
        }
        self.crop[0] = self.crop[0].clamp(0.0, 1.0 - Self::MIN_CROP_EXTENT);
        self.crop[1] = self.crop[1].clamp(0.0, 1.0 - Self::MIN_CROP_EXTENT);
        self.crop[2] = self.crop[2].clamp(self.crop[0] + Self::MIN_CROP_EXTENT, 1.0);
        self.crop[3] = self.crop[3].clamp(self.crop[1] + Self::MIN_CROP_EXTENT, 1.0);
        self.quarter_turns %= 4;
        self.rotation_degrees = finite_clamp(self.rotation_degrees, -45.0, 45.0);
        self.horizontal_transform = finite_clamp(self.horizontal_transform, -30.0, 30.0);
        self.vertical_transform = finite_clamp(self.vertical_transform, -30.0, 30.0);
        self
    }

    pub fn is_identity(self) -> bool {
        let value = self.sanitized();
        value.crop == default_crop_rect()
            && value.quarter_turns == 0
            && value.rotation_degrees.abs() < 1e-4
            && !value.flip_horizontal
            && !value.flip_vertical
            && value.horizontal_transform.abs() < 1e-4
            && value.vertical_transform.abs() < 1e-4
    }

    pub fn crop_pixel_dimensions(self, source_width: u32, source_height: u32) -> (u32, u32) {
        let value = self.sanitized();
        let width = ((value.crop[2] - value.crop[0]) * source_width.max(1) as f32)
            .round()
            .max(1.0) as u32;
        let height = ((value.crop[3] - value.crop[1]) * source_height.max(1) as f32)
            .round()
            .max(1.0) as u32;
        if value.quarter_turns.is_multiple_of(2) {
            (width, height)
        } else {
            (height, width)
        }
    }

    /// Rotates the output by 90°. The crop rotates with the image, so a fixed
    /// aspect ratio swaps orientation to keep describing the same crop.
    pub fn rotate_quarter_turn(&mut self, clockwise: bool) {
        self.quarter_turns = if clockwise {
            (self.quarter_turns + 1) % 4
        } else {
            (self.quarter_turns + 3) % 4
        };
        self.aspect_ratio = self.aspect_ratio.transposed();
    }

    /// The crop's width / height in normalized source units for the chosen
    /// aspect ratio, which applies to the output as displayed (after quarter
    /// turns). `Original` is the uncropped output's own shape.
    pub fn normalized_crop_aspect(self, source_width: u32, source_height: u32) -> Option<f32> {
        let source_aspect = source_width.max(1) as f32 / source_height.max(1) as f32;
        let swapped = self.quarter_turns % 2 == 1;
        let (output_width, output_height) = if swapped {
            (source_height, source_width)
        } else {
            (source_width, source_height)
        };
        let output_ratio = self.aspect_ratio.value(output_width, output_height)?;
        let source_ratio = if swapped {
            1.0 / output_ratio
        } else {
            output_ratio
        };
        let normalized = source_ratio / source_aspect;
        (normalized.is_finite() && normalized > f32::EPSILON).then_some(normalized)
    }

    pub fn fit_crop_inside_transformed_source(
        &mut self,
        source_width: u32,
        source_height: u32,
    ) -> bool {
        let mut geometry = self.sanitized();
        let original_crop = geometry.crop;
        if crop_fits_transformed_source(geometry, original_crop, source_width, source_height) {
            *self = geometry;
            return false;
        }

        let source_width_f = source_width.max(1) as f32;
        let source_height_f = source_height.max(1) as f32;
        let width = (original_crop[2] - original_crop[0]) * source_width_f;
        let height = (original_crop[3] - original_crop[1]) * source_height_f;
        let original_center = [
            (original_crop[0] + original_crop[2]) * 0.5 * source_width_f,
            (original_crop[1] + original_crop[3]) * 0.5 * source_height_f,
        ];

        let mut low = 0.0_f32;
        let mut high = 1.0_f32;
        for _ in 0..32 {
            let mid = (low + high) * 0.5;
            if feasible_crop_center_bounds(
                geometry,
                width * mid,
                height * mid,
                source_width_f,
                source_height_f,
            )
            .is_some()
            {
                low = mid;
            } else {
                high = mid;
            }
        }

        let scale = (low * 0.999_999).clamp(0.0, 1.0);
        let fitted_width = (width * scale)
            .max(Self::MIN_CROP_EXTENT * source_width_f)
            .min(source_width_f);
        let fitted_height = (height * scale)
            .max(Self::MIN_CROP_EXTENT * source_height_f)
            .min(source_height_f);

        let Some(([min_cx, max_cx], [min_cy, max_cy])) = feasible_crop_center_bounds(
            geometry,
            fitted_width,
            fitted_height,
            source_width_f,
            source_height_f,
        ) else {
            return false;
        };
        let center_x = original_center[0].clamp(min_cx, max_cx);
        let center_y = original_center[1].clamp(min_cy, max_cy);
        geometry.crop = [
            (center_x - fitted_width * 0.5) / source_width_f,
            (center_y - fitted_height * 0.5) / source_height_f,
            (center_x + fitted_width * 0.5) / source_width_f,
            (center_y + fitted_height * 0.5) / source_height_f,
        ];
        geometry = geometry.sanitized();
        let changed = geometry.crop != original_crop;
        *self = geometry;
        changed
    }

    pub fn constrain_crop_drag_to_transformed_source(
        self,
        start_crop: [f32; 4],
        proposed_crop: [f32; 4],
        source_width: u32,
        source_height: u32,
    ) -> [f32; 4] {
        let geometry = self.sanitized();
        let proposed = sanitized_crop(proposed_crop);
        if crop_fits_transformed_source(geometry, proposed, source_width, source_height) {
            return proposed;
        }
        let start = sanitized_crop(start_crop);
        if !crop_fits_transformed_source(geometry, start, source_width, source_height) {
            let mut fitted = geometry;
            fitted.crop = start;
            fitted.fit_crop_inside_transformed_source(source_width, source_height);
            return fitted.crop;
        }

        let mut low = 0.0_f32;
        let mut high = 1.0_f32;
        for _ in 0..28 {
            let mid = (low + high) * 0.5;
            let candidate = lerp_crop(start, proposed, mid);
            if crop_fits_transformed_source(geometry, candidate, source_width, source_height) {
                low = mid;
            } else {
                high = mid;
            }
        }
        sanitized_crop(lerp_crop(start, proposed, low * 0.999_999))
    }

    /// Like [`Self::constrain_crop_drag_to_transformed_source`], but each edge
    /// stops independently, so a corner dragged into one source edge keeps
    /// following the pointer along that edge. Only for unconstrained aspect
    /// ratios: the edges are not kept in proportion.
    pub fn constrain_free_crop_resize_to_transformed_source(
        self,
        start_crop: [f32; 4],
        proposed_crop: [f32; 4],
        source_width: u32,
        source_height: u32,
    ) -> [f32; 4] {
        let geometry = self.sanitized();
        let proposed = sanitized_crop(proposed_crop);
        let mut crop = geometry.constrain_crop_drag_to_transformed_source(
            start_crop,
            proposed,
            source_width,
            source_height,
        );
        if !crop_fits_transformed_source(geometry, crop, source_width, source_height) {
            return crop;
        }
        // A second pass lets an edge use room freed by the other one.
        for _ in 0..2 {
            for edge in 0..4 {
                if crop[edge] == proposed[edge] {
                    continue;
                }
                let from = crop[edge];
                let mut low = 0.0_f32;
                let mut high = 1.0_f32;
                for _ in 0..24 {
                    let mid = (low + high) * 0.5;
                    let mut candidate = crop;
                    candidate[edge] = from + (proposed[edge] - from) * mid;
                    if crop_fits_transformed_source(
                        geometry,
                        candidate,
                        source_width,
                        source_height,
                    ) {
                        low = mid;
                    } else {
                        high = mid;
                    }
                }
                crop[edge] = from + (proposed[edge] - from) * low * 0.999_999;
                crop = sanitized_crop(crop);
            }
        }
        crop
    }

    /// Translates `start_crop` by `delta` (normalized source units) without
    /// resizing it. The crop slides along any source edge it reaches instead
    /// of stopping, because for a fixed size the valid centers form an
    /// axis-aligned rectangle.
    pub fn constrain_crop_move_to_transformed_source(
        self,
        start_crop: [f32; 4],
        delta: [f32; 2],
        source_width: u32,
        source_height: u32,
    ) -> [f32; 4] {
        let geometry = self.sanitized();
        let start = sanitized_crop(start_crop);
        let source_width_f = source_width.max(1) as f32;
        let source_height_f = source_height.max(1) as f32;
        let width = (start[2] - start[0]) * source_width_f;
        let height = (start[3] - start[1]) * source_height_f;
        let Some(([min_cx, max_cx], [min_cy, max_cy])) =
            feasible_crop_center_bounds(geometry, width, height, source_width_f, source_height_f)
        else {
            let mut fitted = geometry;
            fitted.crop = start;
            fitted.fit_crop_inside_transformed_source(source_width, source_height);
            return fitted.crop;
        };
        // The stored rectangle must also stay inside [0, 1], or sanitizing
        // would resize it.
        let min_cx = min_cx.max(width * 0.5);
        let max_cx = max_cx.min(source_width_f - width * 0.5);
        let min_cy = min_cy.max(height * 0.5);
        let max_cy = max_cy.min(source_height_f - height * 0.5);
        if min_cx > max_cx || min_cy > max_cy {
            return start;
        }
        let delta = delta.map(|value| if value.is_finite() { value } else { 0.0 });
        let center_x =
            (((start[0] + start[2]) * 0.5 + delta[0]) * source_width_f).clamp(min_cx, max_cx);
        let center_y =
            (((start[1] + start[3]) * 0.5 + delta[1]) * source_height_f).clamp(min_cy, max_cy);
        let half_width = (start[2] - start[0]) * 0.5;
        let half_height = (start[3] - start[1]) * 0.5;
        let center_u = center_x / source_width_f;
        let center_v = center_y / source_height_f;
        [
            center_u - half_width,
            center_v - half_height,
            center_u + half_width,
            center_v + half_height,
        ]
    }
}

fn finite_clamp(value: f32, min: f32, max: f32) -> f32 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lens_mapping_identity_requires_shared_coordinates_and_dimensions() {
        let coordinates = vec![[0.0, 0.0], [3.0, 0.0], [0.0, 2.0], [3.0, 2.0]];
        let lens = LensGeometryMap::new(4, 3, 2, 2, coordinates.clone()).unwrap();
        assert!(lens.shares_mapping(&lens));
        assert!(lens.shares_mapping(&lens.clone()));
        assert!(!lens.shares_mapping(&LensGeometryMap::new(4, 3, 2, 2, coordinates).unwrap()));
        for dimension in 0..4 {
            let mut changed = lens.clone();
            match dimension {
                0 => changed.width += 1,
                1 => changed.height += 1,
                2 => changed.grid_width += 1,
                _ => changed.grid_height += 1,
            }
            assert!(!lens.shares_mapping(&changed));
        }
    }

    #[test]
    fn geometry_defaults_to_identity() {
        let geometry = GeometryTransform::default();
        assert!(geometry.is_identity());
        assert_eq!(geometry.aspect_ratio, CropAspectRatio::Original);
    }

    #[test]
    fn inverse_map_identity_targets_native_pixel_centers() {
        let map = GeometryInverseMap::new(GeometryTransform::default(), 4, 3, 4, 3);
        for (actual, expected) in map.source_position(0.0, 0.0).into_iter().zip([0.0, 0.0]) {
            assert!((actual - expected).abs() < 1e-6);
        }
        for (actual, expected) in map.source_position(3.0, 2.0).into_iter().zip([3.0, 2.0]) {
            assert!((actual - expected).abs() < 1e-6);
        }
        let jacobian = map.pixel_jacobian();
        for (actual, expected) in jacobian.into_iter().flatten().zip([1.0, 0.0, 0.0, 1.0]) {
            assert!((actual - expected).abs() < 1e-6);
        }
    }

    #[test]
    fn inverse_map_combines_quarter_turn_with_output_geometry() {
        let geometry = GeometryTransform {
            quarter_turns: 1,
            ..Default::default()
        };
        let map = GeometryInverseMap::new(geometry, 3, 2, 2, 3);
        for (actual, expected) in map.source_position(0.0, 0.0).into_iter().zip([0.0, 1.0]) {
            assert!((actual - expected).abs() < 1e-6);
        }
        for (actual, expected) in map.source_position(1.0, 2.0).into_iter().zip([2.0, 0.0]) {
            assert!((actual - expected).abs() < 1e-6);
        }
    }

    #[test]
    fn crop_dimensions_follow_normalized_rectangle() {
        let geometry = GeometryTransform {
            crop: [0.25, 0.25, 0.75, 0.75],
            ..Default::default()
        };
        assert_eq!(geometry.crop_pixel_dimensions(4000, 3000), (2000, 1500));
    }

    #[test]
    fn constrained_rotation_shrinks_full_crop_inside_source() {
        let mut geometry = GeometryTransform {
            rotation_degrees: 20.0,
            ..Default::default()
        };
        assert!(!crop_fits_transformed_source(
            geometry,
            geometry.crop,
            4000,
            3000
        ));
        assert!(geometry.fit_crop_inside_transformed_source(4000, 3000));
        assert!(crop_fits_transformed_source(
            geometry,
            geometry.crop,
            4000,
            3000
        ));
        assert!(geometry.crop[0] > 0.0);
        assert!(geometry.crop[1] > 0.0);
        assert!(geometry.crop[2] < 1.0);
        assert!(geometry.crop[3] < 1.0);
    }

    #[test]
    fn quarter_turn_does_not_shrink_valid_crop() {
        let original = [0.1, 0.2, 0.9, 0.8];
        let mut geometry = GeometryTransform {
            crop: original,
            quarter_turns: 1,
            ..Default::default()
        };
        assert!(!geometry.fit_crop_inside_transformed_source(4000, 3000));
        assert_eq!(geometry.crop, original);
    }

    #[test]
    fn constrained_corner_drag_keeps_opposite_corner_fixed() {
        let geometry = GeometryTransform {
            crop: [0.2, 0.2, 0.8, 0.8],
            rotation_degrees: 25.0,
            ..Default::default()
        };
        let mut fitted = geometry;
        fitted.fit_crop_inside_transformed_source(4000, 3000);
        let start = fitted.crop;
        let proposed = [0.0, 0.0, start[2], start[3]];
        let constrained =
            fitted.constrain_crop_drag_to_transformed_source(start, proposed, 4000, 3000);
        assert!((constrained[2] - start[2]).abs() < 1e-6);
        assert!((constrained[3] - start[3]).abs() < 1e-6);
        assert!(crop_fits_transformed_source(
            fitted,
            constrained,
            4000,
            3000
        ));
    }

    #[test]
    fn quarter_turn_keeps_the_crop_and_transposes_its_aspect() {
        let mut geometry = GeometryTransform {
            aspect_ratio: CropAspectRatio::FourThree,
            ..Default::default()
        };
        let before = geometry.normalized_crop_aspect(4000, 3000).unwrap();
        geometry.rotate_quarter_turn(true);
        assert_eq!(geometry.aspect_ratio, CropAspectRatio::ThreeFour);
        let after = geometry.normalized_crop_aspect(4000, 3000).unwrap();
        assert!((before - after).abs() < 1e-6);
        // Original always means the full, uncropped frame.
        geometry.aspect_ratio = CropAspectRatio::Original;
        assert!((geometry.normalized_crop_aspect(4000, 3000).unwrap() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn crop_move_slides_along_the_edge_it_hits() {
        let geometry = GeometryTransform {
            crop: [0.0, 0.2, 0.5, 0.6],
            ..Default::default()
        };
        // Pushing left into the edge while moving down still moves down.
        let moved = geometry.constrain_crop_move_to_transformed_source(
            geometry.crop,
            [-0.2, 0.1],
            4000,
            3000,
        );
        assert!((moved[0] - 0.0).abs() < 1e-6);
        assert!((moved[1] - 0.3).abs() < 1e-6);
        assert!((moved[2] - moved[0] - 0.5).abs() < 1e-6);
        assert!((moved[3] - moved[1] - 0.4).abs() < 1e-6);
    }

    #[test]
    fn rotated_crop_move_slides_and_keeps_its_size() {
        let mut geometry = GeometryTransform {
            crop: [0.3, 0.3, 0.6, 0.6],
            rotation_degrees: 12.0,
            ..Default::default()
        };
        geometry.fit_crop_inside_transformed_source(4000, 3000);
        let start = geometry.crop;
        let mut crop = start;
        // Walk the crop into the left edge, then along it.
        for delta in [[-0.5, 0.0], [-0.5, 0.05], [-0.5, 0.1]] {
            crop = geometry.constrain_crop_move_to_transformed_source(start, delta, 4000, 3000);
            assert!(crop_fits_transformed_source(geometry, crop, 4000, 3000));
            assert!((crop[2] - crop[0] - (start[2] - start[0])).abs() < 1e-5);
            assert!((crop[3] - crop[1] - (start[3] - start[1])).abs() < 1e-5);
        }
        assert!(crop[0] < start[0]);
        assert!((crop[1] - (start[1] + 0.1)).abs() < 1e-5);
    }

    #[test]
    fn free_corner_resize_slides_along_the_edge_it_hits() {
        let geometry = GeometryTransform {
            crop: [0.2, 0.2, 0.8, 0.8],
            rotation_degrees: 10.0,
            ..Default::default()
        };
        let mut fitted = geometry;
        fitted.fit_crop_inside_transformed_source(4000, 3000);
        let start = fitted.crop;
        // The bottom-right corner moves far right (into the edge) and a
        // little down; the downward part should still apply.
        let proposed = [start[0], start[1], 1.5, start[3] + 0.02];
        let proportional =
            fitted.constrain_crop_drag_to_transformed_source(start, proposed, 4000, 3000);
        let free =
            fitted.constrain_free_crop_resize_to_transformed_source(start, proposed, 4000, 3000);
        assert!(crop_fits_transformed_source(fitted, free, 4000, 3000));
        assert!(free[2] >= proportional[2] - 1e-6);
        assert!(free[3] > proportional[3]);
        assert!((free[0] - start[0]).abs() < 1e-6);
        assert!((free[1] - start[1]).abs() < 1e-6);
    }

    #[test]
    fn thumbnail_geometry_identity_is_lossless() {
        let thumbnail = crate::pipeline::RawThumbnail {
            width: 2,
            height: 2,
            rgba: vec![
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
            ],
        };
        assert_eq!(
            transform_thumbnail_geometry(&thumbnail, GeometryTransform::default()).rgba,
            thumbnail.rgba
        );
    }

    #[test]
    fn thumbnail_geometry_applies_crop_and_quarter_turn_dimensions() {
        let thumbnail = crate::pipeline::RawThumbnail {
            width: 8,
            height: 6,
            rgba: vec![128; 8 * 6 * 4],
        };
        let geometry = GeometryTransform {
            crop: [0.25, 0.0, 0.75, 1.0],
            quarter_turns: 1,
            ..Default::default()
        };
        let transformed = transform_thumbnail_geometry(&thumbnail, geometry);
        assert_eq!((transformed.width, transformed.height), (6, 4));
        assert_eq!(transformed.rgba.len(), 6 * 4 * 4);
    }

    #[test]
    fn thumbnail_quarter_turn_uses_exact_pixel_centers() {
        let thumbnail = crate::pipeline::RawThumbnail {
            width: 3,
            height: 2,
            rgba: vec![
                1, 0, 0, 255, 2, 0, 0, 255, 3, 0, 0, 255, 4, 0, 0, 255, 5, 0, 0, 255, 6, 0, 0, 255,
            ],
        };
        let transformed = transform_thumbnail_geometry(
            &thumbnail,
            GeometryTransform {
                quarter_turns: 1,
                ..Default::default()
            },
        );
        let red = transformed
            .rgba
            .chunks_exact(4)
            .map(|pixel| pixel[0])
            .collect::<Vec<_>>();
        assert_eq!(red, vec![4, 1, 5, 2, 6, 3]);
    }
}
