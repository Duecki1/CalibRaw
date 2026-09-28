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
        self.request_generated_mask(AiMaskModel::Subject, frame);
    }

    pub(crate) fn request_sky_mask(&mut self, frame: &eframe::Frame) {
        self.request_generated_mask(AiMaskModel::Sky, frame);
    }

    pub(crate) fn request_depth_mask(&mut self, frame: &eframe::Frame) {
        self.request_generated_mask(AiMaskModel::Depth, frame);
    }

    fn request_generated_mask(&mut self, model: AiMaskModel, frame: &eframe::Frame) {
        if self.foreground_operation_active() {
            self.ui.notice =
                Some("Finish or cancel the current editing operation first.".to_owned());
            return;
        }
        if let Some(mask) = self.masks.generated_cache_mut(model).clone() {
            self.apply_generated_mask(model, mask);
            return;
        }
        if !self.ai_runtime_ready() {
            return;
        }
        if let Err(error) = self.capture_mask_source(frame) {
            self.report_ai_mask_error(error);
            return;
        }
        self.prepare_generated_mask(model);
    }

    fn generated_model_path(&self, model: AiMaskModel) -> PathBuf {
        match model {
            AiMaskModel::Subject => self.birefnet_model_path(),
            AiMaskModel::Sky => self.skyseg_model_path(),
            AiMaskModel::Depth => self.da3_model_path(),
        }
    }

    pub(super) fn generated_model_is_verified(
        &self,
        model: AiMaskModel,
        path: &std::path::Path,
    ) -> bool {
        match model {
            AiMaskModel::Subject => {
                crate::ai_masks::birefnet_model_is_verified(self.ai.birefnet_quality, path)
            }
            AiMaskModel::Sky => crate::ai_masks::skyseg_model_is_verified(path),
            AiMaskModel::Depth => crate::ai_masks::da3_model_is_verified(path),
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
            (AiMaskModel::Depth, true) => "Running Depth Anything 3 locally…".to_owned(),
            (AiMaskModel::Depth, false) => "Preparing Depth Anything 3 download…".to_owned(),
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
                    .send(AiMaskEvent::Finished(Ok(crate::ai_masks::AiMaskResult {
                        width: 2,
                        height: 2,
                        mask: pixels.clone(),
                    })))
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
