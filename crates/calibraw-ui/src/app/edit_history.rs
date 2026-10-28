use super::{
    needs_canonical_mask_source, AppAction, AppTab, CalibRawApp, LensCorrectionState,
    LensfunCorrections,
};
use crate::luts::LookLut;
use crate::pipeline::{
    ExposureParams, GeometryTransform, ManualLensCorrection, MaskGeometry, MaskStack,
    ProcessingStage, RemoveEditState,
};
use eframe::egui;
use std::collections::VecDeque;
use std::sync::Arc;

const EDIT_HISTORY_LIMIT: usize = if crate::platform::PlatformCaps::CURRENT.constrained_memory() {
    128
} else {
    256
};

fn edit_history_interaction_active(ctx: &egui::Context) -> bool {
    super::interaction::pointer_engaged(ctx)
        || (ctx.memory(|memory| memory.focused().is_some())
            && ctx.input(|input| {
                [
                    egui::Key::ArrowUp,
                    egui::Key::ArrowDown,
                    egui::Key::ArrowLeft,
                    egui::Key::ArrowRight,
                ]
                .into_iter()
                .any(|key| input.key_down(key))
            }))
}

#[derive(Clone, Debug, PartialEq)]
struct LensEditState {
    enabled: bool,
    corrections: LensfunCorrections,
    selected_maker: String,
    selected_model: String,
    manual: Option<ManualLensCorrection>,
}

impl LensEditState {
    fn capture(lens: &LensCorrectionState) -> Self {
        Self {
            enabled: lens.enabled,
            corrections: lens.corrections,
            selected_maker: lens.selected_maker.clone(),
            selected_model: lens.selected_model.clone(),
            manual: lens.manual,
        }
    }

    fn matches(&self, lens: &LensCorrectionState) -> bool {
        self.enabled == lens.enabled
            && self.corrections == lens.corrections
            && self.selected_maker == lens.selected_maker
            && self.selected_model == lens.selected_model
            && self.manual == lens.manual
    }

