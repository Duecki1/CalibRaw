use super::*;
use crate::i18n::{tr, tr_fmt};

impl CalibRawApp {
    pub(crate) fn mark_lens_correction_dirty(&mut self) {
        self.note_edit_changed();
        if self.develop.original_raw.is_some() {
            self.develop.lens_correction_dirty = true;
            self.cancel_foreground_operation_if(ForegroundOperationKind::LensCorrection);
            self.ui.notice = None;
            self.egui_ctx.request_repaint();
        }
    }

    /// Records a lens-correction edit made in the Lens Corrections card, the
    /// manual-correction dialog or the missing-profile prompt. The photo is corrected again unless it
    /// already shows what the edit asks for: the original geometry, with no
    /// correction applied or on its way.
    pub(crate) fn lens_correction_edited(&mut self) {
        let showing_request = !self.develop.lens_correction.applied
            && !self.lens_correction_busy()
            && self
                .develop
                .lens_correction
                .requested_correction()
                .is_none();
        if showing_request {
            self.note_edit_changed();
        } else {
            self.mark_lens_correction_dirty();
        }
    }

    pub(in crate::app) fn apply_pending_lens_correction(&mut self, frame: &eframe::Frame) {
        self.queue_pending_lens_correction(frame);
    }

    /// Starts the task that prepares the source again when the lens correction
    /// changed (`lens_correction_dirty`) or the source no longer has the hot
    /// pixel removal the edit asks for, which an edit in the Detail card, an
    /// undo, a paste, a preset or the card's switch changes.
    pub(in crate::app) fn queue_pending_lens_correction(&mut self, frame: &eframe::Frame) {
        self.poll_lens_correction_worker(frame);
        let hot_pixels = self.requested_hot_pixel_removal();
        if hot_pixels == self.develop.hot_pixels_removed {
            self.develop.hot_pixels_failed = None;
        }
        let purpose = if self.develop.lens_correction_dirty {
            SourceTask::Lens
        } else if self.hot_pixel_removal_stale() {
            SourceTask::HotPixels {
                removed: hot_pixels,
            }
        } else {
            return;
        };
        if self.foreground_operation_active() {
            return;
        }

        let Some(original_raw) = self.develop.original_raw.as_ref().map(Arc::clone) else {
            self.develop.lens_correction_dirty = false;
            return;
        };
        // Switched on without a profile, the card removes chromatic
        // aberration alone and the photo keeps its geometry.
        let corrections = SourceCorrections {
            hot_pixels,
            lens: self.develop.lens_correction.requested_correction(),
        };

        if let Some(restored_masks) = self.persistence.lens_restore_masks.take() {
            self.masks.stack = restored_masks;
            self.rehydrate_restored_mask_state();
        }

        // Constrained memory keeps the last sources prepared with and without
        // a lens correction, so switching it does not prepare them again.
        let cached_raws = if self.caps().constrained_memory() {
            self.preview
                .prepared_source(&corrections, self.preview.quality)
        } else {
            None
        };

        let preview_proxy_edge = self.preview.quality.proxy_edge_for_fitted_source(
            self.preview.fitted_viewport_pixels,
            original_raw.width,
            original_raw.height,
            self.develop.geometry,
            original_raw.cfa_kind,
            self.preview_mask_stack().masks.len(),
        );
        self.develop.lens_correction_dirty = false;
        self.start_lens_correction_task(LensCorrectionTaskRequest {
            original_raw,
            corrections,
            purpose,
            preview_quality: self.preview.quality,
            preview_proxy_edge,
            cached_raws,
        });
    }

