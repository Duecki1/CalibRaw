use super::*;

const INTERACTIVE_MASK_INTERVAL: Duration = Duration::from_millis(45);
const SHARED_REFINEMENT_LAYER: usize = MAX_LOCAL_MASKS;

impl MaskState {
    pub(super) fn generated_cache_mut(&mut self, model: AiMaskModel) -> &mut Option<MaskImage> {
        match model {
            AiMaskModel::Subject => &mut self.subject_cache,
            AiMaskModel::Sky => &mut self.sky_cache,
            AiMaskModel::Depth => &mut self.depth_cache,
        }
    }

    pub(in crate::app) fn clear_generated_caches(&mut self) {
        self.subject_cache = None;
        self.sky_cache = None;
        self.depth_cache = None;
    }

    pub(in crate::app) fn restore_generated_caches(&mut self) {
        for model in [AiMaskModel::Subject, AiMaskModel::Sky, AiMaskModel::Depth] {
            if model == AiMaskModel::Depth {
                self.depth_cache = self.stack.scene_depth_image().cloned();
                continue;
            }
            let cached = self
                .stack
                .masks
                .iter()
                .flat_map(|mask| &mask.components)
                .filter(|component| generated_mask_model(component.kind) == Some(model))
                .find_map(|component| match &component.geometry {
                    MaskGeometry::Ai { mask, .. }
                    | MaskGeometry::DepthRange { depth: mask, .. } => mask.clone(),
                    _ => None,
                });
            *self.generated_cache_mut(model) = cached;
        }
    }

    /// Clear mask interaction state and derived caches without changing the stack.
    pub(in crate::app) fn reset_transient_state(&mut self) {
        self.active_tool = None;
        self.brush_mode = BrushMode::Paint;
        self.subject_refinement_active = false;
        self.drag = None;
        self.last_brush_point = None;
        self.touch_gesture_backup = None;
        self.interaction_dirty_layer = None;
        self.interaction_last_upload = None;
        self.interaction_has_uncommitted_change = false;
        self.overlay_revision = self.overlay_revision.wrapping_add(1);
        self.overlay_texture = None;
        self.overlay_texture_key = None;
        self.overlay_blink = None;
        self.thumbnail_group_textures.clear();
        self.thumbnail_component_mask = None;
        self.thumbnail_component_textures.clear();
        self.thumbnail_revision = self.overlay_revision;
        self.source_cache = None;
        self.clear_generated_caches();
        self.dirty_layers.fill(false);
        self.detail_dirty_layers.fill(false);
        self.navigation_dirty_layers.fill(false);
    }

    pub(in crate::app) fn capture_ai_target(
        &self,
        mask_index: usize,
        component_index: usize,
    ) -> Option<AiMaskTarget> {
        let component = self
            .stack
            .masks
            .get(mask_index)
            .and_then(|mask| mask.components.get(component_index))?;
        Some(AiMaskTarget {
            mask_index,
            component_index,
            kind: component.kind,
            geometry: component.geometry.clone(),
        })
    }

    pub(in crate::app) fn resolve_ai_target(
        &self,
        target: &AiMaskTarget,
    ) -> std::result::Result<(usize, usize), String> {
        Self::resolve_ai_target_in_stack(&self.stack, target)
    }