    fn apply_to(&self, lens: &mut LensCorrectionState) {
        lens.enabled = self.enabled;
        lens.corrections = self.corrections;
        lens.selected_maker.clone_from(&self.selected_maker);
        lens.selected_model.clone_from(&self.selected_model);
        lens.manual = self.manual;
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct MaskSelection {
    mask: Option<usize>,
    component: Option<usize>,
}

impl MaskSelection {
    fn capture(navigation: &MaskStack, contents: &MaskStack) -> Self {
        let mask = navigation
            .selected_mask
            .filter(|index| *index < contents.masks.len());
        let component = mask.and_then(|mask_index| {
            navigation.selected_component.filter(|component_index| {
                *component_index < contents.masks[mask_index].components.len()
            })
        });
        Self { mask, component }
    }

    fn updated_from(self, navigation: &MaskStack, contents: &MaskStack) -> Self {
        let Some(mask) = navigation.selected_mask else {
            return Self::default();
        };
        let Some(mask_contents) = contents.masks.get(mask) else {
            return self;
        };
        let component = match navigation.selected_component {
            Some(component) if component < mask_contents.components.len() => Some(component),
            Some(_) if self.mask == Some(mask) => self.component,
            Some(_) | None => None,
        };
        Self {
            mask: Some(mask),
            component,
        }
    }

    fn apply_to(self, masks: &mut MaskStack) {
        masks.selected_mask = self.mask;
        masks.selected_component = self.component;
    }
}

#[derive(Clone, Debug)]
struct EditSnapshot {
    exposure: ExposureParams,
    geometry: GeometryTransform,
    masks: Arc<MaskStack>,
    mask_selection: MaskSelection,
    lens: LensEditState,
    look_lut: Option<LookLut>,
    remove: Arc<RemoveEditState>,
}

impl EditSnapshot {
    fn capture(
        exposure: &ExposureParams,
        geometry: GeometryTransform,
        masks: &MaskStack,
        lens: &LensCorrectionState,
        look_lut: Option<&LookLut>,
    ) -> Self {
        let mut contents = masks.clone();
        contents.selected_mask = None;
        contents.selected_component = None;
        let contents = Arc::new(contents);
        Self {
            exposure: *exposure,
            geometry,
            mask_selection: MaskSelection::capture(masks, &contents),
            masks: contents,
            lens: LensEditState::capture(lens),
            look_lut: look_lut.cloned(),
            remove: Arc::new(RemoveEditState::default()),
        }
    }

    fn capture_successor(
        &self,
        exposure: &ExposureParams,
        geometry: GeometryTransform,
        masks: &MaskStack,
        lens: &LensCorrectionState,
        look_lut: Option<&LookLut>,
        mask_contents_match: bool,
    ) -> Self {
        let contents = if mask_contents_match {
            Arc::clone(&self.masks)
        } else {
            let mut contents = masks.clone();
            contents.selected_mask = None;
            contents.selected_component = None;
            Arc::new(contents)
        };
        Self {
            exposure: *exposure,
            geometry,
            mask_selection: MaskSelection::capture(masks, &contents),
            masks: contents,
            lens: LensEditState::capture(lens),
            look_lut: look_lut.cloned(),
            remove: Arc::clone(&self.remove),
        }
    }

    fn remember_selection(&mut self, masks: &MaskStack) {
        self.mask_selection = self.mask_selection.updated_from(masks, &self.masks);
    }

    fn materialize_masks(&self) -> MaskStack {
        let mut masks = (*self.masks).clone();
        self.mask_selection.apply_to(&mut masks);
        masks
    }
}

pub(crate) struct EditHistory {
    undo: VecDeque<EditSnapshot>,
    redo: VecDeque<EditSnapshot>,
    current: EditSnapshot,
    interaction_pending: bool,
    mask_interaction_pending: bool,
    change_observed: bool,
    mask_change_observed: bool,
    restoring_snapshot: bool,
    committed_revision: u64,
}

impl EditHistory {
    pub(super) fn new(
        exposure: &ExposureParams,
        geometry: GeometryTransform,
        masks: &MaskStack,
        lens: &LensCorrectionState,
        look_lut: Option<&LookLut>,
    ) -> Self {
        Self {
            undo: VecDeque::new(),
            redo: VecDeque::new(),
            current: EditSnapshot::capture(exposure, geometry, masks, lens, look_lut),
            interaction_pending: false,
            mask_interaction_pending: false,
            change_observed: false,
            mask_change_observed: false,
            restoring_snapshot: false,
            committed_revision: 0,
        }
    }

    fn push_bounded(history: &mut VecDeque<EditSnapshot>, snapshot: EditSnapshot) {
        if history.len() == EDIT_HISTORY_LIMIT {
            history.pop_front();
        }
        history.push_back(snapshot);
    }

    pub(super) fn reset(
        &mut self,
        exposure: &ExposureParams,
        geometry: GeometryTransform,
        masks: &MaskStack,
        lens: &LensCorrectionState,
        look_lut: Option<&LookLut>,
        remove: &Arc<RemoveEditState>,
    ) {
        self.undo.clear();
        self.redo.clear();
        self.current = EditSnapshot::capture(exposure, geometry, masks, lens, look_lut);
        self.current.remove = Arc::clone(remove);
        self.interaction_pending = false;
        self.mask_interaction_pending = false;
        self.change_observed = false;
        self.mask_change_observed = false;
        self.restoring_snapshot = false;
    }

    fn note_change(&mut self) {
        if !self.restoring_snapshot {
            self.change_observed = true;
        }
    }

    fn note_mask_change(&mut self) {
        if !self.restoring_snapshot {
            self.change_observed = true;
            self.mask_change_observed = true;
        }
    }

    fn set_restoring_snapshot(&mut self, restoring: bool) {
        self.restoring_snapshot = restoring;
    }

    pub(super) fn observe(
        &mut self,
        exposure: &ExposureParams,
        geometry: GeometryTransform,
        masks: &MaskStack,
        lens: &LensCorrectionState,
        look_lut: Option<&LookLut>,
        interaction_active: bool,
    ) {
        self.current.remember_selection(masks);
        if self.current.geometry != geometry {
            self.change_observed = true;
        }
        if self.mask_change_observed {
            self.mask_interaction_pending = true;
            self.mask_change_observed = false;
        }
        if self.change_observed {
            self.interaction_pending = true;
            self.change_observed = false;
        }
        if !self.interaction_pending || interaction_active {
            return;
        }
        self.commit_current_state(exposure, geometry, masks, lens, look_lut);
    }

    fn commit_current_state(
        &mut self,
        exposure: &ExposureParams,
        geometry: GeometryTransform,
        masks: &MaskStack,
        lens: &LensCorrectionState,
        look_lut: Option<&LookLut>,
    ) {
        let mask_change_pending = self.mask_interaction_pending || self.mask_change_observed;
        self.change_observed = false;
        self.mask_change_observed = false;
        let mask_contents_match = if self.current.masks.masks.len() != masks.masks.len() {
            false
        } else if mask_change_pending {
            self.current.masks.masks == masks.masks
                && self.current.masks.global_effects == masks.global_effects
                && self.current.masks.scene_depth == masks.scene_depth
        } else {
            true
        };
        if self.current.exposure == *exposure
            && self.current.geometry == geometry
            && mask_contents_match
            && self.current.lens.matches(lens)
            && self.current.look_lut.as_ref() == look_lut
        {
            self.current.remember_selection(masks);
            self.interaction_pending = false;
            self.mask_interaction_pending = false;
            return;
        }

        let next = self.current.capture_successor(
            exposure,
            geometry,
            masks,
            lens,
            look_lut,
            mask_contents_match,
        );
        let previous = std::mem::replace(&mut self.current, next);
        Self::push_bounded(&mut self.undo, previous);
        self.redo.clear();
        self.interaction_pending = false;
        self.mask_interaction_pending = false;
        self.committed_revision = self.committed_revision.wrapping_add(1);
    }

    pub(super) fn commit_remove_state(
        &mut self,
        exposure: &ExposureParams,
        geometry: GeometryTransform,
        masks: &MaskStack,
        lens: &LensCorrectionState,
        look_lut: Option<&LookLut>,
        remove: &Arc<RemoveEditState>,
    ) {
        self.commit_current_state(exposure, geometry, masks, lens, look_lut);
        if Arc::ptr_eq(&self.current.remove, remove) {
            return;
        }
        let mut next = self
            .current
            .capture_successor(exposure, geometry, masks, lens, look_lut, true);
        next.remove = Arc::clone(remove);
        let previous = std::mem::replace(&mut self.current, next);
        Self::push_bounded(&mut self.undo, previous);
        self.redo.clear();
        self.committed_revision = self.committed_revision.wrapping_add(1);
    }

    pub(super) fn can_undo(&self) -> bool {
        !self.undo.is_empty()
            || self.interaction_pending
            || self.mask_interaction_pending
            || self.change_observed
            || self.mask_change_observed
    }

    pub(super) fn can_redo(&self) -> bool {
        !self.interaction_pending
            && !self.mask_interaction_pending
            && !self.change_observed
            && !self.mask_change_observed
            && !self.redo.is_empty()
    }

    fn undo(
        &mut self,
        exposure: &ExposureParams,
        geometry: GeometryTransform,
        masks: &MaskStack,
        lens: &LensCorrectionState,
        look_lut: Option<&LookLut>,
    ) -> Option<(EditSnapshot, bool, bool)> {
        self.commit_current_state(exposure, geometry, masks, lens, look_lut);
        let target = self.undo.pop_back()?;
        let masks_changed = !Arc::ptr_eq(&target.masks, &self.current.masks);
        let remove_changed = !Arc::ptr_eq(&target.remove, &self.current.remove);
        let present = std::mem::replace(&mut self.current, target.clone());
        Self::push_bounded(&mut self.redo, present);
        self.committed_revision = self.committed_revision.wrapping_add(1);
        Some((target, masks_changed, remove_changed))
    }

    fn redo(
        &mut self,
        exposure: &ExposureParams,
        geometry: GeometryTransform,
        masks: &MaskStack,
        lens: &LensCorrectionState,
        look_lut: Option<&LookLut>,
    ) -> Option<(EditSnapshot, bool, bool)> {
        self.commit_current_state(exposure, geometry, masks, lens, look_lut);
        let target = self.redo.pop_back()?;
        let masks_changed = !Arc::ptr_eq(&target.masks, &self.current.masks);
        let remove_changed = !Arc::ptr_eq(&target.remove, &self.current.remove);
        let present = std::mem::replace(&mut self.current, target.clone());
        Self::push_bounded(&mut self.undo, present);
        self.committed_revision = self.committed_revision.wrapping_add(1);
        Some((target, masks_changed, remove_changed))
    }

    fn committed_revision(&self) -> u64 {
        self.committed_revision
    }

    fn committed_masks(&self) -> Arc<MaskStack> {
        Arc::clone(&self.current.masks)
    }

    fn committed_remove(&self) -> Arc<RemoveEditState> {
        Arc::clone(&self.current.remove)
    }
}

impl CalibRawApp {
    pub(crate) fn note_edit_changed(&mut self) {
        self.persistence.history.note_change();
    }

    pub(crate) fn note_remove_edit_changed(&mut self) {
        self.persistence.history.commit_remove_state(
            &self.develop.exposure,
            self.develop.geometry,
            &self.masks.stack,
            &self.develop.lens_correction,
            self.develop.look_lut.as_ref(),
            &self.inpaint.edits,
        );
        self.note_mask_source_changed();
        self.queue_preview_processing(ProcessingStage::Raw);
    }

    pub(crate) fn note_geometry_changed(&mut self) {
        self.develop.geometry = self.develop.geometry.sanitized();
        self.develop.geometry_revision = self.develop.geometry_revision.wrapping_add(1);
        // Geometry also changes output parameters (for example crop-relative
        // vignetting), even when an existing detail crop still covers the view.
        self.queue_preview_processing(ProcessingStage::Output);
    }

    pub(crate) fn note_mask_edit_changed(&mut self) {
        super::preview_visibility::PreviewVisibility::invalidate_masks(
            &self.egui_ctx,
            &self.masks.stack,
        );
        self.persistence.history.note_mask_change();
    }

    pub(crate) fn edit_commit_revision(&self) -> u64 {
        self.persistence
            .history
            .committed_revision()
            .wrapping_mul(0x9e37_79b9_7f4a_7c15)
            ^ self.develop.geometry_revision.rotate_left(17)
    }

    pub(crate) fn committed_mask_state_for_persistence(&self) -> Arc<MaskStack> {
        self.persistence.history.committed_masks()
    }

    pub(crate) fn committed_remove_state_for_persistence(&self) -> Arc<RemoveEditState> {
        self.persistence.history.committed_remove()
    }

    pub(crate) fn reset_edit_history(&mut self) {
        self.persistence.lens_restore_masks = None;
        self.persistence.history.reset(
            &self.develop.exposure,
            self.develop.geometry,
            &self.masks.stack,
            &self.develop.lens_correction,
            self.develop.look_lut.as_ref(),
            &self.inpaint.edits,
        );
    }

    pub(crate) fn observe_edit_history(&mut self, ctx: &egui::Context) {
        let interaction_active = edit_history_interaction_active(ctx);
        self.persistence.history.observe(
            &self.develop.exposure,
            self.develop.geometry,
            &self.masks.stack,
            &self.develop.lens_correction,
            self.develop.look_lut.as_ref(),
            interaction_active,
        );
    }

    pub(crate) fn commit_edit_history_now(&mut self) {
        self.finish_mask_geometry_interaction();
        self.persistence.history.observe(
            &self.develop.exposure,
            self.develop.geometry,
            &self.masks.stack,
            &self.develop.lens_correction,
            self.develop.look_lut.as_ref(),
            false,
        );
    }

    pub(crate) fn can_undo_edit(&self) -> bool {
        self.develop.loaded_raw.is_some() && self.persistence.history.can_undo()
    }

    pub(crate) fn can_redo_edit(&self) -> bool {
        self.develop.loaded_raw.is_some() && self.persistence.history.can_redo()
    }

    pub(crate) fn undo_edit(&mut self) {
        self.finish_mask_geometry_interaction();
        let snapshot = self.persistence.history.undo(
            &self.develop.exposure,
            self.develop.geometry,
            &self.masks.stack,
            &self.develop.lens_correction,
            self.develop.look_lut.as_ref(),
        );
        if let Some((snapshot, masks_changed, remove_changed)) = snapshot {
            self.apply_edit_snapshot(snapshot, masks_changed, remove_changed);
            self.ui.notice = Some("Undid edit.".to_owned());
        }
    }

    pub(crate) fn redo_edit(&mut self) {
        self.finish_mask_geometry_interaction();
        let snapshot = self.persistence.history.redo(
            &self.develop.exposure,
            self.develop.geometry,
            &self.masks.stack,
            &self.develop.lens_correction,
            self.develop.look_lut.as_ref(),
        );
        if let Some((snapshot, masks_changed, remove_changed)) = snapshot {
            self.apply_edit_snapshot(snapshot, masks_changed, remove_changed);
            self.ui.notice = Some("Redid edit.".to_owned());
        }
    }

    pub(crate) fn handle_edit_history_shortcuts(&mut self, ctx: &egui::Context) {
        if self.ui.active_tab != AppTab::Develop || !self.edit_history_shortcuts_allowed(ctx) {
            return;
        }
        let redo_shift_z = egui::KeyboardShortcut::new(
            egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
            egui::Key::Z,
        );
        let redo_y = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::Y);
        let undo = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::Z);

        let redo = AppAction::RedoEdit;
        let redo_requested = self.action_enabled(redo)
            && (ctx.input_mut(|input| input.consume_shortcut(&redo_shift_z))
                || ctx.input_mut(|input| input.consume_shortcut(&redo_y)));
        if redo_requested {
            self.dispatch_action(redo);
            return;
        }

        let undo_action = AppAction::UndoEdit;
        if self.action_enabled(undo_action) && ctx.input_mut(|input| input.consume_shortcut(&undo))
        {
            self.dispatch_action(undo_action);
        }
    }