    pub(in crate::app) fn start_lens_correction_task(
        &mut self,
        request: LensCorrectionTaskRequest,
    ) {
        if self.foreground_operation_active() {
            // A hot pixel removal is noticed again (`hot_pixel_removal_stale`).
            if request.purpose == SourceTask::Lens {
                self.develop.lens_correction_dirty = true;
            }
            return;
        }
        let cancellation = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_cancellation = Arc::clone(&cancellation);
        let purpose = request.purpose;
        let progress = match purpose {
            SourceTask::Lens => {
                let lens = request.corrections.lens.as_ref();
                let status_label = lens
                    .map(lens_correction_label)
                    .unwrap_or_else(|| tr("original geometry").to_owned());
                self.develop.lens_correction.catalog.status = if lens.is_some() {
                    tr_fmt(
                        "Applying {status_label}…",
                        &[("status_label", &status_label)],
                    )
                } else {
                    tr("Disabling lens correction…").to_owned()
                };
                ForegroundProgress::indeterminate(if lens.is_some() {
                    tr("Applying lens profile…")
                } else {
                    tr("Restoring original geometry…")
                })
            }
            SourceTask::HotPixels { .. } => {
                ForegroundProgress::indeterminate(tr("Updating hot pixel removal…"))
            }
        };

        let repaint = self.egui_ctx.clone();
        let (sender, receiver) = mpsc::channel();
        let spawn_result = std::thread::Builder::new()
            .name("calibraw-lens-correction".to_owned())
            .spawn(move || {
                use std::sync::atomic::Ordering;
                let result = (|| -> Result<PreparedLensCorrection, String> {
                    if worker_cancellation.load(Ordering::Acquire) {
                        return Err("foreground operation cancelled".to_owned());
                    }
                    let applied_label =
                        request.corrections.lens.as_ref().map(lens_correction_label);
                    let (full_raw, preview_raw) = if let Some(cached_raws) = request.cached_raws {
                        cached_raws
                    } else {
                        let full_raw = request
                            .corrections
                            .apply_shared(&request.original_raw)
                            .map_err(|error| {
                                // Only the lens correction fails.
                                let selection = request
                                    .corrections
                                    .lens
                                    .as_ref()
                                    .map(LensCorrection::label)
                                    .unwrap_or_default();
                                tr_fmt(
                                    "Could not apply {selection}: {error}",
                                    &[("selection", &selection), ("error", &format!("{error:#}"))],
                                )
                            })?;
                        if worker_cancellation.load(Ordering::Acquire) {
                            return Err("foreground operation cancelled".to_owned());
                        }
                        let _ = sender.send(LensCorrectionEvent::Progress(
                            tr("Building preview proxy…").to_owned(),
                        ));
                        repaint.request_repaint();
                        let preview_spec = ProxySpec {
                            max_edge: request.preview_proxy_edge,
                        };
                        let preview_raw =
                            if full_raw.width.max(full_raw.height) <= preview_spec.max_edge {
                                Arc::clone(&full_raw)
                            } else {
                                Arc::new(build_proxy(&full_raw, preview_spec))
                            };
                        (full_raw, preview_raw)
                    };
                    if worker_cancellation.load(Ordering::Acquire) {
                        return Err("foreground operation cancelled".to_owned());
                    }
                    Ok(PreparedLensCorrection {
                        full_raw,
                        preview_raw,
                        applied_label,
                        corrections: request.corrections,
                        preview_quality: request.preview_quality,
                    })
                })();
                let _ = sender.send(LensCorrectionEvent::Finished(result));
                repaint.request_repaint();
            });
        match spawn_result {
            Ok(_) => {
                self.begin_foreground_operation(ForegroundOperation {
                    kind: ForegroundOperationKind::LensCorrection,
                    document_id: self.persistence.document_generation,
                    cancellation,
                    progress,
                    cancelling: false,
                    receiver: ForegroundOperationReceiver::LensCorrection(receiver),
                    context: ForegroundOperationContext::LensCorrection(purpose),
                });
                self.egui_ctx
                    .request_repaint_after(Duration::from_millis(50));
            }
            Err(error) => {
                let message = tr_fmt(
                    "Could not start lens correction: {error}",
                    &[("error", &error)],
                );
                if purpose == SourceTask::Lens {
                    self.develop.lens_correction.enabled = self.develop.lens_correction.applied;
                    self.develop.lens_correction.catalog.status = message.clone();
                }
                self.source_task_failed(purpose, message);
            }
        }
    }

