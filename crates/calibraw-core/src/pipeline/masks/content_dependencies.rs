//! What an edit derives from the rendered image content.

use super::{MaskGeometry, MaskKind, MaskStack};

/// Image-content results an edit depends on.
///
/// Each result is derived from the rendered source image, either by a local AI
/// model or by sampling the image directly. They all go stale together when
/// that image changes, and one update refreshes them together.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ContentDependencies {
    /// Subject and background masks share one subject segmentation.
    pub subject: bool,
    /// Sky masks share one sky segmentation.
    pub sky: bool,
    /// Depth-range masks, depth fog and Relight share one scene depth estimate.
    pub scene_depth: bool,
    /// `(mask, component)` of each object mask with a positive stroke; every
    /// object needs its own segmentation.
    pub objects: Vec<(usize, usize)>,
    /// Luminance and sampled colour ranges read the source image itself.
    pub range_sources: bool,
}

impl ContentDependencies {
    pub fn is_empty(&self) -> bool {
        !self.needs_ai() && !self.range_sources
    }

    /// Whether refreshing these results runs a local AI model.
    pub fn needs_ai(&self) -> bool {
        self.subject || self.sky || self.scene_depth || !self.objects.is_empty()
    }

    /// Model runs one update performs: one per shared result plus one per object.
    pub fn ai_run_count(&self) -> usize {
        usize::from(self.subject)
            + usize::from(self.sky)
            + usize::from(self.scene_depth)
            + self.objects.len()
    }
}

impl MaskStack {
    pub fn content_dependencies(&self) -> ContentDependencies {
        let mut dependencies = ContentDependencies {
            scene_depth: self.has_scene_depth_effect(),
            ..ContentDependencies::default()
        };
        for (mask_index, mask) in self.masks.iter().enumerate() {
            for (component_index, component) in mask.components.iter().enumerate() {
                match (component.kind, &component.geometry) {
                    (MaskKind::Subject | MaskKind::Background, MaskGeometry::Ai { .. }) => {
                        dependencies.subject = true;
                    }
                    (MaskKind::Sky, MaskGeometry::Ai { .. }) => dependencies.sky = true,
                    (MaskKind::DepthRange, MaskGeometry::DepthRange { .. }) => {
                        dependencies.scene_depth = true;
                    }
                    (MaskKind::Object, MaskGeometry::Object { strokes, .. })
                        if strokes
                            .iter()
                            .any(|stroke| stroke.positive && !stroke.points.is_empty()) =>
                    {
                        dependencies.objects.push((mask_index, component_index));
                    }
                    (MaskKind::LuminanceRange, MaskGeometry::LuminanceRange { .. })
                    | (MaskKind::ColorRange, MaskGeometry::ColorRange { sampled: true, .. }) => {
                        dependencies.range_sources = true;
                    }
                    _ => {}
                }
            }
        }
        dependencies
    }

    /// Scene depth is required by a depth-range mask, depth fog or Relight but absent.
    pub fn scene_depth_missing(&self) -> bool {
        self.scene_depth_image().is_none() && self.content_dependencies().scene_depth
    }
}
