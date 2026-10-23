use super::*;

/// What the displayed fitted graph is, as far as deciding whether it must be
/// built again.
#[derive(Clone, Copy, Debug)]
struct FittedGraph {
    /// The longest edge of its proxy.
    edge: u32,
    /// It has a mask layer for each mask it must render.
    has_enough_mask_layers: bool,
    /// It was built with the AI-denoise source the photo now needs.
    ai_source_matches: bool,
}

/// What the fitted graph needs to become what is wanted of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FittedRebuild {
    /// Nothing: it is as wanted.
    None,
    /// Only its mask layers are short: it is built again from the same proxy.
    MaskLayers,
    /// It needs another proxy.
    Proxy,
}

/// A proxy is clearly larger than the density asks for when it exceeds the
/// requested edge by more than an eighth plus a few pixels.
const SHRINK_MARGIN_DIVISOR: u32 = 8;
const SHRINK_MARGIN_PIXELS: u32 = 8;

fn clearly_larger(edge: u32, requested_edge: u32) -> bool {
    edge > requested_edge
        .saturating_add(requested_edge / SHRINK_MARGIN_DIVISOR + SHRINK_MARGIN_PIXELS)
}

/// A proxy at least as large as `requested_edge` is kept (within
/// `PROXY_EDGE_TOLERANCE`), unless the Preview quality setting just changed
/// (`shrink_oversized`) and it is clearly larger than the new density needs.
/// `visibility_changed`: the previewed masks or preset changed what the graph
/// must hold.
fn fitted_rebuild(
    current: Option<FittedGraph>,
    requested_edge: u32,
    shrink_oversized: bool,
    visibility_changed: bool,
) -> FittedRebuild {
    let Some(graph) = current.filter(|graph| {
        !visibility_changed
            && graph.edge.saturating_add(PROXY_EDGE_TOLERANCE) >= requested_edge
            && !(shrink_oversized && clearly_larger(graph.edge, requested_edge))
            && graph.ai_source_matches
    }) else {
        return FittedRebuild::Proxy;
    };
    if graph.has_enough_mask_layers {
        FittedRebuild::None
    } else {
        FittedRebuild::MaskLayers
    }
}

impl CalibRawApp {
    pub(crate) fn preview_base_pipeline(&self) -> Option<&PreviewPipeline> {
        self.preview.gpu_pipeline.as_ref()
    }

    pub(crate) fn preview_is_preparing(&self) -> bool {
        self.develop.load_receiver.is_some()
            || self.ai_denoise_running()
            || self.preview.rebuild_receiver.is_some()
            || self.preview.quality_dirty
    }

    pub(crate) fn preview_quality_changed(&mut self) {
        let preview_source = self.develop.loaded_raw.clone();
        self.persist_performance_settings();
        if preview_source.is_some() || self.develop.load_receiver.is_some() {
            self.preview.quality_dirty = true;
            self.preview.shrink_oversized = true;
            self.note_preview_motion();
        }
    }

    /// Whether the fitted graph has fewer mask layers than the masks it must
    /// render. Its layers are sized for the masks it was built with
    /// (`fitted_pipeline_options`), so it is rebuilt once there are more.
    pub(in crate::app) fn fitted_mask_capacity_stale(&self) -> bool {
        self.preview.pipeline().is_some_and(|pipeline| {
            pipeline.mask_layer_capacity() < self.preview_mask_stack().masks.len().max(1)
        })
    }

    /// The longest edge the fitted preview should have: one rendered pixel
    /// per screen pixel (scaled by the quality tier) of the image as it is
    /// fitted into the canvas, which is what the load path asks for too.
    /// Zoomed in, the sharp crop (`PreviewDetail`) covers the view, so the
    /// fitted preview keeps the edge it has.
    pub(in crate::app) fn requested_preview_edge_for_source(&self, full_raw: &LoadedRaw) -> u32 {
        let fitted = self.preview.quality.proxy_edge_for_fitted_source(
            self.preview.fitted_viewport_pixels,
            full_raw.width,
            full_raw.height,
            self.develop.geometry,
            full_raw.cfa_kind,
            self.preview_mask_stack().masks.len(),
        );
        if self.preview.zoom > DETAIL_ZOOM_START {
            return self
                .develop
                .preview_raw
                .as_ref()
                .map(|raw| raw.width.max(raw.height))
                .unwrap_or(fitted)
                .min(full_raw.width.max(full_raw.height));
        }
        fitted
    }