    /// Whether the source is being prepared again, or will be: for the lens
    /// correction or for the hot pixel removal.
    pub(crate) fn lens_correction_busy(&self) -> bool {
        self.develop.lens_correction_dirty
            || self.hot_pixel_removal_stale()
            || self.foreground_operation_is(ForegroundOperationKind::LensCorrection)
    }

    /// Whether the open photo's hot pixels can be repaired, which the Detail
    /// card offers (`hot_pixel_removal_applies`).
    pub(crate) fn hot_pixel_removal_available(&self) -> bool {
        self.develop
            .original_raw
            .as_deref()
            .is_some_and(hot_pixel_removal_applies)
    }

    /// Whether the edit asks for the photo's hot pixels repaired
    /// (`source_removes_hot_pixels`).
    fn requested_hot_pixel_removal(&self) -> bool {
        self.develop
            .original_raw
            .as_deref()
            .is_some_and(|raw| source_removes_hot_pixels(&self.develop.exposure, raw))
    }

    /// Whether the source must be prepared again for the edit's hot pixel
    /// removal: the edit asks for the other than it was prepared with, which
    /// has not failed to be prepared.
    fn hot_pixel_removal_stale(&self) -> bool {
        let requested = self.requested_hot_pixel_removal();
        requested != self.develop.hot_pixels_removed
            && self.develop.hot_pixels_failed != Some(requested)
    }

    /// Reports a source the task for `purpose` could not prepare or install.
    /// A hot pixel removal is not tried again until the edit asks for the
    /// other (`hot_pixel_removal_stale`).
    fn source_task_failed(&mut self, purpose: SourceTask, message: String) {
        match purpose {
            SourceTask::Lens => self.report_error(ErrorKind::LensCorrection, message),
            SourceTask::HotPixels { removed } => {
                self.develop.hot_pixels_failed = Some(removed);
                self.report_error(ErrorKind::Preview, message);
            }
        }
    }

    /// Where memory is constrained, keeps the installed source and its preview
    /// proxy at `quality` (`PreviewState::keep_prepared_source`).
    pub(in crate::app) fn keep_installed_source(&mut self, quality: PreviewQuality) {
        if !self.caps().constrained_memory() {
            return;
        }
        let (Some(full_raw), Some(preview_raw)) = (
            self.develop.loaded_raw.clone(),
            self.develop.preview_raw.clone(),
        ) else {
            return;
        };
        let lens = &self.develop.lens_correction;
        self.preview.keep_prepared_source(PreparedSource {
            corrections: SourceCorrections {
                hot_pixels: self.develop.hot_pixels_removed,
                lens: lens.applied.then(|| lens.requested_correction()).flatten(),
            },
            quality,
            full_raw,
            preview_raw,
        });
    }