    pub(in crate::app) fn resolve_ai_target_in_stack(
        stack: &MaskStack,
        target: &AiMaskTarget,
    ) -> std::result::Result<(usize, usize), String> {
        // Copied masks can have identical geometry. Prefer the captured slot
        // while it still matches, and only search for a moved target otherwise.
        if stack
            .masks
            .get(target.mask_index)
            .and_then(|mask| mask.components.get(target.component_index))
            .is_some_and(|component| {
                component.kind == target.kind && component.geometry == target.geometry
            })
        {
            return Ok((target.mask_index, target.component_index));
        }

        let matches = stack
            .masks
            .iter()
            .enumerate()
            .flat_map(|(mask_index, mask)| {
                mask.components
                    .iter()
                    .enumerate()
                    .map(move |(component_index, component)| {
                        (mask_index, component_index, component)
                    })
            })
            .filter(|(_, _, component)| {
                component.kind == target.kind && component.geometry == target.geometry
            })
            .map(|(mask_index, component_index, _)| (mask_index, component_index))
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [location] => Ok(*location),
            [] => {
                let message = match stack.masks.get(target.mask_index) {
                    None => "The target mask was deleted before inference completed.",
                    Some(mask) if mask.components.get(target.component_index).is_none() => {
                        "The target mask component was deleted before inference completed."
                    }
                    Some(mask) if mask.components[target.component_index].kind != target.kind => {
                        "The target component changed type before inference completed."
                    }
                    Some(_) => "The target component changed before inference completed.",
                };
                Err(message.to_owned())
            }
            _ => Err(
                "The target component is ambiguous after editing; the stale result was discarded."
                    .to_owned(),
            ),
        }
    }

    fn mark_geometry_dirty(&mut self, layer: usize) {
        if layer < MAX_LOCAL_MASKS {
            self.dirty_layers[layer] = true;
            self.detail_dirty_layers[layer] = true;
            self.navigation_dirty_layers[layer] = true;
        }
        self.overlay_revision = self.overlay_revision.wrapping_add(1);
    }

    fn mark_all_layers_dirty(&mut self) {
        self.dirty_layers.fill(true);
        self.detail_dirty_layers.fill(true);
        self.navigation_dirty_layers.fill(true);
        self.overlay_revision = self.overlay_revision.wrapping_add(1);
    }
}

impl CalibRawApp {
    pub(crate) fn reset_masks(&mut self) {
        let masks_changed = !self.masks.stack.masks.is_empty()
            || !self.masks.stack.subject_refinement.is_empty()
            || self.masks.stack.scene_depth.is_some();

        self.finish_mask_geometry_interaction();
        self.masks.stack.clear();
        self.masks.reset_transient_state();
        self.develop_ui.mask_section = MaskSection::Properties;

        self.invalidate_generated_mask_sources();
        self.reset_ai_update_state();
        self.ai.update_needed = false;
        self.ai.object_error_dialog = None;

        if masks_changed {
            self.mark_all_mask_layers_dirty();
        }
        self.egui_ctx.request_repaint();
    }

    pub(crate) fn mark_mask_adjustments_dirty(&mut self) {
        self.note_mask_edit_changed();
        if self.preview.gpu_pipeline.is_none() {
            return;
        }
        self.queue_preview_processing(ProcessingStage::Output);
    }

    pub(crate) fn mark_mask_geometry_dirty(&mut self, layer: usize) {
        self.masks.mark_geometry_dirty(layer);
        self.mark_mask_adjustments_dirty();
    }

    pub(crate) fn note_mask_geometry_interaction(&mut self, layer: usize) {
        if self.masks.interaction_dirty_layer != Some(layer) {
            self.finish_mask_geometry_interaction();
            self.masks.interaction_dirty_layer = Some(layer);
            self.masks.interaction_last_upload = None;
        }

        // Live coverage follows every edit, independently of throttled uploads.
        self.masks.overlay_revision = self.masks.overlay_revision.wrapping_add(1);
        self.masks.interaction_has_uncommitted_change = true;
        self.flush_mask_geometry_interaction();
    }

    pub(crate) fn note_subject_refinement_interaction(&mut self) {
        if self.masks.interaction_dirty_layer != Some(SHARED_REFINEMENT_LAYER) {
            self.finish_mask_geometry_interaction();
            self.masks.interaction_dirty_layer = Some(SHARED_REFINEMENT_LAYER);
            self.masks.interaction_last_upload = None;
        }

        // Shared refinement coverage must also update between upload commits.
        self.masks.overlay_revision = self.masks.overlay_revision.wrapping_add(1);
        self.masks.interaction_has_uncommitted_change = true;
        self.flush_mask_geometry_interaction();
    }

