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
        self.foreground_operation_kind()
            .and_then(ForegroundOperationKind::ai_feature)
            .and_then(generated_model)
            .is_none()
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

    pub(in crate::app) fn request_generated_mask(
        &mut self,
        model: AiMaskModel,
        frame: &eframe::Frame,
    ) -> bool {
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
        self.request_content_job(generated_feature(model));
        true
    }

    pub(super) fn generated_model_path(&self, model: AiMaskModel) -> PathBuf {
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
            kind: ForegroundOperationKind::Ai(generated_feature(model)),
            document_id: self.persistence.document_generation,
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
        let Some(model) = self
            .foreground_operation_kind()
            .and_then(ForegroundOperationKind::ai_feature)
            .and_then(generated_model)
        else {
            return;
        };
        let Some(mut operation) = self.foreground_operation.take() else {
            return;
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

        let cancelled = operation.is_cancelled();
        let stale = !operation.is_for_document(self.persistence.document_generation);

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

        if self.ai.update.is_some() {
            self.advance_ai_update(succeeded, cancelled || stale, error_message);
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
    use calibraw_ai::AiFeature;

    fn app_with_depth_fog() -> CalibRawApp {
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
        app
    }

    fn assert_no_ai_started(app: &CalibRawApp) {
        assert!(!app.foreground_operation_active());
        assert!(app.ai.consent.is_none());
        assert!(app.ai.update.is_none());
    }

    #[test]
    fn missing_fog_depth_waits_for_the_user_instead_of_running() {
        // As after loading or pasting depth fog onto an image without depth.
        let mut app = app_with_depth_fog();
        let already_required = app.masks.stack.content_dependencies();
        app.request_new_content_dependencies(&already_required, &eframe::Frame::_new_kittest());
        assert_no_ai_started(&app);
        assert!(app.ui.notice.is_none());
        assert!(app.ai_update_needed());
    }

    #[test]
    fn adding_depth_fog_requests_scene_depth() {
        let mut app = app_with_depth_fog();
        let fog = app.masks.stack.global_effects.pop().unwrap();
        let before = app.masks.stack.content_dependencies();
        app.masks.stack.global_effects.push(fog);

        app.request_new_content_dependencies(&before, &eframe::Frame::_new_kittest());
        // A headless frame cannot capture the source; the error proves the
        // edit asked for depth.
        assert_eq!(
            app.ui.notice.as_deref(),
            Some("The GPU preview is not available.")
        );
    }

    #[test]
    fn adding_relight_requests_scene_depth_like_depth_fog() {
        let mut app = app_with_depth_fog();
        app.masks.stack.global_effects.clear();
        let before = app.masks.stack.content_dependencies();
        app.masks
            .stack
            .global_effects
            .push(crate::pipeline::EffectComponent::new(
                crate::pipeline::MaskEffect::Relight,
            ));

        app.request_new_content_dependencies(&before, &eframe::Frame::_new_kittest());
        assert_eq!(
            app.ui.notice.as_deref(),
            Some("The GPU preview is not available.")
        );
    }

    #[test]
    fn adding_depth_fog_reuses_cached_depth_without_inference() {
        let mut app = app_with_depth_fog();
        let fog = app.masks.stack.global_effects.pop().unwrap();
        let depth = MaskImage::new(2, 2, vec![0, 85, 170, 255]).unwrap();
        app.masks.depth_cache = Some(depth.clone());
        let before = app.masks.stack.content_dependencies();
        app.masks.stack.global_effects.push(fog);

        app.request_new_content_dependencies(&before, &eframe::Frame::_new_kittest());
        assert_eq!(app.masks.stack.scene_depth.as_ref(), Some(&depth));
        assert!(app.masks.stack.masks.is_empty());
        assert_no_ai_started(&app);
        assert!(!app.ai_update_needed());
    }

    #[test]
    fn fog_without_depth_needs_no_scene_depth() {
        let mut app = app_with_depth_fog();
        let mut fog = app.masks.stack.global_effects.pop().unwrap();
        let before = app.masks.stack.content_dependencies();
        fog.settings.fog.depth_enabled = false;
        app.masks.stack.global_effects.push(fog);

        app.request_new_content_dependencies(&before, &eframe::Frame::_new_kittest());
        assert_no_ai_started(&app);
        assert!(app.ui.notice.is_none());
        assert!(!app.ai_update_needed());
    }

    #[test]
    fn source_changes_keep_fog_depth_and_mark_it_for_update() {
        for depth_range in [false, true] {
            for lens_change in [false, true] {
                let mut app = app_with_depth_fog();
                if depth_range {
                    app.masks.stack.add_mask(MaskKind::DepthRange).unwrap();
                }
                let depth = MaskImage::new(2, 2, vec![0, 85, 170, 255]).unwrap();
                app.apply_generated_mask(AiMaskModel::Depth, depth.clone());
                app.reset_edit_history();

                if lens_change {
                    app.note_lens_correction_changed_for_masks();
                } else {
                    app.note_remove_edit_changed();
                }

                // Fog keeps rendering with the previous depth until the user updates.
                assert_eq!(app.masks.stack.scene_depth_image(), Some(&depth));
                assert!(app.masks.depth_cache.is_none());
                assert!(app.ai_update_needed());
                assert_no_ai_started(&app);
                assert!(app.ui.notice.is_none());
            }
        }
    }

    #[test]
    fn failed_scene_depth_keeps_fog_depth_enabled_and_updatable() {
        for (cancelled, stale) in [(false, false), (true, false), (false, true)] {
            let mut app = app_with_depth_fog();
            let (sender, receiver) = std::sync::mpsc::channel();
            sender
                .send(AiMaskEvent::Finished(Err("Inference failed".to_owned())))
                .unwrap();
            assert!(app.begin_foreground_operation(ForegroundOperation {
                kind: ForegroundOperationKind::Ai(AiFeature::SceneDepth),
                document_id: app
                    .persistence
                    .document_generation
                    .wrapping_add(u64::from(stale)),
                cancellation: Arc::new(std::sync::atomic::AtomicBool::new(cancelled)),
                progress: ForegroundProgress::indeterminate("Testing depth failure"),
                cancelling: false,
                receiver: ForegroundOperationReceiver::AiMask(receiver),
                context: ForegroundOperationContext::AiMask,
            }));
            app.poll_ai_mask_worker();

            assert!(!app.foreground_operation_active());
            assert!(app.masks.stack.global_effects[0].settings.fog.depth_enabled);
            assert!(app.ai_update_needed());
            assert_eq!(app.ui.notice.is_some(), !cancelled);
        }
    }

    #[test]
    fn generated_worker_results_route_by_model_and_reject_cancelled_or_stale_jobs() {
        for (model, kind) in [
            (AiMaskModel::Subject, AiFeature::Subject),
            (AiMaskModel::Sky, AiFeature::Sky),
            (AiMaskModel::Depth, AiFeature::SceneDepth),
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
                    kind: ForegroundOperationKind::Ai(kind),
                    document_id: app
                        .persistence
                        .document_generation
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
