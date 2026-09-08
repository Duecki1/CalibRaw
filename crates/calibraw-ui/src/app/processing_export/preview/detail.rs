use super::*;

impl CalibRawApp {
    pub(in crate::app) fn advance_preview_detail(&mut self, _frame: &eframe::Frame) {
        if self.preview.zoom <= DETAIL_ZOOM_START {
            if let Some(old) = self.preview.detail.take() {
                if let Some(texture_id) = old.pipeline.egui_texture_id {
                    self.retire_egui_texture(texture_id);
                }
            }
            self.preview.motion_at = None;
            self.preview.detail_pending_stage = None;
            self.preview.detail_urgent = false;
            return;
        }
        if self.preview.touch_navigation_active
            || self.ui.active_tab != AppTab::Develop
            || self.preview.quality_dirty
            || self.develop.lens_correction_dirty
            || self.lens_correction_busy()
            || self.develop.load_receiver.is_some()
            || self.foreground_operation_is(ForegroundOperationKind::AiDenoise)
            || self.preview.detail_rebuild_receiver.is_some()
        {
            return;
        }
        if self.preview_detail_is_current() {
            return;
        }

        let delay = detail_refresh_delay(
            self.preview.motion_at,
            self.preview.detail_urgent,
            Instant::now(),
        );
        if !delay.is_zero() {
            self.egui_ctx.request_repaint_after(delay);
            return;
        }

        let Some(source_raw) = self.develop.loaded_raw.as_ref().map(Arc::clone) else {
            return;
        };
        let request = PreviewDetailRequest {
            source_raw,
            revision: self.preview.revision,
            visible: self.preview.visible_uv,
            viewport_pixels: self.preview.source_viewport_pixels(),
            quality: self.preview.quality,
            processing_halo: self.preview_detail_halo(),
            exposure: if self.preview.original_requested {
                self.preview.original_exposure
            } else {
                self.develop.target_exposure
            },
        };
        self.preview.motion_at = None;
        self.preview.detail_urgent = false;

        let (sender, receiver) = std::sync::mpsc::channel();
        let context = self.egui_ctx.clone();
        match std::thread::Builder::new()
            .name("calibraw-preview-detail".to_owned())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    prepare_preview_detail(request)
                }))
                .unwrap_or_else(|panic| {
                    let message = panic
                        .downcast_ref::<&str>()
                        .copied()
                        .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
                        .unwrap_or("unknown panic");
                    Err(anyhow::anyhow!(
                        "zoom preview preparation panicked: {message}"
                    ))
                })
                .map_err(|error| format!("{error:#}"));
                let _ = sender.send(PreviewDetailRebuildEvent::Finished(result));
                context.request_repaint();
            }) {
            Ok(_) => {
                self.preview.detail_rebuild_receiver = Some(receiver);
                self.egui_ctx
                    .request_repaint_after(Duration::from_millis(50));
            }
            Err(error) => {
                self.retry_preview_detail_later();
                self.ui.notice = Some(format!("Could not start zoom-preview preparation: {error}"));
            }
        }
    }

    pub(in crate::app) fn poll_preview_detail_rebuild_worker(&mut self, frame: &eframe::Frame) {
        let received = self
            .preview
            .detail_rebuild_receiver
            .as_ref()
            .map(std::sync::mpsc::Receiver::try_recv);
        let event = match received {
            Some(Ok(event)) => Some(event),
            Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                self.preview.detail_rebuild_receiver = None;
                self.retry_preview_detail_later();
                self.ui.notice = Some("Zoom-preview worker stopped unexpectedly.".to_owned());
                None
            }
            Some(Err(std::sync::mpsc::TryRecvError::Empty)) | None => None,
        };
        let Some(PreviewDetailRebuildEvent::Finished(result)) = event else {
            return;
        };
        self.preview.detail_rebuild_receiver = None;
        let prepared = match result {
            Ok(prepared) => prepared,
            Err(error) => {
                self.retry_preview_detail_later();
                self.ui.notice = Some(format!("Could not prepare the zoomed preview: {error}"));
                return;
            }
        };
        let source_is_current = self
            .develop
            .loaded_raw
            .as_ref()
            .is_some_and(|raw| Arc::ptr_eq(raw, &prepared.source_raw));
        if self.preview.zoom <= DETAIL_ZOOM_START
            || self.ui.active_tab != AppTab::Develop
            || !source_is_current
            || prepared.revision != self.preview.revision
            || prepared.processing_halo < self.preview_detail_halo()
            || prepared.quality != self.preview.quality
            || self.preview.quality_dirty
        {
            if source_is_current && self.preview.zoom > DETAIL_ZOOM_START {
                self.preview.detail_urgent = true;
                self.preview.motion_at = Some(Instant::now());
            }
            return;
        }
        // The user may have returned to the already-sharp cached view while
        // this worker was running. Never replace it with a coarser late result.
        if self.preview_detail_is_current() {
            return;
        }
        // Coalesce navigation while the one preparation worker is running.
        // A completely offscreen result is no longer worth uploading to the GPU.
        if self.preview.touch_navigation_active
            || !(0..2).all(|axis| {
                prepared.visible.max[axis] > self.preview.visible_uv.min[axis]
                    && prepared.visible.min[axis] < self.preview.visible_uv.max[axis]
            })
        {
            self.egui_ctx.request_repaint();
            return;
        }
        if self.install_prepared_preview_detail(frame, prepared) {
            self.preview.detail_pending_stage = None;
            if self.preview.original_requested {
                self.preview.original_rendered_state = None;
            }
        } else {
            // Avoid a tight allocation/retry loop after a transient GPU failure.
            self.retry_preview_detail_later();
        }
        self.egui_ctx.request_repaint();
    }

    fn retry_preview_detail_later(&mut self) {
        self.preview.detail_urgent = false;
        self.preview.motion_at = (self.preview.zoom > DETAIL_ZOOM_START)
            .then(|| Instant::now() + Duration::from_secs(1));
        self.egui_ctx
            .request_repaint_after(Duration::from_secs(1) + zoom_detail_idle_delay());
    }

    fn install_prepared_preview_detail(
        &mut self,
        frame: &eframe::Frame,
        prepared: PreparedPreviewDetail,
    ) -> bool {
        let Some(full_raw) = self.develop.loaded_raw.as_ref().map(Arc::clone) else {
            return false;
        };
        let Some(render_state) = frame.wgpu_render_state() else {
            self.preview.detail_urgent = true;
            self.preview.motion_at = Some(Instant::now());
            return false;
        };
        let PreparedPreviewDetail {
            revision,
            visible,
            texture_uv_rect,
            source_origin,
            source_size,
            raw: detail_raw,
            processing_halo,
            ..
        } = prepared;
        let [x0, y0] = source_origin;
        let [crop_width, crop_height] = source_size;
        let virtual_full_width = ((detail_raw.width as f64 * full_raw.width as f64
            / crop_width.max(1) as f64)
            .round() as u32)
            .max(detail_raw.width);
        let virtual_full_height = ((detail_raw.height as f64 * full_raw.height as f64
            / crop_height.max(1) as f64)
            .round() as u32)
            .max(detail_raw.height);
        let virtual_origin_x =
            (x0 as f64 / full_raw.width.max(1) as f64 * virtual_full_width as f64).round() as i32;
        let virtual_origin_y =
            (y0 as f64 / full_raw.height.max(1) as f64 * virtual_full_height as f64).round() as i32;
        let mask_region = detail_mask_source_region(
            &self.masks.stack,
            source_origin,
            source_size,
            full_raw.width,
            full_raw.height,
        );
        if detail_uses_opposed_chroma(&full_raw, &self.develop.target_exposure) {
            full_raw.inpaint_opposed_chroma_for_exposure(&self.develop.target_exposure);
        }
        let params = GpuParams::new_for_tile(
            &self.develop.target_exposure,
            &self.masks.stack,
            &detail_raw,
            virtual_origin_x,
            virtual_origin_y,
            virtual_full_width,
            virtual_full_height,
        )
        .with_vignette_geometry(self.develop.geometry)
        .with_mask_uv_rect_and_extent(
            mask_source_region_uv(mask_region, full_raw.width, full_raw.height),
            mask_region_texture_extent(mask_region, detail_mask_edge()),
        );
        let normal_tone_is_current = !matches!(
            self.preview.pending_stage,
            Some(ProcessingStage::Raw | ProcessingStage::Tone)
        );
        let full_frame_tone_pipeline = if normal_tone_is_current {
            self.preview.gpu_pipeline.as_ref().or_else(|| {
                self.preview
                    .navigation
                    .as_ref()
                    .map(|preview| &preview.pipeline)
            })
        } else {
            self.preview
                .navigation
                .as_ref()
                .map(|preview| &preview.pipeline)
                .or(self.preview.gpu_pipeline.as_ref())
        };
        let required_mask_layers = self.masks.stack.masks.len().max(1);
        if let Some(detail) = self.preview.detail.as_mut().filter(|detail| {
            detail.pipeline.width == detail_raw.width
                && detail.pipeline.height == detail_raw.height
                && detail
                    .pipeline
                    .tone_guide_supports_origin(virtual_origin_x, virtual_origin_y)
                && detail.pipeline.mask_layer_capacity() >= required_mask_layers
        }) {
            if let Err(error) = detail
                .pipeline
                .upload_raw_tile(&render_state.queue, &detail_raw)
            {
                self.ui.notice = Some(format!(
                    "Could not update the zoomed preview crop: {error:#}"
                ));
                return false;
            }
            if let Err(error) = Self::upload_detail_masks(
                &detail.pipeline,
                &render_state.queue,
                &self.masks.stack,
                &full_raw,
                mask_region,
                None,
            ) {
                self.ui.notice = Some(error);
                return false;
            }
            if let Err(error) = detail.pipeline.dispatch_stage_with_remove(
                &render_state.queue,
                &render_state.device,
                &params,
                ProcessingStage::Raw,
                RemoveSceneContext::new(
                    &self.inpaint.edits,
                    &full_raw,
                    &self.develop.target_exposure,
                    [x0 as f32, y0 as f32],
                    [crop_width as f32, crop_height as f32],
                ),
            ) {
                self.ui.notice = Some(format!(
                    "Could not apply Remove to zoomed preview: {error:#}"
                ));
                return false;
            }
            if let Some(full_frame) = full_frame_tone_pipeline {
                detail
                    .pipeline
                    .dispatch_tone_guide_with_inherited_statistics(
                        &render_state.queue,
                        &render_state.device,
                        &params,
                        full_frame,
                    );
            } else {
                detail.pipeline.dispatch_stage(
                    &render_state.queue,
                    &render_state.device,
                    &params,
                    ProcessingStage::Tone,
                );
            }
            detail.pipeline.dispatch_stage(
                &render_state.queue,
                &render_state.device,
                &params,
                ProcessingStage::Output,
            );
            detail.uv_rect = visible;
            detail.texture_uv_rect = texture_uv_rect;
            detail.revision = revision;
            detail.raw = detail_raw;
            detail.source_origin = source_origin;
            detail.source_size = source_size;
            detail.processing_halo = processing_halo;
            detail.full_source_size = [full_raw.width, full_raw.height];
            detail.mask_source_region = mask_region;
            detail.virtual_origin = [virtual_origin_x, virtual_origin_y];
            detail.virtual_full_size = [virtual_full_width, virtual_full_height];
            self.masks.detail_dirty_layers.fill(false);
            self.egui_ctx.request_repaint();
            return true;
        }

        // On mobile the fitted graph and one detail graph share the budget.
        // Release an incompatible cached graph only once its replacement is
        // ready to upload, keeping the fitted image available on failure.
        #[cfg(target_os = "android")]
        if let Some(old) = self.preview.detail.take() {
            if let Some(id) = old.pipeline.egui_texture_id {
                render_state.renderer.write().free_texture(&id);
            }
        }
        let Some(program_template) = self.preview.gpu_pipeline.as_ref() else {
            return false;
        };
        let mut pipeline = match RawGpuPipeline::new_headless_reusing_programs_with_mask_edge(
            &render_state.device,
            &render_state.queue,
            &detail_raw,
            &params,
            ProcessingQuality::Preview,
            program_template,
            detail_mask_edge(),
        ) {
            Ok(pipeline) => pipeline,
            Err(error) => {
                self.ui.notice = Some(format!("Could not render the zoomed preview: {error:#}"));
                return false;
            }
        };
        if let Err(error) = Self::upload_detail_masks(
            &pipeline,
            &render_state.queue,
            &self.masks.stack,
            &full_raw,
            mask_region,
            None,
        ) {
            self.ui.notice = Some(error);
            return false;
        }
        if let Err(error) = pipeline.dispatch_stage_with_remove(
            &render_state.queue,
            &render_state.device,
            &params,
            ProcessingStage::Raw,
            RemoveSceneContext::new(
                &self.inpaint.edits,
                &full_raw,
                &self.develop.target_exposure,
                [x0 as f32, y0 as f32],
                [crop_width as f32, crop_height as f32],
            ),
        ) {
            self.ui.notice = Some(format!(
                "Could not apply Remove to zoomed preview: {error:#}"
            ));
            return false;
        }
        if let Some(full_frame) = full_frame_tone_pipeline {
            pipeline.dispatch_tone_guide_with_inherited_statistics(
                &render_state.queue,
                &render_state.device,
                &params,
                full_frame,
            );
        } else {
            pipeline.dispatch_stage(
                &render_state.queue,
                &render_state.device,
                &params,
                ProcessingStage::Tone,
            );
        }
        pipeline.dispatch_stage(
            &render_state.queue,
            &render_state.device,
            &params,
            ProcessingStage::Output,
        );

        let mut renderer = render_state.renderer.write();
        if let Some(old) = self.preview.detail.take() {
            if let Some(texture_id) = old.pipeline.egui_texture_id {
                self.retire_egui_texture(texture_id);
            }
        }
        pipeline.register_egui_texture(&render_state.device, &mut renderer);
        drop(renderer);

        self.preview.detail = Some(PreviewDetail {
            pipeline,
            uv_rect: visible,
            texture_uv_rect,
            revision,
            raw: detail_raw,
            processing_halo,
            source_origin,
            source_size,
            full_source_size: [full_raw.width, full_raw.height],
            mask_source_region: mask_region,
            virtual_origin: [virtual_origin_x, virtual_origin_y],
            virtual_full_size: [virtual_full_width, virtual_full_height],
        });
        self.masks.detail_dirty_layers.fill(false);
        self.egui_ctx.request_repaint();
        true
    }
}