    pub(crate) fn flush_mask_geometry_interaction(&mut self) {
        if !self.masks.interaction_has_uncommitted_change {
            return;
        }
        let Some(layer) = self.masks.interaction_dirty_layer else {
            return;
        };
        let now = Instant::now();
        let interval = if self.defer_background_mask_processing() {
            Duration::from_millis(16)
        } else {
            INTERACTIVE_MASK_INTERVAL
        };
        let remaining = self
            .masks
            .interaction_last_upload
            .map_or(Duration::ZERO, |last| {
                interval.saturating_sub(now.saturating_duration_since(last))
            });
        if !remaining.is_zero() {
            self.egui_ctx.request_repaint_after(remaining);
            return;
        }
        if layer == SHARED_REFINEMENT_LAYER {
            self.mark_all_mask_layers_dirty();
        } else {
            self.mark_mask_geometry_dirty(layer);
        }
        self.masks.interaction_last_upload = Some(now);
        self.masks.interaction_has_uncommitted_change = false;
    }

    pub(crate) fn finish_mask_geometry_interaction(&mut self) {
        let layer = self.masks.interaction_dirty_layer.take();
        let should_refine = self.preview.detail.as_ref().is_some_and(|detail| {
            detail.mask_texture_extent
                != crate::pipeline::mask_region_texture_extent(
                    detail.mask_source_region,
                    detail.pipeline.gpu().mask_atlas_edge(),
                )
        });
        let should_commit = self.masks.interaction_has_uncommitted_change || should_refine;
        self.masks.interaction_last_upload = None;
        self.masks.interaction_has_uncommitted_change = false;
        if should_commit {
            if let Some(layer) = layer {
                if layer == MAX_LOCAL_MASKS {
                    self.mark_all_mask_layers_dirty();
                } else {
                    self.mark_mask_geometry_dirty(layer);
                }
            }
        }
    }

    pub(crate) fn begin_mask_touch_gesture(&mut self, mask_index: usize, component_index: usize) {
        if self.masks.touch_gesture_backup.is_some() {
            return;
        }
        let Some(geometry) = self
            .masks
            .stack
            .masks
            .get(mask_index)
            .and_then(|mask| mask.components.get(component_index))
            .map(|component| component.geometry.clone())
        else {
            return;
        };
        self.masks.touch_gesture_backup = Some(MaskTouchGestureBackup {
            mask_index,
            component_index,
            geometry,
            subject_refinement: self
                .masks
                .subject_refinement_active
                .then(|| self.masks.stack.subject_refinement.clone()),
            object_cache: self.ai.object_cache.clone(),
        });
    }

    pub(crate) fn commit_mask_touch_gesture(&mut self) {
        self.masks.touch_gesture_backup = None;
    }

    pub(crate) fn cancel_mask_touch_gesture(&mut self) {
        let Some(backup) = self.masks.touch_gesture_backup.take() else {
            self.masks.last_brush_point = None;
            self.masks.drag = None;
            return;
        };
        let restored = self
            .masks
            .stack
            .masks
            .get_mut(backup.mask_index)
            .and_then(|mask| mask.components.get_mut(backup.component_index))
            .is_some_and(|component| {
                component.geometry = backup.geometry;
                true
            });
        let refinement_restored = if let Some(subject_refinement) = backup.subject_refinement {
            self.masks.stack.subject_refinement = subject_refinement;
            true
        } else {
            false
        };
        self.ai.object_cache = backup.object_cache;
        self.cancel_foreground_operation_if(ForegroundOperationKind::Ai(
            calibraw_ai::AiFeature::Object,
        ));
        self.masks.last_brush_point = None;
        self.masks.drag = None;
        self.masks.interaction_dirty_layer = None;
        self.masks.interaction_last_upload = None;
        self.masks.interaction_has_uncommitted_change = false;
        if refinement_restored {
            self.mark_all_mask_layers_dirty();
        } else if restored {
            self.mark_mask_geometry_dirty(backup.mask_index);
        }
    }

