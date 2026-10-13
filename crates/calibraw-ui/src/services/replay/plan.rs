//! Which edit categories a replay shows, in order, and the edit state after each.

use crate::pipeline::{
    EffectComponent, ExposureParams, GeometryTransform, LocalMask, MaskEffect, MaskStack,
    RemoveEditState,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReplayStageKind {
    Edit,
    Crop,
    Rotate,
    Transform,
    Masks,
    Effects,
    Remove,
}

impl ReplayStageKind {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Edit => "Edit",
            Self::Crop => "Crop",
            Self::Rotate => "Rotate",
            Self::Transform => "Transform",
            Self::Masks => "Masks",
            Self::Effects => "Effects",
            Self::Remove => "Remove",
        }
    }
}

#[derive(Clone)]
pub(super) struct ReplayRenderState {
    pub(super) exposure: ExposureParams,
    pub(super) geometry: GeometryTransform,
    pub(super) masks: MaskStack,
    pub(super) remove: RemoveEditState,
}

impl ReplayRenderState {
    pub(super) fn original(original_exposure: ExposureParams) -> Self {
        Self {
            exposure: original_exposure,
            geometry: GeometryTransform::default(),
            masks: MaskStack::default(),
            remove: RemoveEditState::default(),
        }
    }
}

pub(super) struct ReplayStage {
    pub(super) kind: ReplayStageKind,
    pub(super) state: ReplayRenderState,
}

pub(super) fn replay_stage_plan(
    original_exposure: ExposureParams,
    final_exposure: ExposureParams,
    final_geometry: GeometryTransform,
    final_masks: &MaskStack,
    final_remove: &RemoveEditState,
) -> Vec<ReplayStage> {
    let final_geometry = final_geometry.sanitized();
    let mut current = ReplayRenderState::original(original_exposure);
    let mut stages = Vec::new();

    if final_exposure != original_exposure {
        current.exposure = final_exposure;
        stages.push(ReplayStage {
            kind: ReplayStageKind::Edit,
            state: current.clone(),
        });
    }

    if crop_used(final_geometry) {
        current.geometry.crop = final_geometry.crop;
        current.geometry.aspect_ratio = final_geometry.aspect_ratio;
        stages.push(ReplayStage {
            kind: ReplayStageKind::Crop,
            state: current.clone(),
        });
    }

    if rotate_used(final_geometry) {
        current.geometry.quarter_turns = final_geometry.quarter_turns;
        current.geometry.rotation_degrees = final_geometry.rotation_degrees;
        current.geometry.flip_horizontal = final_geometry.flip_horizontal;
        current.geometry.flip_vertical = final_geometry.flip_vertical;
        stages.push(ReplayStage {
            kind: ReplayStageKind::Rotate,
            state: current.clone(),
        });
    }

    if transform_used(final_geometry) {
        current.geometry.horizontal_transform = final_geometry.horizontal_transform;
        current.geometry.vertical_transform = final_geometry.vertical_transform;
        stages.push(ReplayStage {
            kind: ReplayStageKind::Transform,
            state: current.clone(),
        });
    }

    let adjustment_masks = masks_without_effects(final_masks);
    if masks_used(&adjustment_masks) {
        current.masks = adjustment_masks;
        stages.push(ReplayStage {
            kind: ReplayStageKind::Masks,
            state: current.clone(),
        });
    }

    if effects_used(final_masks) {
        current.masks = final_masks.clone();
        stages.push(ReplayStage {
            kind: ReplayStageKind::Effects,
            state: current.clone(),
        });
    }

    if remove_used(final_remove) {
        current.remove = final_remove.clone();
        stages.push(ReplayStage {
            kind: ReplayStageKind::Remove,
            state: current,
        });
    }

    stages
}

fn masks_without_effects(masks: &MaskStack) -> MaskStack {
    let mut adjustments = masks.clone();
    adjustments.global_effects.clear();
    for mask in &mut adjustments.masks {
        mask.effect_components.clear();
        // Legacy effect masks do not apply their stored adjustments.
        if mask.effect != MaskEffect::Adjustment {
            mask.adjustments_enabled = false;
        }
        mask.effect = MaskEffect::Adjustment;
        mask.effect_settings = Default::default();
    }
    adjustments
}

fn mask_is_visible(mask: &LocalMask) -> bool {
    mask.enabled
        && mask.opacity > 1e-6
        && mask
            .components
            .iter()
            .any(|component| component.enabled && component.geometry.is_initialized())
}

