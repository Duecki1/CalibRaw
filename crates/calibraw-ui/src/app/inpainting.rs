use super::*;
use calibraw_ai::AiFeature;

impl InpaintState {
    pub(crate) fn reset_for_document(&mut self) {
        self.cancel_processing();
        self.edits = Arc::new(RemoveEditState::default());
        self.source_point = None;
        self.aligned_offset = None;
        self.hovered_stroke = None;
        self.selected_stroke = None;
        self.stroke_opacity_edit_pending = false;
    }

    fn cancel_processing(&mut self) {
        if let Some(cancellation) = self.cancellation.take() {
            cancellation.store(true, std::sync::atomic::Ordering::Release);
        }
        self.source_pick_active = false;
        self.source_placement_press = false;
        self.active_points.clear();
        self.last_brush_uv = None;
        self.pending_brush = None;
        self.pending_retouch = None;
        self.receiver = None;
        self.processing_progress = None;
    }

    pub(crate) fn worker_active(&self) -> bool {
        self.receiver.is_some()
    }
}

impl CalibRawApp {
    pub(crate) fn reset_inpainting_state(&mut self) {
        self.inpaint.reset_for_document();
        if self.ai_consent_is_for(AiFeature::Remove) {
            self.ai.consent = None;
        }
    }

    pub(crate) fn inpaint_processing(&self) -> bool {
        self.inpaint.worker_active() || self.ai_consent_is_for(AiFeature::Remove)
    }

    pub(crate) fn install_remove_edits(&mut self, edits: Arc<RemoveEditState>) {
        self.cancel_remove_processing();
        self.inpaint.edits = edits;
        self.inpaint.hovered_stroke = None;
        self.inpaint.selected_stroke = None;
        self.inpaint.stroke_opacity_edit_pending = false;
    }

    pub(crate) fn cancel_remove_processing(&mut self) {
        self.inpaint.cancel_processing();
        if self.ai_consent_is_for(AiFeature::Remove) {
            self.ai.consent = None;
        }
    }

    pub(crate) fn clear_inpainting_tool(&mut self) {
        self.finish_inpaint_stroke_opacity_edit();
        self.cancel_remove_processing();
        let tool = self.inpaint.tool;
        if !self
            .inpaint
            .edits
            .strokes
            .iter()
            .any(|stroke| tool.matches_stroke_tool(stroke.retouch.map(|retouch| retouch.tool)))
        {
            return;
        }
        Arc::make_mut(&mut self.inpaint.edits)
            .strokes
            .retain(|stroke| !tool.matches_stroke_tool(stroke.retouch.map(|retouch| retouch.tool)));
        self.inpaint.hovered_stroke = None;
        self.inpaint.selected_stroke = None;
        self.note_remove_edit_changed();
        self.egui_ctx.request_repaint();
    }

    pub(crate) fn delete_inpaint_stroke(&mut self, index: usize) {
        self.finish_inpaint_stroke_opacity_edit();
        self.cancel_remove_processing();
        if index >= self.inpaint.edits.strokes.len() {
            return;
        }
        Arc::make_mut(&mut self.inpaint.edits).strokes.remove(index);
        self.inpaint.hovered_stroke = None;
        self.inpaint.selected_stroke = None;
        self.note_remove_edit_changed();
        self.egui_ctx.request_repaint();
    }

    pub(crate) fn set_inpaint_stroke_opacity(&mut self, index: usize, opacity: f32) {
        if self.inpaint_processing() || !opacity.is_finite() {
            return;
        }
        let opacity = opacity.clamp(0.0, 1.0);
        let Some(current) = self.inpaint.edits.strokes.get(index) else {
            return;
        };
        if (current.opacity - opacity).abs() <= f32::EPSILON {
            return;
        }
        let Some(stroke) = Arc::make_mut(&mut self.inpaint.edits)
            .strokes
            .get_mut(index)
        else {
            return;
        };
        stroke.opacity = opacity;
        self.inpaint.stroke_opacity_edit_pending = true;
        self.queue_preview_processing(ProcessingStage::Raw);
        self.egui_ctx.request_repaint();
    }

    pub(crate) fn finish_inpaint_stroke_opacity_edit(&mut self) {
        if !std::mem::take(&mut self.inpaint.stroke_opacity_edit_pending) {
            return;
        }
        self.note_remove_edit_changed();
    }