    pub(crate) fn mark_all_mask_layers_dirty(&mut self) {
        self.masks.mark_all_layers_dirty();
        self.mark_mask_adjustments_dirty();
    }

    pub(crate) fn sync_selected_mask_tool(&mut self) {
        self.masks.thumbnail_component_mask = None;
        let kind = self
            .masks
            .stack
            .selected_component()
            .map(|component| component.kind);
        if let Some(kind) = kind {
            self.select_mask_tool(kind);
        } else {
            self.masks.active_tool = None;
        }
    }

    pub(crate) fn activate_mask_tool(&mut self, kind: MaskKind) {
        self.select_mask_tool(kind);
        if matches!(kind, MaskKind::Brush | MaskKind::Object) {
            self.masks.brush_mode = BrushMode::Paint;
        }
    }

    pub(crate) fn select_mask_tool(&mut self, kind: MaskKind) {
        self.finish_mask_geometry_interaction();
        self.masks.active_tool =
            (kind.is_available() && kind != MaskKind::Fullscreen).then_some(kind);
        self.masks.drag = None;
        self.masks.last_brush_point = None;
        self.masks.touch_gesture_backup = None;
        if !matches!(kind, MaskKind::Subject | MaskKind::Background) {
            self.masks.subject_refinement_active = false;
        }
    }

    pub(crate) fn blink_selected_mask(&mut self) {
        self.masks.overlay_blink = Some((std::time::Instant::now(), MaskOverlayBlink::GroupTwice));
        self.egui_ctx.request_repaint();
    }

    pub(crate) fn blink_selected_component(&mut self) {
        self.masks.overlay_blink = Some((
            std::time::Instant::now(),
            MaskOverlayBlink::ComponentThenGroup,
        ));
        self.egui_ctx.request_repaint();
    }

    /// The image content results were made from changed. Results stay in place
    /// so edits keep rendering, but they are marked for an update and running
    /// content jobs stop because their output would already be stale.
    pub(in crate::app) fn invalidate_generated_mask_sources(&mut self) {
        self.masks.source_cache = None;
        self.masks.clear_generated_caches();
        self.ai.object_cache = None;
        if self.content_job_active() {
            self.cancel_foreground_operation();
        }
        self.ai.object_pending_target = None;
        self.ai.update = None;
    }

    pub(crate) fn note_mask_source_changed(&mut self) {
        self.invalidate_generated_mask_sources();
        self.ai.update_needed = !self.masks.stack.content_dependencies().is_empty();
    }

    pub(crate) fn note_lens_correction_changed_for_masks(&mut self) {
        self.note_mask_source_changed();
        self.masks.overlay_revision = self.masks.overlay_revision.wrapping_add(1);
    }

    #[cfg(not(target_os = "android"))]
    pub(crate) fn validate_onnx_runtime_for_ai(&mut self) -> bool {
        if self.ai.runtime_mode == OnnxRuntimeMode::Automatic {
            return true;
        }
        let (Some(runtime_path), Some(runtime_sha256)) =
            (self.ai.runtime_path.clone(), self.ai.runtime_sha256.clone())
        else {
            self.ui.notice = Some(
                "Manual ONNX Runtime mode requires a shared library under Settings. Select one or switch to Automatic."
                    .to_owned(),
            );
            return false;
        };
        match calibraw_ai::ai_masks::probe_runtime_subprocess(&runtime_path, &runtime_sha256) {
            Ok(()) => true,
            Err(error) => {
                self.ui.notice = Some(format!(
                    "ONNX Runtime validation failed: {error:#}. Select a different onnxruntime.dll in Settings."
                ));
                false
            }
        }
    }

