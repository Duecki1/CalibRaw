use super::*;

impl CalibRawApp {
    #[cfg(not(target_os = "android"))]
    pub(crate) fn set_birefnet_quality(&mut self, quality: BiRefNetQuality) {
        if self.ai.birefnet_quality == quality {
            return;
        }
        self.ai.birefnet_quality = quality;
        self.masks.subject_cache = None;
        self.persist_performance_settings();
    }

    #[cfg(not(target_os = "android"))]
    pub(crate) fn set_subject_crop_refinement(&mut self, enabled: bool) {
        if self.ai.subject_crop_refinement == enabled {
            return;
        }
        self.ai.subject_crop_refinement = enabled;
        self.masks.subject_cache = None;
        self.persist_performance_settings();
    }

    pub(crate) fn birefnet_quality_change_enabled(&self) -> bool {
        !matches!(
            self.foreground_operation_kind(),
            Some(
                ForegroundOperationKind::SubjectMask
                    | ForegroundOperationKind::SkyMask
                    | ForegroundOperationKind::DepthMask
            )
        )
    }

    pub(crate) fn request_subject_mask(&mut self, frame: &eframe::Frame) {
        self.ai.object_error_dialog = None;
        let _ = self.request_generated_mask(AiMaskModel::Subject, frame);
    }

    pub(crate) fn request_sky_mask(&mut self, frame: &eframe::Frame) {
        let _ = self.request_generated_mask(AiMaskModel::Sky, frame);
    }

    pub(crate) fn request_depth_mask(&mut self, frame: &eframe::Frame) {
        let _ = self.request_generated_mask(AiMaskModel::Depth, frame);
    }

    /// Ensures active fog with depth enabled has shared scene depth.
    /// A cancelled consent prompt is latched until fog is removed/disabled, depth becomes
    /// available, or the source changes, so the dialog is not reopened every frame.
    pub(in crate::app) fn ensure_fog_scene_depth(&mut self, frame: &eframe::Frame) {
        // A DepthRange fallback can still contain the previous source's depth.
        // Fresh inference fills the cache even if other masks still need updating.
        let depth_is_stale = self.ai.masks_need_update && self.masks.depth_cache.is_none();
        let needs_depth = self.masks.stack.has_depth_fog_effect()
            && (self.masks.stack.scene_depth_image().is_none() || depth_is_stale);
        if !needs_depth {
            self.masks.fog_depth_auto_requested = false;
            return;
        }
        if self.masks.fog_depth_auto_requested
            || self.foreground_operation_active()
            || self.ai.consent.is_open()
        {
            return;
        }

        self.masks.fog_depth_auto_requested =
            self.request_generated_mask(AiMaskModel::Depth, frame);
        if !self.masks.fog_depth_auto_requested {
            self.disable_fog_depth();
        }
    }

    fn disable_fog_depth(&mut self) {
        let mut changed = false;
        let mut disable = |component: &mut crate::pipeline::EffectComponent| {
            if component.effect == crate::pipeline::MaskEffect::Fog
                && component.is_active()
                && component.settings.fog.depth_enabled
            {
                component.settings.fog.depth_enabled = false;
                changed = true;
            }
        };
        for component in &mut self.masks.stack.global_effects {
            disable(component);
        }
        for mask in &mut self.masks.stack.masks {
            if !mask.enabled || mask.opacity <= 0.0 {
                continue;
            }
            for component in &mut mask.effect_components {
                disable(component);
            }
            let mut legacy = crate::pipeline::EffectComponent {
                effect: mask.effect,
                enabled: true,
                settings: mask.effect_settings,
            };
            disable(&mut legacy);
            mask.effect_settings = legacy.settings;
        }
        self.masks.fog_depth_auto_requested = false;
        if changed {
            self.mark_all_mask_layers_dirty();
        }
    }

    fn request_generated_mask(&mut self, model: AiMaskModel, frame: &eframe::Frame) -> bool {
        if self.foreground_operation_active() {
            self.ui.notice =
                Some("Finish or cancel the current editing operation first.".to_owned());
            return false;
        }
        if let Some(mask) = self.masks.generated_cache_mut(model).clone() {
            self.apply_generated_mask(model, mask);
            return true;
        }
        if !self.ai_runtime_ready() {
            return false;
        }
        if let Err(error) = self.capture_mask_source(frame) {
            self.report_ai_mask_error(error);
            return false;
        }
        self.prepare_generated_mask(model);
        true
    }