impl PreviewDetail {
    pub(super) fn needs_native_refinement(&self, requested: &PreviewDetailPlan) -> bool {
        if self.raw.is_pre_demosaiced_raster()
            || [self.raw.width, self.raw.height] == self.source_size
        {
            return false;
        }
        settled_detail_uses_native_source(
            true,
            requested.size[0],
            requested.size[1],
            requested.edge,
        )
    }
}

fn detail_refresh_delay(motion_at: Option<Instant>, urgent: bool, now: Instant) -> Duration {
    if urgent {
        return Duration::ZERO;
    }
    motion_at.map_or(Duration::ZERO, |at| {
        (at + zoom_detail_idle_delay()).saturating_duration_since(now)
    })
}

struct PreviewDetailRequest {
    source_raw: Arc<LoadedRaw>,
    revision: u64,
    visible: PreviewUvRect,
    viewport_pixels: [u32; 2],
    quality: PreviewQuality,
    exposure: ExposureParams,
    processing_halo: u32,
}

fn prepare_preview_detail(request: PreviewDetailRequest) -> anyhow::Result<PreparedPreviewDetail> {
    let PreviewDetailRequest {
        source_raw,
        revision,
        visible,
        viewport_pixels,
        quality,
        exposure,
        processing_halo,
    } = request;
    let plan = PreviewDetailPlan::new(
        [source_raw.width, source_raw.height],
        source_raw.cfa_kind,
        visible,
        viewport_pixels,
        quality,
        processing_halo,
    );
    let [x0, y0] = plan.origin;
    let [crop_width, crop_height] = plan.size;
    let [x1, y1] = [x0 + crop_width, y0 + crop_height];
    let source_size = plan.size;
    let crop_uv = PreviewUvRect {
        min: [
            x0 as f32 / source_raw.width.max(1) as f32,
            y0 as f32 / source_raw.height.max(1) as f32,
        ],
        max: [
            x1 as f32 / source_raw.width.max(1) as f32,
            y1 as f32 / source_raw.height.max(1) as f32,
        ],
    };
    let requested_edge = plan.edge;
    if detail_uses_opposed_chroma(&source_raw, &exposure) {
        source_raw.inpaint_opposed_chroma_for_exposure(&exposure);
    }
    let raw = Arc::new(
        if settled_detail_uses_native_source(
            !source_raw.is_pre_demosaiced_raster(),
            source_size[0],
            source_size[1],
            requested_edge,
        ) {
            // The detail texture is sampled down to the viewport only after the
            // complete GPU graph has run.  Keeping the mosaic native here is what
            // preserves clipping decisions, CFA detail, and sensor-noise scale.
            let mut native =
                crate::pipeline::crop_raw(&source_raw, x0, y0, source_size[0], source_size[1]);
            native.opposed_chroma_cache = Arc::clone(&source_raw.opposed_chroma_cache);
            native
        } else {
            build_region_proxy(
                &source_raw,
                x0,
                y0,
                source_size[0],
                source_size[1],
                ProxySpec {
                    max_edge: requested_edge,
                },
            )
        },
    );
    let visible = detail_display_uv(visible, crop_uv, [raw.width, raw.height], processing_halo);
    Ok(PreparedPreviewDetail {
        source_raw,
        processing_halo,
        revision,
        quality,
        visible,
        texture_uv_rect: detail_texture_uv(visible, crop_uv),
        source_origin: [x0, y0],
        source_size,
        raw,
    })
}