    fn apply_edit_snapshot(
        &mut self,
        snapshot: EditSnapshot,
        masks_changed: bool,
        remove_changed: bool,
    ) {
        let lens_changed = !snapshot.lens.matches(&self.develop.lens_correction);
        let look_lut_changed = snapshot.look_lut != self.develop.look_lut;
        let geometry_changed = snapshot.geometry != self.develop.geometry;
        let ai_denoise_changed =
            snapshot.exposure.ai_denoise_enabled != self.develop.exposure.ai_denoise_enabled;
        self.cancel_document_bound_foreground_operation();

        self.persistence.history.set_restoring_snapshot(true);
        self.develop.exposure = snapshot.exposure;
        self.develop.exposure.sanitize_tone_curves();
        if masks_changed {
            self.masks.stack = snapshot.materialize_masks();
        } else {
            snapshot.mask_selection.apply_to(&mut self.masks.stack);
        }
        if remove_changed {
            if let Some(cancellation) = self.inpaint.cancellation.take() {
                cancellation.store(true, std::sync::atomic::Ordering::Release);
            }
            self.inpaint.edits = Arc::clone(&snapshot.remove);
            self.inpaint.active_points.clear();
            self.inpaint.pending_brush = None;
            if self.ai_consent_is_for(calibraw_ai::AiFeature::Remove) {
                self.ai.consent = None;
            }
            self.inpaint.receiver = None;
            self.inpaint.processing_progress = None;
        }
        snapshot.lens.apply_to(&mut self.develop.lens_correction);
        if look_lut_changed {
            self.install_look_lut(snapshot.look_lut.clone());
        }
        if geometry_changed {
            self.develop.geometry = snapshot.geometry;
            // The restored crop is the one later straighten or transform
            // changes refit, and a drag from the replaced view must not
            // continue against it.
            self.develop_ui.crop_constraint_reference = Some(self.develop.geometry.crop);
            self.develop_ui.cancel_crop_drags();
            self.note_geometry_changed();
        }
        self.rehydrate_restored_mask_state();
        if remove_changed {
            self.note_mask_source_changed();
        }
        if ai_denoise_changed && self.develop.exposure.ai_denoise_enabled {
            self.ai.denoise_resume_pending = true;
        }

        if lens_changed {
            self.persistence.lens_restore_masks = Some(std::mem::take(&mut self.masks.stack));
            self.mark_lens_correction_dirty();
        } else {
            if masks_changed {
                self.mark_all_mask_layers_dirty();
            }
            self.mark_pipeline_dirty();
            if remove_changed {
                self.queue_preview_processing(ProcessingStage::Raw);
            }
            if ai_denoise_changed && self.develop.exposure.ai_denoise_enabled {
                self.preview.quality_dirty = true;
                self.preview.detail = None;
                self.preview.navigation = None;
            }
        }
        self.persistence.history.set_restoring_snapshot(false);
    }

