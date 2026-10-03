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
    /// Depth-range masks and depth fog share one scene depth estimate.
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
            scene_depth: self.has_depth_fog_effect(),
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

    /// Scene depth is required by a depth-range mask or depth fog but absent.
    pub fn scene_depth_missing(&self) -> bool {
        self.scene_depth_image().is_none() && self.content_dependencies().scene_depth
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{EffectComponent, MaskEffect, MaskImage, ObjectStroke};

    fn stack_with(kinds: &[MaskKind]) -> MaskStack {
        let mut stack = MaskStack::default();
        for &kind in kinds {
            stack.add_mask(kind).unwrap();
        }
        stack
    }

    #[test]
    fn empty_and_manual_stacks_have_no_dependencies() {
        assert!(MaskStack::default().content_dependencies().is_empty());
        let stack = stack_with(&[MaskKind::Brush, MaskKind::Radial, MaskKind::Linear]);
        assert!(stack.content_dependencies().is_empty());
    }

    #[test]
    fn shared_results_are_reported_once() {
        let stack = stack_with(&[
            MaskKind::Subject,
            MaskKind::Background,
            MaskKind::Sky,
            MaskKind::Sky,
            MaskKind::DepthRange,
        ]);
        let dependencies = stack.content_dependencies();
        assert!(dependencies.subject && dependencies.sky && dependencies.scene_depth);
        assert_eq!(dependencies.ai_run_count(), 3);
    }

    #[test]
    fn depth_fog_needs_scene_depth_like_a_depth_mask() {
        let mut stack = MaskStack::default();
        stack
            .global_effects
            .push(EffectComponent::new(MaskEffect::Fog));
        assert!(stack.content_dependencies().scene_depth);
        assert!(stack.scene_depth_missing());

        stack.scene_depth = Some(MaskImage::new(1, 1, vec![128]).unwrap());
        assert!(!stack.scene_depth_missing());

        stack.global_effects[0].settings.fog.depth_enabled = false;
        stack.scene_depth = None;
        assert!(!stack.content_dependencies().scene_depth);
        assert!(!stack.scene_depth_missing());
    }

    #[test]
    fn objects_need_a_positive_stroke_and_colour_ranges_a_sample() {
        let mut stack = stack_with(&[MaskKind::Object, MaskKind::ColorRange]);
        assert!(stack.content_dependencies().is_empty());

        if let MaskGeometry::Object { strokes, .. } = &mut stack.masks[0].components[0].geometry {
            strokes.push(ObjectStroke {
                points: vec![[0.5, 0.5]],
                positive: true,
                brush_size: 0.0,
            });
        }
        if let MaskGeometry::ColorRange { sampled, .. } = &mut stack.masks[1].components[0].geometry
        {
            *sampled = true;
        }
        let dependencies = stack.content_dependencies();
        assert_eq!(dependencies.objects, vec![(0, 0)]);
        assert!(dependencies.range_sources);
        assert!(!dependencies.scene_depth);
    }
}