fn settled_detail_uses_native_source(
    is_sensor_raw: bool,
    region_width: u32,
    region_height: u32,
    requested_edge: u32,
) -> bool {
    if !is_sensor_raw {
        return false;
    }

    // A settled detail may spend more memory than the continuously updated
    // proxy, but remains bounded so a near-fit view cannot allocate a full
    // high-megapixel processing graph.  As zoom increases the native region
    // eventually fits and becomes the authoritative fidelity path.
    let platform_limit = if cfg!(target_os = "android") {
        2_048
    } else {
        4_096
    };
    let native_edge = requested_edge.saturating_mul(2).min(platform_limit);
    region_width.max(region_height) <= native_edge
        && PreviewQuality::bounded_source_edge(
            region_width,
            region_height,
            region_width.max(region_height),
        ) == region_width.max(region_height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_detail_recovers_without_a_motion_timer() {
        assert_eq!(
            detail_refresh_delay(None, false, Instant::now()),
            Duration::ZERO
        );
    }

    #[test]
    fn navigation_debounces_but_edits_can_refresh_immediately() {
        let now = Instant::now();
        assert_eq!(
            detail_refresh_delay(Some(now), false, now),
            zoom_detail_idle_delay()
        );
        assert_eq!(detail_refresh_delay(Some(now), true, now), Duration::ZERO);
        assert_eq!(
            detail_refresh_delay(Some(now), false, now + zoom_detail_idle_delay()),
            Duration::ZERO
        );
        assert!(
            detail_refresh_delay(Some(now + Duration::from_secs(1)), false, now)
                >= Duration::from_secs(1)
        );
    }

    #[test]
    fn settled_sensor_detail_uses_native_samples_before_the_resource_limit() {
        assert!(settled_detail_uses_native_source(true, 3_800, 2_500, 2_000));
    }

    #[test]
    fn wide_detail_remains_an_explicit_bounded_approximation() {
        assert!(!settled_detail_uses_native_source(
            true, 6_000, 4_000, 2_000
        ));
    }

    #[test]
    fn developed_rasters_keep_their_existing_resize_path() {
        assert!(!settled_detail_uses_native_source(false, 1_000, 700, 2_000));
    }
}
