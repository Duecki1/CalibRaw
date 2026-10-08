use super::*;

impl CalibRawApp {
    pub(in crate::app) fn interactive_detail_mask_edit(&self) -> bool {
        self.preview.zoom > DETAIL_ZOOM_START
            && self.masks.interaction_dirty_layer.is_some()
            && self.egui_ctx.input(|input| input.pointer.primary_down())
    }

    pub(in crate::app) fn defer_background_mask_processing(&self) -> bool {
        self.interactive_detail_mask_edit()
            && !self.preview.white_balance_refresh_pending
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
        let preview_source = self.preview_source_raw();
        self.note_edit_changed();
        let next_exposure = self.preview_exposure();
        if self.preview.gpu_pipeline.is_none() {
            self.develop.target_exposure = next_exposure;
            return;
        }

        if let Some(stage) = affected_stage(&self.develop.target_exposure, &next_exposure) {
            if self.develop.target_exposure.temperature != next_exposure.temperature
                || self.develop.target_exposure.tint != next_exposure.tint
            {
                self.preview.white_balance_refresh_pending = true;
            }
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
        let preview_source = self.preview_source_raw();
        let result = preview_source.as_ref().and_then(|raw| {
            raw.white_balance_offsets_from_area(area[0], area[1], self.develop.exposure.black_point)
        });
        self.develop_ui.cancel_white_balance_picker();
        let Some((temperature, tint)) = result else {
            self.report_error(ErrorKind::Sampling, "Could not estimate white balance there. Choose a brighter, unclipped neutral area."
                    .to_owned(),);
            self.egui_ctx.request_repaint();
            return false;
        };
        self.develop.exposure.temperature = temperature;
        self.develop.exposure.tint = tint;
        self.ui.notice = Some("White balance sampled from the selected image area.".to_owned());
        self.mark_pipeline_dirty();
        true
    }

    pub(in crate::app) fn advance_zoomed_processing(&mut self, frame: &eframe::Frame) {
        let preview_masks = self.preview_mask_stack();
        let preview_source = self.preview_source_raw();
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

    pub(in crate::app) fn advance_processing(&mut self, frame: &eframe::Frame) {
        let exact_white_balance_refresh = self.preview.white_balance_refresh_pending;
        if !self
            .preview
            .interactive_render_ready
            .load(std::sync::atomic::Ordering::Acquire)
        {
            // Edits continue to update target_exposure while the GPU is busy.
            // The completion callback requests another frame for the newest
            // value, avoiding an unbounded queue of obsolete scrub renders.
            return;
        }
        if self.defer_background_mask_processing() {
            // The sharp crop covers this view. Coalesce GPU work and postpone
            // the fitted/navigation fallbacks until the slider is released.
            if self.preview.detail_pending_stage.is_some() {
                self.preview
                    .interactive_render_ready
                    .store(false, std::sync::atomic::Ordering::Release);
                self.advance_zoomed_processing(frame);
                self.finish_interactive_preview_render(frame);
            }
            return;
        }
        if exact_white_balance_refresh {
            self.preview
                .interactive_render_ready
                .store(false, std::sync::atomic::Ordering::Release);
        }

        let drain_detail = |app: &mut Self| {
            for _ in 0..3 {
                let before = app.preview.detail_pending_stage;
                if before.is_none() {
                    break;
                }
                app.advance_zoomed_processing(frame);
                if app.preview.detail_pending_stage == before {
                    break;
                }
            }
        };

        let preview_masks = self.preview_mask_stack();
        let preview_source = self.preview_source_raw();
        if self.preview.zoom > DETAIL_ZOOM_START {
            if exact_white_balance_refresh {
                drain_detail(self);
            } else {
                self.advance_zoomed_processing(frame);
            }
            // Refresh the fitted fallback too: panning can expose any part of it.
        }

        if exact_white_balance_refresh {
            for _ in 0..3 {
                let before = self.preview.pending_stage;
                if before.is_none() {
                    break;
                }
                self.advance_main_processing_stage(frame, &preview_masks, &preview_source);
                if self.preview.pending_stage == before {
                    break;
                }
            }
            // Full-frame tone statistics supersede the temporary statistics
            // used by a zoomed crop, so finish that small downstream refresh too.
            if self.preview.zoom > DETAIL_ZOOM_START {
                drain_detail(self);
            }
            self.preview.white_balance_refresh_pending = false;

            self.finish_interactive_preview_render(frame);
            return;
        }

        self.advance_main_processing_stage(frame, &preview_masks, &preview_source);
    }

    fn finish_interactive_preview_render(&self, frame: &eframe::Frame) {
        if let Some(render_state) = frame.wgpu_render_state() {
            let ready = Arc::clone(&self.preview.interactive_render_ready);
            let repaint = self.egui_ctx.clone();
            render_state.queue.on_submitted_work_done(move || {
                ready.store(true, std::sync::atomic::Ordering::Release);
                repaint.request_repaint();
            });
        } else {
            self.preview
                .interactive_render_ready
                .store(true, std::sync::atomic::Ordering::Release);
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
