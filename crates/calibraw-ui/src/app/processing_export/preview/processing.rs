use super::*;

impl CalibRawApp {
    pub(in crate::app) fn interactive_detail_mask_edit(&self) -> bool {
        self.preview.zoom > DETAIL_ZOOM_START
            && self.masks.interaction_dirty_layer.is_some()
            && self.egui_ctx.input(|input| input.pointer.primary_down())
    }

    /// Whether only the zoomed crop follows the current drag: a control or a
    /// mask is dragged while zoomed in, the sharp crop covers the view and
    /// the drag changes only the Output stage. The fitted preview and the
    /// navigation proxy keep their pending work until the pointer lifts.
    ///
    /// Raw and Tone changes still update every preview, because the crop
    /// takes its tone statistics from a full-frame one.
    pub(in crate::app) fn defer_background_preview_processing(&self) -> bool {
        self.preview.zoom > DETAIL_ZOOM_START
            && self.egui_ctx.input(|input| input.pointer.primary_down())
            && [
                self.preview.pending_stage,
                self.preview.navigation_pending_stage,
                self.preview.detail_pending_stage,
            ]
            .iter()
            .all(|stage| stage.is_none_or(|stage| stage == ProcessingStage::Output))
            && self.preview_detail_is_current()
    }

    pub(crate) fn mark_pipeline_dirty(&mut self) {
        let preview_source = self.develop.loaded_raw.clone();
        self.note_edit_changed();
        let next_exposure = self.preview_exposure();
        if self.preview.gpu_pipeline.is_none() {
            self.develop.target_exposure = next_exposure;
            return;
        }

        if let Some(stage) = affected_stage(&self.develop.target_exposure, &next_exposure) {
            self.develop.target_exposure = next_exposure;
            if matches!(stage, ProcessingStage::Raw) {
                if let Some(full_raw) = preview_source.as_ref() {
                    if full_raw.uses_opposed_chroma(&self.develop.target_exposure) {
                        full_raw.inpaint_opposed_chroma_for_exposure(&self.develop.target_exposure);
                    }
                }
            }
            self.queue_preview_processing(stage);
        }
    }

    pub(crate) fn apply_white_balance_area(&mut self, area: [[f32; 2]; 2]) -> bool {
        let preview_source = self.develop.loaded_raw.clone();
        let result = preview_source.as_ref().and_then(|raw| {
            raw.white_balance_offsets_from_area(area[0], area[1], self.develop.exposure.black_point)
                .and_then(|offsets| raw.white_balance_from_offsets(offsets))
        });
        self.develop_ui.cancel_white_balance_picker();
        let Some(white_balance) = result else {
            self.report_error(ErrorKind::Sampling, "Could not estimate white balance there. Choose a brighter, unclipped neutral area."
                    .to_owned(),);
            self.egui_ctx.request_repaint();
            return false;
        };
        self.develop.exposure.white_balance = white_balance;
        self.ui.notice = Some("White balance sampled from the selected image area.".to_owned());
        self.mark_pipeline_dirty();
        true
    }

