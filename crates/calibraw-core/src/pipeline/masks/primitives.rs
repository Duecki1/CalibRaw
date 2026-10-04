//! Building blocks of mask geometry: path points, brush dabs, strokes and mask images.

use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct PathPoint {
    pub position: [f32; 2],
    #[serde(default)]
    pub handle_in: [f32; 2],
    #[serde(default)]
    pub handle_out: [f32; 2],
}

impl PathPoint {
    pub const fn corner(position: [f32; 2]) -> Self {
        Self {
            position,
            handle_in: [0.0, 0.0],
            handle_out: [0.0, 0.0],
        }
    }

    pub fn incoming(self) -> [f32; 2] {
        [
            self.position[0] + self.handle_in[0],
            self.position[1] + self.handle_in[1],
        ]
    }

    pub fn outgoing(self) -> [f32; 2] {
        [
            self.position[0] + self.handle_out[0],
            self.position[1] + self.handle_out[1],
        ]
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub enum MaskCombineMode {
    #[default]
    Add,
    Subtract,
    Intersect,
}

impl MaskCombineMode {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Add => "Add",
            Self::Subtract => "Subtract",
            Self::Intersect => "Intersect",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BrushMode {
    #[default]
    Paint,
    Erase,
}

impl BrushMode {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Paint => "Brush",
            Self::Erase => "Eraser",
        }
    }

    pub fn dab_opacity(self, opacity_enabled: bool, opacity: f32) -> f32 {
        let magnitude = if opacity_enabled {
            opacity.clamp(0.0, 1.0)
        } else {
            1.0
        };
        match self {
            Self::Paint => magnitude,
            Self::Erase => -magnitude,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct BrushDab {
    pub center: [f32; 2],
    pub opacity: f32,
    pub size: f32,
    pub feather: f32,
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct SubjectRefinement {
    #[serde(default = "default_subject_refinement_size")]
    pub size: f32,
    #[serde(default = "default_subject_refinement_feather")]
    pub feather: f32,
    #[serde(default = "default_subject_refinement_flow")]
    pub flow: f32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stroke_starts: Vec<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dabs: Vec<BrushDab>,
}

impl Default for SubjectRefinement {
    fn default() -> Self {
        Self {
            size: default_subject_refinement_size(),
            feather: default_subject_refinement_feather(),
            flow: default_subject_refinement_flow(),
            stroke_starts: Vec::new(),
            dabs: Vec::new(),
        }
    }
}

impl SubjectRefinement {
    pub fn is_empty(&self) -> bool {
        self.dabs.is_empty()
    }

    pub fn clear(&mut self) {
        self.dabs.clear();
        self.stroke_starts.clear();
    }

    pub fn composite(&self, raw_ai_mask: &MaskImage) -> Option<MaskImage> {
        if self.is_empty() {
            return Some(raw_ai_mask.clone());
        }
        let delta = rasterize_subject_refinement_delta(
            MaskRasterSpace::new(
                raw_ai_mask.width,
                raw_ai_mask.height,
                raw_ai_mask.width,
                raw_ai_mask.height,
            ),
            self,
        );
        let pixels = raw_ai_mask
            .pixels
            .iter()
            .copied()
            .zip(delta)
            .map(|(raw, delta)| {
                let probability = raw as f32 / 255.0;
                ((probability + delta).clamp(0.0, 1.0) * 255.0 + 0.5) as u8
            })
            .collect();
        MaskImage::new(raw_ai_mask.width, raw_ai_mask.height, pixels)
    }
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ObjectStroke {
    pub points: Vec<[f32; 2]>,
    pub positive: bool,
    #[serde(default)]
    pub brush_size: f32,
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct MaskImage {
    pub width: u32,
    pub height: u32,
    #[serde(with = "base64_arc_bytes")]
    pub pixels: Arc<[u8]>,
    #[serde(skip, default = "unit_sampling_rect")]
    pub(super) sampling_rect: [f32; 4],
}

impl MaskImage {
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Option<Self> {
        let pixel_count = usize::try_from(width)
            .ok()?
            .checked_mul(usize::try_from(height).ok()?)?;
        (pixels.len() == pixel_count).then(|| Self {
            width,
            height,
            pixels: pixels.into(),
            sampling_rect: unit_sampling_rect(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct MaskRgbImage {
    pub width: u32,
    pub height: u32,
    #[serde(with = "base64_arc_bytes")]
    pub rgba: Arc<[u8]>,
    #[serde(skip, default = "unit_sampling_rect")]
    pub(super) sampling_rect: [f32; 4],
}

impl MaskRgbImage {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Option<Self> {
        let byte_count = usize::try_from(width)
            .ok()?
            .checked_mul(usize::try_from(height).ok()?)?
            .checked_mul(4)?;
        (rgba.len() == byte_count).then(|| Self {
            width,
            height,
            rgba: rgba.into(),
            sampling_rect: unit_sampling_rect(),
        })
    }
}

fn unit_sampling_rect() -> [f32; 4] {
    [0.0, 0.0, 1.0, 1.0]
}

mod base64_arc_bytes {
    use base64::Engine as _;
    use serde::{Deserialize, Deserializer, Serializer};
    use std::sync::Arc;

    pub(super) fn serialize<S>(bytes: &Arc<[u8]>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(&base64::display::Base64Display::new(
            bytes.as_ref(),
            &base64::engine::general_purpose::STANDARD,
        ))
    }

    pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<Arc<[u8]>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let encoded = String::deserialize(deserializer)?;
        base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map(Arc::from)
            .map_err(serde::de::Error::custom)
    }
}

impl Default for BrushDab {
    fn default() -> Self {
        Self {
            center: [0.5, 0.5],
            opacity: 1.0,
            size: 0.055,
            feather: 0.55,
        }
    }
}
