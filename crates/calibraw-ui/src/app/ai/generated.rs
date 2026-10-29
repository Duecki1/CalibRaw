use super::*;
use crate::i18n::{tr, tr_fmt};

impl CalibRawApp {
    pub(crate) fn set_subject_mask_quality(
        &mut self,
        quality: calibraw_ai::models::subject::SubjectMaskQuality,
    ) {
        if self.ai.subject_mask_quality == quality {
            return;
        }
        self.ai.subject_mask_quality = quality;
        self.masks.subject_cache = None;
        self.persist_performance_settings();
    }

    pub(crate) fn ai_mask_generation_idle(&self) -> bool {
        self.foreground_operation_kind()
            .and_then(ForegroundOperationKind::ai_feature)
            .and_then(generated_model)
            .is_none()
    }

    pub(crate) fn request_subject_mask(&mut self, frame: &eframe::Frame) {
        self.ui.error_dialogs.dismiss_kind(ErrorKind::AiMask);
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
                Some(tr("Finish or cancel the current editing operation first.").to_owned());
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

    /// `subject_precision` comes from the caller, so one job's path, check and
    /// download all name the same subject model file.
    pub(super) fn generated_model_path(
        &self,
        model: AiMaskModel,
        subject_precision: SubjectModelPrecision,
    ) -> PathBuf {
        match model {
            AiMaskModel::Subject => self.birefnet_model_path(subject_precision),
            AiMaskModel::Sky => self.skyseg_model_path(),
            AiMaskModel::Depth => self.depth_model_path(),
        }
    }

    pub(super) fn generated_model_is_verified(
        &self,
        model: AiMaskModel,
        subject_precision: SubjectModelPrecision,
        path: &std::path::Path,
    ) -> bool {
        match model {
            AiMaskModel::Subject => calibraw_ai::models::subject::birefnet_model_is_verified(
                self.ai.subject_mask_quality,
                subject_precision,
                path,
            ),
            AiMaskModel::Sky => calibraw_ai::models::sky::skyseg_model_is_verified(path),
            AiMaskModel::Depth => calibraw_ai::models::depth::depth_model_is_verified(path),
        }
    }

    fn generated_mask_progress(&self, model: AiMaskModel, inferencing: bool) -> String {
        match (model, inferencing) {
            (AiMaskModel::Subject, true) => tr_fmt(
                "Running {name} locally…",
                &[("name", &self.subject_model().name)],
            ),
            (AiMaskModel::Subject, false) => tr_fmt(
                "Preparing {download_label} download…",
                &[("download_label", &self.subject_model().download_label)],
            ),
            (AiMaskModel::Sky, true) => tr("Running SkySeg U2Net locally…").to_owned(),
            (AiMaskModel::Sky, false) => tr("Preparing SkySeg U2Net download…").to_owned(),
            (AiMaskModel::Depth, true) => tr_fmt(
                "Running {name} locally…",
                &[("name", &calibraw_ai::models::depth::MODEL.name)],
            ),
            (AiMaskModel::Depth, false) => tr_fmt(
                "Preparing {name} download…",
                &[("name", &calibraw_ai::models::depth::MODEL.name)],
            ),
        }
    }

    pub(super) fn start_generated_mask_worker(&mut self, model: AiMaskModel, allow_download: bool) {
        if self.foreground_operation_active() {
            return;
        }
        let Some(source) = self.masks.source_cache.clone() else {
            self.report_error(
                ErrorKind::AiMask,
                tr_fmt(
                    "The preview could not be prepared for {model} selection.",
                    &[("model", &model.noun())],
                ),
            );
            return;
        };
        let subject_precision = self.subject_model_precision();
        let model_path = self.generated_model_path(model, subject_precision);
        let model_present = self.generated_model_is_verified(model, subject_precision, &model_path);
        let cancellation = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let receiver = spawn_ai_mask(
            AiMaskWorkerRequest {
                model,
                subject_quality: self.ai.subject_mask_quality,
                subject_precision,
                model_path,
                allow_download,
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
        let finished = poll_ai_job(
            self.platform.as_ref(),
            receiver,
            || {
                tr_fmt(
                    "The {model}-mask worker stopped unexpectedly.",
                    &[("model", &model.noun())],
                )
            },
            |update| {
                operation.progress = match update {
                    AiJobUpdate::Download(progress) => model_download_progress(progress),
                    AiJobUpdate::Progress(AiMaskProgress::Inferencing) => {
                        ForegroundProgress::indeterminate(self.generated_mask_progress(model, true))
                    }
                }
            },
        );
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
                    // Hardened subject alpha is exactly 0 wherever BiRefNet
                    // found nothing, so an all-zero mask means no subject.
                    if model == AiMaskModel::Subject && result.mask.iter().all(|&alpha| alpha == 0)
                    {
                        error_message = Some(
                            tr("Subject selection found no subject in this photo.").to_owned(),
                        );
                    } else if let Some(mask) = result.into_probability_mask() {
                        self.apply_generated_mask(model, mask);
                        succeeded = true;
                    } else {
                        error_message = Some(tr_fmt(
                            "{model} selection returned an invalid mask image.",
                            &[("model", &tr(model.label()))],
                        ));
                    }
                }
                Err(error) => {
                    error_message = Some(tr_fmt(
                        "{model} selection failed: {error}",
                        &[("model", &tr(model.label())), ("error", &error)],
                    ))
                }
            }
        }

        if self.ai.update.is_some() {
            self.advance_ai_update(succeeded, cancelled || stale, error_message);
        } else if !cancelled && !succeeded {
            let message = error_message.unwrap_or_else(|| {
                if stale {
                    tr_fmt(
                        "{model} selection became stale before inference completed.",
                        &[("model", &tr(model.label()))],
                    )
                } else {
                    tr_fmt(
                        "{model} selection did not produce a mask.",
                        &[("model", &tr(model.label()))],
                    )
                }
            });
            self.report_error(ErrorKind::AiMask, message);
        }
        self.egui_ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use calibraw_ai::AiFeature;

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
                sender
                    .send(AiMaskEvent::Progress(AiMaskProgress::Inferencing))
                    .unwrap();
                sender
                    .send(AiMaskEvent::Finished(Ok(
                        calibraw_ai::models::masks::AiMaskResult {
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