    pub(in crate::app) fn advance_zoomed_processing(&mut self, frame: &eframe::Frame) {
        let preview_masks = self.preview_mask_stack();
        let preview_source = self.develop.loaded_raw.clone();
        let Some(stage) = self.preview.detail_pending_stage else {
            return;
        };
        let Some(detail) = self
            .preview
            .detail
            .as_ref()
            .filter(|detail| detail.revision == self.preview.revision)
        else {
            return;
        };
        if detail.pipeline.gpu().mask_layer_capacity() < preview_masks.masks.len().max(1) {
            if let Some(detail) = self.preview.detail.as_mut() {
                detail.revision = self.preview.revision.wrapping_sub(1);
            }
            self.preview.detail_pending_stage = None;
            self.preview.detail_urgent = true;
            self.preview.motion_at = Some(Instant::now());
            self.egui_ctx.request_repaint();
            return;
        }
        let Some(full_raw) = preview_source.as_ref() else {
            self.preview.detail_pending_stage = None;
            return;
        };
        let Some(render_state) = frame.wgpu_render_state() else {
            return;
        };

        let detail_raw = Arc::clone(&detail.raw);
        let virtual_origin = detail.virtual_origin;
        let virtual_full_size = detail.virtual_full_size;
        if stage == ProcessingStage::Raw
            && !detail_raw.is_pre_demosaiced_raster()
            && full_raw.uses_opposed_chroma(&self.develop.target_exposure)
        {
            // Crops and proxies share the full sensor cache. Populate the exact
            // WB/black/clip/AI key before GpuParams reads it from the derived RAW.
            full_raw.inpaint_opposed_chroma_for_exposure(&self.develop.target_exposure);
        }
        let interactive = self.interactive_detail_mask_edit();
        let mask_region = detail_mask_update_region(
            &preview_masks,
            detail.source_origin,
            detail.source_size,
            [full_raw.width, full_raw.height],
            Some(detail.mask_source_region),
            interactive,
        );
        let mask_extent = detail_mask_texture_extent(
            mask_region,
            detail.pipeline.gpu().mask_atlas_edge(),
            interactive,
        );
        let mapping_changed =
            detail.mask_source_region != mask_region || detail.mask_texture_extent != mask_extent;
        let params = GpuParams::new_for_tile(
            &self.develop.target_exposure,
            &preview_masks,
            &detail_raw,
            virtual_origin[0],
            virtual_origin[1],
            virtual_full_size[0],
            virtual_full_size[1],
        )
        .with_vignette_geometry(self.develop.geometry)
        .with_mask_uv_rect_and_extent(
            mask_source_region_uv(mask_region, full_raw.width, full_raw.height),
            mask_extent,
        );

        let full_frame_tone_pipeline = full_frame_tone_pipeline(
            self.preview.gpu_pipeline.as_ref(),
            self.preview.navigation.as_ref(),
            self.preview.pending_stage,
        );
        let Some(detail) = self.preview.detail.as_mut() else {
            return;
        };
        if mapping_changed
            || (stage == ProcessingStage::Output
                && self.masks.detail_dirty_layers.iter().any(|dirty| *dirty))
        {
            if let Err(error) = Self::upload_detail_masks(
                detail.pipeline.gpu(),
                &render_state.queue,
                &preview_masks,
                full_raw,
                mask_region,
                mask_extent,
                (!mapping_changed).then_some(&self.masks.detail_dirty_layers),
            ) {
                self.report_error(ErrorKind::Preview, error);
                self.preview.detail_pending_stage = None;
                return;
            }
            detail.mask_source_region = mask_region;
            detail.mask_texture_extent = mask_extent;
            self.masks.detail_dirty_layers.fill(false);
        }

        if stage == ProcessingStage::Tone {
            if let Some(full_frame) = full_frame_tone_pipeline {
                detail
                    .pipeline
                    .gpu()
                    .dispatch_tone_guide_with_inherited_statistics(
                        &render_state.queue,
                        &render_state.device,
                        &params,
                        full_frame,
                    );
            } else {
                detail.pipeline.gpu().dispatch_stage(
                    &render_state.queue,
                    &render_state.device,
                    &params,
                    ProcessingStage::Tone,
                );
            }
        } else {
            if stage == ProcessingStage::Output {
                if let Some(full_frame) = full_frame_tone_pipeline {
                    detail.pipeline.gpu().inherit_tone_statistics(
                        &render_state.queue,
                        &render_state.device,
                        &params,
                        full_frame,
                    );
                }
            }
            if let Err(error) = detail.pipeline.gpu().dispatch_stage_with_remove(
                &render_state.queue,
                &render_state.device,
                &params,
                stage,
                RemoveSceneContext::new(
                    &self.inpaint.edits,
                    full_raw,
                    &self.develop.target_exposure,
                    [
                        detail.source_origin[0] as f32,
                        detail.source_origin[1] as f32,
                    ],
                    [detail.source_size[0] as f32, detail.source_size[1] as f32],
                ),
            ) {
                self.report_error(
                    ErrorKind::Preview,
                    format!("Could not apply Remove to zoomed preview: {error:#}"),
                );
                self.preview.detail_pending_stage = None;
                return;
            }
        }
        self.preview.detail_pending_stage = match stage {
            ProcessingStage::Raw => Some(ProcessingStage::Tone),
            ProcessingStage::Tone => Some(ProcessingStage::Output),
            ProcessingStage::Output => None,
        };
        if self.preview.detail_pending_stage.is_none() {
            detail.revision = self.preview.revision;
            self.preview.detail_urgent = false;
        }
        self.egui_ctx.request_repaint();
    }

    /// Submits the pending work of every preview as one batch, once the
    /// previous batch has finished on the GPU (`PreviewDispatchGate`).
    pub(in crate::app) fn advance_preview_dispatch(&mut self, frame: &eframe::Frame) {
        let render_state = frame.wgpu_render_state();
        if render_state
            .is_some_and(|render_state| !self.preview.dispatch_gate.poll_open(&render_state.device))
        {
            // Edits keep updating target_exposure; the gate requests the
            // frame that renders the newest value.
            return;
        }
        let before = self.preview.dispatch_progress();
        self.advance_navigation_preview(frame);
        self.advance_processing(frame);
        // Every dispatch advances or clears a pending stage, so an unchanged
        // state means nothing was submitted and the gate can stay open.
        if let Some(render_state) = render_state {
            if self.preview.dispatch_progress() != before {
                self.preview
                    .dispatch_gate
                    .close_until_idle(&render_state.queue, &self.egui_ctx);
            }
        }
    }

