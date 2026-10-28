use super::raw_loader::RawThumbnail;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

mod crop_fit;
mod inverse_map;
mod lens_map;
mod lines;
mod mapping;
mod perspective;
mod straighten;
mod thumbnail;
use crop_fit::*;
pub use inverse_map::*;
pub use lens_map::*;
pub use lines::LineAnalysisImage;
pub use mapping::{GeometryMapping, Homography};
pub use perspective::{
    estimate_perspective, solve_guided_perspective, PerspectiveCorrection, PerspectiveEstimate,
    PerspectiveGuide,
};
pub use straighten::{estimate_straighten_rotation, StraightenEstimate};
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
    /// Horizontal skew, degrees: source rows shift sideways by the tangent of this angle per
    /// pixel down.
    #[serde(default, rename = "horizontal_transform")]
    pub horizontal_skew_degrees: f32,
    /// Vertical skew, degrees: source columns shift down by the tangent of this angle per pixel
    /// to the right.
    #[serde(default, rename = "vertical_transform")]
    pub vertical_skew_degrees: f32,
    /// Applied to the source before everything else; sidecars with one are written as schema 3
    /// (`migrations::older_readers`).
    #[serde(default, skip_serializing_if = "PerspectiveCorrection::is_identity")]
    pub perspective: PerspectiveCorrection,
    /// Stretch of the output along its axes before quarter turns, from -100 to 100: its width
    /// over its height is multiplied by `2^(value / 100)` at the same pixel count. Sidecars with
    /// one are written as schema 3.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub aspect_stretch: f32,
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
            horizontal_skew_degrees: 0.0,
            vertical_skew_degrees: 0.0,
            perspective: PerspectiveCorrection::default(),
            aspect_stretch: 0.0,
        }
    }
}

