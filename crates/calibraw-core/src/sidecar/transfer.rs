//! Moving selected edit categories from one edit state to another. Adjustment
//! paste and presets both go through [`transfer_edits`].
use super::{default_edit_state, EditState};
use crate::pipeline::{AdjustmentGroup, AdjustmentGroupSet, MaskGeometry, MaskKind, MaskStack};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// The categories Library > Copy Adjustments stores, as saved in the app
/// settings. [`EditSelection`] is the general form used to apply them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct AdjustmentCopySettings {
    #[serde(default = "default_true")]
    pub adjustments: bool,
    #[serde(default)]
    pub geometry: bool,
    #[serde(default = "default_true")]
    pub camera_profile: bool,
    #[serde(default = "default_true")]
    pub masks: bool,
    #[serde(default = "default_true")]
    pub ai_masks: bool,
    #[serde(default)]
    pub lens_correction: bool,
}

const fn default_true() -> bool {
    true
}

impl Default for AdjustmentCopySettings {
    fn default() -> Self {
        Self {
            adjustments: true,
            geometry: false,
            camera_profile: true,
            masks: true,
            ai_masks: true,
            lens_correction: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AdjustmentPasteMode {
    /// Overwrite only the selected categories.
    #[default]
    Merge,
    /// Clear the destination edit first, then apply the selected categories.
    Replace,
}

/// The edit categories a transfer moves from its source.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct EditSelection {
    /// Global develop settings, one group per sidebar card. The Effects group
    /// also holds the global effects, which the Adjustments tab lists below
    /// the Effects card.
    pub adjustment_groups: AdjustmentGroupSet,
    /// RAW decoding settings that no sidebar card owns. See
    /// [`crate::pipeline::ExposureParams::copy_raw_processing_from`].
    pub raw_processing: bool,
    pub geometry: bool,
    pub camera_profile: bool,
    /// Brush, path, fullscreen, radial and linear components.
    pub masks: bool,
    /// Subject, background, sky, object and range components.
    pub ai_masks: bool,
    pub lens_correction: bool,
}

impl EditSelection {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Whether the transfer changes any global develop setting.
    pub fn includes_exposure(&self) -> bool {
        !self.adjustment_groups.is_empty() || self.raw_processing
    }

    pub fn includes_masks(&self) -> bool {
        self.masks || self.ai_masks
    }
}

impl From<AdjustmentCopySettings> for EditSelection {
    fn from(settings: AdjustmentCopySettings) -> Self {
        Self {
            adjustment_groups: if settings.adjustments {
                AdjustmentGroupSet::ALL
            } else {
                AdjustmentGroupSet::EMPTY
            },
            raw_processing: settings.adjustments,
            geometry: settings.geometry,
            camera_profile: settings.camera_profile,
            masks: settings.masks,
            ai_masks: settings.ai_masks,
            lens_correction: settings.lens_correction,
        }
    }
}

/// Whether any setting `group` moves differs from its default: the group's
/// develop settings, and for the Effects group also the global effects.
pub fn adjustment_group_is_edited(edits: &EditState, group: AdjustmentGroup) -> bool {
    edits.exposure.group_is_edited(group)
        || (group == AdjustmentGroup::Effects && !edits.masks.global_effects.is_empty())
}

/// Mask kinds drawn by hand. Every other kind is generated from image content.
pub(crate) fn is_manual_mask_kind(kind: MaskKind) -> bool {
    matches!(
        kind,
        MaskKind::Brush
            | MaskKind::Fullscreen
            | MaskKind::Radial
            | MaskKind::Linear
            | MaskKind::Path
    )
}

fn filtered_mask_stack(masks: &MaskStack, include_manual: bool, include_ai: bool) -> MaskStack {
    MaskStack {
        masks: masks
            .masks
            .iter()
            .filter_map(|mask| {
                let mut selected = mask.clone();
                selected.components.retain(|component| {
                    if is_manual_mask_kind(component.kind) {
                        include_manual
                    } else {
                        include_ai
                    }
                });
                (!selected.components.is_empty()).then_some(selected)
            })
            .collect(),
        subject_refinement: if include_ai {
            masks.subject_refinement.clone()
        } else {
            Default::default()
        },
        ..Default::default()
    }
}

fn replace_selected_mask_categories(
    destination: &mut MaskStack,
    source: &MaskStack,
    include_manual: bool,
    include_ai: bool,
) {
    // Depth is tied to the destination image, and global effects belong to
    // the Effects group, so both stay.
    let scene_depth = destination.scene_depth.take();
    let global_effects = std::mem::take(&mut destination.global_effects);
    if include_manual && include_ai {
        *destination = source.clone();
        clear_copied_depth_images(destination);
    } else {
        let mut merged = filtered_mask_stack(destination, !include_manual, !include_ai);
        let mut copied = filtered_mask_stack(source, include_manual, include_ai);
        clear_copied_depth_images(&mut copied);
        if include_ai {
            merged.subject_refinement = copied.subject_refinement;
        }
        merged.masks.extend(copied.masks);
        *destination = merged;
    }
    destination.scene_depth = scene_depth;
    destination.global_effects = global_effects;
}

fn clear_copied_depth_images(masks: &mut MaskStack) {
    for component in masks.masks.iter_mut().flat_map(|mask| &mut mask.components) {
        if let MaskGeometry::DepthRange { depth, .. } = &mut component.geometry {
            *depth = None;
        }
    }
}

/// Applies the categories in `selection` from `source` to `destination`.
///
/// Inpainting and the destination's scene depth always stay with the
/// destination image. Copied masks that depend on image content are flagged
/// for regeneration through `ai_masks_need_update`.
pub fn transfer_edits(
    destination: &mut EditState,
    source: &EditState,
    selection: EditSelection,
    mode: AdjustmentPasteMode,
) {
    if mode == AdjustmentPasteMode::Replace {
        let remove = Arc::clone(&destination.remove);
        let scene_depth = destination.masks.scene_depth.clone();
        *destination = default_edit_state();
        destination.remove = remove;
        Arc::make_mut(&mut destination.masks).scene_depth = scene_depth;
    }
    for group in selection.adjustment_groups.iter() {
        destination
            .exposure
            .copy_group_from(&source.exposure, group);
    }
    if selection
        .adjustment_groups
        .contains(AdjustmentGroup::Effects)
        && destination.masks.global_effects != source.masks.global_effects
    {
        Arc::make_mut(&mut destination.masks).global_effects = source.masks.global_effects.clone();
        // Pasted depth fog or Relight needs this image's own scene depth.
        destination.ai_masks_need_update |= destination.masks.scene_depth_missing();
    }
    if selection.raw_processing {
        destination
            .exposure
            .copy_raw_processing_from(&source.exposure);
    }
    if selection.geometry {
        destination.geometry = source.geometry;
    }
    if selection.camera_profile {
        let camera_profile_changed = destination.camera_profile != source.camera_profile;
        destination.camera_profile = source.camera_profile.clone();
        if camera_profile_changed && !destination.masks.content_dependencies().is_empty() {
            destination.ai_masks_need_update = true;
        }
    }
    if selection.includes_masks() {
        let previous_ai_masks_need_update = destination.ai_masks_need_update;
        let previous_subject_refinement = destination.subject_refinement.clone();
        let mut masks = destination.masks.as_ref().clone();
        replace_selected_mask_categories(
            &mut masks,
            &source.masks,
            selection.masks,
            selection.ai_masks,
        );
        destination.masks = Arc::new(masks);
        destination.subject_refinement = if selection.ai_masks {
            source.subject_refinement.clone().or_else(|| {
                (!source.masks.subject_refinement.is_empty())
                    .then(|| source.masks.subject_refinement.clone())
            })
        } else {
            previous_subject_refinement
        };
        destination.ai_masks_need_update = if selection.ai_masks {
            source.ai_masks_need_update || !destination.masks.content_dependencies().is_empty()
        } else {
            previous_ai_masks_need_update
        };
        // Pasted depth fog or depth masks need this image's own scene depth.
        destination.ai_masks_need_update |= destination.masks.scene_depth_missing();
    }
    if selection.lens_correction {
        let lens_changed = destination.lens != source.lens;
        destination.lens = source.lens.clone();
        if lens_changed && !destination.masks.content_dependencies().is_empty() {
            destination.ai_masks_need_update = true;
        }
    }
}
