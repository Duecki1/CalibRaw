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

    /// Depth estimated from now on uses `quality`'s normal model; existing
    /// depth keeps its normals.
    pub(crate) fn set_scene_depth_quality(
        &mut self,
        quality: calibraw_ai::models::scene_geometry::SceneDepthQuality,
    ) {
        if self.ai.scene_depth_quality == quality {
            return;
        }
        self.ai.scene_depth_quality = quality;
        self.masks.depth_cache = None;
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
        if let Some(result) = self.masks.generated_cache(model) {
            self.apply_generated_mask(model, result);
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
            AiMaskModel::Depth => {
                calibraw_ai::models::depth::depth_model_is_verified(path)
                    && self.scene_geometry_model().is_verified()
            }
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
                "Running {name} and {normals} locally…",
                &[
                    ("name", &calibraw_ai::models::depth::MODEL.name),
                    ("normals", &self.ai.scene_depth_quality.model().name),
                ],
            ),
            (AiMaskModel::Depth, false) => tr_fmt(
                "Preparing {name} and {normals} download…",
                &[
                    ("name", &calibraw_ai::models::depth::MODEL.name),
                    ("normals", &self.ai.scene_depth_quality.model().name),
                ],
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
                scene_geometry: (model == AiMaskModel::Depth).then(|| self.scene_geometry_model()),
                focal_length_35mm: self.known_focal_length(),
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

    /// Applies a `model` result and keeps it for the open photo: scene depth
    /// for the edit and its Depth Range components, a probability for the
    /// components of `model`. `false`, changing nothing, when the result is
    /// not of `model`'s kind.
    pub(super) fn apply_generated_mask(
        &mut self,
        model: AiMaskModel,
        result: AiMaskResult,
    ) -> bool {
        match (model, result) {
            (AiMaskModel::Depth, AiMaskResult::SceneDepth(depth)) => {
                self.masks.depth_cache = Some(depth.clone());
                self.masks.stack.set_scene_depth(Some(depth));
            }
            (AiMaskModel::Subject | AiMaskModel::Sky, AiMaskResult::Probability(mask)) => {
                let cache = if model == AiMaskModel::Subject {
                    &mut self.masks.subject_cache
                } else {
                    &mut self.masks.sky_cache
                };
                *cache = Some(mask.clone());
                for component in self
                    .masks
                    .stack
                    .masks
                    .iter_mut()
                    .flat_map(|mask| &mut mask.components)
                {
                    if generated_mask_model(component.kind) == Some(model) {
                        if let MaskGeometry::Ai { mask: target, .. } = &mut component.geometry {
                            *target = Some(mask.clone());
                        }
                    }
                }
            }
            _ => return false,
        }
        self.mark_all_mask_layers_dirty();
        self.blink_selected_mask();
        true
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
                    let no_subject = matches!(
                        &result,
                        AiMaskResult::Probability(mask)
                            if model == AiMaskModel::Subject
                                && mask.pixels.iter().all(|&alpha| alpha == 0)
                    );
                    if no_subject {
                        error_message = Some(
                            tr("Subject selection found no subject in this photo.").to_owned(),
                        );
                    } else if self.apply_generated_mask(model, result) {
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
    use calibraw_ai::pipeline::{DisparityMap, SceneCamera, SceneNormals};
    use calibraw_ai::AiFeature;

    #[test]
    fn generated_worker_results_route_by_model_and_reject_cancelled_or_stale_jobs() {
        let mask = MaskImage::new(2, 2, vec![0, 85, 170, 255]).unwrap();
        let depth = SceneDepth::new(
            DisparityMap::new(2, 2, vec![0, 20_000, 40_000, 65_535]).unwrap(),
            MaskImage::new(2, 2, vec![255, 0, 0, 0]).unwrap(),
            SceneNormals::new(mask.clone(), mask.clone()).unwrap(),
            SceneCamera {
                focal_tan: 0.4,
                fit: [1.0, 0.1],
                anchors: [1.0, 0.0],
                frame_relief: 9.0,
            },
        )
        .unwrap();
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
                let result = if model == AiMaskModel::Depth {
                    AiMaskResult::SceneDepth(depth.clone())
                } else {
                    AiMaskResult::Probability(mask.clone())
                };
                let (sender, receiver) = std::sync::mpsc::channel();
                sender
                    .send(AiMaskEvent::Progress(AiMaskProgress::Inferencing))
                    .unwrap();
                sender.send(AiMaskEvent::Finished(Ok(result))).unwrap();
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
                let depth_accepted = accepted && model == AiMaskModel::Depth;
                assert_eq!(
                    app.masks.stack.scene_depth.as_ref(),
                    depth_accepted.then_some(&depth)
                );
                assert_eq!(
                    app.masks.depth_cache.as_ref(),
                    depth_accepted.then_some(&depth)
                );
                for (cache_model, cache) in [
                    (AiMaskModel::Subject, &app.masks.subject_cache),
                    (AiMaskModel::Sky, &app.masks.sky_cache),
                ] {
                    assert_eq!(
                        cache.as_ref(),
                        (accepted && model == cache_model).then_some(&mask)
                    );
                }
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
            }
        }
    }
}