impl GeometryTransform {
    pub const MIN_CROP_EXTENT: f32 = 0.01;
    /// Largest [`Self::aspect_stretch`] either way, which doubles or halves the output's width
    /// over its height.
    pub const MAX_ASPECT_STRETCH: f32 = 100.0;

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
        self.horizontal_skew_degrees = finite_clamp(self.horizontal_skew_degrees, -30.0, 30.0);
        self.vertical_skew_degrees = finite_clamp(self.vertical_skew_degrees, -30.0, 30.0);
        self.perspective = self.perspective.sanitized();
        self.aspect_stretch = finite_clamp(
            self.aspect_stretch,
            -Self::MAX_ASPECT_STRETCH,
            Self::MAX_ASPECT_STRETCH,
        );
        self
    }

    pub fn is_identity(self) -> bool {
        let value = self.sanitized();
        value.crop == default_crop_rect()
            && value.quarter_turns == 0
            && value.rotation_degrees.abs() < 1e-4
            && !value.flip_horizontal
            && !value.flip_vertical
            && value.horizontal_skew_degrees.abs() < 1e-4
            && value.vertical_skew_degrees.abs() < 1e-4
            && value.perspective.is_identity()
            && value.aspect_stretch.abs() < 1e-4
    }

    /// Output size in pixels: the crop's share of the corrected frame, stretched for the aspect
    /// (see [`GeometryMapping`]).
    pub fn crop_pixel_dimensions(self, source_width: u32, source_height: u32) -> (u32, u32) {
        let value = self.sanitized();
        let [frame_width, frame_height] =
            GeometryMapping::new(value, source_width, source_height).frame_size();
        let [stretch_x, stretch_y] = value.aspect_stretch_factors().map(|factor| factor as f32);
        let width = ((value.crop[2] - value.crop[0]) * frame_width * stretch_x)
            .round()
            .max(1.0) as u32;
        let height = ((value.crop[3] - value.crop[1]) * frame_height * stretch_y)
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

    /// The crop's width / height in units normalized to the corrected frame for the chosen
    /// aspect ratio, which applies to the output as displayed (after the aspect stretch and
    /// quarter turns). `Original` is the source's own shape.
    pub fn normalized_crop_aspect(self, source_width: u32, source_height: u32) -> Option<f32> {
        let [frame_width, frame_height] =
            GeometryMapping::new(self, source_width, source_height).frame_size();
        let [stretch_x, stretch_y] = self
            .sanitized()
            .aspect_stretch_factors()
            .map(|factor| factor as f32);
        let frame_aspect = frame_width * stretch_x / (frame_height * stretch_y);
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
        let normalized = source_ratio / frame_aspect;
        (normalized.is_finite() && normalized > f32::EPSILON).then_some(normalized)
    }

    /// Reshapes the crop about its centre to the chosen aspect ratio by shortening one side,
    /// keeping it within the frame; `Free` leaves it as it is.
    pub fn conform_crop_to_aspect(&mut self, source_width: u32, source_height: u32) {
        let Some(target_normalized_ratio) =
            self.normalized_crop_aspect(source_width, source_height)
        else {
            return;
        };
        let crop = self.crop;
        let center_x = (crop[0] + crop[2]) * 0.5;
        let center_y = (crop[1] + crop[3]) * 0.5;
        let mut width = crop[2] - crop[0];
        let mut height = crop[3] - crop[1];
        if width / height.max(f32::EPSILON) > target_normalized_ratio {
            width = height * target_normalized_ratio;
        } else {
            height = width / target_normalized_ratio;
        }
        width = width.clamp(Self::MIN_CROP_EXTENT, 1.0);
        height = height.clamp(Self::MIN_CROP_EXTENT, 1.0);
        let left = (center_x - width * 0.5).clamp(0.0, 1.0 - width);
        let top = (center_y - height * 0.5).clamp(0.0, 1.0 - height);
        self.crop = [left, top, left + width, top + height];
    }

    /// Shrinks the crop about its centre until it fits inside the transformed source, moving
    /// it no further than needed. Returns whether it changed.
    pub fn fit_crop_inside_transformed_source(
        &mut self,
        source_width: u32,
        source_height: u32,
    ) -> bool {
        let mut geometry = self.sanitized();
        let original_crop = geometry.crop;
        let mapping = GeometryMapping::new(geometry, source_width, source_height);
        if crop_fits(&mapping) {
            *self = geometry;
            return false;
        }

        let size = mapping.crop_size();
        let frame = mapping.frame_size_f64();
        let mut low = 0.0_f64;
        let mut high = 1.0_f64;
        for _ in 0..32 {
            let mid = (low + high) * 0.5;
            if feasible_crop_centers(&mapping, [size[0] * mid, size[1] * mid]).is_some() {
                low = mid;
            } else {
                high = mid;
            }
        }

        let scale = (low * 0.999_999).clamp(0.0, 1.0);
        let min_extent = f64::from(Self::MIN_CROP_EXTENT);
        let fitted = [0, 1].map(|axis| {
            (size[axis] * scale)
                .max(min_extent * frame[axis])
                .min(frame[axis])
        });
        let Some(centers) = feasible_crop_centers(&mapping, fitted) else {
            return false;
        };
        geometry.crop = crop_around(centers.nearest(mapping.crop_center()), fitted, frame);
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
        let mapping = GeometryMapping::new(geometry, source_width, source_height);
        let proposed = sanitized_crop(proposed_crop);
        if crop_fits(&mapping.with_crop(proposed)) {
            return proposed;
        }
        let start = sanitized_crop(start_crop);
        if !crop_fits(&mapping.with_crop(start)) {
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
            if crop_fits(&mapping.with_crop(candidate)) {
                low = mid;
            } else {
                high = mid;
            }
        }
        sanitized_crop(lerp_crop(start, proposed, low * 0.999_999))
    }

    /// Whether `crop` (normalized to the corrected frame) lies inside the transformed source.
    pub fn contains_crop(self, crop: [f32; 4], source_width: u32, source_height: u32) -> bool {
        crop_fits(
            &GeometryMapping::new(self, source_width, source_height)
                .with_crop(sanitized_crop(crop)),
        )
    }

    /// Translates `start_crop` by `delta` (normalized to the corrected frame) without resizing
    /// it. The crop slides along any source edge it reaches instead of stopping, because for a
    /// fixed size the valid centres form a convex polygon.
    pub fn constrain_crop_move_to_transformed_source(
        self,
        start_crop: [f32; 4],
        delta: [f32; 2],
        source_width: u32,
        source_height: u32,
    ) -> [f32; 4] {
        let geometry = self.sanitized();
        let start = sanitized_crop(start_crop);
        let mapping = GeometryMapping::new(geometry, source_width, source_height).with_crop(start);
        let size = mapping.crop_size();
        let frame = mapping.frame_size_f64();
        let Some(centers) = feasible_crop_centers(&mapping, size) else {
            let mut fitted = geometry;
            fitted.crop = start;
            fitted.fit_crop_inside_transformed_source(source_width, source_height);
            return fitted.crop;
        };
        // The stored rectangle must also stay inside the frame, or sanitizing would resize it.
        let Some(centers) = centers.within(
            [size[0] * 0.5, size[1] * 0.5],
            [frame[0] - size[0] * 0.5, frame[1] - size[1] * 0.5],
        ) else {
            return start;
        };
        let delta = delta.map(|value| {
            if value.is_finite() {
                f64::from(value)
            } else {
                0.0
            }
        });
        let start_center = mapping.crop_center();
        let center = centers.nearest([
            start_center[0] + delta[0] * frame[0],
            start_center[1] + delta[1] * frame[1],
        ]);
        let half_width = (start[2] - start[0]) * 0.5;
        let half_height = (start[3] - start[1]) * 0.5;
        let center_u = (center[0] / frame[0]) as f32;
        let center_v = (center[1] / frame[1]) as f32;
        [
            center_u - half_width,
            center_v - half_height,
            center_u + half_width,
            center_v + half_height,
        ]
    }

    /// The perspective correction as the photo is displayed, after flips and quarter turns:
    /// `[vertical, horizontal]` in degrees, where a positive vertical widens the top of the
    /// displayed photo and a positive horizontal widens its right side.
    pub fn displayed_perspective(self) -> [f32; 2] {
        let perspective = self.perspective;
        DISPLAYED_SIDES.map(|side| {
            let [pitch, yaw] = self.source_side(side);
            pitch * perspective.pitch_degrees + yaw * perspective.yaw_degrees
        })
    }

    /// Sets the perspective correction from displayed angles (see
    /// [`Self::displayed_perspective`]), keeping its focal length.
    pub fn set_displayed_perspective(&mut self, displayed: [f32; 2]) {
        for (side, degrees) in DISPLAYED_SIDES.into_iter().zip(displayed) {
            let [pitch, yaw] = self.source_side(side);
            if pitch != 0.0 {
                self.perspective.pitch_degrees = pitch * degrees;
            } else {
                self.perspective.yaw_degrees = yaw * degrees;
            }
        }
    }

    /// The aspect stretch as the photo is displayed, after quarter turns: positive values make
    /// it wider.
    pub fn displayed_aspect_stretch(self) -> f32 {
        if self.quarter_turns % 2 == 1 {
            -self.aspect_stretch
        } else {
            self.aspect_stretch
        }
    }

    /// Sets the aspect stretch from its displayed value (see
    /// [`Self::displayed_aspect_stretch`]).
    pub fn set_displayed_aspect_stretch(&mut self, displayed: f32) {
        self.aspect_stretch = if self.quarter_turns % 2 == 1 {
            -displayed
        } else {
            displayed
        };
    }

    /// How much the aspect stretch scales the displayed photo horizontally and vertically.
    pub fn displayed_stretch_factors(self) -> [f32; 2] {
        let [x, y] = self
            .sanitized()
            .aspect_stretch_factors()
            .map(|factor| factor as f32);
        if self.quarter_turns % 2 == 1 {
            [y, x]
        } else {
            [x, y]
        }
    }

    /// How much [`Self::aspect_stretch`] scales the output's width and height, before quarter
    /// turns. Their product is 1, and both are exactly 1 without a stretch.
    fn aspect_stretch_factors(self) -> [f64; 2] {
        // Each side takes half of the ratio's change, 2^(value / 100).
        let half_log2 = 0.5 * f64::from(self.aspect_stretch) / 100.0;
        [half_log2.exp2(), (-half_log2).exp2()]
    }

    /// Which source side the displayed side `side` shows, as `[pitch, yaw]` signs: widening it
    /// adds to exactly one of them.
    fn source_side(self, [x, y]: [i8; 2]) -> [f32; 2] {
        // Undo the quarter turns, then the flips; rotation and skew do not change which side
        // is which.
        let [x, y] = match self.quarter_turns % 4 {
            0 => [x, y],
            1 => [y, -x],
            2 => [-x, -y],
            _ => [-y, x],
        };
        let x = if self.flip_horizontal { -x } else { x };
        let y = if self.flip_vertical { -y } else { y };
        // A positive pitch widens the top (y = -1), a positive yaw the right side (x = 1).
        [f32::from(-y), f32::from(x)]
    }
}

/// The displayed top and right side, as directions in y-down coordinates.
const DISPLAYED_SIDES: [[i8; 2]; 2] = [[0, -1], [1, 0]];

fn is_zero(value: &f32) -> bool {
    *value == 0.0
}

fn finite_clamp(value: f32, min: f32, max: f32) -> f32 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        0.0
    }
}
