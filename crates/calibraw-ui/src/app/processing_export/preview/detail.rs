use super::*;
use crate::i18n::{tr, tr_fmt};

impl CalibRawApp {
    pub(in crate::app) fn advance_preview_detail(&mut self, _frame: &eframe::Frame) {
        let preview_source = self.develop.loaded_raw.clone();
        // The zoomed detail is a second full GPU pipeline. It gives way to a
        // GPU AI job and is rebuilt when the job ends.
        if self.preview.zoom <= DETAIL_ZOOM_START || self.ai_job_may_use_gpu() {
            self.preview.cancel_detail_rebuild();
            self.preview.detail = None;
            self.preview.motion_at = None;
            self.preview.detail_pending_stage = None;
            self.preview.detail_urgent = false;
            self.preview.detail_block = None;
            return;
        }
        // While a control is dragged the reduced crop follows it
        // (`interactive`); the sharp one is refreshed once the drag ends.
        if self.interactive_following() {
            return;
        }
        // Fingers on the photo do not block the refresh: each move restarts
        // the idle delay below, so the crop is prepared once a pinch or pan
        // pauses.
        let blocked = if self.ui.active_tab != AppTab::Develop {
            Some("another tab is open")
        } else if self.preview.quality_dirty {
            Some("the fitted preview is being rebuilt")
        } else if self.develop.lens_correction_dirty || self.lens_correction_busy() {
            Some("lens correction is running")
        } else if self.develop.load_receiver.is_some() {
            Some("a photo is loading")
        } else if self.ai_denoise_running() {
            Some("AI denoise is running")
        } else if self.preview.detail_rebuild.is_some() {
            Some("the previous crop is still being prepared")
        } else {
            None
        };
        if blocked != self.preview.detail_block {
            self.preview.detail_block = blocked;
            if let Some(reason) = blocked {
                calibraw_core::diagnostics::record(format!(
                    "Zoom detail waits: {reason} (zoom {:.2})",
                    self.preview.zoom
                ));
            }
        }
        if blocked.is_some() {
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

        let Some(source_raw) = preview_source.as_ref().map(Arc::clone) else {
            return;
        };
        let view = self
            .preview
            .detail_view([source_raw.width, source_raw.height]);
        calibraw_core::diagnostics::record(format!(
            "Zoom detail requested: zoom {:.2}, visible {:?}, canvas {:?}, visible pixels {:?}",
            self.preview.zoom, self.preview.visible_uv, view.canvas_pixels, view.visible_pixels
        ));
        let cancel = Arc::new(AtomicBool::new(false));
        let request = PreviewDetailRequest {
            cancel: Arc::clone(&cancel),
            source_raw,
            revision: self.preview.revision,
            visible: self.preview.visible_uv,
            view,
            quality: self.preview.quality,
            processing_halo: self.preview_detail_halo(),
            graph: self.detail_graph(),
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
                    Err(anyhow::anyhow!(tr_fmt(
                        "zoom preview preparation panicked: {message}",
                        &[("message", &message)],
                    )))
                })
                .map_err(|error| format!("{error:#}"));
                let _ = sender.send(match result {
                    Ok(Some(prepared)) => PreviewDetailRebuildEvent::Finished(Ok(prepared)),
                    Ok(None) => PreviewDetailRebuildEvent::Cancelled,
                    Err(error) => PreviewDetailRebuildEvent::Finished(Err(error)),
                });
                context.request_repaint();
            }) {
            Ok(_) => {
                self.preview.detail_rebuild = Some(DetailRebuild::new(receiver, cancel));
                self.egui_ctx
                    .request_repaint_after(Duration::from_millis(50));
            }
            Err(error) => {
                self.retry_preview_detail_later();
                self.report_error(
                    ErrorKind::Preview,
                    tr_fmt(
                        "Could not start zoom-preview preparation: {error}",
                        &[("error", &error)],
                    ),
                );
            }
        }
    }

    pub(in crate::app) fn poll_preview_detail_rebuild_worker(&mut self, frame: &eframe::Frame) {
        // A crop that is ready waits for the end of the drag a reduced
        // preview follows: installing it renders the whole crop on the GPU,
        // which the drag would then have to wait behind.
        if self.interactive_following() {
            return;
        }
        let preview_source = self.develop.loaded_raw.clone();
        let received = self
            .preview
            .detail_rebuild
            .as_ref()
            .map(|rebuild| rebuild.receiver.try_recv());
        let event = match received {
            Some(Ok(event)) => Some(event),
            Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                self.preview.detail_rebuild = None;
                self.retry_preview_detail_later();
                self.report_error(
                    ErrorKind::Preview,
                    tr("Zoom-preview worker stopped unexpectedly.").to_owned(),
                );
                None
            }
            Some(Err(std::sync::mpsc::TryRecvError::Empty)) | None => None,
        };
        let Some(event) = event else {
            return;
        };
        self.preview.detail_rebuild = None;
        let PreviewDetailRebuildEvent::Finished(result) = event else {
            // Cancelled because the view moved on: the next crop is requested
            // once it settles.
            calibraw_core::diagnostics::record(
                "Zoom detail cancelled: the view moved on before it was ready",
            );
            self.egui_ctx.request_repaint();
            return;
        };
        let prepared = match result {
            Ok(prepared) => prepared,
            Err(error) => {
                self.retry_preview_detail_later();
                self.report_error(
                    ErrorKind::Preview,
                    tr_fmt(
                        "Could not prepare the zoomed preview: {error}",
                        &[("error", &error)],
                    ),
                );
                return;
            }
        };
        let source_is_current = preview_source
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
            calibraw_core::diagnostics::record(format!(
                "Zoom detail discarded as outdated (zoom {:.2}, revision {} vs {}, quality dirty {})",
                self.preview.zoom, prepared.revision, self.preview.revision, self.preview.quality_dirty
            ));
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
        // Coalesce navigation while the one preparation worker is running: under
        // fingers, a move since the request means the view has left what was
        // prepared. A completely offscreen result is no longer worth uploading
        // to the GPU.
        if (self.preview.touch_navigation_active && self.preview.motion_at.is_some())
            || !(0..2).all(|axis| {
                prepared.visible.max[axis] > self.preview.visible_uv.min[axis]
                    && prepared.visible.min[axis] < self.preview.visible_uv.max[axis]
            })
        {
            self.egui_ctx.request_repaint();
            return;
        }
        let installed_at = Instant::now();
        if self.install_prepared_preview_detail(frame, prepared) {
            calibraw_core::diagnostics::record(format!(
                "Zoom detail installed in {:.3}s: {}",
                installed_at.elapsed().as_secs_f64(),
                self.preview.detail.as_ref().map_or_else(
                    || tr("no texture").to_owned(),
                    |detail| tr_fmt(
                        "texture {width}x{height} for source crop {source_size}",
                        &[
                            ("width", &detail.pipeline.gpu().width),
                            ("height", &detail.pipeline.gpu().height),
                            ("source_size", &format!("{:?}", detail.source_size))
                        ]
                    )
                )
            ));
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
        let preview_masks = self.preview_mask_stack();
        let preview_source = self.develop.loaded_raw.clone();
        let Some(full_raw) = preview_source.as_ref().map(Arc::clone) else {
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
        let ([virtual_origin_x, virtual_origin_y], [virtual_full_width, virtual_full_height]) =
            detail_virtual_frame(
                [detail_raw.width, detail_raw.height],
                [full_raw.width, full_raw.height],
                source_origin,
                source_size,
            );
        let interactive = self.interactive_detail_mask_edit();
        let mask_region = detail_mask_update_region(
            &preview_masks,
            source_origin,
            source_size,
            [full_raw.width, full_raw.height],
            None,
            interactive,
        );
        let mask_extent = detail_mask_texture_extent(mask_region, detail_mask_edge(), interactive);
        if full_raw.uses_opposed_chroma(&self.develop.target_exposure) {
            full_raw.inpaint_opposed_chroma_for_exposure(&self.develop.target_exposure);
        }
        let placement = DetailPlacement {
            virtual_origin: [virtual_origin_x, virtual_origin_y],
            virtual_full_size: [virtual_full_width, virtual_full_height],
            full_size: [full_raw.width, full_raw.height],
            mask_region,
            mask_extent,
        };
        let params = placement
            .params(
                &self.develop.target_exposure,
                &preview_masks,
                self.develop.geometry,
                &detail_raw,
                self.preview_profile_color(&detail_raw),
            )
            .with_look_lut(self.develop.target_look.as_ref());
        let full_frame_tone_pipeline = full_frame_tone_pipeline(
            self.preview.gpu_pipeline.as_ref(),
            self.preview.navigation.as_ref(),
            self.preview.pending_stage,
        );
        let required_mask_layers = preview_masks.masks.len().max(1);
        if let Some(detail) = self.preview.detail.as_mut().filter(|detail| {
            detail.pipeline.gpu().width == detail_raw.width
                && detail.pipeline.gpu().height == detail_raw.height
                && detail.pipeline.gpu().pixel_layout() == detail_raw.pixel_layout()
                && detail
                    .pipeline
                    .gpu()
                    .tone_guide_supports_origin(virtual_origin_x, virtual_origin_y)
                && detail.pipeline.gpu().mask_layer_capacity() >= required_mask_layers
        }) {
            if let Err(error) = detail
                .pipeline
                .gpu()
                .upload_raw_tile(&render_state.queue, &detail_raw)
            {
                self.report_error(
                    ErrorKind::Preview,
                    tr_fmt(
                        "Could not update the zoomed preview crop: {error}",
                        &[("error", &format!("{error:#}"))],
                    ),
                );
                return false;
            }
            if let Err(error) = Self::upload_detail_masks(
                detail.pipeline.gpu(),
                &render_state.queue,
                &preview_masks,
                &full_raw,
                mask_region,
                mask_extent,
                None,
            ) {
                self.report_error(ErrorKind::Preview, error);
                return false;
            }
            if let Err(error) = detail.pipeline.gpu().dispatch_stage_with_remove(
                &render_state.queue,
                &render_state.device,
                &params,
                ProcessingStage::Raw,
                detail_remove_scene(
                    &self.inpaint.edits,
                    &full_raw,
                    &self.develop.target_exposure,
                    source_origin,
                    source_size,
                ),
            ) {
                self.report_error(
                    ErrorKind::Preview,
                    tr_fmt(
                        "Could not apply Remove to zoomed preview: {error}",
                        &[("error", &format!("{error:#}"))],
                    ),
                );
                return false;
            }
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
            detail.pipeline.gpu().dispatch_stage(
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
            detail.mask_texture_extent = mask_extent;
            detail.virtual_origin = [virtual_origin_x, virtual_origin_y];
            detail.virtual_full_size = [virtual_full_width, virtual_full_height];
            self.masks.detail_dirty_layers.fill(false);
            self.egui_ctx.request_repaint();
            return true;
        }

        // On mobile the fitted graph and one detail graph share the budget.
        // Release an incompatible cached graph only once its replacement is
        // ready to upload, keeping the fitted image available on failure.
        if self.caps().constrained_memory() {
            if let Some(old) = self.preview.detail.take() {
                old.pipeline.free_now(&mut render_state.renderer.write());
            }
        }
        let Some(program_template) = self.preview.pipeline() else {
            return false;
        };
        let pipeline = match RawGpuPipeline::new(
            &render_state.device,
            &render_state.queue,
            &detail_raw,
            &params,
            detail_pipeline_options(preview_masks.masks.len())
                .profile_preview(crate::app::lifecycle::reserves_profile_preview(&detail_raw))
                .programs(&program_template.program_template()),
        ) {
            Ok(pipeline) => pipeline,
            Err(error) => {
                self.report_error(
                    ErrorKind::Preview,
                    tr_fmt(
                        "Could not render the zoomed preview: {error}",
                        &[("error", &format!("{error:#}"))],
                    ),
                );
                return false;
            }
        };
        if let Err(error) = Self::upload_detail_masks(
            &pipeline,
            &render_state.queue,
            &preview_masks,
            &full_raw,
            mask_region,
            mask_extent,
            None,
        ) {
            self.report_error(ErrorKind::Preview, error);
            return false;
        }
        if let Err(error) = pipeline.dispatch_stage_with_remove(
            &render_state.queue,
            &render_state.device,
            &params,
            ProcessingStage::Raw,
            detail_remove_scene(
                &self.inpaint.edits,
                &full_raw,
                &self.develop.target_exposure,
                source_origin,
                source_size,
            ),
        ) {
            self.report_error(
                ErrorKind::Preview,
                tr_fmt(
                    "Could not apply Remove to zoomed preview: {error}",
                    &[("error", &format!("{error:#}"))],
                ),
            );
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

        self.preview.detail = None;
        let pipeline = self.present_pipeline(pipeline, render_state);

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
            mask_texture_extent: mask_extent,
            virtual_origin: [virtual_origin_x, virtual_origin_y],
            virtual_full_size: [virtual_full_width, virtual_full_height],
        });
        self.masks.detail_dirty_layers.fill(false);
        self.egui_ctx.request_repaint();
        true
    }
}