    pub(crate) fn preview_source_region_changed(&mut self) {
        let preview_source = self.develop.loaded_raw.clone();
        if self.preview.zoom > DETAIL_ZOOM_START {
            return;
        }
        if self.preview.shrink_oversized {
            self.preview.quality_dirty = true;
        }
        if let (Some(full_raw), Some(preview_raw)) =
            (preview_source.as_ref(), self.develop.preview_raw.as_ref())
        {
            let target = self.requested_preview_edge_for_source(full_raw);
            if preview_raw
                .width
                .max(preview_raw.height)
                .saturating_add(PROXY_EDGE_TOLERANCE)
                < target
            {
                self.preview.quality_dirty = true;
            }
        }
    }

    /// Records the physical sizes the preview is laid out at: `visible` is the
    /// part of the image in view, `fitted` the whole image at fit, which can
    /// extend behind an overlay panel. Returns whether either changed.
    pub(crate) fn set_preview_viewport_pixels(
        &mut self,
        visible: [u32; 2],
        fitted: [u32; 2],
    ) -> bool {
        let preview_source = self.develop.loaded_raw.clone();
        if self.preview.viewport_pixels == visible && self.preview.fitted_viewport_pixels == fitted
        {
            return false;
        }
        self.preview.viewport_pixels = visible;
        self.preview.fitted_viewport_pixels = fitted;

        if self.preview.zoom > DETAIL_ZOOM_START {
            self.note_preview_motion();
        }

        if self.develop.load_receiver.is_some() {
            self.preview.quality_dirty = true;
        }

        if let (Some(full_raw), Some(preview_raw)) =
            (preview_source.as_ref(), self.develop.preview_raw.as_ref())
        {
            let target_edge = self.requested_preview_edge_for_source(full_raw);
            let current_edge = preview_raw.width.max(preview_raw.height);
            if current_edge.saturating_add(PROXY_EDGE_TOLERANCE) < target_edge {
                self.preview.quality_dirty = true;
            }
        }
        true
    }

    pub(in crate::app) fn upload_preview_masks(
        pipeline: &RawGpuPipeline,
        queue: &wgpu::Queue,
        masks: &MaskStack,
        raw: &LoadedRaw,
    ) -> Result<(), String> {
        let edge = pipeline.mask_atlas_edge();
        for layer in 0..masks.masks.len().min(MAX_LOCAL_MASKS) {
            let layer_started = std::time::Instant::now();
            let bytes = masks.rasterize_layer_region_f16(
                layer,
                [edge, edge],
                [0, 0, raw.width, raw.height],
                [raw.width, raw.height],
                raw.lens_geometry.as_deref(),
            );
            let raster_elapsed = layer_started.elapsed();
            pipeline
                .update_mask_layer(queue, layer, &bytes)
                .map_err(|error| format!("Could not update preview mask: {error:#}"))?;
            calibraw_core::diagnostics::record(format!(
                "Preview mask layer {} rasterized/uploaded in {:.3}s (raster {:.3}s)",
                layer + 1,
                layer_started.elapsed().as_secs_f64(),
                raster_elapsed.as_secs_f64()
            ));
        }
        pipeline
            .update_light_rays_mask_layers(
                queue,
                masks,
                raw.width,
                raw.height,
                raw.lens_geometry.as_deref(),
            )
            .map_err(|error| format!("Could not update Light Rays mask: {error:#}"))?;
        Ok(())
    }

