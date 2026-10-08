use super::*;

impl CalibRawApp {
    pub(in crate::app) fn advance_navigation_preview(&mut self, frame: &eframe::Frame) {
        let preview_masks = self.preview_mask_stack();
        let preview_source = self.preview_source_raw();
        if self.ai_denoise_running() {
            return;
        }
        if self.defer_background_mask_processing()
            || !self
                .preview
                .interactive_render_ready
                .load(std::sync::atomic::Ordering::Acquire)
        {
            return;
        }
        let zoomed = self.preview.zoom > DETAIL_ZOOM_START;
        let should_update = self.preview.navigation_pending_stage.is_some();
        let should_exist = zoomed && (self.preview.navigation.is_some() || should_update);
        if !should_exist && !should_update {
            self.preview.navigation = None;
            return;
        }
        let Some(full_raw) = preview_source.as_ref().map(Arc::clone) else {
            self.preview.navigation_pending_stage = None;
            return;
        };
        let Some(render_state) = frame.wgpu_render_state() else {
            return;
        };

        let navigation_capacity_stale = self.preview.navigation.as_ref().is_some_and(|preview| {
            preview.pipeline.gpu().mask_layer_capacity() < preview_masks.masks.len().max(1)
        });
        if navigation_capacity_stale {
            self.preview.navigation = None;
        }

        if self.preview.navigation.is_none() {
            if !should_exist {
                self.preview.navigation_pending_stage = None;
                return;
            }
            if full_raw.uses_opposed_chroma(&self.develop.target_exposure) {
                full_raw.inpaint_opposed_chroma_for_exposure(&self.develop.target_exposure);
            }
            let raw = if full_raw.width.max(full_raw.height) <= navigation_proxy_edge() {
                Arc::clone(&full_raw)
            } else {
                Arc::new(build_proxy(
                    &full_raw,
                    ProxySpec {
                        max_edge: navigation_proxy_edge(),
                    },
                ))
            };
            let params = GpuParams::new(&self.develop.target_exposure, &preview_masks, &raw)
                .with_vignette_geometry(self.develop.geometry);
            let Some(template) = self.preview.pipeline() else {
                return;
            };
            let pipeline = match RawGpuPipeline::new(
                &render_state.device,
                &render_state.queue,
                &raw,
                &params,
                PipelineOptions::new(ProcessingQuality::Preview)
                    .mask_atlas_edge(navigation_mask_edge())
                    .programs(&template.program_template()),
            ) {
                Ok(pipeline) => pipeline,
                Err(error) => {
                    self.report_error(
                        "Preview failed",
                        format!("Could not prepare the adjusted navigation preview: {error:#}"),
                    );
                    self.preview.navigation_pending_stage = None;
                    return;
                }
            };
            if let Err(error) =
                Self::upload_preview_masks(&pipeline, &render_state.queue, &preview_masks, &raw)
            {
                self.report_error("Preview failed", error);
                self.preview.navigation_pending_stage = None;
                return;
            }
            if let Err(error) = pipeline.recompute_with_remove(
                &render_state.queue,
                &render_state.device,
                &params,
                RemoveSceneContext::full_frame(
                    &self.inpaint.edits,
                    &full_raw,
                    &self.develop.target_exposure,
                ),
            ) {
                self.report_error(
                    "Preview failed",
                    format!("Could not apply Remove to navigation preview: {error:#}"),
                );
                self.preview.navigation_pending_stage = None;
                return;
            }
            let pipeline = self.present_pipeline(pipeline, render_state);
            self.preview.navigation = Some(PreviewNavigation { pipeline, raw });
            self.preview.navigation_pending_stage = None;
            self.masks.navigation_dirty_layers.fill(false);
            self.egui_ctx.request_repaint();
            return;
        }

        let Some(stage) = self.preview.navigation_pending_stage else {
            return;
        };
        let Some(preview) = self.preview.navigation.as_mut() else {
            return;
        };
        if self
            .masks
            .navigation_dirty_layers
            .iter()
            .any(|dirty| *dirty)
        {
            if let Err(error) = Self::upload_dirty_preview_masks(
                preview.pipeline.gpu(),
                &render_state.queue,
                &preview_masks,
                &preview.raw,
                &mut self.masks.navigation_dirty_layers,
            ) {
                self.report_error("Preview failed", error);
                self.preview.navigation_pending_stage = None;
                return;
            }
        }

        if matches!(stage, ProcessingStage::Raw)
            && full_raw.uses_opposed_chroma(&self.develop.target_exposure)
        {
            full_raw.inpaint_opposed_chroma_for_exposure(&self.develop.target_exposure);
        }
        let params = GpuParams::new(&self.develop.target_exposure, &preview_masks, &preview.raw)
            .with_vignette_geometry(self.develop.geometry);
        let stages = match stage {
            ProcessingStage::Raw => &[
                ProcessingStage::Raw,
                ProcessingStage::Tone,
                ProcessingStage::Output,
            ][..],
            ProcessingStage::Tone => &[ProcessingStage::Tone, ProcessingStage::Output][..],
            ProcessingStage::Output => &[ProcessingStage::Output][..],
        };
        for stage in stages {
            if let Err(error) = preview.pipeline.gpu().dispatch_stage_with_remove(
                &render_state.queue,
                &render_state.device,
                &params,
                *stage,
                RemoveSceneContext::full_frame(
                    &self.inpaint.edits,
                    &full_raw,
                    &self.develop.target_exposure,
                ),
            ) {
                self.report_error(
                    "Preview failed",
                    format!("Could not apply Remove to navigation preview: {error:#}"),
                );
                self.preview.navigation_pending_stage = None;
                return;
            }
        }
        self.preview.navigation_pending_stage = None;
        self.egui_ctx.request_repaint();
    }
}
