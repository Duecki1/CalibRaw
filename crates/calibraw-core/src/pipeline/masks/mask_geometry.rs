//! `MaskGeometry`: the shape of one mask component and its serialized defaults.

use super::*;

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum MaskGeometry {
    Fullscreen,
    Brush {
        size: f32,
        feather: f32,
        #[serde(default)]
        opacity_enabled: bool,
        #[serde(default = "default_brush_opacity")]
        opacity: f32,
        #[serde(default = "default_brush_overlap_enabled")]
        overlap_enabled: bool,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        stroke_starts: Vec<usize>,
        dabs: Vec<BrushDab>,
    },
    Radial {
        center: [f32; 2],
        radius: [f32; 2],
        rotation: f32,
        feather: f32,
        initialized: bool,
    },
    Linear {
        start: [f32; 2],
        end: [f32; 2],
        feather: f32,
        initialized: bool,
    },
    Path {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        points: Vec<PathPoint>,
        #[serde(default)]
        grow: f32,
        #[serde(default)]
        feather: f32,
    },
    Ai {
        mask: Option<MaskImage>,
        #[serde(default)]
        grow: f32,
        feather: f32,
    },
    Object {
        mask: Option<MaskImage>,
        #[serde(default)]
        grow: f32,
        feather: f32,
        #[serde(default = "default_object_brush_size")]
        brush_size: f32,
        #[serde(default = "default_object_edge_refine")]
        edge_refine: f32,
        #[serde(default)]
        strokes: Vec<ObjectStroke>,
    },
    LuminanceRange {
        #[serde(default, skip_serializing)]
        source: Option<MaskRgbImage>,
        low: f32,
        high: f32,
        #[serde(default)]
        grow: f32,
        /// Softness below `low`; also the mask's shared shape feather.
        feather: f32,
        /// Softness above `high`. Absent in sidecars written before the two
        /// edges could differ, where `feather` softened both.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        high_feather: Option<f32>,
    },
    ColorRange {
        #[serde(default, skip_serializing)]
        source: Option<MaskRgbImage>,
        sample: [f32; 3],
        tolerance: f32,
        #[serde(default)]
        grow: f32,
        feather: f32,
        sampled: bool,
    },
    DepthRange {
        depth: Option<MaskImage>,
        #[serde(flatten)]
        range: DepthRangeSettings,
    },
    Placeholder,
}

/// Relative-depth selection with independently softened near and far cutoffs.
#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(from = "crate::migrations::depth_range_feather::StoredDepthRange")]
pub struct DepthRangeSettings {
    pub near: f32,
    pub far: f32,
    // Retain the schema's existing range-feather key for the near end.
    // The added far_feather overrides it for the far end on newer readers.
    #[serde(rename = "feather")]
    pub near_feather: f32,
    pub far_feather: f32,
}

impl Default for DepthRangeSettings {
    fn default() -> Self {
        Self {
            near: 0.0,
            far: 0.5,
            near_feather: 0.1,
            far_feather: 0.1,
        }
    }
}

impl DepthRangeSettings {
    /// The UI curve and mask rasterizer use the same depth response.
    pub fn weight(&self, depth: f32) -> f32 {
        let near = self.near.clamp(0.0, 1.0);
        let far = self.far.clamp(near, 1.0);
        let smooth = |value: f32| {
            let value = value.clamp(0.0, 1.0);
            value * value * (3.0 - 2.0 * value)
        };
        let near_width = self.near_feather.clamp(0.0, 1.0);
        let far_width = self.far_feather.clamp(0.0, 1.0);
        let lower = if near_width > 0.0 && near > 0.0 {
            smooth((depth - near) / near_width + 0.5)
        } else {
            f32::from(depth >= near)
        };
        let upper = if far_width > 0.0 && far < 1.0 {
            1.0 - smooth((depth - far) / far_width + 0.5)
        } else {
            f32::from(depth <= far)
        };
        lower * upper
    }
}