    pub(in crate::app) fn start_remove_request(
        &mut self,
        frame: &eframe::Frame,
        existing: RemoveEditState,
        brush: RemoveBrushStroke,
        allow_download: bool,
    ) -> bool {
        let Some(raw) = self.develop.loaded_raw.as_ref().cloned() else {
            self.ui.notice = Some("Open a RAW image before using Remove.".to_owned());
            return false;
        };
        let Some(render_state) = frame.wgpu_render_state() else {
            self.ui.notice = Some("GPU rendering is unavailable for Remove.".to_owned());
            return false;
        };
        #[cfg(not(target_os = "android"))]
        let (runtime_path, runtime_sha256) = self.onnx_runtime_for_ai();
        #[cfg(target_os = "android")]
        let runtime_path = None;
        #[cfg(target_os = "android")]
        let runtime_sha256 = None;
        let cancellation = Arc::new(AtomicBool::new(false));
        let request = RemoveRequest {
            device: render_state.device.clone(),
            queue: render_state.queue.clone(),
            raw,
            geometry: self.develop.geometry.sanitized(),
            exposure: self.develop.exposure,
            masks: self.masks.stack.clone(),
            existing,
            brush: brush.clone(),
            opacity: self.inpaint.brush_opacity,
            model_path: self.big_lama_model_path(),
            allow_download,
            runtime_path,
            runtime_sha256,
            program_prewarm: self.export.gpu_prewarm.clone(),
            cancellation: Arc::clone(&cancellation),
        };
        self.inpaint.pending_brush = Some(brush);
        self.inpaint.pending_retouch = None;
        self.inpaint.processing_progress = Some(ForegroundProgress::indeterminate(
            "Preparing local context…",
        ));
        self.inpaint.cancellation = Some(cancellation);
        self.inpaint.receiver = Some(spawn_remove(request));
        self.egui_ctx
            .request_repaint_after(Duration::from_millis(30));
        true
    }

    pub(crate) fn start_remove_worker(&mut self, frame: &eframe::Frame, brush: RemoveBrushStroke) {
        if self.inpaint_processing() || brush.points.is_empty() {
            return;
        }
        self.inpaint.pending_brush = Some(brush);
        self.inpaint.pending_retouch = None;
        if self.ai_job_may_start(AiFeature::Remove) {
            self.start_ai_job(AiFeature::Remove, false, frame);
        }
    }

    pub(crate) fn start_retouch_worker(
        &mut self,
        frame: &eframe::Frame,
        brush: RemoveBrushStroke,
        retouch: RetouchStroke,
    ) {
        if self.inpaint_processing() || brush.points.is_empty() {
            return;
        }
        let Some(raw) = self.develop.loaded_raw.as_ref().cloned() else {
            self.ui.notice = Some("Open a RAW image before using retouch brushes.".to_owned());
            return;
        };
        let Some(render_state) = frame.wgpu_render_state() else {
            self.ui.notice = Some("GPU rendering is unavailable for retouch brushes.".to_owned());
            return;
        };
        let cancellation = Arc::new(AtomicBool::new(false));
        let request = RetouchRequest {
            device: render_state.device.clone(),
            queue: render_state.queue.clone(),
            raw,
            geometry: self.develop.geometry.sanitized(),
            exposure: self.develop.exposure,
            masks: self.masks.stack.clone(),
            existing: self.inpaint.edits.as_ref().clone(),
            brush: brush.clone(),
            retouch,
            program_prewarm: self.export.gpu_prewarm.clone(),
            cancellation: Arc::clone(&cancellation),
        };
        self.inpaint.pending_brush = Some(brush);
        self.inpaint.pending_retouch = Some(retouch);
        self.inpaint.processing_progress = Some(ForegroundProgress::indeterminate(format!(
            "Applying {} locally…",
            retouch.tool.label()
        )));
        self.inpaint.cancellation = Some(cancellation);
        self.inpaint.receiver = Some(spawn_retouch(request));
        self.egui_ctx
            .request_repaint_after(Duration::from_millis(16));
    }

    pub(crate) fn show_remove_progress_dialog(&mut self, ctx: &egui::Context) {
        if self.inpaint.receiver.is_none() || self.inpaint.pending_retouch.is_some() {
            return;
        }
        let Some(progress) = self.inpaint.processing_progress.as_ref() else {
            return;
        };
        if super::foreground::show_processing_dialog(
            ctx,
            "remove-operation-progress",
            "Applying Remove",
            progress,
            false,
        ) {
            self.cancel_remove_processing();
        }
    }

