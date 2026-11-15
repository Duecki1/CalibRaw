use super::detail_render::{self, DetailRenderJob};
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
        let Some(rebuild) = self.preview.detail_rebuild.take() else {
            return;
        };
        let result = match event {
            PreviewDetailRebuildEvent::Finished(result) => result,
            PreviewDetailRebuildEvent::Cancelled => {
                // Cancelled because the view moved on: the next crop is
                // requested once it settles.
                calibraw_core::diagnostics::record(
                    "Zoom detail cancelled: the view moved on before it was ready",
                );
                self.egui_ctx.request_repaint();
                return;
            }
            PreviewDetailRebuildEvent::RawRendered(result) => {
                self.continue_detail_render(frame, rebuild, result);
                return;
            }
            PreviewDetailRebuildEvent::Rendered(result) => {
                self.install_rendered_detail(frame, result);
                return;
            }
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
        if !self.start_detail_render(frame, rebuild, prepared) {
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

    /// Starts rendering `prepared` on a worker (`detail_render`), which the
    /// crop being followed by `rebuild` then waits for; `false`, after
    /// reporting why where it matters, if it could not start.
    fn start_detail_render(
        &mut self,
        frame: &eframe::Frame,
        rebuild: DetailRebuild,
        prepared: PreparedPreviewDetail,
    ) -> bool {
        let preview_masks = self.preview_mask_stack();
        let Some(full_raw) = self.develop.loaded_raw.clone() else {
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
        let (virtual_origin, virtual_full_size) = detail_virtual_frame(
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
        if full_raw.uses_highlight_model(&self.develop.target_exposure) {
            full_raw.prime_highlight_model(&self.develop.target_exposure);
        }
        let placement = DetailPlacement {
            virtual_origin,
            virtual_full_size,
            full_size: [full_raw.width, full_raw.height],
            mask_region,
            mask_extent,
        };
        let render_tier = self.sharp_render_tier();
        let params = placement
            .params(
                &self.develop.target_exposure,
                &preview_masks,
                self.develop.geometry,
                &detail_raw,
                self.preview_profile_color(&detail_raw),
            )
            .with_look_lut(self.develop.target_look.as_ref())
            .with_render_tier(render_tier);
        self.note_sharp_render_tier(&params, render_tier);

        // The current crop's graph is refilled when it fits; the canvas keeps
        // painting its last render meanwhile.
        let required_mask_layers = preview_masks.masks.len().max(1);
        let reused = self
            .preview
            .detail
            .as_mut()
            .filter(|detail| {
                detail.pipeline.gpu().is_some_and(|gpu| {
                    gpu.width == detail_raw.width
                        && gpu.height == detail_raw.height
                        && gpu.pixel_layout() == detail_raw.pixel_layout()
                        && gpu.tone_guide_supports_origin(virtual_origin[0], virtual_origin[1])
                        && gpu.mask_layer_capacity() >= required_mask_layers
                })
            })
            .and_then(|detail| detail.pipeline.lend());
        // On mobile the fitted graph and one detail graph share the budget:
        // release an incompatible graph before its replacement is built. Its
        // last render stays on the canvas until the new crop is installed.
        if reused.is_none() && self.caps().constrained_memory() {
            if let Some(old) = self.preview.detail.as_mut() {
                old.pipeline
                    .free_graph_now(&mut render_state.renderer.write());
            }
        }
        let template = self
            .preview
            .pipeline()
            .map(RawGpuPipeline::program_template);
        let Some(template) = template else {
            return false;
        };
        let crop = PreviewDetail {
            pipeline: (),
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
            virtual_origin,
            virtual_full_size,
        };
        let job = DetailRenderJob {
            device: render_state.device.clone(),
            queue: render_state.queue.clone(),
            reused,
            template,
            profile_preview: crate::app::lifecycle::reserves_profile_preview(&crop.raw),
            full_raw,
            masks: preview_masks,
            params,
            remove: Arc::clone(&self.inpaint.edits),
            exposure: self.develop.target_exposure,
            crop,
            started: Instant::now(),
        };
        // The job renders every stage and uploads every mask layer of the edit
        // as it is now. What changes while it runs is queued again and
        // rendered once the crop is installed.
        self.masks.detail_dirty_layers.fill(false);
        self.preview.detail_pending_stage = None;
        match detail_render::spawn_raw_stage(job, self.egui_ctx.clone()) {
            Ok(receiver) => {
                self.preview.detail_rebuild = Some(rebuild.continued(receiver));
                true
            }
            Err(error) => {
                self.drop_lent_detail_graph();
                self.report_error(ErrorKind::Preview, error);
                false
            }
        }
    }

    /// The RAW stage of a crop has rendered: the UI thread adds the full
    /// frame's statistics and maps, which only it may read, and the worker
    /// renders the rest.
    fn continue_detail_render(
        &mut self,
        frame: &eframe::Frame,
        rebuild: DetailRebuild,
        result: Result<Box<DetailRender>, String>,
    ) {
        let mut render = match result {
            Ok(render) => render,
            Err(error) => {
                self.detail_render_failed(error);
                return;
            }
        };
        if !self.detail_render_wanted(render.job()) {
            return;
        }
        let Some(render_state) = frame.wgpu_render_state() else {
            self.drop_lent_detail_graph();
            return;
        };
        let full_frame = full_frame_tone_pipeline(
            self.preview.gpu_pipeline.as_ref(),
            self.preview.navigation.as_ref(),
            self.preview.pending_stage,
        );
        if let Some(full_frame) = full_frame {
            render.gpu().inherit_full_frame_tone(
                &render_state.queue,
                &render_state.device,
                &render.job().params,
                full_frame,
            );
            render.note_inherited();
        }
        match detail_render::spawn_remaining_stages(*render, self.egui_ctx.clone()) {
            Ok(receiver) => self.preview.detail_rebuild = Some(rebuild.continued(receiver)),
            Err(error) => self.detail_render_failed(error),
        }
    }

    /// A worker has rendered a crop and the GPU has run it: the crop's graph
    /// returns, its output becomes the shown picture and the crop replaces
    /// the previous one, all in this frame.
    fn install_rendered_detail(
        &mut self,
        frame: &eframe::Frame,
        result: Result<Box<DetailRender>, String>,
    ) {
        let render = match result {
            Ok(render) => render,
            Err(error) => {
                self.detail_render_failed(error);
                return;
            }
        };
        if !self.detail_render_wanted(render.job()) {
            return;
        }
        let Some(render_state) = frame.wgpu_render_state() else {
            self.drop_lent_detail_graph();
            return;
        };
        let (gpu, job) = render.into_parts();
        let size = [gpu.width, gpu.height];
        // The replaced crop's copy is kept when it fits; the new picture goes
        // into it before this frame paints it.
        let shown = match self.preview.detail.take() {
            Some(old) if old.pipeline.size() == size => old.pipeline.into_shown(),
            _ => ShownTexture::new(
                size,
                &render_state.device,
                &mut render_state.renderer.write(),
                &self.preview.retired_textures,
            ),
        };
        let pipeline = CropPipeline::new(self.present_pipeline(gpu, render_state), shown);
        pipeline.show_completed(&render_state.device, &render_state.queue);
        calibraw_core::diagnostics::record(format!(
            "Zoom detail installed in {:.3}s: texture {}x{} for source crop {:?}",
            job.started.elapsed().as_secs_f64(),
            size[0],
            size[1],
            job.crop.source_size,
        ));
        self.preview.detail = Some(job.crop.with_pipeline(pipeline));
        if self.preview.original_requested {
            self.preview.original_rendered_state = None;
        }
        self.egui_ctx.request_repaint();
    }

    /// Whether a crop a worker renders for `job` is still worth finishing;
    /// if not, its graph goes and a crop that lent it is dropped. The rules
    /// are the ones a prepared crop is discarded by.
    fn detail_render_wanted(&mut self, job: &DetailRenderJob) -> bool {
        let source_is_current = self
            .develop
            .loaded_raw
            .as_ref()
            .is_some_and(|raw| Arc::ptr_eq(raw, &job.full_raw));
        let visible = job.crop.uv_rect;
        let on_screen = (0..2).all(|axis| {
            visible.max[axis] > self.preview.visible_uv.min[axis]
                && visible.min[axis] < self.preview.visible_uv.max[axis]
        });
        let wanted = self.preview.zoom > DETAIL_ZOOM_START
            && self.ui.active_tab == AppTab::Develop
            && source_is_current
            && on_screen
            && !self.ai_job_may_use_gpu();
        if !wanted {
            calibraw_core::diagnostics::record(format!(
                "Zoom detail render dropped as outdated (zoom {:.2}, revision {} vs {})",
                self.preview.zoom, job.crop.revision, self.preview.revision
            ));
            self.drop_lent_detail_graph();
            self.egui_ctx.request_repaint();
        }
        wanted
    }

    fn detail_render_failed(&mut self, error: String) {
        self.drop_lent_detail_graph();
        self.retry_preview_detail_later();
        self.report_error(ErrorKind::Preview, error);
    }

    /// Drops the crop that has no graph (lent to the worker, or released for
    /// the worker's new one) and will get none back.
    fn drop_lent_detail_graph(&mut self) {
        if self
            .preview
            .detail
            .as_ref()
            .is_some_and(|detail| detail.pipeline.gpu().is_none())
        {
            self.preview.detail = None;
        }
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

impl<P> PreviewDetail<P> {
    /// The same crop with `pipeline` as its graph.
    pub(super) fn with_pipeline<Q>(self, pipeline: Q) -> PreviewDetail<Q> {
        let Self {
            pipeline: _,
            uv_rect,
            texture_uv_rect,
            revision,
            raw,
            source_origin,
            source_size,
            mask_source_region,
            mask_texture_extent,
            virtual_origin,
            virtual_full_size,
            full_source_size,
            processing_halo,
        } = self;
        PreviewDetail {
            pipeline,
            uv_rect,
            texture_uv_rect,
            revision,
            raw,
            source_origin,
            source_size,
            mask_source_region,
            mask_texture_extent,
            virtual_origin,
            virtual_full_size,
            full_source_size,
            processing_halo,
        }
    }

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
    if source_raw.uses_highlight_model(&exposure) {
        source_raw.prime_highlight_model(&exposure);
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
            native.highlight_model_cache = Arc::clone(&source_raw.highlight_model_cache);
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
