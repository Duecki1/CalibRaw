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
        self.ui.error_dialogs.dismiss_kind(ErrorKind::AiMask);

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
        let interval = if self.defer_background_preview_processing() {
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
        self.ai.people.forget();
        if self.content_job_active() {
            self.cancel_foreground_operation();
        }
        self.ai.object_pending_target = None;
        self.ai.people.pending.clear();
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

    pub(crate) fn validate_onnx_runtime_for_ai(&mut self) -> bool {
        if self.ai.runtime_mode == OnnxRuntimeMode::Automatic {
            return true;
        }
        let Some((runtime_path, runtime_sha256)) = self.ai.manual_runtime.clone() else {
            self.report_error(ErrorKind::AiRuntime, "Manual ONNX Runtime mode requires a shared library under Settings. Select one or switch to Automatic."
                    .to_owned(),);
            return false;
        };
        match calibraw_ai::runtime::probe_runtime_subprocess(&runtime_path, &runtime_sha256) {
            Ok(()) => true,
            Err(error) => {
                self.report_error(ErrorKind::AiRuntime, format!(
                    "ONNX Runtime validation failed: {error:#}. Select a different onnxruntime.dll in Settings."
                ));
                false
            }
        }
    }

    pub(in crate::app) fn ai_runtime_ready(&mut self) -> bool {
        !self.caps().configurable_ai_runtime || self.validate_onnx_runtime_for_ai()
    }
}