    pub(in crate::app) fn ai_runtime_ready(&mut self) -> bool {
        #[cfg(target_os = "android")]
        {
            true
        }
        #[cfg(not(target_os = "android"))]
        {
            self.validate_onnx_runtime_for_ai()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mask_state() -> MaskState {
        MaskState {
            stack: MaskStack::default(),
            active_tool: None,
            brush_mode: BrushMode::default(),
            subject_refinement_active: false,
            drag: None,
            last_brush_point: None,
            touch_gesture_backup: None,
            interaction_dirty_layer: None,
            interaction_last_upload: None,
            interaction_has_uncommitted_change: false,
            overlay_revision: 10,
            overlay_texture: None,
            overlay_texture_key: None,
            overlay_blink: None,
            thumbnail_revision: 0,
            thumbnail_group_textures: Vec::new(),
            thumbnail_component_mask: None,
            thumbnail_component_textures: Vec::new(),
            source_cache: None,
            subject_cache: None,
            sky_cache: None,
            depth_cache: None,
            dirty_layers: [false; MAX_LOCAL_MASKS],
            detail_dirty_layers: [false; MAX_LOCAL_MASKS],
            navigation_dirty_layers: [false; MAX_LOCAL_MASKS],
        }
    }

    #[test]
    fn scene_depth_without_selection_is_dirty_persisted_and_reused() {
        let mut app = CalibRawApp::empty(&egui::Context::default());
        app.masks
            .stack
            .global_effects
            .push(crate::pipeline::EffectComponent::new(
                crate::pipeline::MaskEffect::Fog,
            ));
        app.reset_edit_history();
        app.masks.dirty_layers.fill(false);
        app.masks.detail_dirty_layers.fill(false);
        app.masks.navigation_dirty_layers.fill(false);
        let revision = app.masks.overlay_revision;
        let depth = MaskImage::new(2, 2, vec![0, 85, 170, 255]).unwrap();

        app.apply_generated_mask(AiMaskModel::Depth, depth.clone());
        assert!(app.masks.stack.masks.is_empty());
        assert_eq!(app.masks.stack.scene_depth.as_ref(), Some(&depth));
        assert_eq!(app.masks.overlay_revision, revision.wrapping_add(1));
        assert!(app.masks.dirty_layers.iter().all(|dirty| *dirty));
        assert!(app.masks.detail_dirty_layers.iter().all(|dirty| *dirty));
        assert!(app.masks.navigation_dirty_layers.iter().all(|dirty| *dirty));
        app.commit_edit_history_now();
        let committed = app.committed_mask_state_for_persistence();
        assert_eq!(committed.scene_depth.as_ref(), Some(&depth));

        let edits = crate::sidecar::EditState {
            masks: committed,
            ..crate::sidecar::default_edit_state()
        };
        let restored = crate::sidecar::decode(&crate::sidecar::encode(edits).unwrap()).unwrap();
        app.masks.reset_transient_state();
        app.masks.stack = (*restored.edits.masks).clone();
        app.rehydrate_restored_mask_state();
        assert_eq!(app.masks.depth_cache.as_ref(), Some(&depth));
        assert!(app.masks.stack.masks.is_empty());

        // A later depth selection reuses fog's persisted map without inference.
        app.masks.stack.add_mask(MaskKind::DepthRange).unwrap();
        app.request_depth_mask(&eframe::Frame::_new_kittest());
        assert!(app
            .masks
            .stack
            .selected_component()
            .unwrap()
            .geometry
            .is_initialized());
        assert!(!app.foreground_operation_active());
        assert!(app.ai.consent.is_none());
        assert!(app.masks.source_cache.is_none());
    }

    #[test]
    fn scene_depth_reset_is_persisted_without_selections() {
        let mut app = CalibRawApp::empty(&egui::Context::default());
        app.masks.stack.scene_depth = Some(MaskImage::new(2, 2, vec![0, 85, 170, 255]).unwrap());
        app.reset_edit_history();
        assert!(app
            .committed_mask_state_for_persistence()
            .scene_depth
            .is_some());

        app.reset_masks();
        app.commit_edit_history_now();
        assert!(app
            .committed_mask_state_for_persistence()
            .scene_depth
            .is_none());
        assert!(app.masks.depth_cache.is_none());
    }

    #[test]
    fn scene_depth_cache_prefers_shared_map_and_reuses_legacy_depth() {
        let mut state = mask_state();
        let legacy = MaskImage::new(2, 2, vec![0, 85, 170, 255]).unwrap();
        state.stack.add_mask(MaskKind::DepthRange).unwrap();
        if let MaskGeometry::DepthRange { depth, .. } =
            &mut state.stack.selected_component_mut().unwrap().geometry
        {
            *depth = Some(legacy.clone());
        }
        state.restore_generated_caches();
        assert_eq!(state.depth_cache.as_ref(), Some(&legacy));

        let scene = MaskImage::new(2, 2, vec![255, 170, 85, 0]).unwrap();
        state.stack.scene_depth = Some(scene.clone());
        state.restore_generated_caches();
        assert_eq!(state.depth_cache.as_ref(), Some(&scene));
        assert!(Arc::ptr_eq(
            &state.depth_cache.as_ref().unwrap().pixels,
            &state.stack.scene_depth.as_ref().unwrap().pixels,
        ));
    }

    #[test]
    fn generated_caches_follow_sidecar_restore_source_changes_and_reset() {
        let ctx = egui::Context::default();
        let mut app = CalibRawApp::empty(&ctx);
        for kind in [
            MaskKind::Subject,
            MaskKind::Background,
            MaskKind::Sky,
            MaskKind::DepthRange,
        ] {
            app.masks.stack.add_mask(kind).unwrap();
        }
        let subject = MaskImage::new(2, 2, vec![0, 255, 255, 0]).unwrap();
        let sky = MaskImage::new(2, 2, vec![255, 255, 0, 0]).unwrap();
        let depth = MaskImage::new(2, 2, vec![0, 85, 170, 255]).unwrap();
        app.apply_generated_mask(AiMaskModel::Subject, subject.clone());
        app.apply_generated_mask(AiMaskModel::Sky, sky.clone());
        app.apply_generated_mask(AiMaskModel::Depth, depth.clone());
        assert_eq!(app.masks.subject_cache.as_ref(), Some(&subject));
        assert_eq!(app.masks.sky_cache.as_ref(), Some(&sky));
        assert_eq!(app.masks.depth_cache.as_ref(), Some(&depth));

        let edits = crate::sidecar::EditState {
            masks: Arc::new(app.masks.stack.clone()),
            ..crate::sidecar::default_edit_state()
        };
        let loaded = crate::sidecar::decode(&crate::sidecar::encode(edits).unwrap()).unwrap();
        app.masks.reset_transient_state();
        assert!(app.masks.subject_cache.is_none());
        assert!(app.masks.sky_cache.is_none());
        assert!(app.masks.depth_cache.is_none());
        app.masks.stack = (*loaded.edits.masks).clone();
        app.rehydrate_restored_mask_state();
        assert_eq!(app.masks.subject_cache.as_ref(), Some(&subject));
        assert_eq!(app.masks.sky_cache.as_ref(), Some(&sky));
        assert_eq!(app.masks.depth_cache.as_ref(), Some(&depth));
        if let MaskGeometry::DepthRange {
            depth: Some(restored),
            ..
        } = &app.masks.stack.masks[3].components[0].geometry
        {
            assert!(Arc::ptr_eq(
                &restored.pixels,
                &app.masks.depth_cache.as_ref().unwrap().pixels
            ));
        } else {
            panic!("depth was not restored");
        }

        // Cached selections must work without a preview, model, or runtime.
        let frame = eframe::Frame::_new_kittest();
        for kind in [MaskKind::Subject, MaskKind::Sky, MaskKind::DepthRange] {
            app.masks.stack.add_mask(kind).unwrap();
            match kind {
                MaskKind::Subject => app.request_subject_mask(&frame),
                MaskKind::Sky => app.request_sky_mask(&frame),
                MaskKind::DepthRange => app.request_depth_mask(&frame),
                _ => unreachable!(),
            }
            assert!(app
                .masks
                .stack
                .selected_component()
                .unwrap()
                .geometry
                .is_initialized());
            assert!(!app.foreground_operation_active());
            assert!(app.ai.consent.is_none());
            assert!(app.masks.source_cache.is_none());
        }

        app.note_mask_source_changed();
        assert!(app.ai.update_needed);
        assert!(app.masks.subject_cache.is_none());
        assert!(app.masks.sky_cache.is_none());
        assert!(app.masks.depth_cache.is_none());
        app.rehydrate_restored_mask_state();
        assert!(app.masks.subject_cache.is_none());
        assert!(app.masks.sky_cache.is_none());
        assert!(app.masks.depth_cache.is_none());
    }

    #[test]
    fn geometry_invalidation_marks_every_preview_for_the_layer() {
        let mut state = mask_state();
        state.mark_geometry_dirty(3);
        assert!(state.dirty_layers[3]);
        assert!(state.detail_dirty_layers[3]);
        assert!(state.navigation_dirty_layers[3]);
        assert_eq!(state.overlay_revision, 11);
        assert!(!state.dirty_layers[2]);
    }

    #[test]
    fn full_invalidation_marks_all_mask_previews() {
        let mut state = mask_state();
        state.mark_all_layers_dirty();
        assert!(state.dirty_layers.iter().all(|dirty| *dirty));
        assert!(state.detail_dirty_layers.iter().all(|dirty| *dirty));
        assert!(state.navigation_dirty_layers.iter().all(|dirty| *dirty));
        assert_eq!(state.overlay_revision, 11);
    }

    #[test]
    fn ai_mask_update_resolves_identical_objects_in_copied_masks() {
        let mut state = mask_state();
        state.stack.add_mask(MaskKind::Subject);
        state
            .stack
            .add_component(MaskKind::Object, crate::pipeline::MaskCombineMode::Add);
        assert!(state.stack.duplicate_mask(0, true));

        // Both copies can produce the same pixels on every refresh. Updating
        // the first must not make the second impossible to resolve either.
        for _ in 0..2 {
            for mask_index in 0..2 {
                let target = state.capture_ai_target(mask_index, 1).unwrap();
                assert_eq!(state.resolve_ai_target(&target), Ok((mask_index, 1)));
                let MaskGeometry::Object { mask, .. } =
                    &mut state.stack.masks[mask_index].components[1].geometry
                else {
                    panic!("expected an object mask");
                };
                *mask = MaskImage::new(2, 2, vec![0, 255, 255, 0]);
            }
        }
    }

    #[test]
    fn ai_mask_update_resolves_identical_components_in_the_same_mask() {
        let mut state = mask_state();
        state.stack.add_mask(MaskKind::Object);
        assert!(state.stack.duplicate_component(0, 0, true));

        for component_index in 0..2 {
            let target = state.capture_ai_target(0, component_index).unwrap();
            assert_eq!(state.resolve_ai_target(&target), Ok((0, component_index)));
        }
    }

    #[test]
    fn ai_mask_update_rejects_ambiguous_targets_after_reordering() {
        let mut state = mask_state();
        state.stack.add_mask(MaskKind::Object);
        let target = state.capture_ai_target(0, 0).unwrap();
        assert!(state.stack.duplicate_component(0, 0, false));
        state
            .stack
            .add_component(MaskKind::Brush, crate::pipeline::MaskCombineMode::Add);
        assert_eq!(state.stack.move_submask_component(0, 2, 0, 0), Some((0, 0)));

        assert!(state
            .resolve_ai_target(&target)
            .unwrap_err()
            .contains("ambiguous"));
    }
}