fn default_object_brush_size() -> f32 {
    0.055
}

pub(super) fn default_subject_refinement_size() -> f32 {
    0.035
}

pub(super) fn default_subject_refinement_feather() -> f32 {
    0.55
}

pub(super) fn default_subject_refinement_flow() -> f32 {
    1.0
}

fn default_brush_opacity() -> f32 {
    1.0
}

fn default_brush_overlap_enabled() -> bool {
    true
}

fn default_object_edge_refine() -> f32 {
    0.55
}

impl MaskGeometry {
    pub fn for_kind(kind: MaskKind) -> Self {
        match kind {
            MaskKind::Fullscreen => Self::Fullscreen,
            MaskKind::Brush => Self::Brush {
                size: 0.055,
                feather: 0.55,
                opacity_enabled: false,
                opacity: default_brush_opacity(),
                overlap_enabled: default_brush_overlap_enabled(),
                stroke_starts: Vec::new(),
                dabs: Vec::new(),
            },
            MaskKind::Radial => Self::Radial {
                center: [0.5, 0.5],
                radius: [0.22, 0.16],
                rotation: 0.0,
                feather: 0.55,
                initialized: false,
            },
            MaskKind::Linear => Self::Linear {
                start: [0.35, 0.5],
                end: [0.65, 0.5],
                feather: 1.0,
                initialized: false,
            },
            MaskKind::Path => Self::Path {
                points: Vec::new(),
                grow: 0.0,
                feather: 0.0,
            },
            MaskKind::Subject | MaskKind::Background | MaskKind::Sky => Self::Ai {
                mask: None,
                grow: 0.0,
                feather: 0.0,
            },
            MaskKind::Object => Self::Object {
                mask: None,
                grow: 0.0,
                feather: 0.0,
                brush_size: default_object_brush_size(),
                edge_refine: default_object_edge_refine(),
                strokes: Vec::new(),
            },
            MaskKind::LuminanceRange => Self::LuminanceRange {
                source: None,
                low: 0.2,
                high: 0.8,
                grow: 0.0,
                feather: 0.15,
                high_feather: None,
            },
            MaskKind::ColorRange => Self::ColorRange {
                source: None,
                sample: [0.5; 3],
                tolerance: 0.18,
                grow: 0.0,
                feather: 0.12,
                sampled: false,
            },
            MaskKind::DepthRange => Self::DepthRange {
                depth: None,
                range: DepthRangeSettings::default(),
            },
        }
    }

    pub fn is_initialized(&self) -> bool {
        match self {
            Self::Fullscreen => true,
            Self::Brush { dabs, .. } => !dabs.is_empty(),
            Self::Radial { initialized, .. } | Self::Linear { initialized, .. } => *initialized,
            Self::Path { points, .. } => points.len() >= 3,
            Self::Ai { mask, .. } | Self::Object { mask, .. } => mask.is_some(),
            Self::DepthRange { depth, .. } => depth.is_some(),
            Self::LuminanceRange { source, .. } => source.is_some(),
            Self::ColorRange {
                source, sampled, ..
            } => source.is_some() && *sampled,
            Self::Placeholder => false,
        }
    }

    pub fn set_feather(&mut self, value: f32) -> bool {
        if let Self::Brush { feather, dabs, .. } = self {
            if !set_if_changed(feather, value) {
                return false;
            }
            for dab in dabs {
                dab.feather = value;
            }
            return true;
        }
        let feather = match self {
            Self::Radial { feather, .. }
            | Self::Linear { feather, .. }
            | Self::Path { feather, .. }
            | Self::Ai { feather, .. }
            | Self::Object { feather, .. }
            | Self::LuminanceRange { feather, .. }
            | Self::ColorRange { feather, .. } => feather,
            Self::Fullscreen | Self::Brush { .. } | Self::DepthRange { .. } | Self::Placeholder => {
                return false
            }
        };
        set_if_changed(feather, value)
    }
}