    pub(in crate::app) fn poll_lens_correction_worker(&mut self, frame: &eframe::Frame) {
        if !self.foreground_operation_is(ForegroundOperationKind::LensCorrection) {
            return;
        }
        let Some(mut operation) = self.foreground_operation.take() else {
            return;
        };
        let ForegroundOperationReceiver::LensCorrection(receiver) = &operation.receiver else {
            self.foreground_operation = Some(operation);
            return;
        };
        let (events, disconnected) = drain_worker_events(Some(receiver), |event| {
            matches!(event, LensCorrectionEvent::Finished(_))
        });

        let mut finished = None;
        for event in events {
            match event {
                LensCorrectionEvent::Progress(phase) => {
                    operation.progress = ForegroundProgress::indeterminate(phase);
                }
                LensCorrectionEvent::Finished(result) => finished = Some(result),
            }
        }
        if finished.is_none() && disconnected {
            finished = Some(Err(
                tr("Lens-correction worker stopped unexpectedly.").to_owned()
            ));
        }
        let Some(result) = finished else {
            self.foreground_operation = Some(operation);
            return;
        };

        let stale = !operation.accepts_result(self.persistence.document_generation);
        if stale {
            return;
        }
        // The task starts with its purpose as the context.
        let purpose = match operation.context {
            ForegroundOperationContext::LensCorrection(purpose) => purpose,
            _ => SourceTask::Lens,
        };
        let prepared = match result {
            Ok(prepared) => prepared,
            Err(error) => {
                if !error.contains("cancelled") {
                    match purpose {
                        SourceTask::Lens => {
                            self.develop.lens_correction.enabled =
                                self.develop.lens_correction.applied;
                            self.develop.lens_correction.catalog.status = error;
                            self.report_error(
                                ErrorKind::LensCorrection,
                                tr("Lens correction failed; restored the previous preview.")
                                    .to_owned(),
                            );
                        }
                        SourceTask::HotPixels { .. } => self.source_task_failed(purpose, error),
                    }
                }
                return;
            }
        };
        operation.progress = ForegroundProgress::indeterminate(tr("Preparing GPU preview…"));
        if let Err(message) = self.install_prepared_source(&operation, prepared, purpose, frame) {
            self.source_task_failed(purpose, message);
        }
    }