    /// Renders every pending stage of the zoomed crop and the fitted
    /// preview, so each batch ends in a displayed update.
    fn advance_processing(&mut self, frame: &eframe::Frame) {
        let zoomed = self.preview.zoom > DETAIL_ZOOM_START;
        // Decided before the crop renders, which can make it current.
        let defer_fitted = self.defer_background_preview_processing();
        if zoomed {
            self.drain_zoomed_processing(frame);
        }
        if defer_fitted {
            return;
        }
        // The fitted preview follows while zoomed too: panning can expose any
        // part of it.
        let preview_masks = self.preview_mask_stack();
        let preview_source = self.develop.loaded_raw.clone();
        self.drain_pending_stages(
            |preview| preview.pending_stage,
            |app| app.advance_main_processing_stage(frame, &preview_masks, &preview_source),
        );
        // Full-frame tone statistics supersede the temporary statistics
        // used by a zoomed crop, so finish that small downstream refresh too.
        if zoomed {
            self.drain_zoomed_processing(frame);
        }
    }

    fn drain_zoomed_processing(&mut self, frame: &eframe::Frame) {
        self.drain_pending_stages(
            |preview| preview.detail_pending_stage,
            |app| app.advance_zoomed_processing(frame),
        );
    }

    /// Runs `advance` until the stage `pending` reads is done or stops
    /// changing, at most once per stage (Raw, Tone, Output).
    fn drain_pending_stages(
        &mut self,
        pending: impl Fn(&PreviewState) -> Option<ProcessingStage>,
        mut advance: impl FnMut(&mut Self),
    ) {
        const STAGES: usize = 3;
        for _ in 0..STAGES {
            let before = pending(&self.preview);
            if before.is_none() {
                break;
            }
            advance(self);
            if pending(&self.preview) == before {
                break;
            }
        }
    }

    fn advance_main_processing_stage(
        &mut self,
        frame: &eframe::Frame,
        preview_masks: &MaskStack,
        preview_source: &Option<Arc<LoadedRaw>>,
    ) {
        let Some(stage) = self.preview.pending_stage else {
            return;
        };
        let (Some(raw), Some(pipeline)) = (&self.develop.preview_raw, self.preview.pipeline())
        else {
            self.preview.pending_stage = None;
            return;
        };
        let Some(render_state) = frame.wgpu_render_state() else {
            return;
        };

        if stage == ProcessingStage::Output && self.masks.dirty_layers.iter().any(|dirty| *dirty) {
            if let Err(error) = Self::upload_dirty_preview_masks(
                pipeline,
                &render_state.queue,
                preview_masks,
                raw,
                &mut self.masks.dirty_layers,
            ) {
                self.report_error(ErrorKind::Preview, error);
                self.preview.pending_stage = None;
                return;
            }
        }

        let params = GpuParams::new(&self.develop.target_exposure, preview_masks, raw)
            .with_vignette_geometry(self.develop.geometry);
        let Some(full_raw) = preview_source.as_ref() else {
            self.preview.pending_stage = None;
            return;
        };
        if let Err(error) = pipeline.dispatch_stage_with_remove(
            &render_state.queue,
            &render_state.device,
            &params,
            stage,
            RemoveSceneContext::full_frame(
                &self.inpaint.edits,
                full_raw,
                &self.develop.target_exposure,
            ),
        ) {
            self.report_error(
                ErrorKind::Preview,
                format!("Could not apply Remove to preview: {error:#}"),
            );
            self.preview.pending_stage = None;
            return;
        }
        if stage == ProcessingStage::Tone
            && self
                .preview
                .detail
                .as_ref()
                .is_some_and(|detail| detail.revision == self.preview.revision)
        {
            // Once full-frame statistics are ready, replace the temporary
            // navigation statistics in the crop without repeating RAW processing.
            self.preview.detail_pending_stage = Some(
                self.preview
                    .detail_pending_stage
                    .map_or(ProcessingStage::Tone, |pending| {
                        pending.min(ProcessingStage::Tone)
                    }),
            );
        }
        self.preview.pending_stage = match stage {
            ProcessingStage::Raw => Some(ProcessingStage::Tone),
            ProcessingStage::Tone => Some(ProcessingStage::Output),
            ProcessingStage::Output => None,
        };
    }
}