/// Where a crop sits in the frame the whole photo would fill at the crop's
/// density: the origin and the size of that frame. The crop's graph places
/// vignette, masks and tone guide in it, as if it were a tile of the photo.
/// `raw_size` is the crop's graph, `source_origin` and `source_size` the crop
/// in photo pixels.
pub(super) fn detail_virtual_frame(
    raw_size: [u32; 2],
    full_size: [u32; 2],
    source_origin: [u32; 2],
    source_size: [u32; 2],
) -> ([i32; 2], [u32; 2]) {
    let size = [0, 1].map(|axis| {
        ((f64::from(raw_size[axis]) * f64::from(full_size[axis])
            / f64::from(source_size[axis].max(1)))
        .round() as u32)
            .max(raw_size[axis])
    });
    let origin = [0, 1].map(|axis| {
        (f64::from(source_origin[axis]) / f64::from(full_size[axis].max(1)) * f64::from(size[axis]))
            .round() as i32
    });
    (origin, size)
}

/// Where a crop's graph sits in the photo, which its GPU parameters place the
/// vignette, the masks and the tone guide by.
#[derive(Clone, Copy)]
pub(super) struct DetailPlacement {
    /// The crop in the frame the photo would fill at its density
    /// (`detail_virtual_frame`).
    pub virtual_origin: [i32; 2],
    pub virtual_full_size: [u32; 2],
    /// The photo's size in pixels.
    pub full_size: [u32; 2],
    /// The part of the photo the masks are rasterized for, and the size of
    /// that raster.
    pub mask_region: [u32; 4],
    pub mask_extent: [u32; 2],
}