    pub(in crate::app) fn upload_detail_masks(
        pipeline: &RawGpuPipeline,
        queue: &wgpu::Queue,
        masks: &MaskStack,
        full_raw: &LoadedRaw,
        region: [u32; 4],
        extent: [u32; 2],
        dirty_layers: Option<&[bool; MAX_LOCAL_MASKS]>,
    ) -> Result<(), String> {
        for layer in 0..masks.masks.len().min(pipeline.mask_layer_capacity()) {
            if dirty_layers.is_some_and(|dirty| !dirty[layer]) {
                continue;
            }
            let bytes = masks.rasterize_layer_region_f16(
                layer,
                extent,
                region,
                [full_raw.width, full_raw.height],
                full_raw.lens_geometry.as_deref(),
            );
            pipeline
                .update_mask_layer_region(queue, layer, extent[0], extent[1], &bytes)
                .map_err(|error| format!("Could not update zoomed local mask: {error:#}"))?;
        }
        pipeline
            .update_dirty_light_rays_mask_layers(
                queue,
                masks,
                full_raw.width,
                full_raw.height,
                full_raw.lens_geometry.as_deref(),
                dirty_layers,
            )
            .map_err(|error| format!("Could not update zoomed Light Rays mask: {error:#}"))?;
        Ok(())
    }

    pub(in crate::app) fn upload_dirty_preview_masks(
        pipeline: &RawGpuPipeline,
        queue: &wgpu::Queue,
        masks: &MaskStack,
        raw: &LoadedRaw,
        dirty_layers: &mut [bool; MAX_LOCAL_MASKS],
    ) -> Result<(), String> {
        let edge = pipeline.mask_atlas_edge();
        let layer_count = masks.masks.len().min(pipeline.mask_layer_capacity());
        for (layer, dirty) in dirty_layers.iter().take(layer_count).enumerate() {
            if !dirty {
                continue;
            }
            let bytes = masks.rasterize_layer_region_f16(
                layer,
                [edge, edge],
                [0, 0, raw.width, raw.height],
                [raw.width, raw.height],
                raw.lens_geometry.as_deref(),
            );
            pipeline
                .update_mask_layer(queue, layer, &bytes)
                .map_err(|error| format!("Could not update preview mask: {error:#}"))?;
        }
        pipeline
            .update_dirty_light_rays_mask_layers(
                queue,
                masks,
                raw.width,
                raw.height,
                raw.lens_geometry.as_deref(),
                Some(dirty_layers),
            )
            .map_err(|error| format!("Could not update Light Rays mask: {error:#}"))?;
        // A shared-depth refresh marks every possible layer dirty. Slots outside
        // this graph's capacity are unused and must not prevent Output dispatch.
        dirty_layers.fill(false);
        Ok(())
    }

    pub(in crate::app) fn poll_preview_rebuild_worker(&mut self, frame: &eframe::Frame) {
        let preview_masks = self.preview_mask_stack();
        let preview_source = self.develop.loaded_raw.clone();
        let received = self
            .preview
            .rebuild_receiver
            .as_ref()
            .map(std::sync::mpsc::Receiver::try_recv);
        let event = match received {
            Some(Ok(event)) => Some(event),
            Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                self.preview.rebuild_receiver = None;
                self.preview.quality_dirty = false;
                self.report_error(
                    ErrorKind::Preview,
                    "Preview rebuild worker stopped unexpectedly.".to_owned(),
                );
                None
            }
            Some(Err(std::sync::mpsc::TryRecvError::Empty)) | None => None,
        };
        let Some(PreviewRebuildEvent::Finished(result)) = event else {
            return;
        };
        self.preview.rebuild_receiver = None;
        let prepared = match result {
            Ok(prepared) => prepared,
            Err(error) => {
                self.preview.quality_dirty = false;
                self.report_error(
                    ErrorKind::Preview,
                    format!("Could not prepare the preview proxy: {error}"),
                );
                return;
            }
        };
        let source_is_current = preview_source
            .as_ref()
            .is_some_and(|raw| Arc::ptr_eq(raw, &prepared.source_raw));
        if !source_is_current || prepared.ai_enabled != self.preview_exposure().ai_denoise_enabled {
            self.preview.quality_dirty |= source_is_current;
            return;
        }
        let Some(render_state) = frame.wgpu_render_state() else {
            self.preview.quality_dirty = true;
            return;
        };
        let params = GpuParams::new(
            &self.preview_exposure(),
            &preview_masks,
            &prepared.preview_raw,
        )
        .with_vignette_geometry(self.develop.geometry);
        let program_template = self
            .preview
            .pipeline()
            .map(RawGpuPipeline::program_template)
            .or_else(|| self.preview.program_template.clone());
        if let Some(template) = program_template.as_ref() {
            self.preview.program_template = Some(template.clone());
        }

