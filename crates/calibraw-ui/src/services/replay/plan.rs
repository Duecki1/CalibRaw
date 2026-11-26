//! Which stages a replay shows, in order, and the edit state after each.
//!
//! Every stage's state includes all earlier stages, so the stages build the
//! edit up from the photo as it opened. The replay still ends on a separately
//! rendered final edit, so a setting no stage takes over cannot leave the last
//! frame wrong.

use super::frames::title_is_drawable;
use super::ReplayRequest;
use crate::i18n::tr;
use crate::luts::ResolvedLook;
use crate::pipeline::{
    AdjustmentGroup, EffectComponent, ExposureParams, GeometryTransform, LocalMask, MaskEffect,
    MaskStack, RemoveEditState,
};

/// How finely a replay breaks the edit into stages; a setting, saved by its
/// serialized name.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReplayDetail {
    /// One stage per category: the photo-wide edit, crop, rotation,
    /// transform, masks, effects and Remove.
    #[default]
    Normal,
    /// One stage per Develop card and per mask. Each mask's area is
    /// highlighted before its edit applies.
    Detailed,
}

impl ReplayDetail {
    pub(crate) const ALL: [Self; 2] = [Self::Normal, Self::Detailed];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Normal => tr("Normal"),
            Self::Detailed => tr("Detailed"),
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
    /// Shown over the stage in the video's bitmap font, which is English only.
    pub(super) title: String,
    /// The mask, by index into `state.masks`, whose area is highlighted
    /// before this stage applies it.
    pub(super) highlight: Option<usize>,
    pub(super) state: ReplayRenderState,
}

pub(super) fn replay_stage_plan(request: &ReplayRequest) -> Vec<ReplayStage> {
    let detail = request.detail;
    let mut plan = StagePlan {
        current: ReplayRenderState::original(request.original_exposure),
        stages: Vec::new(),
    };
    let look = request.final_look.as_ref();
    match detail {
        ReplayDetail::Normal => plan.photo_wide_edit(request.final_exposure, look),
        ReplayDetail::Detailed => plan.develop_cards(&request.final_exposure, look),
    }
    plan.geometry(request.final_geometry.sanitized());
    match detail {
        ReplayDetail::Normal => plan.masks_and_effects(&request.final_masks),
        ReplayDetail::Detailed => plan.each_mask_then_effects(&request.final_masks),
    }
    plan.remove(&request.final_remove);
    plan.stages
}

/// The stages so far and the edit state after the last of them.
struct StagePlan {
    current: ReplayRenderState,
    stages: Vec<ReplayStage>,
}

impl StagePlan {
    fn push(&mut self, title: impl Into<String>, highlight: Option<usize>) {
        self.stages.push(ReplayStage {
            title: title.into(),
            highlight,
            state: self.current.clone(),
        });
    }

    /// One stage for every photo-wide setting. The LUT is shown with them.
    fn photo_wide_edit(&mut self, final_exposure: ExposureParams, look: Option<&ResolvedLook>) {
        if final_exposure != self.current.exposure || look.is_some() {
            self.current.exposure = final_exposure;
            self.current.look = look.cloned();
            self.push("Edit", None);
        }
    }

    /// One stage for each Develop card that changed, then the look. Every
    /// setting belongs to exactly one of the copied sections
    /// (`ExposureParams::copy_group_from`), so after them the exposure is the
    /// final one.
    fn develop_cards(&mut self, final_exposure: &ExposureParams, look: Option<&ResolvedLook>) {
        self.exposure_step("Raw Processing", |exposure| {
            exposure.copy_raw_processing_from(final_exposure);
        });
        for group in AdjustmentGroup::ALL {
            self.exposure_step(group.label(), |exposure| {
                exposure.copy_group_from(final_exposure, group);
            });
        }
        self.exposure_step("Lens Corrections", |exposure| {
            exposure.copy_lens_correction_from(final_exposure);
        });
        if let Some(look) = look {
            self.current.look = Some(look.clone());
            self.push("Look", None);
        }
    }

    /// Applies `copy` to the current exposure and adds a stage if that changed it.
    fn exposure_step(&mut self, title: &str, copy: impl FnOnce(&mut ExposureParams)) {
        let before = self.current.exposure;
        copy(&mut self.current.exposure);
        if self.current.exposure != before {
            self.push(title, None);
        }
    }

    fn geometry(&mut self, final_geometry: GeometryTransform) {
        if crop_used(final_geometry) {
            self.current.geometry.crop = final_geometry.crop;
            self.current.geometry.aspect_ratio = final_geometry.aspect_ratio;
            self.push("Crop", None);
        }

        if rotate_used(final_geometry) {
            self.current.geometry.quarter_turns = final_geometry.quarter_turns;
            self.current.geometry.rotation_degrees = final_geometry.rotation_degrees;
            self.current.geometry.flip_horizontal = final_geometry.flip_horizontal;
            self.current.geometry.flip_vertical = final_geometry.flip_vertical;
            self.push("Rotate", None);
        }

        if transform_used(final_geometry) {
            self.current.geometry.horizontal_skew_degrees = final_geometry.horizontal_skew_degrees;
            self.current.geometry.vertical_skew_degrees = final_geometry.vertical_skew_degrees;
            self.current.geometry.perspective = final_geometry.perspective;
            self.current.geometry.aspect_stretch = final_geometry.aspect_stretch;
            self.push("Transform", None);
        }
    }

    /// Every mask's adjustments in one stage, then every effect in another.
    fn masks_and_effects(&mut self, final_masks: &MaskStack) {
        let adjustment_masks = masks_without_effects(final_masks);
        if masks_used(&adjustment_masks) {
            self.current.masks = adjustment_masks;
            self.push("Masks", None);
        }

        if effects_used(final_masks) {
            self.current.masks = final_masks.clone();
            self.push("Effects", None);
        }
    }

    /// One highlighted stage per mask that changes the photo, in stack order,
    /// with its adjustments and its effects, then the photo-wide effects.
    /// Masks render in stack order, so switching them on one by one shows
    /// what each adds.
    fn each_mask_then_effects(&mut self, final_masks: &MaskStack) {
        let shown: Vec<usize> = final_masks
            .masks
            .iter()
            .enumerate()
            .filter(|(_, mask)| mask_is_visible(mask) && mask.has_active_edit())
            .map(|(index, _)| index)
            .collect();
        let global_effects = final_masks
            .global_effects
            .iter()
            .any(EffectComponent::is_active);
        if shown.is_empty() && !global_effects {
            return;
        }

        let mut masks = final_masks.clone();
        masks.global_effects.clear();
        for &index in &shown {
            masks.masks[index].enabled = false;
        }
        self.current.masks = masks;
        for index in shown {
            let mask = &mut self.current.masks.masks[index];
            mask.enabled = true;
            let title = mask_title(mask, index);
            self.push(title, Some(index));
        }

        if global_effects {
            self.current
                .masks
                .global_effects
                .clone_from(&final_masks.global_effects);
            self.push("Effects", None);
        }
    }

    fn remove(&mut self, final_remove: &RemoveEditState) {
        if remove_used(final_remove) {
            self.current.remove = final_remove.clone();
            self.push("Remove", None);
        }
    }
}

/// The mask's name when the video's font can draw it, otherwise its number
/// in the stack, as the Masks panel numbers new masks.
fn mask_title(mask: &LocalMask, index: usize) -> String {
    let name = mask.name.trim();
    if !name.is_empty() && title_is_drawable(name) {
        name.to_owned()
    } else {
        format!("Mask {}", index + 1)
    }
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
