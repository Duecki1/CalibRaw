use super::*;

impl CalibRawApp {
    pub(in crate::app) fn interactive_detail_mask_edit(&self) -> bool {
        self.preview.zoom > DETAIL_ZOOM_START
            && self.masks.interaction_dirty_layer.is_some()
            && self.egui_ctx.input(|input| input.pointer.primary_down())
    }

    /// Whether the fitted preview waits for the end of the current drag, which
    /// something else follows: the reduced preview of a dragged control
    /// (`interactive`), or, zoomed in, the crop alone. That is when a control
    /// or a mask is dragged while the sharp crop covers the view and the drag
    /// changes only the Output stage. The fitted preview and the navigation
    /// proxy keep their pending work until the pointer lifts.
    ///
    /// Raw and Tone changes still update every preview, because the crop
    /// takes its tone statistics from a full-frame one.
    pub(in crate::app) fn defer_background_preview_processing(&self) -> bool {
        self.interactive_following() || self.defer_for_zoomed_drag()
    }

    /// Whether the navigation proxy waits for the end of the current drag. It
    /// only supplies the crop's tone statistics, which Raw and Tone changes
    /// move: the reduced crop needs them too, so they are not waited for.
    pub(in crate::app) fn defer_navigation_preview(&self) -> bool {
        if self.interactive_following() && self.preview.zoomed_in() {
            return self
                .preview
                .navigation_pending_stage
                .is_none_or(|stage| stage == ProcessingStage::Output);
        }
        self.defer_background_preview_processing()
    }

    fn defer_for_zoomed_drag(&self) -> bool {
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
            raw.white_balance_offsets_from_area(
                area[0],
                area[1],
                self.develop.exposure.black_point,
                &self.develop.exposure.film,
            )
            .and_then(|offsets| raw.white_balance_from_offsets(offsets))
        });
        self.develop_ui.cancel_area_picker();
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

        if stage == ProcessingStage::Raw
            && !detail.raw.is_pre_demosaiced_raster()
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
        let params = DetailPlacement {
            mask_region,
            mask_extent,
            ..detail.placement()
        }
        .params(
            &self.develop.target_exposure,
            &preview_masks,
            self.develop.geometry,
            &detail.raw,
            self.preview_profile_color(&detail.raw),
        )
        .with_look_lut(self.develop.target_look.as_ref());

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

        if let Err(error) = dispatch_detail_stage(
            detail.pipeline.gpu(),
            &render_state.queue,
            &render_state.device,
            &params,
            stage,
            full_frame_tone_pipeline,
            detail.remove_scene(&self.inpaint.edits, full_raw, &self.develop.target_exposure),
        ) {
            self.report_error(
                ErrorKind::Preview,
                format!("Could not apply Remove to zoomed preview: {error:#}"),
            );
            self.preview.detail_pending_stage = None;
            return;
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
            let after = self.preview.dispatch_progress();
            if after != before {
                // What the batch is timed as is what the user waits for: the
                // reduced preview while it follows a drag, else the sharp
                // preview that is shown.
                let batch = if after.interactive() != before.interactive() {
                    Batch::Interactive
                } else if self.preview.zoomed_in() {
                    if before.detail().is_some() && after.detail() != before.detail() {
                        Batch::Detail
                    } else {
                        Batch::Other
                    }
                } else if before.fitted().is_some() && after.fitted() != before.fitted() {
                    Batch::Fitted
                } else {
                    Batch::Other
                };
                self.preview.dispatch_gate.close_until_idle(
                    &render_state.queue,
                    &self.egui_ctx,
                    batch,
                );
            }
        }
    }

    /// Renders every pending stage of the zoomed crop and the fitted
    /// preview, so each batch ends in a displayed update.
    fn advance_processing(&mut self, frame: &eframe::Frame) {
        let zoomed = self.preview.zoom > DETAIL_ZOOM_START;
        // Decided before the crop renders, which can make it current.
        let defer_fitted = self.defer_background_preview_processing();
        let following_drag = self.interactive_following();
        // The reduced preview follows a drag in the sharp one's place, which
        // keeps what it owes until the drag ends.
        if zoomed && !following_drag {
            self.drain_zoomed_processing(frame);
        }
        if following_drag {
            self.drain_pending_stages(
                |preview| preview.interactive.pending_stage(),
                |app| app.advance_interactive_stage(frame),
            );
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
        if self.fitted_mask_capacity_stale() {
            // Dispatching would read mask layers the graph does not have. Keep
            // the last frame until the rebuilt graph, which renders every
            // pending change, replaces it.
            self.preview.quality_dirty = true;
            self.egui_ctx.request_repaint();
            return;
        }
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

        let params = GpuParams::new_with_profile_color(
            &self.develop.target_exposure,
            preview_masks,
            raw,
            self.preview_profile_color(raw),
        )
        .with_vignette_geometry(self.develop.geometry)
        .with_look_lut(self.develop.target_look.as_ref());
        let Some(full_raw) = preview_source.as_ref() else {
            self.preview.pending_stage = None;
            return;
        };
        let look_lut_failure = pipeline
            .prepare_look_lut(&render_state.queue, params.look_lut())
            .err();
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
        if let Some(error) = look_lut_failure {
            self.report_look_lut_upload_failure(&error);
        }
    }
}

/// Renders `stage` of a zoomed crop's graph. The crop takes its tone
/// statistics from a full-frame graph (`full_frame`) when there is one, so the
/// tone does not depend on the part of the photo in view.
pub(in crate::app) fn dispatch_detail_stage(
    pipeline: &RawGpuPipeline,
    queue: &wgpu::Queue,
    device: &wgpu::Device,
    params: &GpuParams,
    stage: ProcessingStage,
    full_frame: Option<&RawGpuPipeline>,
    remove: RemoveSceneContext<'_>,
) -> anyhow::Result<()> {
    if stage == ProcessingStage::Tone {
        match full_frame {
            Some(full_frame) => pipeline
                .dispatch_tone_guide_with_inherited_statistics(queue, device, params, full_frame),
            None => pipeline.dispatch_stage(queue, device, params, ProcessingStage::Tone),
        }
        return Ok(());
    }
    if stage == ProcessingStage::Output {
        if let Some(full_frame) = full_frame {
            pipeline.inherit_tone_statistics(queue, device, params, full_frame);
        }
    }
    pipeline.dispatch_stage_with_remove(queue, device, params, stage, remove)
}