        let options = PipelineOptions {
            programs: program_template.as_ref(),
            ..fitted_pipeline_options(preview_masks.masks.len())
        };
        let build_pipeline = || {
            RawGpuPipeline::new(
                &render_state.device,
                &render_state.queue,
                &prepared.preview_raw,
                &params,
                options,
            )
        };
        let pipeline_started = Instant::now();
        let mut pipeline_result = build_pipeline();
        let needs_in_place_replacement = self.preview.gpu_pipeline.is_some()
            && pipeline_result
                .as_ref()
                .err()
                .is_some_and(|error| error.to_string().contains("GPU pipelines already reserve"));
        if needs_in_place_replacement {
            calibraw_core::diagnostics::record(
                "DPI preview replacement exceeded coexistence budget; released old graph and reused its compiled programs",
            );
            let previous = self.take_preview_pipeline_and_release_textures();
            drop(previous);
            pipeline_result = build_pipeline();
        }
        let pipeline = match pipeline_result {
            Ok(pipeline) => pipeline,
            Err(error) => {
                self.preview.quality_dirty = false;
                self.report_error(
                    ErrorKind::Preview,
                    format!("Could not rebuild the GPU preview: {error:#}"),
                );
                return;
            }
        };
        calibraw_core::diagnostics::record(format!(
            "DPI preview GPU graph prepared on the UI thread in {:.3}s",
            pipeline_started.elapsed().as_secs_f64()
        ));
        if let Err(error) = Self::upload_preview_masks(
            &pipeline,
            &render_state.queue,
            &preview_masks,
            &prepared.preview_raw,
        ) {
            self.report_error(ErrorKind::Preview, error);
            self.preview.quality_dirty = false;
            return;
        }
        if let Some(full_raw) = preview_source.as_ref() {
            if let Err(error) = pipeline.recompute_with_remove(
                &render_state.queue,
                &render_state.device,
                &params,
                RemoveSceneContext::new(
                    &self.inpaint.edits,
                    full_raw,
                    &self.preview_exposure(),
                    [0.0, 0.0],
                    [full_raw.width as f32, full_raw.height as f32],
                ),
            ) {
                self.report_error(
                    ErrorKind::Preview,
                    format!("Could not apply Remove to rebuilt preview: {error:#}"),
                );
                self.preview.quality_dirty = false;
                return;
            }
        } else {
            pipeline.recompute(&render_state.queue, &render_state.device, &params);
        }
        drop(self.take_preview_pipeline_and_release_textures());

