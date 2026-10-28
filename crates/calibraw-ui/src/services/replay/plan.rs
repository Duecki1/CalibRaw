//! Which edit categories a replay shows, in order, and the edit state after each.

use crate::luts::ResolvedLook;
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
    pub(super) look: Option<ResolvedLook>,
}

impl ReplayRenderState {
    pub(super) fn original(original_exposure: ExposureParams) -> Self {
        Self {
            exposure: original_exposure,
            geometry: GeometryTransform::default(),
            masks: MaskStack::default(),
            remove: RemoveEditState::default(),
            look: None,
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
    final_look: Option<&ResolvedLook>,
) -> Vec<ReplayStage> {
    let final_geometry = final_geometry.sanitized();
    let mut current = ReplayRenderState::original(original_exposure);
    let mut stages = Vec::new();

    // The LUT is shown with the global adjustments.
    if final_exposure != original_exposure || final_look.is_some() {
        current.exposure = final_exposure;
        current.look = final_look.cloned();
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
        current.geometry.horizontal_skew_degrees = final_geometry.horizontal_skew_degrees;
        current.geometry.vertical_skew_degrees = final_geometry.vertical_skew_degrees;
        current.geometry.perspective = final_geometry.perspective;
        current.geometry.aspect_stretch = final_geometry.aspect_stretch;
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
    geometry.horizontal_skew_degrees.abs() >= 1e-4
        || geometry.vertical_skew_degrees.abs() >= 1e-4
        || !geometry.perspective.is_identity()
        || geometry.aspect_stretch.abs() >= 1e-4
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