fn effects_used(masks: &MaskStack) -> bool {
    masks.global_effects.iter().any(EffectComponent::is_active)
        || masks
            .masks
            .iter()
            .filter(|mask| mask_is_visible(mask))
            .any(|mask| {
                mask.effect_components
                    .iter()
                    .any(EffectComponent::is_active)
                    || (mask.effect != MaskEffect::Adjustment
                        && EffectComponent {
                            effect: mask.effect,
                            enabled: true,
                            settings: mask.effect_settings,
                        }
                        .is_active())
            })
}

fn crop_used(geometry: GeometryTransform) -> bool {
    geometry.crop != GeometryTransform::default().crop
}

fn rotate_used(geometry: GeometryTransform) -> bool {
    geometry.quarter_turns != 0
        || geometry.rotation_degrees.abs() >= 1e-4
        || geometry.flip_horizontal
        || geometry.flip_vertical
}

fn transform_used(geometry: GeometryTransform) -> bool {
    geometry.horizontal_transform.abs() >= 1e-4 || geometry.vertical_transform.abs() >= 1e-4
}

fn masks_used(masks: &MaskStack) -> bool {
    masks.global_effects.iter().any(EffectComponent::is_active)
        || masks.masks.iter().any(|mask| {
            if !mask.enabled || mask.opacity <= 1e-6 {
                return false;
            }
            if !mask
                .components
                .iter()
                .any(|component| component.enabled && component.geometry.is_initialized())
            {
                return false;
            }

            mask.has_active_edit()
        })
}

