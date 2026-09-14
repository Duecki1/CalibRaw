//! Session-only eye controls. Saved edits, history, export and mask sources never
//! contain this state. Only the Develop preview reads projected copies.
use super::*;
use std::collections::HashSet;

#[derive(Clone, Default)]
pub(crate) struct PreviewVisibility {
    hidden: HashSet<(Option<usize>, &'static str)>,
    scope: Option<usize>,
    pending: bool,
    source_changed: bool,
    rebuild_required: bool,
    masks: Option<Arc<MaskStack>>,
    mask_names: Vec<String>,
}

impl PreviewVisibility {
    fn id() -> egui::Id {
        egui::Id::new("temporary-preview-card-visibility")
    }
    fn read(ctx: &egui::Context) -> Self {
        ctx.data(|data| data.get_temp::<Self>(Self::id()))
            .unwrap_or_default()
    }
    fn write(self, ctx: &egui::Context) {
        ctx.data_mut(|data| data.insert_temp(Self::id(), self));
    }
    pub(crate) fn clear(ctx: &egui::Context) {
        ctx.data_mut(|data| data.remove::<Self>(Self::id()));
    }
    pub(crate) fn set_mask_scope(ctx: &egui::Context, scope: Option<usize>) {
        let mut state = Self::read(ctx);
        state.scope = scope;
        state.write(ctx);
    }
    pub(crate) fn visible(ctx: &egui::Context, title: &'static str) -> bool {
        let state = Self::read(ctx);
        !state.hidden.contains(&(state.scope, title))
    }
    pub(crate) fn toggle(ctx: &egui::Context, title: &'static str) {
        let mut state = Self::read(ctx);
        let key = (state.scope, title);
        if !state.hidden.remove(&key) {
            state.hidden.insert(key);
        }
        state.pending = true;
        state.source_changed |= key == (None, "Lens Corrections");
        state.masks = None;
        state.write(ctx);
        ctx.request_repaint();
    }
    pub(crate) fn show(ctx: &egui::Context, title: &'static str) {
        if !Self::visible(ctx, title) {
            Self::toggle(ctx, title);
        }
    }
    pub(in crate::app) fn requires_rebuild(ctx: &egui::Context) -> bool {
        Self::read(ctx).rebuild_required
    }
    pub(in crate::app) fn request_rebuild(ctx: &egui::Context) {
        let mut state = Self::read(ctx);
        state.rebuild_required = true;
        state.write(ctx);
    }
    pub(in crate::app) fn rebuilt(ctx: &egui::Context) {
        let mut state = Self::read(ctx);
        state.rebuild_required = false;
        state.write(ctx);
    }
    pub(crate) fn invalidate_masks(ctx: &egui::Context, masks: &MaskStack) {
        let mut state = Self::read(ctx);
        let names: Vec<_> = masks.masks.iter().map(|mask| mask.name.clone()).collect();
        if state.mask_names != names {
            // Structural edits must never transfer an eye state to a different mask.
            state.hidden.retain(|(scope, _)| scope.is_none());
            state.mask_names = names;
        }
        state.masks = None;
        state.write(ctx);
    }
    fn group(title: &str) -> Option<crate::pipeline::AdjustmentGroup> {
        use crate::pipeline::AdjustmentGroup;
        Some(match title {
            "Light" => AdjustmentGroup::Light,
            "Tone Curve" => AdjustmentGroup::ToneCurve,
            "Color" => AdjustmentGroup::Color,
            "Color Grading" => AdjustmentGroup::ColorGrading,
            "Detail" => AdjustmentGroup::Detail,
            "Effects" => AdjustmentGroup::Effects,
            "Color Mixer" => AdjustmentGroup::ColorMixer,
            _ => return None,
        })
    }
    fn exposure(&self, saved: ExposureParams) -> ExposureParams {
        let mut preview = saved;
        for (scope, title) in &self.hidden {
            if scope.is_none() {
                if let Some(group) = Self::group(title) {
                    preview.reset_group(group);
                    if group == crate::pipeline::AdjustmentGroup::Detail {
                        preview.sharpen_amount = 0.0;
                    }
                }
            }
        }
        preview
    }
    fn project_masks(&self, saved: &MaskStack) -> MaskStack {
        let mut preview = saved.clone();
        for (scope, title) in &self.hidden {
            let Some(index) = *scope else {
                continue;
            };
            if *title == "Subject refinement" {
                preview.subject_refinement.clear();
            } else if let Some(mask) = preview.masks.get_mut(index) {
                if let Some(group) = Self::group(title) {
                    mask.adjustments.reset_group(group);
                } else {
                    mask.enabled = false;
                }
            }
        }
        preview
    }
}

impl CalibRawApp {
    pub(in crate::app) fn preview_exposure(&self) -> ExposureParams {
        PreviewVisibility::read(&self.egui_ctx).exposure(self.develop.exposure)
    }
    pub(in crate::app) fn preview_mask_stack(&self) -> Arc<MaskStack> {
        let mut state = PreviewVisibility::read(&self.egui_ctx);
        if let Some(masks) = state.masks {
            return masks;
        }
        let masks = Arc::new(state.project_masks(&self.masks.stack));
        state.masks = Some(Arc::clone(&masks));
        state.write(&self.egui_ctx);
        masks
    }
    pub(in crate::app) fn preview_source_raw(&self) -> Option<Arc<LoadedRaw>> {
        let state = PreviewVisibility::read(&self.egui_ctx);
        if state.hidden.contains(&(None, "Lens Corrections")) {
            self.develop.original_raw.clone()
        } else {
            self.develop.loaded_raw.clone()
        }
    }
    pub(in crate::app) fn sync_preview_visibility(&mut self) {
        let mut state = PreviewVisibility::read(&self.egui_ctx);
        if !state.pending {
            return;
        }
        let exposure = state.exposure(self.develop.exposure);
        let source_changed = state.source_changed
            || exposure.ai_denoise_enabled != self.develop.target_exposure.ai_denoise_enabled;
        state.pending = false;
        state.source_changed = false;
        state.rebuild_required |= source_changed;
        state.write(&self.egui_ctx);
        self.develop.target_exposure = exposure;
        self.preview.original_rendered_state = None;
        self.masks.dirty_layers.fill(true);
        self.masks.detail_dirty_layers.fill(true);
        self.masks.navigation_dirty_layers.fill(true);
        if source_changed {
            self.preview.quality_dirty = true;
            self.preview.detail_rebuild_receiver = None;
        }
        self.queue_preview_processing(ProcessingStage::Raw);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{BrushDab, LocalMask};

    fn edits() -> crate::sidecar::EditState {
        let mut edits = crate::sidecar::default_edit_state();
        edits.exposure.exposure = 1.75;
        edits.exposure.temperature = 23.0;
        edits.exposure.ai_denoise_enabled = true;
        let mut masks = MaskStack::default();
        for index in 0..2 {
            let mut mask = LocalMask::new(MaskKind::Brush, index + 1);
            mask.adjustments.exposure = 1.5 + index as f32;
            mask.adjustments.hue = 30.0;
            if let MaskGeometry::Brush { dabs, .. } = &mut mask.components[0].geometry {
                dabs.push(BrushDab::default());
            }
            masks.masks.push(mask);
        }
        edits.masks = Arc::new(masks);
        edits
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn eye_sync_and_shutdown_never_queue_or_write_a_sidecar() {
        let ctx = egui::Context::default();
        let mut app = CalibRawApp::empty(&ctx);
        let saved = edits();
        app.develop.exposure = saved.exposure;
        app.develop.target_exposure = saved.exposure;
        app.masks.stack = (*saved.masks).clone();
        let original =
            Arc::new(LoadedRaw::from_scene_linear_rec2020(8, 8, vec![0.2; 8 * 8 * 3]).unwrap());
        let corrected =
            Arc::new(LoadedRaw::from_scene_linear_rec2020(8, 8, vec![0.4; 8 * 8 * 3]).unwrap());
        app.develop.original_raw = Some(Arc::clone(&original));
        app.develop.loaded_raw = Some(Arc::clone(&corrected));
        app.develop.preview_raw = Some(Arc::clone(&corrected));
        app.develop.lens_correction.enabled = true;
        app.develop.lens_correction.applied = true;
        app.reset_edit_history();
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("photo.dng");
        let sidecar =
            crate::sidecar::save_desktop(&path, app.capture_sidecar_edit_state()).unwrap();
        let bytes = std::fs::read(&sidecar).unwrap();
        let modified = std::fs::metadata(&sidecar).unwrap().modified().unwrap();
        app.persistence.sidecar_target =
            Some(crate::sidecar::SidecarTarget::Desktop { raw_path: path });
        app.persistence.sidecar_saved_revision = Some(app.edit_commit_revision());
        let revision = app.edit_commit_revision();

        for title in ["Light", "Detail", "Lens Corrections"] {
            PreviewVisibility::toggle(&ctx, title);
        }
        PreviewVisibility::set_mask_scope(&ctx, Some(0));
        PreviewVisibility::toggle(&ctx, "Mask Properties");
        PreviewVisibility::set_mask_scope(&ctx, None);
        app.sync_preview_visibility();
        assert_eq!(app.develop.target_exposure.exposure, 0.0);
        assert!(!app.develop.target_exposure.ai_denoise_enabled);
        assert!(!app.preview_mask_stack().masks[0].enabled);
        assert!(Arc::ptr_eq(&app.preview_source_raw().unwrap(), &original));
        assert!(PreviewVisibility::requires_rebuild(&ctx));
        assert!(app.develop.lens_correction.enabled);
        assert!(Arc::ptr_eq(
            app.develop.loaded_raw.as_ref().unwrap(),
            &corrected
        ));
        assert_eq!(app.develop.exposure, saved.exposure);
        assert_eq!(app.masks.stack, *saved.masks);
        app.observe_edit_history(&ctx);
        app.schedule_sidecar_autosave(&ctx, false);
        app.flush_sidecar_on_exit();
        assert_eq!(app.edit_commit_revision(), revision);
        assert!(app.persistence.sidecar_pending.is_empty());
        assert!(app.persistence.sidecar_in_flight.is_none());
        assert!(app.persistence.sidecar_autosave_deadline.is_none());
        assert_eq!(std::fs::read(&sidecar).unwrap(), bytes);
        assert_eq!(
            std::fs::metadata(&sidecar).unwrap().modified().unwrap(),
            modified
        );
    }

    #[test]
    fn eyes_never_change_saved_values_sidecar_bytes_or_history() {
        let ctx = egui::Context::default();
        let saved = edits();
        let bytes = crate::sidecar::encode(saved.clone()).unwrap();
        let lens = LensCorrectionState::default();
        let mut history =
            crate::app::edit_history::EditHistory::new(&saved.exposure, &saved.masks, &lens);
        assert!(!history.can_undo());
        for title in [
            "Light",
            "Tone Curve",
            "Color",
            "Color Grading",
            "Detail",
            "Effects",
            "Color Mixer",
            "Lens Corrections",
        ] {
            PreviewVisibility::toggle(&ctx, title);
        }
        PreviewVisibility::set_mask_scope(&ctx, Some(0));
        for title in [
            "Light",
            "Tone Curve",
            "Color",
            "Color Grading",
            "Effects",
            "Color Mixer",
            "Mask Properties",
            "Mask type",
            "Blur",
            "Subject refinement",
        ] {
            PreviewVisibility::toggle(&ctx, title);
        }
        let visibility = PreviewVisibility::read(&ctx);
        let preview = visibility.exposure(saved.exposure);
        let masks = visibility.project_masks(&saved.masks);
        assert_eq!(preview.exposure, 0.0);
        assert_eq!(preview.sharpen_amount, 0.0);
        assert!(!preview.ai_denoise_enabled);
        assert!(!masks.masks[0].enabled);
        assert_eq!(masks.masks[1], saved.masks.masks[1]);
        assert_eq!(saved.exposure.exposure, 1.75);
        assert_eq!(saved.masks.masks[0].adjustments.exposure, 1.5);
        history.observe(&saved.exposure, &saved.masks, &lens, false);
        assert!(!history.can_undo());
        assert_eq!(crate::sidecar::encode(saved.clone()).unwrap(), bytes);

        // Closing/crashing discards the context. Reopen directly from the same
        // saved bytes: every value is already intact, with all eyes visible.
        let reopened = crate::sidecar::decode(&bytes).unwrap();
        let fresh = egui::Context::default();
        assert!(PreviewVisibility::visible(&fresh, "Light"));
        assert_eq!(
            PreviewVisibility::read(&fresh).exposure(reopened.edits.exposure),
            saved.exposure
        );
        assert_eq!(
            PreviewVisibility::read(&fresh)
                .project_masks(&reopened.edits.masks)
                .masks,
            saved.masks.masks
        );
    }

    #[test]
    fn saving_another_card_while_hidden_keeps_the_hidden_card_values() {
        let ctx = egui::Context::default();
        let mut saved = edits();
        PreviewVisibility::toggle(&ctx, "Light");
        saved.exposure.hue = 71.0;
        let reopened =
            crate::sidecar::decode(&crate::sidecar::encode(saved.clone()).unwrap()).unwrap();
        assert_eq!(reopened.edits.exposure.exposure, 1.75);
        assert_eq!(reopened.edits.exposure.hue, 71.0);
        PreviewVisibility::toggle(&ctx, "Light");
        assert_eq!(
            PreviewVisibility::read(&ctx).exposure(saved.exposure),
            saved.exposure
        );
    }

    #[test]
    fn resetting_a_hidden_mask_card_preserves_other_cards_and_masks() {
        let ctx = egui::Context::default();
        let mut saved = edits();
        let before = saved.masks.clone();
        PreviewVisibility::set_mask_scope(&ctx, Some(0));
        PreviewVisibility::toggle(&ctx, "Light");
        Arc::make_mut(&mut saved.masks).masks[0]
            .adjustments
            .reset_group(crate::pipeline::AdjustmentGroup::Light);
        PreviewVisibility::show(&ctx, "Light");
        assert_eq!(saved.masks.masks[0].adjustments.exposure, 0.0);
        assert_eq!(saved.masks.masks[0].adjustments.hue, 30.0);
        assert_eq!(saved.masks.masks[0].components, before.masks[0].components);
        assert_eq!(saved.masks.masks[1], before.masks[1]);
        let reopened = crate::sidecar::decode(&crate::sidecar::encode(saved).unwrap()).unwrap();
        assert_eq!(reopened.edits.masks.masks.len(), 2);
        assert_eq!(reopened.edits.exposure.exposure, 1.75);
    }
}