impl DetailPlacement {
    /// The parameters of the graph built from `raw` for `exposure`, rendering
    /// `profile_color`: `raw`'s own, or a previewed camera profile's.
    pub(super) fn params(
        self,
        exposure: &ExposureParams,
        masks: &MaskStack,
        geometry: GeometryTransform,
        raw: &LoadedRaw,
        profile_color: &Arc<crate::pipeline::ProfileColor>,
    ) -> GpuParams {
        GpuParams::new_for_tile_with_profile_color(
            exposure,
            masks,
            raw,
            profile_color,
            self.virtual_origin,
            self.virtual_full_size,
        )
        .with_vignette_geometry(geometry)
        .with_mask_uv_rect_and_extent(
            mask_source_region_uv(self.mask_region, self.full_size[0], self.full_size[1]),
            self.mask_extent,
        )
    }
}

/// The Remove edits as they apply to a crop at `source_origin` and
/// `source_size` of `full_raw`.
fn detail_remove_scene<'a>(
    edits: &'a RemoveEditState,
    full_raw: &'a LoadedRaw,
    exposure: &'a ExposureParams,
    source_origin: [u32; 2],
    source_size: [u32; 2],
) -> RemoveSceneContext<'a> {
    RemoveSceneContext::new(
        edits,
        full_raw,
        exposure,
        source_origin.map(|origin| origin as f32),
        source_size.map(|size| size as f32),
    )
}