fn remove_used(remove: &RemoveEditState) -> bool {
    remove.strokes.iter().any(|stroke| {
        stroke.composite_opacity() > 1e-6
            && (stroke.retouch.is_some()
                || !stroke.patches.is_empty()
                || !stroke.brush.points.is_empty())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::MaskKind;

    #[test]
    fn effects_are_applied_after_edit_and_mask_adjustments() {
        use crate::pipeline::{EffectComponent, LocalMask, MaskEffect};
        let original = ExposureParams::scene_referred_default();
        let mut exposure = original;
        exposure.exposure = 1.0;
        let mut masks = MaskStack::default();
        let mut mask = LocalMask::new(MaskKind::Fullscreen, 1);
        mask.adjustments.exposure = 0.25;
        mask.effect_components
            .push(EffectComponent::new(MaskEffect::Blur));
        mask.effect_components
            .push(EffectComponent::new(MaskEffect::Halation));
        masks.masks.push(mask);
        masks
            .global_effects
            .push(EffectComponent::new(MaskEffect::Glow));
        let stages = replay_stage_plan(
            original,
            exposure,
            GeometryTransform::default(),
            &masks,
            &RemoveEditState::default(),
        );
        assert_eq!(
            stages.iter().map(|stage| stage.kind).collect::<Vec<_>>(),
            [
                ReplayStageKind::Edit,
                ReplayStageKind::Masks,
                ReplayStageKind::Effects
            ]
        );
        assert_eq!(stages[0].state.exposure, exposure);
        assert!(stages[0].state.masks.global_effects.is_empty());
        let adjustments = &stages[1].state.masks;
        assert!(adjustments.global_effects.is_empty());
        assert!(adjustments.masks[0].effect_components.is_empty());
        assert_eq!(adjustments.masks[0].adjustments.exposure, 0.25);
        assert_eq!(stages[2].state.masks, masks);
        assert_eq!(stages[2].state.exposure, exposure);
    }

    #[test]
    fn effects_only_skip_edit_and_masks_including_legacy_masks() {
        use crate::pipeline::{EffectComponent, LocalMask, MaskEffect};
        let original = ExposureParams::scene_referred_default();
        let mut local = LocalMask::new(MaskKind::Fullscreen, 1);
        local
            .effect_components
            .push(EffectComponent::new(MaskEffect::Blur));
        local.effect_components[0].settings.blur.amount = 0.6;
        let mut legacy = LocalMask::new(MaskKind::Fullscreen, 2);
        legacy.effect = MaskEffect::Blur;
        legacy.effect_settings.blur.amount = 0.6;
        legacy.adjustments.exposure = 2.0;
        let mut global = EffectComponent::new(MaskEffect::Glow);
        global.settings.glow.amount = 0.6;
        for masks in [
            MaskStack {
                masks: vec![local],
                ..Default::default()
            },
            MaskStack {
                masks: vec![legacy],
                ..Default::default()
            },
            MaskStack {
                global_effects: vec![global],
                ..Default::default()
            },
        ] {
            let stages = replay_stage_plan(
                original,
                original,
                GeometryTransform::default(),
                &masks,
                &RemoveEditState::default(),
            );
            assert_eq!(stages.len(), 1);
            assert_eq!(stages[0].kind, ReplayStageKind::Effects);
            assert_eq!(stages[0].state.masks, masks);
        }
    }

    #[test]
    fn invisible_effects_and_inactive_effect_settings_do_not_add_stages() {
        use crate::pipeline::{EffectComponent, LocalMask, MaskEffect};
        let original = ExposureParams::scene_referred_default();
        let mut mask = LocalMask::new(MaskKind::Fullscreen, 1);
        let mut effect = EffectComponent::new(MaskEffect::Blur);
        effect.settings.blur.amount = 0.5;
        mask.effect_components.push(effect.clone());
        for kind in 0..3 {
            let mut hidden = mask.clone();
            match kind {
                0 => hidden.enabled = false,
                1 => hidden.opacity = 0.0,
                _ => hidden.components[0].enabled = false,
            }
            effect.enabled = false;
            let masks = MaskStack {
                masks: vec![hidden],
                global_effects: vec![effect.clone()],
                ..Default::default()
            };
            let stages = replay_stage_plan(
                original,
                original,
                GeometryTransform::default(),
                &masks,
                &RemoveEditState::default(),
            );
            assert!(stages.is_empty());
        }
    }

    #[test]
    fn stage_usage_helpers_split_geometry_categories() {
        let mut geometry = GeometryTransform::default();
        assert!(!crop_used(geometry));
        assert!(!rotate_used(geometry));
        assert!(!transform_used(geometry));

        geometry.crop = [0.1, 0.2, 0.9, 0.8];
        assert!(crop_used(geometry));
        assert!(!rotate_used(geometry));
        assert!(!transform_used(geometry));

        geometry.quarter_turns = 1;
        assert!(rotate_used(geometry));
        geometry.horizontal_transform = 4.0;
        assert!(transform_used(geometry));
    }

    #[test]
    fn stage_plan_is_ordered_and_skips_unused_categories() {
        let original_exposure = ExposureParams::scene_referred_default();
        let mut final_exposure = original_exposure;
        final_exposure.exposure = 0.75;
        let geometry = GeometryTransform {
            crop: [0.1, 0.1, 0.9, 0.9],
            quarter_turns: 1,
            vertical_transform: 3.0,
            ..GeometryTransform::default()
        };
        let mut masks = MaskStack::default();
        let mut mask = crate::pipeline::LocalMask::new(MaskKind::Fullscreen, 1);
        mask.adjustments.exposure = 0.5;
        masks.masks.push(mask);
        let mut remove = RemoveEditState::default();
        let mut remove_stroke = crate::pipeline::RemoveStroke::default();
        remove_stroke
            .brush
            .points
            .push(crate::pipeline::RemoveBrushPoint {
                x: 0.5,
                y: 0.5,
                radius: 12.0,
            });
        remove.strokes.push(remove_stroke);

        let stages =
            replay_stage_plan(original_exposure, final_exposure, geometry, &masks, &remove);
        let kinds = stages.iter().map(|stage| stage.kind).collect::<Vec<_>>();
        assert_eq!(
            kinds,
            vec![
                ReplayStageKind::Edit,
                ReplayStageKind::Crop,
                ReplayStageKind::Rotate,
                ReplayStageKind::Transform,
                ReplayStageKind::Masks,
                ReplayStageKind::Remove,
            ]
        );

        let stages = replay_stage_plan(
            original_exposure,
            original_exposure,
            GeometryTransform::default(),
            &MaskStack::default(),
            &RemoveEditState::default(),
        );
        assert!(stages.is_empty());
    }

    #[test]
    fn unused_masks_do_not_create_a_replay_stage() {
        let original_exposure = ExposureParams::scene_referred_default();
        let mut masks = MaskStack::default();
        masks
            .masks
            .push(crate::pipeline::LocalMask::new(MaskKind::Brush, 1));
        assert!(!masks_used(&masks));

        let mut active = crate::pipeline::LocalMask::new(MaskKind::Fullscreen, 1);
        active.adjustments.exposure = 0.25;
        masks.masks.push(active);
        assert!(masks_used(&masks));

        let stages = replay_stage_plan(
            original_exposure,
            original_exposure,
            GeometryTransform::default(),
            &masks,
            &RemoveEditState::default(),
        );
        assert_eq!(stages.len(), 1);
        assert_eq!(stages[0].kind, ReplayStageKind::Masks);
    }

    #[test]
    fn unused_remove_entries_do_not_create_a_replay_stage() {
        let mut remove = RemoveEditState::default();
        remove
            .strokes
            .push(crate::pipeline::RemoveStroke::default());
        assert!(!remove_used(&remove));

        remove.strokes[0]
            .brush
            .points
            .push(crate::pipeline::RemoveBrushPoint {
                x: 0.5,
                y: 0.5,
                radius: 8.0,
            });
        assert!(remove_used(&remove));
    }
}