    /// Shows the source the task prepared for `purpose`. Fails when the
    /// preview cannot show it, keeping the previous one; returns without
    /// installing it when the document changed meanwhile.
    fn install_prepared_source(
        &mut self,
        operation: &ForegroundOperation,
        prepared: PreparedLensCorrection,
        purpose: SourceTask,
        frame: &eframe::Frame,
    ) -> Result<(), String> {
        let Some(render_state) = frame.wgpu_render_state() else {
            return Err(tr("eframe is not running with the wgpu backend.").to_owned());
        };
        if prepared
            .full_raw
            .uses_highlight_model(&self.preview_exposure())
        {
            prepared
                .full_raw
                .prime_highlight_model(&self.preview_exposure());
        }

        // Constrained memory updates the current preview in place instead of
        // building a second pipeline next to it.
        if self.caps().constrained_memory() {
            let Some(pipeline) = self.preview.pipeline() else {
                return Err(tr("The preview pipeline is unavailable.").to_owned());
            };
            pipeline
                .upload_raw_tile(&render_state.queue, &prepared.preview_raw)
                .map_err(|error| {
                    tr_fmt(
                        "Could not update the lens-corrected preview pixels: {error}",
                        &[("error", &format!("{error:#}"))],
                    )
                })?;
            let params = GpuParams::new(
                &self.preview_exposure(),
                &self.preview_mask_stack(),
                &prepared.preview_raw,
            )
            .with_vignette_geometry(self.develop.geometry)
            .with_look_lut(self.preview_look().as_ref());
            pipeline
                .recompute_with_remove(
                    &render_state.queue,
                    &render_state.device,
                    &params,
                    RemoveSceneContext::new(
                        &self.inpaint.edits,
                        &prepared.full_raw,
                        &self.preview_exposure(),
                        [0.0, 0.0],
                        [
                            prepared.full_raw.width as f32,
                            prepared.full_raw.height as f32,
                        ],
                    ),
                )
                .map_err(|error| {
                    tr_fmt(
                        "Could not apply Remove to lens preview: {error}",
                        &[("error", &format!("{error:#}"))],
                    )
                })?;
            self.preview.keep_prepared_source(PreparedSource {
                corrections: prepared.corrections.clone(),
                quality: prepared.preview_quality,
                full_raw: Arc::clone(&prepared.full_raw),
                preview_raw: Arc::clone(&prepared.preview_raw),
            });
        } else {
            let preview_masks = self.preview_mask_stack();
            let params = GpuParams::new(
                &self.preview_exposure(),
                &preview_masks,
                &prepared.preview_raw,
            )
            .with_vignette_geometry(self.develop.geometry)
            .with_look_lut(self.preview_look().as_ref());
            let pipeline = RawGpuPipeline::new(
                &render_state.device,
                &render_state.queue,
                &prepared.preview_raw,
                &params,
                fitted_pipeline_options(preview_masks.masks.len()).profile_preview(
                    crate::app::lifecycle::reserves_profile_preview(&prepared.preview_raw),
                ),
            )
            .map_err(|error| {
                tr_fmt(
                    "Could not rebuild the corrected GPU preview: {error}",
                    &[("error", &format!("{error:#}"))],
                )
            })?;
            Self::upload_preview_masks(
                &pipeline,
                &render_state.queue,
                &preview_masks,
                &prepared.preview_raw,
            )?;
            pipeline
                .recompute_with_remove(
                    &render_state.queue,
                    &render_state.device,
                    &params,
                    RemoveSceneContext::new(
                        &self.inpaint.edits,
                        &prepared.full_raw,
                        &self.preview_exposure(),
                        [0.0, 0.0],
                        [
                            prepared.full_raw.width as f32,
                            prepared.full_raw.height as f32,
                        ],
                    ),
                )
                .map_err(|error| {
                    tr_fmt(
                        "Could not apply Remove to corrected preview: {error}",
                        &[("error", &format!("{error:#}"))],
                    )
                })?;

            if !operation.accepts_result(self.persistence.document_generation) {
                return Ok(());
            }
            self.take_preview_pipeline_and_release_textures();
            self.preview.gpu_pipeline = Some(self.present_pipeline(pipeline, render_state));
        }

        if !operation.accepts_result(self.persistence.document_generation) {
            return Ok(());
        }

        match purpose {
            SourceTask::Lens => {
                self.rehydrate_restored_mask_state();
                self.note_lens_correction_changed_for_masks();
                self.preview.reset_view();
                self.develop.lens_correction.applied = prepared.applied_label.is_some();
                self.develop.lens_correction.catalog.status = prepared.applied_label.map_or_else(
                    || tr("Lens correction disabled; using the original geometry.").to_owned(),
                    |label| tr_fmt("Applied {label}", &[("label", &label)]),
                );
            }
            SourceTask::HotPixels { .. } => {
                // The geometry stays, and with it the view; what was shown of
                // the previous source is made again from this one.
                self.note_mask_source_changed();
                self.discard_auxiliary_previews();
                self.preview.drop_detail_rebuild();
                self.preview.revision = self.preview.revision.wrapping_add(1);
            }
        }
        self.masks.dirty_layers = [false; MAX_LOCAL_MASKS];
        self.masks.detail_dirty_layers = [false; MAX_LOCAL_MASKS];
        self.masks.navigation_dirty_layers = [false; MAX_LOCAL_MASKS];
        self.develop.loaded_raw = Some(prepared.full_raw);
        self.develop.preview_raw = Some(prepared.preview_raw);
        self.develop.hot_pixels_removed = prepared.corrections.hot_pixels;
        self.preview.detail_pending_stage = None;
        self.preview.navigation_pending_stage = None;
        self.preview.detail_urgent = false;
        self.develop.target_exposure = self.preview_exposure();
        self.develop.target_look = self.preview_look();
        self.preview.pending_stage = None;
        self.ui.notice = None;
        self.resume_persisted_ai_denoise(frame);
        self.egui_ctx.request_repaint();
        Ok(())
    }
}

/// The name of a lens correction shown in the status line. A lens profile is
/// named from its metadata; the manual correction's name is translated here.
fn lens_correction_label(correction: &LensCorrection) -> String {
    match correction {
        LensCorrection::Profile(lens) => lens.label(),
        LensCorrection::Manual(_) => tr("manual lens correction").to_owned(),
    }
}