    fn generated_model_path(&self, model: AiMaskModel) -> PathBuf {
        match model {
            AiMaskModel::Subject => self.birefnet_model_path(),
            AiMaskModel::Sky => self.skyseg_model_path(),
            AiMaskModel::Depth => self.depth_model_path(),
        }
    }

    pub(super) fn generated_model_is_verified(
        &self,
        model: AiMaskModel,
        path: &std::path::Path,
    ) -> bool {
        match model {
            AiMaskModel::Subject => {
                calibraw_ai::ai_masks::birefnet_model_is_verified(self.ai.birefnet_quality, path)
            }
            AiMaskModel::Sky => calibraw_ai::ai_masks::skyseg_model_is_verified(path),
            AiMaskModel::Depth => calibraw_ai::ai_masks::depth_model_is_verified(path),
        }
    }

    // Shared by new selections and explicit refreshes. Refreshes deliberately
    // bypass the result cache after capturing the current image source.
    pub(super) fn prepare_generated_mask(&mut self, model: AiMaskModel) {
        let path = self.generated_model_path(model);
        let runtime_download_needed = self.automatic_onnx_runtime_download_needed();
        let model_download_needed = !self.generated_model_is_verified(model, &path);
        if !model_download_needed && !runtime_download_needed {
            self.ai.consent = AiConsentState::None;
            self.start_generated_mask_worker(model, false);
        } else {
            self.ai.consent = match model {
                AiMaskModel::Subject => AiConsentState::Subject {
                    runtime_download_needed,
                },
                AiMaskModel::Sky => AiConsentState::Sky {
                    runtime_download_needed,
                },
                AiMaskModel::Depth => AiConsentState::Depth {
                    runtime_download_needed,
                    model_download_needed,
                },
            };
            self.egui_ctx.request_repaint();
        }
    }

    fn generated_mask_progress(&self, model: AiMaskModel, inferencing: bool) -> String {
        match (model, inferencing) {
            (AiMaskModel::Subject, true) => format!(
                "Running {} quality locally with {}…",
                self.ai.birefnet_quality.label(),
                self.ai.birefnet_quality.model().checkpoint
            ),
            (AiMaskModel::Subject, false) => format!(
                "Preparing {} download…",
                self.ai.birefnet_quality.model().download_label
            ),
            (AiMaskModel::Sky, true) => "Running SkySeg U2Net locally…".to_owned(),
            (AiMaskModel::Sky, false) => "Preparing SkySeg U2Net download…".to_owned(),
            (AiMaskModel::Depth, true) => {
                format!(
                    "Running {} locally…",
                    calibraw_ai::ai_masks::DEPTH_MODEL.name
                )
            }
            (AiMaskModel::Depth, false) => {
                format!(
                    "Preparing {} download…",
                    calibraw_ai::ai_masks::DEPTH_MODEL.name
                )
            }
        }
    }

    pub(super) fn start_generated_mask_worker(&mut self, model: AiMaskModel, allow_download: bool) {
        if self.foreground_operation_active() {
            return;
        }
        let Some(source) = self.masks.source_cache.clone() else {
            self.ui.notice = Some(format!(
                "The preview could not be prepared for {} selection.",
                model.noun()
            ));
            return;
        };
        let model_path = self.generated_model_path(model);
        let model_present = self.generated_model_is_verified(model, &model_path);
        #[cfg(not(target_os = "android"))]
        let (runtime_path, runtime_sha256) = self.onnx_runtime_for_ai();
        #[cfg(target_os = "android")]
        let (runtime_path, runtime_sha256) = (None, None);
        #[cfg(not(target_os = "android"))]
        let crop_refinement = model == AiMaskModel::Subject && self.ai.subject_crop_refinement;
        #[cfg(target_os = "android")]
        let crop_refinement = false;
        let cancellation = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let receiver = spawn_ai_mask(
            AiMaskWorkerRequest {
                model,
                quality: self.ai.birefnet_quality,
                crop_refinement,
                model_path,
                allow_download,
                runtime_path,
                runtime_sha256,
                width: source.width,
                height: source.height,
                rgba: source.rgba.to_vec(),
            },
            Arc::clone(&cancellation),
        );
        self.begin_foreground_operation(ForegroundOperation {
            kind: match model {
                AiMaskModel::Subject => ForegroundOperationKind::SubjectMask,
                AiMaskModel::Sky => ForegroundOperationKind::SkyMask,
                AiMaskModel::Depth => ForegroundOperationKind::DepthMask,
            },
            document_id: self.persistence.sidecar_generation,
            cancellation,
            progress: ForegroundProgress::indeterminate(
                self.generated_mask_progress(model, model_present),
            ),
            cancelling: false,
            receiver: ForegroundOperationReceiver::AiMask(receiver),
            context: ForegroundOperationContext::AiMask,
        });
    }