        crate::app::preview_visibility::PreviewVisibility::rebuilt(&self.egui_ctx);
        self.preview.program_template = Some(pipeline.program_template());
        self.develop.preview_raw = Some(prepared.preview_raw);
        self.preview.gpu_pipeline = Some(self.present_pipeline(pipeline, render_state));
        if self.caps().constrained_memory() {
            if self.develop.lens_correction.applied {
                if let (Some(selection), Some(full_raw), Some(preview_raw)) = (
                    self.develop.lens_correction.requested_correction(),
                    preview_source.as_ref(),
                    self.develop.preview_raw.as_ref(),
                ) {
                    self.preview.lens_corrected_cache = Some((
                        selection,
                        prepared.quality,
                        Arc::clone(full_raw),
                        Arc::clone(preview_raw),
                    ));
                }
            } else if !self.develop.lens_correction.applied {
                if let Some(preview_raw) = self.develop.preview_raw.as_ref() {
                    self.preview.lens_original_cache =
                        Some((prepared.quality, Arc::clone(preview_raw)));
                }
            }
        }
        self.develop.target_exposure = self.preview_exposure();
        self.preview.pending_stage = None;
        self.preview.detail_pending_stage = None;
        self.preview.navigation_pending_stage = None;
        self.preview.detail_urgent = false;
        self.masks.dirty_layers.fill(false);
        self.masks.detail_dirty_layers.fill(false);
        self.masks.navigation_dirty_layers.fill(false);
        self.preview.revision = self.preview.revision.wrapping_add(1);
        self.preview.motion_at = (self.preview.zoom > DETAIL_ZOOM_START).then(Instant::now);
        if let (Some(full), Some(preview)) = (&preview_source, &self.develop.preview_raw) {
            self.develop.image_status = format!(
                "{} {} — full {}×{}, preview {}×{} ({})",
                full.camera_make,
                full.camera_model,
                full.width,
                full.height,
                preview.width,
                preview.height,
                prepared.quality.label(),
            );
            let latest_edge = self.requested_preview_edge_for_source(full);
            if prepared.quality != self.preview.quality
                || preview
                    .width
                    .max(preview.height)
                    .saturating_add(PROXY_EDGE_TOLERANCE)
                    < latest_edge
            {
                self.preview.quality_dirty = true;
            }
        }
        calibraw_core::diagnostics::record(format!(
            "DPI preview rebuild installed: edge {} -> {}x{} ({})",
            prepared.requested_edge,
            self.develop.preview_raw.as_ref().map_or(0, |raw| raw.width),
            self.develop
                .preview_raw
                .as_ref()
                .map_or(0, |raw| raw.height),
            prepared.quality.label(),
        ));
    }

    pub(in crate::app) fn apply_pending_preview_quality(&mut self, _frame: &eframe::Frame) {
        let preview_source = self.develop.loaded_raw.clone();
        if self.preview.rebuild_receiver.is_some() {
            return;
        }
        if !self.preview.quality_dirty
            || self.develop.load_receiver.is_some()
            || self.lens_correction_busy()
        {
            return;
        }
        // Constrained memory cannot rebuild the preview while AI denoise runs.
        if self.caps().constrained_memory() && self.ai_denoise_running() {
            return;
        }
        let Some(source_raw) = preview_source.as_ref().map(Arc::clone) else {
            self.preview.quality_dirty = false;
            return;
        };
        let requested_edge = self.requested_preview_edge_for_source(&source_raw);
        let quality = self.preview.quality;
        let ai_enabled = self.preview_exposure().ai_denoise_enabled;

        // While zoomed the fitted proxy is left alone, so a request to shrink it
        // waits for the view to return (`preview_source_region_changed`).
        let shrink_oversized = self.preview.zoom <= DETAIL_ZOOM_START
            && std::mem::take(&mut self.preview.shrink_oversized);
        let current = self
            .develop
            .preview_raw
            .as_ref()
            .zip(self.preview.pipeline())
            .map(|(raw, pipeline)| FittedGraph {
                edge: raw.width.max(raw.height),
                has_enough_mask_layers: !self.fitted_mask_capacity_stale(),
                ai_source_matches: pipeline
                    .immutable_ai_source_matches(source_raw.cfa_kind, ai_enabled),
            });
        let rebuild = fitted_rebuild(
            current,
            requested_edge,
            shrink_oversized,
            crate::app::preview_visibility::PreviewVisibility::requires_rebuild(&self.egui_ctx),
        );
        if rebuild == FittedRebuild::None {
            self.preview.quality_dirty = false;
            return;
        }
        // Only the mask layers are short: the proxy it was built with stays.
        let reused_preview = (rebuild == FittedRebuild::MaskLayers)
            .then(|| self.develop.preview_raw.clone())
            .flatten();

        if source_raw.uses_opposed_chroma(&self.develop.target_exposure) {
            source_raw.inpaint_opposed_chroma_for_exposure(&self.develop.target_exposure);
        }

        self.discard_auxiliary_previews();
        let (sender, receiver) = std::sync::mpsc::channel();
        let context = self.egui_ctx.clone();
        let worker_source = Arc::clone(&source_raw);
        let spawn = std::thread::Builder::new()
            .name("calibraw-preview-rebuild".to_owned())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let preview_raw = if let Some(reused) = reused_preview {
                        reused
                    } else if worker_source.width.max(worker_source.height) <= requested_edge {
                        Arc::clone(&worker_source)
                    } else {
                        Arc::new(build_proxy(
                            &worker_source,
                            ProxySpec {
                                max_edge: requested_edge,
                            },
                        ))
                    };
                    Ok::<_, anyhow::Error>(PreparedPreviewRebuild {
                        source_raw: worker_source,
                        preview_raw,
                        quality,
                        requested_edge,
                        ai_enabled,
                    })
                }))
                .unwrap_or_else(|panic| {
                    let message = panic
                        .downcast_ref::<&str>()
                        .copied()
                        .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
                        .unwrap_or("unknown panic");
                    Err(anyhow::anyhow!("preview rebuild panicked: {message}"))
                })
                .map_err(|error| format!("{error:#}"));
                let _ = sender.send(PreviewRebuildEvent::Finished(result));
                context.request_repaint();
            });
        match spawn {
            Ok(_) => {
                self.preview.quality_dirty = false;
                self.preview.rebuild_receiver = Some(receiver);
                self.egui_ctx
                    .request_repaint_after(Duration::from_millis(50));
            }
            Err(error) => {
                self.report_error(
                    ErrorKind::Preview,
                    format!("Could not start preview rebuild: {error}"),
                );
            }
        }
    }
}