    pub(crate) fn advance_remove_worker(&mut self, _frame: &eframe::Frame) {
        let mut events = Vec::new();
        if let Some(receiver) = self.inpaint.receiver.as_ref() {
            while let Ok(event) = receiver.try_recv() {
                events.push(event);
            }
        }
        if events.is_empty() {
            if self.inpaint.receiver.is_some() {
                self.egui_ctx
                    .request_repaint_after(Duration::from_millis(50));
            }
            return;
        }
        for event in events {
            match event {
                RemoveEvent::DownloadProgress(progress) => {
                    self.inpaint.processing_progress = Some(
                        ForegroundProgress::units(
                            progress.downloaded,
                            progress.total,
                            Some("bytes".to_owned()),
                            format!("Downloading {}", progress.label),
                        )
                        .with_detail(format!(
                            "{:.1} / {:.1} MB",
                            progress.downloaded as f64 / 1_000_000.0,
                            progress.total as f64 / 1_000_000.0
                        )),
                    );
                }
                RemoveEvent::Processing { completed, total } => {
                    // A stroke is filled in one or more passes; show which.
                    self.inpaint.processing_progress = Some(if total > 1 {
                        ForegroundProgress::units(
                            completed as u64,
                            total as u64,
                            Some("areas".to_owned()),
                            "Applying Big-LaMa",
                        )
                    } else {
                        ForegroundProgress::indeterminate("Applying Big-LaMa…")
                    });
                }
                RemoveEvent::Finished(result) => {
                    self.inpaint.receiver = None;
                    self.inpaint.cancellation = None;
                    let pending_brush = self.inpaint.pending_brush.take();
                    let pending_retouch = self.inpaint.pending_retouch.take();
                    self.inpaint.processing_progress = None;
                    match result {
                        Ok(stroke) => {
                            let applied_tool = stroke
                                .retouch
                                .map(|retouch| retouch.tool.label())
                                .unwrap_or("Remove");
                            Arc::make_mut(&mut self.inpaint.edits).strokes.push(stroke);
                            self.inpaint.selected_stroke = None;
                            self.inpaint.hovered_stroke = None;
                            self.note_remove_edit_changed();
                            self.ui.notice = Some(format!("{applied_tool} applied."));
                        }
                        Err(error) => {
                            if error.contains("consent to its download again") {
                                self.inpaint.pending_brush = pending_brush;
                                let runtime_download_needed =
                                    self.automatic_onnx_runtime_download_needed();
                                self.ai.consent = Some(AiConsent {
                                    feature: AiFeature::Remove,
                                    runtime_download_needed,
                                    origin: AiJobOrigin::Requested,
                                });
                                self.ui.notice = Some(
                                    "Big-LaMa needs to be installed or re-verified before Remove can continue."
                                        .to_owned(),
                                );
                            } else if !error.contains("cancelled") {
                                let tool = pending_retouch
                                    .map(|retouch| retouch.tool.label())
                                    .unwrap_or("Remove");
                                self.ui.notice = Some(format!("{tool} failed: {error}"));
                                calibraw_core::diagnostics::record(format!(
                                    "{tool} failed: {error}"
                                ));
                                log::error!("{tool} failed: {error}");
                            }
                        }
                    }
                }
            }
        }
        self.egui_ctx.request_repaint();
    }
}

#[cfg(all(test, not(target_os = "android")))]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn cancellation_reset_and_restore_reject_late_strokes_and_preserve_their_edit_state() {
        for transition in ["cancel", "reset", "restore"] {
            let mut app = CalibRawApp::empty(&egui::Context::default());
            let cancellation = Arc::new(AtomicBool::new(false));
            let (sender, receiver) = mpsc::channel();
            let edits = Arc::new(RemoveEditState {
                strokes: vec![crate::pipeline::RemoveStroke::default()],
            });
            let restored = Arc::new(RemoveEditState::default());
            app.inpaint.edits = Arc::clone(&edits);
            app.inpaint.receiver = Some(receiver);
            app.inpaint.cancellation = Some(Arc::clone(&cancellation));
            app.inpaint.pending_brush = Some(RemoveBrushStroke::default());
            app.inpaint.active_points.push(RemoveBrushPoint::default());
            app.inpaint.processing_progress = Some(ForegroundProgress::indeterminate("Testing"));
            app.inpaint.last_brush_uv = Some([0.3, 0.4]);
            app.inpaint.source_point = Some([0.1, 0.2]);
            app.inpaint.aligned_offset = Some([0.2, 0.2]);
            app.inpaint.source_pick_active = true;
            app.inpaint.selected_stroke = Some(0);
            app.ai.consent = Some(AiConsent {
                feature: AiFeature::Remove,
                runtime_download_needed: false,
                origin: AiJobOrigin::Requested,
            });

            match transition {
                "cancel" => app.cancel_remove_processing(),
                "reset" => app.reset_inpainting_state(),
                "restore" => app.install_remove_edits(Arc::clone(&restored)),
                _ => unreachable!(),
            }

            assert!(cancellation.load(Ordering::Acquire));
            assert!(sender
                .send(RemoveEvent::Finished(Ok(
                    crate::pipeline::RemoveStroke::default()
                )))
                .is_err());
            assert!(!app.inpaint_processing());
            assert!(app.inpaint.pending_brush.is_none());
            assert!(app.inpaint.active_points.is_empty());
            assert!(app.inpaint.processing_progress.is_none());
            assert!(app.inpaint.last_brush_uv.is_none());
            assert!(!app.inpaint.source_pick_active);
            if transition == "reset" {
                assert!(app.inpaint.edits.strokes.is_empty());
                assert!(app.inpaint.source_point.is_none());
                assert!(app.inpaint.aligned_offset.is_none());
            } else {
                assert!(Arc::ptr_eq(
                    &app.inpaint.edits,
                    if transition == "cancel" {
                        &edits
                    } else {
                        &restored
                    },
                ));
                assert_eq!(app.inpaint.source_point, Some([0.1, 0.2]));
                assert_eq!(app.inpaint.aligned_offset, Some([0.2, 0.2]));
            }
            assert_eq!(
                app.inpaint.selected_stroke,
                (transition == "cancel").then_some(0)
            );
        }
    }
}