    pub(super) fn rehydrate_restored_mask_state(&mut self) {
        self.masks.active_tool = self
            .masks
            .stack
            .selected_component()
            .map(|component| component.kind)
            .filter(|kind| kind.is_available());
        self.masks.subject_refinement_active &= self.masks.active_tool.is_some_and(|kind| {
            matches!(
                kind,
                crate::pipeline::MaskKind::Subject | crate::pipeline::MaskKind::Background
            )
        });
        self.masks.drag = None;
        self.masks.last_brush_point = None;
        self.masks.touch_gesture_backup = None;
        self.masks.interaction_dirty_layer = None;
        self.masks.interaction_last_upload = None;
        self.masks.interaction_has_uncommitted_change = false;
        self.masks.overlay_revision = self.masks.overlay_revision.wrapping_add(1);
        self.masks.overlay_texture = None;
        self.masks.overlay_texture_key = None;
        self.masks.overlay_blink = None;
        self.masks.thumbnail_group_textures.clear();
        self.masks.thumbnail_component_mask = None;
        self.masks.thumbnail_component_textures.clear();
        self.masks.thumbnail_revision = self.masks.overlay_revision;
        // Object inference caches carry the prompt strokes they were decoded
        // for, so a restored object mask must not reuse one from before the
        // undo or paste.
        self.ai.object_cache = None;
        self.ai.object_pending_target = None;

        let restored_source = self.masks.stack.masks.iter().find_map(|mask| {
            mask.components
                .iter()
                .find_map(|component| match &component.geometry {
                    MaskGeometry::LuminanceRange {
                        source: Some(source),
                        ..
                    }
                    | MaskGeometry::ColorRange {
                        source: Some(source),
                        ..
                    } => Some(source.clone()),
                    _ => None,
                })
        });
        if restored_source.is_some() || !needs_canonical_mask_source(&self.masks.stack) {
            self.masks.source_cache = restored_source;
        }
        // People masks being made were for the components before the restore.
        self.ai.people.composes.clear();
        if self.ai.update_needed {
            self.masks.clear_generated_caches();
        } else {
            self.masks.restore_generated_caches();
        }
        self.reset_ai_update_state();
        if self.ai.update_needed {
            self.ai.update_needed = !self.masks.stack.content_dependencies().is_empty();
        }
    }
}