#[cfg(test)]
mod mask_upload_tests {
    use super::*;
    use crate::pipeline::{MaskImage, MaskKind};

    #[test]
    fn the_fitted_graph_is_built_with_spare_layers_for_its_masks_only() {
        let gpu = calibraw_gpu::request_headless_device(&calibraw_gpu::HeadlessDeviceRequest {
            label: "fitted mask capacity test",
            adapter: calibraw_gpu::HeadlessAdapter::Software,
            texture_dimension: wgpu::Limits::default().max_texture_dimension_2d,
        });
        let (device, queue) = match gpu {
            Ok(gpu) => (gpu.device, gpu.queue),
            Err(calibraw_gpu::HeadlessDeviceError::NoAdapter(_)) => {
                eprintln!("fitted mask capacity test skipped: no headless GPU adapter");
                return;
            }
            Err(error) => panic!("{error}"),
        };
        let raw = LoadedRaw::from_scene_linear_rec2020(16, 16, vec![0.2; 16 * 16 * 3]).unwrap();
        let exposure = ExposureParams::default();
        let mut masks = MaskStack::default();
        for _ in 0..3 {
            masks.add_mask(MaskKind::Fullscreen);
        }
        let params = GpuParams::new(&exposure, &masks, &raw);
        let pipeline = RawGpuPipeline::new(
            &device,
            &queue,
            &raw,
            &params,
            fitted_pipeline_options(masks.masks.len()),
        )
        .unwrap();
        assert_eq!(pipeline.mask_layer_capacity(), 4);
        let layer = vec![0x3C00u16; (pipeline.mask_atlas_edge().pow(2)) as usize] /* f16 1.0 */;
        assert!(pipeline.update_mask_layer(&queue, 3, &layer).is_ok());
        assert!(pipeline.update_mask_layer(&queue, 4, &layer).is_err());

        // A fourth mask still fits; a fifth needs a rebuilt graph.
        masks.add_mask(MaskKind::Fullscreen);
        assert!(masks.masks.len() <= pipeline.mask_layer_capacity());
        masks.add_mask(MaskKind::Fullscreen);
        assert!(masks.masks.len() > pipeline.mask_layer_capacity());
        let rebuilt = RawGpuPipeline::new(
            &device,
            &queue,
            &raw,
            &GpuParams::new(&exposure, &masks, &raw),
            fitted_pipeline_options(masks.masks.len()),
        )
        .unwrap();
        assert_eq!(rebuilt.mask_layer_capacity(), 8);
    }