impl PreviewDetail {
    pub(super) fn placement(&self) -> DetailPlacement {
        DetailPlacement {
            virtual_origin: self.virtual_origin,
            virtual_full_size: self.virtual_full_size,
            full_size: self.full_source_size,
            mask_region: self.mask_source_region,
            mask_extent: self.mask_texture_extent,
        }
    }

    /// The Remove edits as they apply to this crop of `full_raw`.
    pub(super) fn remove_scene<'a>(
        &self,
        edits: &'a RemoveEditState,
        full_raw: &'a LoadedRaw,
        exposure: &'a ExposureParams,
    ) -> RemoveSceneContext<'a> {
        detail_remove_scene(
            edits,
            full_raw,
            exposure,
            self.source_origin,
            self.source_size,
        )
    }

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
            self.raw.cfa_kind,
            requested.graph,
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
    /// Set when the crop is no longer wanted, which ends the preparation.
    cancel: Arc<AtomicBool>,
    source_raw: Arc<LoadedRaw>,
    revision: u64,
    visible: PreviewUvRect,
    view: DetailView,
    quality: PreviewQuality,
    exposure: ExposureParams,
    processing_halo: u32,
    graph: DetailGraph,
}

/// Prepares the crop of a request, or returns `None` if it was cancelled.
fn prepare_preview_detail(
    request: PreviewDetailRequest,
) -> anyhow::Result<Option<PreparedPreviewDetail>> {
    let PreviewDetailRequest {
        cancel,
        source_raw,
        revision,
        visible,
        view,
        quality,
        exposure,
        processing_halo,
        graph,
    } = request;
    let plan = PreviewDetailPlan::new(
        [source_raw.width, source_raw.height],
        source_raw.cfa_kind,
        visible,
        view,
        quality,
        processing_halo,
        graph,
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
    let prepare_started = Instant::now();
    calibraw_core::diagnostics::record(format!(
        "Zoom detail plan: crop {}x{} at {:?}, edge {}",
        crop_width, crop_height, plan.origin, requested_edge
    ));
    if cancel.load(std::sync::atomic::Ordering::Acquire) {
        return Ok(None);
    }
    if source_raw.uses_opposed_chroma(&exposure) {
        source_raw.inpaint_opposed_chroma_for_exposure(&exposure);
    }
    let raw = Arc::new(
        if settled_detail_uses_native_source(
            !source_raw.is_pre_demosaiced_raster(),
            source_size[0],
            source_size[1],
            requested_edge,
            source_raw.cfa_kind,
            graph,
        ) {
            // The detail texture is sampled down to the viewport only after the
            // complete GPU graph has run.  Keeping the mosaic native here is what
            // preserves clipping decisions, CFA detail, and sensor-noise scale.
            let mut native =
                crate::pipeline::crop_raw(&source_raw, x0, y0, source_size[0], source_size[1]);
            native.opposed_chroma_cache = Arc::clone(&source_raw.opposed_chroma_cache);
            native
        } else {
            let Some(proxy) = build_region_proxy_until(
                &source_raw,
                x0,
                y0,
                source_size[0],
                source_size[1],
                ProxySpec {
                    max_edge: requested_edge,
                },
                &cancel,
            ) else {
                return Ok(None);
            };
            proxy
        },
    );
    calibraw_core::diagnostics::record(format!(
        "Zoom detail source prepared in {:.3}s: {}x{}",
        prepare_started.elapsed().as_secs_f64(),
        raw.width,
        raw.height
    ));
    let visible = detail_display_uv(visible, crop_uv, [raw.width, raw.height], processing_halo);
    Ok(Some(PreparedPreviewDetail {
        source_raw,
        processing_halo,
        revision,
        quality,
        visible,
        texture_uv_rect: detail_texture_uv(visible, crop_uv),
        source_origin: [x0, y0],
        source_size,
        raw,
    }))
}

fn settled_detail_uses_native_source(
    is_sensor_raw: bool,
    region_width: u32,
    region_height: u32,
    requested_edge: u32,
    cfa_kind: crate::pipeline::CfaKind,
    graph: DetailGraph,
) -> bool {
    if !is_sensor_raw {
        return false;
    }

    // A settled detail may spend more memory than the continuously updated
    // proxy, but remains bounded so a near-fit view cannot allocate a full
    // high-megapixel processing graph.  As zoom increases the native region
    // eventually fits and becomes the authoritative fidelity path.
    let platform_limit = if PlatformCaps::CURRENT.constrained_memory() {
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
            cfa_kind,
            graph.preview_graph(),
        ) == region_width.max(region_height)
}