    pub(super) fn apply_generated_mask(&mut self, model: AiMaskModel, mask: MaskImage) {
        *self.masks.generated_cache_mut(model) = Some(mask.clone());
        if model == AiMaskModel::Depth {
            self.masks.stack.scene_depth = Some(mask.clone());
        }
        for component in self
            .masks
            .stack
            .masks
            .iter_mut()
            .flat_map(|mask| &mut mask.components)
        {
            if generated_mask_model(component.kind) == Some(model) {
                match &mut component.geometry {
                    MaskGeometry::Ai { mask: target, .. }
                    | MaskGeometry::DepthRange { depth: target, .. } => {
                        *target = Some(mask.clone())
                    }
                    _ => {}
                }
            }
        }
        self.mark_all_mask_layers_dirty();
        self.blink_selected_mask();
    }

    pub(in crate::app) fn poll_ai_mask_worker(&mut self) {
        if !matches!(
            self.foreground_operation_kind(),
            Some(
                ForegroundOperationKind::SubjectMask
                    | ForegroundOperationKind::SkyMask
                    | ForegroundOperationKind::DepthMask
            )
        ) {
            return;
        }
        let Some(mut operation) = self.foreground_operation.take() else {
            return;
        };
        let model = match operation.kind {
            ForegroundOperationKind::SubjectMask => AiMaskModel::Subject,
            ForegroundOperationKind::SkyMask => AiMaskModel::Sky,
            ForegroundOperationKind::DepthMask => AiMaskModel::Depth,
            _ => {
                self.foreground_operation = Some(operation);
                return;
            }
        };
        let ForegroundOperationReceiver::AiMask(receiver) = &operation.receiver else {
            self.foreground_operation = Some(operation);
            return;
        };
        let (events, disconnected) = drain_worker_events(Some(receiver), |event| {
            matches!(event, AiMaskEvent::Finished(_))
        });

        let mut finished = None;
        for event in events {
            match event {
                AiMaskEvent::DownloadProgress(progress) => {
                    operation.progress = ForegroundProgress::units(
                        progress.downloaded,
                        progress.total,
                        Some("bytes".to_owned()),
                        format!("Downloading {}", progress.label),
                    )
                    .with_detail(format!(
                        "{:.1} / {:.1} MB",
                        progress.downloaded as f64 / 1_000_000.0,
                        progress.total as f64 / 1_000_000.0
                    ));
                }
                AiMaskEvent::Inferencing => {
                    operation.progress = ForegroundProgress::indeterminate(
                        self.generated_mask_progress(model, true),
                    );
                }
                AiMaskEvent::Finished(result) => finished = Some(result),
            }
        }
        if finished.is_none() && disconnected {
            finished = Some(Err(format!(
                "The {}-mask worker stopped unexpectedly.",
                model.noun()
            )));
        }
        let Some(result) = finished else {
            self.foreground_operation = Some(operation);
            return;
        };

        let updating_all = self.ai.mask_update_active
            && (model != AiMaskModel::Subject || self.ai.mask_update_subject_pending);
        let cancelled = operation.is_cancelled();
        let stale = operation.document_id != self.persistence.sidecar_generation;

        let mut succeeded = false;
        let mut error_message = None;
        if !cancelled && !stale {
            match result {
                Ok(result) => {
                    if let Some(mask) = result.into_probability_mask() {
                        self.apply_generated_mask(model, mask);
                        succeeded = true;
                    } else {
                        error_message = Some(format!(
                            "{} selection returned an invalid mask image.",
                            model.label()
                        ));
                    }
                }
                Err(error) => {
                    error_message = Some(format!("{} selection failed: {error}", model.label()))
                }
            }
        }

        if model == AiMaskModel::Depth
            && self.masks.stack.has_depth_fog_effect()
            && !cancelled
            && !stale
            && !succeeded
        {
            self.disable_fog_depth();
        }

        if updating_all {
            if cancelled || stale {
                self.cancel_ai_mask_update();
            } else {
                if model == AiMaskModel::Subject {
                    self.ai.mask_update_subject_pending = false;
                }
                self.ai.mask_update_failed |= !succeeded;
                if let Some(message) = error_message {
                    self.ui.notice = Some(message);
                }
                self.continue_ai_mask_update();
            }
        } else if !cancelled && !succeeded {
            let message = error_message.unwrap_or_else(|| {
                if stale {
                    format!(
                        "{} selection became stale before inference completed.",
                        model.label()
                    )
                } else {
                    format!("{} selection did not produce a mask.", model.label())
                }
            });
            self.ui.notice = Some(message);
        }
        self.egui_ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fog_without_depth_does_not_start_inference() {
        let mut app = CalibRawApp::empty(&egui::Context::default());
        let mut fog = crate::pipeline::EffectComponent::new(crate::pipeline::MaskEffect::Fog);
        fog.settings.fog.depth_enabled = false;
        app.masks.stack.global_effects.push(fog);
        app.ensure_fog_scene_depth(&eframe::Frame::_new_kittest());
        assert!(app.ui.notice.is_none());
        assert!(!app.ai.consent.is_open());
        assert!(!app.foreground_operation_active());
    }

    #[test]
    fn fog_depth_failure_disables_depth_except_for_cancelled_or_stale_jobs() {
        for (cancelled, stale) in [(false, false), (true, false), (false, true)] {
            for failure in 0..3 {
                let mut app = CalibRawApp::empty(&egui::Context::default());
                let fog = crate::pipeline::EffectComponent::new(crate::pipeline::MaskEffect::Fog);
                app.masks.stack.global_effects.push(fog.clone());
                app.masks.stack.add_mask(MaskKind::Fullscreen).unwrap();
                app.masks.stack.masks[0].effect_components.push(fog.clone());
                app.masks.stack.masks[0].effect = crate::pipeline::MaskEffect::Fog;
                app.masks.stack.masks[0].effect_settings = fog.settings;
                app.reset_edit_history();
                // Fog may also be added while a manually requested depth job is running.
                app.masks.fog_depth_auto_requested = failure != 0;
                let (sender, receiver) = std::sync::mpsc::channel();
                match failure {
                    0 => sender
                        .send(AiMaskEvent::Finished(Err("Inference failed".to_owned())))
                        .unwrap(),
                    1 => sender
                        .send(AiMaskEvent::Finished(Ok(
                            calibraw_ai::ai_masks::AiMaskResult {
                                width: 2,
                                height: 2,
                                mask: vec![],
                            },
                        )))
                        .unwrap(),
                    _ => {}
                }
                drop(sender);
                assert!(app.begin_foreground_operation(ForegroundOperation {
                    kind: ForegroundOperationKind::DepthMask,
                    document_id: app
                        .persistence
                        .sidecar_generation
                        .wrapping_add(u64::from(stale)),
                    cancellation: Arc::new(std::sync::atomic::AtomicBool::new(cancelled)),
                    progress: ForegroundProgress::indeterminate("Testing depth failure"),
                    cancelling: false,
                    receiver: ForegroundOperationReceiver::AiMask(receiver),
                    context: ForegroundOperationContext::AiMask,
                }));
                app.poll_ai_mask_worker();
                let depth_enabled = cancelled || stale;
                assert_eq!(
                    app.masks.stack.global_effects[0].settings.fog.depth_enabled,
                    depth_enabled
                );
                assert_eq!(
                    app.masks.stack.masks[0].effect_components[0]
                        .settings
                        .fog
                        .depth_enabled,
                    depth_enabled
                );
                assert_eq!(
                    app.masks.stack.masks[0].effect_settings.fog.depth_enabled,
                    depth_enabled
                );
                assert!(app.masks.stack.has_fog_effect());
                if !depth_enabled {
                    app.commit_edit_history_now();
                    assert!(
                        !app.committed_mask_state_for_persistence().global_effects[0]
                            .settings
                            .fog
                            .depth_enabled
                    );
                    app.ui.notice = None;
                    app.ensure_fog_scene_depth(&eframe::Frame::_new_kittest());
                    assert!(app.ui.notice.is_none());
                }
            }
        }
    }

    #[test]
    fn fog_only_depth_is_invalidated_after_remove_changes() {
        assert_fog_depth_regenerates_after_source_change(false);
    }

    #[test]
    fn fog_depth_range_fallback_is_refreshed_after_lens_changes() {
        assert_fog_depth_regenerates_after_source_change(true);
    }

    fn assert_fog_depth_regenerates_after_source_change(depth_range_fallback: bool) {
        let mut app = CalibRawApp::empty(&egui::Context::default());
        #[cfg(not(target_os = "android"))]
        {
            app.ai.runtime_mode = OnnxRuntimeMode::Automatic;
        }
        app.masks
            .stack
            .global_effects
            .push(crate::pipeline::EffectComponent::new(
                crate::pipeline::MaskEffect::Fog,
            ));
        if depth_range_fallback {
            app.masks.stack.add_mask(MaskKind::DepthRange).unwrap();
        }
        let old_depth = MaskImage::new(2, 2, vec![0, 85, 170, 255]).unwrap();
        app.apply_generated_mask(AiMaskModel::Depth, old_depth.clone());
        if depth_range_fallback {
            app.masks.stack.scene_depth = None;
        }
        app.reset_edit_history();
        // A prior request (including cancelled consent) must not block a new source.
        app.masks.fog_depth_auto_requested = true;

        if depth_range_fallback {
            app.note_lens_correction_changed_for_masks();
        } else {
            app.note_remove_edit_changed();
        }
        assert!(app.masks.stack.scene_depth.is_none());
        assert!(app.masks.depth_cache.is_none());
        assert!(!app.masks.fog_depth_auto_requested);
        assert_eq!(app.ai.masks_need_update, depth_range_fallback);
        app.commit_edit_history_now();
        assert!(app
            .committed_mask_state_for_persistence()
            .scene_depth
            .is_none());
        // Keep the selection usable until regeneration finishes, but do not treat
        // its old depth as current scene data.
        assert_eq!(
            app.masks.stack.scene_depth_image(),
            depth_range_fallback.then_some(&old_depth),
        );

        let frame = eframe::Frame::_new_kittest();
        app.ensure_fog_scene_depth(&frame);
        // A headless frame cannot capture the source. This error proves fog tried
        // to regenerate instead of silently accepting persisted/fallback depth.
        assert_eq!(
            app.ui.notice.as_deref(),
            Some("The GPU preview is not available.")
        );
        assert!(!app.masks.fog_depth_auto_requested);

        assert!(!app.masks.stack.global_effects[0].settings.fog.depth_enabled);
        app.masks.stack.global_effects[0].settings.fog.depth_enabled = true;
        let fresh_depth = MaskImage::new(2, 2, vec![255, 170, 85, 0]).unwrap();
        app.apply_generated_mask(AiMaskModel::Depth, fresh_depth.clone());
        app.ui.notice = None;
        app.ensure_fog_scene_depth(&frame);
        assert!(app.ui.notice.is_none());
        assert!(!app.masks.fog_depth_auto_requested);
        assert_eq!(app.masks.stack.scene_depth_image(), Some(&fresh_depth));
        assert_eq!(app.masks.depth_cache.as_ref(), Some(&fresh_depth));
        assert_eq!(app.ai.masks_need_update, depth_range_fallback);
        if depth_range_fallback {
            assert!(matches!(
                &app.masks.stack.selected_component().unwrap().geometry,
                MaskGeometry::DepthRange { depth: Some(depth), .. } if depth == &fresh_depth
            ));
        } else {
            assert!(app.masks.stack.masks.is_empty());
        }
    }

    #[test]
    fn fog_automatically_reuses_cached_depth_without_a_depth_mask() {
        let mut app = CalibRawApp::empty(&egui::Context::default());
        app.masks
            .stack
            .global_effects
            .push(crate::pipeline::EffectComponent::new(
                crate::pipeline::MaskEffect::Fog,
            ));
        let depth = MaskImage::new(2, 2, vec![0, 85, 170, 255]).unwrap();
        app.masks.depth_cache = Some(depth.clone());
        let frame = eframe::Frame::_new_kittest();

        app.ensure_fog_scene_depth(&frame);

        assert_eq!(app.masks.stack.scene_depth.as_ref(), Some(&depth));
        assert!(app.masks.stack.masks.is_empty());
        assert!(!app.foreground_operation_active());
        assert!(!app.ai.consent.is_open());

        // Once depth is available the one-shot latch is re-armed for a future
        // fog/depth invalidation instead of staying permanently suppressed.
        app.ensure_fog_scene_depth(&frame);
        assert!(!app.masks.fog_depth_auto_requested);
    }

    #[test]
    fn generated_worker_results_route_by_model_and_reject_cancelled_or_stale_jobs() {
        for (model, kind) in [
            (AiMaskModel::Subject, ForegroundOperationKind::SubjectMask),
            (AiMaskModel::Sky, ForegroundOperationKind::SkyMask),
            (AiMaskModel::Depth, ForegroundOperationKind::DepthMask),
        ] {
            for (cancelled, stale) in [(false, false), (true, false), (false, true)] {
                let mut app = CalibRawApp::empty(&egui::Context::default());
                for mask_kind in [
                    MaskKind::Subject,
                    MaskKind::Background,
                    MaskKind::Sky,
                    MaskKind::DepthRange,
                ] {
                    app.masks.stack.add_mask(mask_kind).unwrap();
                }
                let pixels = vec![0, 85, 170, 255];
                let (sender, receiver) = std::sync::mpsc::channel();
                sender.send(AiMaskEvent::Inferencing).unwrap();
                sender
                    .send(AiMaskEvent::Finished(Ok(
                        calibraw_ai::ai_masks::AiMaskResult {
                            width: 2,
                            height: 2,
                            mask: pixels.clone(),
                        },
                    )))
                    .unwrap();
                assert!(app.begin_foreground_operation(ForegroundOperation {
                    kind,
                    document_id: app
                        .persistence
                        .sidecar_generation
                        .wrapping_add(u64::from(stale)),
                    cancellation: Arc::new(std::sync::atomic::AtomicBool::new(cancelled)),
                    progress: ForegroundProgress::indeterminate("Testing mask result"),
                    cancelling: false,
                    receiver: ForegroundOperationReceiver::AiMask(receiver),
                    context: ForegroundOperationContext::AiMask,
                }));
                app.poll_ai_mask_worker();
                assert!(!app.foreground_operation_active());
                let accepted = !cancelled && !stale;
                assert_eq!(
                    app.masks.stack.scene_depth.as_ref(),
                    (accepted && model == AiMaskModel::Depth)
                        .then(|| MaskImage::new(2, 2, pixels.clone()).unwrap())
                        .as_ref()
                );
                assert_eq!(
                    app.masks.generated_cache_mut(model).as_ref(),
                    accepted
                        .then(|| MaskImage::new(2, 2, pixels.clone()).unwrap())
                        .as_ref()
                );
                for component in app
                    .masks
                    .stack
                    .masks
                    .iter()
                    .flat_map(|mask| &mask.components)
                {
                    assert_eq!(
                        component.geometry.is_initialized(),
                        accepted && generated_mask_model(component.kind) == Some(model)
                    );
                }
                for other_model in [AiMaskModel::Subject, AiMaskModel::Sky, AiMaskModel::Depth] {
                    if other_model != model {
                        assert!(app.masks.generated_cache_mut(other_model).is_none());
                    }
                }
            }
        }
    }
}