    #[test]
    fn shared_depth_refresh_uploads_compact_atlases_and_clears_unused_dirty_slots() {
        let gpu = calibraw_gpu::request_headless_device(&calibraw_gpu::HeadlessDeviceRequest {
            label: "compact preview mask upload test",
            adapter: calibraw_gpu::HeadlessAdapter::Software,
            texture_dimension: wgpu::Limits::default().max_texture_dimension_2d,
        });
        let (device, queue) = match gpu {
            Ok(gpu) => (gpu.device, gpu.queue),
            Err(calibraw_gpu::HeadlessDeviceError::NoAdapter(_)) => {
                eprintln!("compact preview mask upload test skipped: no headless GPU adapter");
                return;
            }
            Err(error) => panic!("{error}"),
        };
        let raw = LoadedRaw::from_scene_linear_rec2020(16, 16, vec![0.2; 16 * 16 * 3]).unwrap();
        let exposure = ExposureParams::default();
        for mask_count in [0, 2] {
            let mut masks = MaskStack {
                scene_depth: Some(MaskImage::new(2, 2, vec![0, 85, 170, 255]).unwrap()),
                ..Default::default()
            };
            for _ in 0..mask_count {
                masks.add_mask(MaskKind::Fullscreen);
            }
            let params = GpuParams::new(&exposure, &masks, &raw);
            let pipeline = RawGpuPipeline::new(
                &device,
                &queue,
                &raw,
                &params,
                PipelineOptions::new(ProcessingQuality::Preview).mask_atlas_edge(16),
            )
            .unwrap();
            assert!(pipeline.mask_layer_capacity() < MAX_LOCAL_MASKS);
            let mut dirty_layers = [true; MAX_LOCAL_MASKS];
            CalibRawApp::upload_dirty_preview_masks(
                &pipeline,
                &queue,
                &masks,
                &raw,
                &mut dirty_layers,
            )
            .unwrap();
            assert!(dirty_layers.iter().all(|dirty| !dirty));
            pipeline.recompute(&queue, &device, &params);
            let pixels = pipeline
                .read_output_region_blocking(&device, &queue, 0, 0, raw.width, raw.height)
                .unwrap();
            assert_eq!(pixels.len(), 16 * 16 * 4);
            assert!(pixels.iter().any(|value| *value > 0));
        }
    }
}

#[cfg(test)]
mod fitted_rebuild_tests {
    use super::*;

    fn graph(edge: u32) -> Option<FittedGraph> {
        Some(FittedGraph {
            edge,
            has_enough_mask_layers: true,
            ai_source_matches: true,
        })
    }

    #[test]
    fn a_proxy_that_is_large_enough_is_kept() {
        for edge in [1_600, 1_605, 2_400, 6_000] {
            assert_eq!(
                fitted_rebuild(graph(edge), 1_600, false, false),
                FittedRebuild::None,
                "{edge}"
            );
        }
    }

    #[test]
    fn a_proxy_that_is_too_small_is_replaced() {
        assert_eq!(
            fitted_rebuild(graph(1_594), 1_600, false, false),
            FittedRebuild::Proxy
        );
        assert_eq!(
            fitted_rebuild(None, 1_600, false, false),
            FittedRebuild::Proxy
        );
    }

    #[test]
    fn only_a_changed_setting_shrinks_a_clearly_larger_proxy() {
        // 12.5% and 8 pixels over what is asked is clearly larger.
        assert_eq!(
            fitted_rebuild(graph(1_809), 1_600, true, false),
            FittedRebuild::Proxy
        );
        assert_eq!(
            fitted_rebuild(graph(1_808), 1_600, true, false),
            FittedRebuild::None
        );
        assert_eq!(
            fitted_rebuild(graph(4_000), 1_600, false, false),
            FittedRebuild::None
        );
    }

    #[test]
    fn short_mask_layers_keep_the_proxy_and_a_mismatch_replaces_it() {
        let short = |ai_source_matches| {
            Some(FittedGraph {
                edge: 1_600,
                has_enough_mask_layers: false,
                ai_source_matches,
            })
        };
        assert_eq!(
            fitted_rebuild(short(true), 1_600, false, false),
            FittedRebuild::MaskLayers
        );
        assert_eq!(
            fitted_rebuild(short(false), 1_600, false, false),
            FittedRebuild::Proxy
        );
        assert_eq!(
            fitted_rebuild(graph(1_600), 1_600, false, true),
            FittedRebuild::Proxy
        );
        // A proxy that is too small is replaced whatever else is wrong.
        assert_eq!(
            fitted_rebuild(
                Some(FittedGraph {
                    edge: 800,
                    has_enough_mask_layers: false,
                    ai_source_matches: true,
                }),
                1_600,
                false,
                false
            ),
            FittedRebuild::Proxy
        );
    }
}
