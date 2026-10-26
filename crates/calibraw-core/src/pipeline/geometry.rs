use super::raw_loader::RawThumbnail;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

mod crop_fit;
mod inverse_map;
mod lens_map;
mod straighten;
mod thumbnail;
use crop_fit::*;
pub use inverse_map::*;
pub use lens_map::*;
pub use straighten::{estimate_straighten_rotation, LineAnalysisImage, StraightenEstimate};
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
    /// A ratio the user entered, `width` × `height` of the output, each in
    /// `1..=MAX_TERM`. Serialized as `{"custom": {"width": 5, "height": 4}}`;
    /// sidecars with it are written as schema 2, which v1.2 reports as newer.
    Custom {
        width: u16,
        height: u16,
    },
}

impl CropAspectRatio {
    /// The largest term of a custom ratio.
    pub const MAX_TERM: u16 = 999;

    /// Width and height of a fixed ratio; `None` for `Free` and `Original`.
    pub const fn terms(self) -> Option<(u16, u16)> {
        match self {
            Self::Free | Self::Original => None,
            Self::Square => Some((1, 1)),
            Self::FourThree => Some((4, 3)),
            Self::ThreeFour => Some((3, 4)),
            Self::ThreeTwo => Some((3, 2)),
            Self::TwoThree => Some((2, 3)),
            Self::SixteenNine => Some((16, 9)),
            Self::NineSixteen => Some((9, 16)),
            Self::Custom { width, height } => Some((clamp_term(width), clamp_term(height))),
        }
    }

    pub fn value(self, source_width: u32, source_height: u32) -> Option<f32> {
        match self {
            Self::Free => None,
            Self::Original => Some(source_width.max(1) as f32 / source_height.max(1) as f32),
            fixed => fixed
                .terms()
                .map(|(width, height)| f32::from(width) / f32::from(height)),
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
            Self::Custom { width, height } => Self::Custom {
                width: height,
                height: width,
            },
            other => other,
        }
    }

    /// Custom terms clamped to `1..=MAX_TERM`, as decoded files may hold any.
    pub const fn sanitized(self) -> Self {
        match self {
            Self::Custom { width, height } => Self::Custom {
                width: clamp_term(width),
                height: clamp_term(height),
            },
            other => other,
        }
    }
}

const fn clamp_term(term: u16) -> u16 {
    if term == 0 {
        1
    } else if term > CropAspectRatio::MAX_TERM {
        CropAspectRatio::MAX_TERM
    } else {
        term
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
        self.aspect_ratio = self.aspect_ratio.sanitized();
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

    /// Whether `crop` (normalized source units) lies inside the rotated,
    /// sheared and flipped source.
    pub fn contains_crop(self, crop: [f32; 4], source_width: u32, source_height: u32) -> bool {
        crop_fits_transformed_source(self.sanitized(), crop, source_width, source_height)
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
