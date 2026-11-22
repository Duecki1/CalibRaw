//! Rendering tiles and streaming them to the encoder in row bands.

use super::*;
use crate::pipeline::ExportTile;
use std::cell::RefCell;

pub(super) fn render_export_output<W: Write>(
    context: ExportContext<'_>,
    request: ExportRequest<'_>,
    output: &mut W,
    row_format: ExportRowFormat,
) -> Result<()> {
    validate_export_dimensions(request.window.width, request.window.height)?;
    if !request.window.is_whole()
        || !request.geometry.is_identity()
        || request.raw.lens_geometry.is_some()
    {
        return render_geometry_output(context, request, output, row_format);
    }

    let output_transform = request.color.transform.as_ref();
    let mut resizer = LinearLightResizer::new_with_format(
        request.raw.width,
        request.raw.height,
        request.window.width,
        request.window.height,
        row_format,
    )?
    .with_watermark(PreparedWatermark::for_request(&request)?);
    stream_tiled_linear_rows(context, request, |source_y, source| {
        resizer.push_source_row(source_y, source, output_transform, output)
    })?;
    resizer.finish(output_transform, output)?;
    Ok(())
}

pub(super) fn stream_tiled_linear_rows<F>(
    context: ExportContext<'_>,
    request: ExportRequest<'_>,
    mut row_sink: F,
) -> Result<()>
where
    F: FnMut(u32, &[f32]) -> Result<()>,
{
    let ExportContext {
        device,
        queue,
        events,
        cancellation,
        program_prewarm,
        memory,
    } = context;
    let ExportRequest {
        raw,
        exposure,
        masks,
        remove,
        look,
        outputs: _,
        tile_spec,
        window,
        keep_metadata: _,
        metadata: _,
        geometry,
        bit_depth: _,
        color: _,
        watermark: _,
    } = request;
    let export_started = Instant::now();
    ensure_export_not_cancelled(cancellation)?;
    anyhow::ensure!(
        !exposure.ai_denoise_enabled || raw.ai_denoised_image().is_some(),
        "AI denoise is enabled but its full-resolution RawNIND result is not ready"
    );
    validate_export_dimensions(window.width, window.height)?;
    if exposure.highlight_method == crate::pipeline::HighlightReconstructionMethod::InpaintOpposed
        || (raw.cfa_kind == CfaKind::XTrans
            && exposure.highlight_method == crate::pipeline::HighlightReconstructionMethod::Lch)
    {
        raw.prime_highlight_model(exposure);
    }
    let plan = TilePlan::new(raw.width, raw.height, tile_spec);
    calibraw_core::diagnostics::record(format!(
        "Tiled export plan: source={}x{} requested_output={}x{} tiles={} core={} halo={} linear_row_stream=f32",
        raw.width,
        raw.height,
        window.width,
        window.height,
        plan.tile_count(),
        tile_spec.core_edge,
        tile_spec.halo,
    ));
    let first = *plan
        .tiles
        .first()
        .context("cannot export an empty RAW image")?;
    let first_raw = extract_padded_tile(raw, first);
    let first_mask_region = tile_mask_source_region(
        masks,
        first.global_origin_x,
        first.global_origin_y,
        first.padded_width,
        first.padded_height,
        raw.width,
        raw.height,
    );
    let first_params = GpuParams::new_for_tile(
        exposure,
        masks,
        &first_raw,
        first.global_origin_x,
        first.global_origin_y,
        raw.width,
        raw.height,
    )
    .with_vignette_geometry(geometry)
    .with_mask_uv_rect(mask_source_region_uv(
        first_mask_region,
        raw.width,
        raw.height,
    ))
    .with_look_lut(look);
    let pipeline_started = Instant::now();
    let export_mask_edge = export_tile_mask_atlas_edge(
        masks,
        [raw.width, raw.height],
        [first.padded_width, first.padded_height],
    );
    let tile_pipeline = RawGpuPipeline::new_reusing(
        device,
        queue,
        &first_raw,
        &first_params,
        PipelineOptions::new(ProcessingQuality::High).mask_atlas_edge(export_mask_edge),
        program_prewarm,
    )
    .context("create reusable full-quality export pipeline")?;
    // The export runs on its own thread: it waits between chunks of GPU work,
    // so the UI's frames are not queued behind whole tiles.
    tile_pipeline.pace_submissions(true);
    let setup = tile_pipeline.setup_times();
    calibraw_core::diagnostics::record(format!(
        "Full-quality export pipeline prepared in {:.3}s (resources {:.3}s, shader modules {:.3}s, program compilation {:.3}s); padded_tile={}x{} viewport-local mask_atlas={}x{} R16F",
        pipeline_started.elapsed().as_secs_f64(),
        setup.resources.as_secs_f64(),
        setup.shader_modules.as_secs_f64(),
        setup.programs.as_secs_f64(),
        first_raw.width,
        first_raw.height,
        export_mask_edge,
        export_mask_edge
    ));
    let gpu_timing = crate::stage_timing::begin_export(device);
    tile_pipeline
        .prepare_look_lut(queue, first_params.look_lut())
        .context("upload the LUT")?;
    tile_pipeline
        .update_full_image_mask_layers(
            queue,
            masks,
            raw.width,
            raw.height,
            raw.lens_geometry.as_deref(),
        )
        .context("upload full-image masks")?;

    let clock = ExportClock::default();
    let mask_bits = RefCell::new(Vec::new());
    let probe = super::diagnostics::ExportProbe::requested()?;
    let tile_inputs = ExportTileInputs {
        pipeline: &tile_pipeline,
        device,
        queue,
        raw,
        exposure,
        masks,
        remove,
        look,
        geometry,
        raw_stage_kept: false,
        clock: &clock,
        mask_bits: &mask_bits,
    };
    let mut tile_scratch = first_raw;
    if let Some(probe) = probe.as_ref().filter(|probe| probe.raw_only()) {
        return probe.run(&tile_inputs, &plan, &mut tile_scratch, None);
    }
    let tone_analysis_started = Instant::now();
    if let Some(timing) = &gpu_timing {
        timing.phase("tone prepass");
    }
    let mut watch_gpu = export_gpu_watcher(events, cancellation);
    // The prepass runs the RAW stage of every tile; kept, the final pass need not run it again.
    // Copying the scene costs the CPU more than the RAW stage costs a computer's GPU, so only
    // a device with a limited GPU working set (a phone or tablet), whose GPU bounds the
    // export, keeps it.
    let mut raw_stage_store = crate::pipeline::gpu_working_set_is_limited()
        .then(|| RawStageStore::new(&plan, tile_pipeline.scene_texel_bytes(), memory))
        .flatten();
    match &raw_stage_store {
        Some(store) => calibraw_core::diagnostics::record(format!(
            "Export RAW-stage scene ({:.1} MiB) kept from the tone prepass",
            store.bytes() as f64 / (1024.0 * 1024.0)
        )),
        None => calibraw_core::diagnostics::record(
            "Export RAW-stage scene not kept (unlimited GPU working set or not enough memory); the final pass runs the RAW stage again",
        ),
    }
    let mut pending_scene = None;
    tile_pipeline.begin_export_tone_analysis(queue, device);
    for (index, tile) in plan.tiles.iter().copied().enumerate() {
        ensure_export_not_cancelled(cancellation)?;
        trace_export_tile("tone prepass", index, &plan, tile);
        if index != 0 {
            clock.time("tile extraction", || {
                extract_padded_tile_into(raw, tile, &mut tile_scratch);
            });
        }
        clock
            .time("RAW upload", || {
                tile_pipeline.upload_raw_tile(queue, &tile_scratch)
            })
            .with_context(|| format!("upload tone-analysis tile {}", index + 1))?;
        let tone_params = GpuParams::new_for_tile(
            exposure,
            masks,
            &tile_scratch,
            tile.global_origin_x,
            tile.global_origin_y,
            raw.width,
            raw.height,
        )
        .with_vignette_geometry(geometry)
        .with_look_lut(look)
        .with_global_tone_histogram_bounds(
            tile.core_x,
            tile.core_y,
            tile.core_width,
            tile.core_height,
        );
        clock
            .time("tone prepass passes", || {
                tile_pipeline.accumulate_export_tone_tile_with_remove(
                    queue,
                    device,
                    &tone_params,
                    RemoveSceneContext::new(
                        remove,
                        raw,
                        exposure,
                        [tile.global_origin_x as f32, tile.global_origin_y as f32],
                        [tile.padded_width as f32, tile.padded_height as f32],
                    ),
                )
            })
            .with_context(|| format!("apply Remove to tone-analysis tile {}", index + 1))?;
        if let Some(store) = &mut raw_stage_store {
            let region = store.owned_region(tile);
            let readback = tile_pipeline
                .begin_scene_region_readback(device, queue, region.origin, region.extent)
                .with_context(|| format!("queue RAW-stage readback of tile {}", index + 1))?;
            // The previous tile's texels are read while this tile renders.
            if let Some((previous, previous_index, readback)) =
                pending_scene.replace((tile, index, readback))
            {
                clock.time("kept scene readback", || {
                    store_raw_stage_tile(
                        store,
                        device,
                        &mut watch_gpu,
                        previous,
                        previous_index,
                        readback,
                    )
                })?;
            }
        }
    }
    if let (Some(store), Some((tile, index, readback))) =
        (&mut raw_stage_store, pending_scene.take())
    {
        clock.time("kept scene readback", || {
            store_raw_stage_tile(store, device, &mut watch_gpu, tile, index, readback)
        })?;
    }
    tile_pipeline.finish_export_tone_analysis(queue, device);
    calibraw_core::diagnostics::record(format!(
        "Exact full-resolution tone-analysis prepass queued in {:.3}s across {} tiles",
        tone_analysis_started.elapsed().as_secs_f64(),
        plan.tile_count()
    ));
    record_tone_statistics(&tile_pipeline, device, queue);

    let tile_inputs = ExportTileInputs {
        raw_stage_kept: raw_stage_store.is_some(),
        ..tile_inputs
    };
    // A Lens Blur whose discs reach past the export halo reads the whole
    // image (defocus.rs): every tile adds its core to Lens Blur's full-image
    // map before any tile renders. It comes before the Glow's, whose tiles
    // render Lens Blur from it.
    if tile_pipeline
        .begin_export_defocus(queue, device, &first_params)
        .context("prepare Lens Blur's full-image map")?
    {
        if let Some(timing) = &gpu_timing {
            timing.phase("Lens Blur map prepass");
        }
        let defocus_started = Instant::now();
        for (index, tile) in plan.tiles.iter().copied().enumerate() {
            ensure_export_not_cancelled(cancellation)?;
            trace_export_tile("Lens Blur map", index, &plan, tile);
            let params = tile_inputs
                .upload(tile, index, &mut tile_scratch)?
                .with_global_tone_histogram_bounds(
                    tile.core_x,
                    tile.core_y,
                    tile.core_width,
                    tile.core_height,
                );
            tile_inputs.load_scene(tile, index, &params, raw_stage_store.as_ref())?;
            clock.time("Lens Blur map passes", || {
                tile_pipeline.accumulate_export_defocus_tile(queue, device, &params);
            });
        }
        tile_pipeline.finish_export_defocus(queue, device, &first_params);
        calibraw_core::diagnostics::record(format!(
            "Lens Blur full-image map prepass queued in {:.3}s across {} tiles",
            defocus_started.elapsed().as_secs_f64(),
            plan.tile_count()
        ));
    }
    // A Glow whose spread reaches past the export halo reads the whole image
    // (styled_glow.rs): every tile adds its core to the Glow's full-image map
    // before any tile renders. A single tile too, as its padding takes it past
    // the image's edges.
    if tile_pipeline
        .begin_export_styled_glow(queue, device, &first_params)
        .context("prepare the Glow's full-image map")?
    {
        if let Some(timing) = &gpu_timing {
            timing.phase("Glow map prepass");
        }
        let glow_started = Instant::now();
        for (index, tile) in plan.tiles.iter().copied().enumerate() {
            ensure_export_not_cancelled(cancellation)?;
            trace_export_tile("Glow map", index, &plan, tile);
            let params = tile_inputs
                .upload(tile, index, &mut tile_scratch)?
                .with_global_tone_histogram_bounds(
                    tile.core_x,
                    tile.core_y,
                    tile.core_width,
                    tile.core_height,
                );
            tile_inputs.load_scene(tile, index, &params, raw_stage_store.as_ref())?;
            clock.time("Glow map passes", || {
                tile_pipeline.accumulate_export_styled_glow_tile(queue, device, &params);
            });
        }
        tile_pipeline.finish_export_styled_glow(queue, device, &first_params);
        calibraw_core::diagnostics::record(format!(
            "Glow full-image map prepass queued in {:.3}s across {} tiles",
            glow_started.elapsed().as_secs_f64(),
            plan.tile_count()
        ));
    }

    if let Some(probe) = probe.as_ref().filter(|probe| probe.captures_stages()) {
        return probe.run(
            &tile_inputs,
            &plan,
            &mut tile_scratch,
            raw_stage_store.as_ref(),
        );
    }
    if let Some(timing) = &gpu_timing {
        timing.phase("final tiles");
    }
    let total_tiles = plan.tile_count();
    let synchronous_readback = probe.as_ref().is_some_and(|probe| probe.synchronous());
    if synchronous_readback {
        calibraw_core::diagnostics::record("Export probe: synchronous tile readback enabled");
    }
    let mut completed_tiles = 0usize;
    let mut first_progress_logged = false;
    let mut tile_index = 0usize;

    while tile_index < plan.tiles.len() {
        ensure_export_not_cancelled(cancellation)?;
        let band_y = plan.tiles[tile_index].core_y;
        let band_height = plan.tiles[tile_index].core_height;
        let band_start = tile_index;
        while tile_index < plan.tiles.len() && plan.tiles[tile_index].core_y == band_y {
            tile_index += 1;
        }

        let band_values = checked_rgb_len(raw.width, band_height)?;
        let mut band = Vec::new();
        band.try_reserve_exact(band_values)
            .context("reserve bounded export source band")?;
        clock.time("band allocation", || band.resize(band_values, 0.0f32));
        let mut pending_readback = None;
        for (absolute_index, tile) in plan.tiles[band_start..tile_index]
            .iter()
            .copied()
            .enumerate()
        {
            ensure_export_not_cancelled(cancellation)?;
            let global_index = band_start + absolute_index;
            trace_export_tile("final", global_index, &plan, tile);
            let params = tile_inputs.upload(tile, global_index, &mut tile_scratch)?;
            tile_inputs.load_scene(tile, global_index, &params, raw_stage_store.as_ref())?;
            clock.time("final passes", || {
                tile_pipeline.dispatch_export_tile_from_scene(queue, device, &params);
            });
            // An export must not lack an effect whose resources did not fit.
            tile_pipeline
                .ensure_effects_rendered()
                .with_context(|| format!("render export tile {}", global_index + 1))?;
            let readback = tile_pipeline
                .begin_display_linear_region_readback(
                    device,
                    queue,
                    tile.local_core_x,
                    tile.local_core_y,
                    tile.core_width,
                    tile.core_height,
                )
                .with_context(|| format!("queue export tile readback {}", global_index + 1))?;

            let previous = if synchronous_readback {
                Some((tile, global_index, readback))
            } else {
                pending_readback.replace((tile, global_index, readback))
            };
            if let Some((previous_tile, previous_index, previous_readback)) = previous {
                let rgb = clock
                    .time("tile readback", || {
                        previous_readback.finish(device, GpuWait::Watched(&mut watch_gpu))
                    })
                    .inspect_err(|error| {
                        if probe.is_some() && error
                            .downcast_ref::<super::super::gpu::NonFiniteReadback>()
                            .is_some()
                        {
                            calibraw_core::diagnostics::record(format!("Export failure capture: invalid readback tile {}, current GPU tile {}", previous_index + 1, global_index + 1));
                            tile_pipeline.capture_failed_export(
                                device,
                                queue,
                                [
                                    tile.local_core_x,
                                    tile.local_core_y,
                                    tile.core_width,
                                    tile.core_height,
                                ],
                            );
                        }
                    })
                    .with_context(|| format!("read export tile {}", previous_index + 1))?;
                clock.time("tile stitching", || {
                    stitch_linear_tile_into_band(&mut band, raw.width, band_y, previous_tile, &rgb)
                })?;
                report_completed_export_tile(
                    events,
                    &mut completed_tiles,
                    total_tiles,
                    &mut first_progress_logged,
                    export_started,
                    &rgb,
                );
            }
        }

        if let Some((last_tile, last_index, last_readback)) = pending_readback.take() {
            let rgb = clock
                .time("tile readback", || {
                    last_readback.finish(device, GpuWait::Watched(&mut watch_gpu))
                })
                .inspect_err(|error| {
                    if probe.is_some()
                        && error
                            .downcast_ref::<super::super::gpu::NonFiniteReadback>()
                            .is_some()
                    {
                        calibraw_core::diagnostics::record(format!(
                            "Export failure capture: invalid readback and current GPU tile {}",
                            last_index + 1
                        ));
                        tile_pipeline.capture_failed_export(
                            device,
                            queue,
                            [
                                last_tile.local_core_x,
                                last_tile.local_core_y,
                                last_tile.core_width,
                                last_tile.core_height,
                            ],
                        );
                    }
                })
                .with_context(|| format!("read export tile {}", last_index + 1))?;
            clock.time("tile stitching", || {
                stitch_linear_tile_into_band(&mut band, raw.width, band_y, last_tile, &rgb)
            })?;
            report_completed_export_tile(
                events,
                &mut completed_tiles,
                total_tiles,
                &mut first_progress_logged,
                export_started,
                &rgb,
            );
        }

        let source_row_values = checked_rgb_len(raw.width, 1)?;
        for local_y in 0..band_height {
            ensure_export_not_cancelled(cancellation)?;
            let start = usize::try_from(local_y)
                .ok()
                .and_then(|row| row.checked_mul(source_row_values))
                .context("source export row offset overflow")?;
            let end = start
                .checked_add(source_row_values)
                .context("source export row end overflow")?;
            let source_y = band_y + local_y;
            clock.time("output rows", || row_sink(source_y, &band[start..end]))?;
        }
    }
    clock.record();
    if let Some(timing) = gpu_timing {
        timing.finish();
    }
    Ok(())
}

/// The wall time the export thread spent per kind of work, for the
/// diagnostics: what an export costs besides its GPU passes.
#[derive(Default)]
struct ExportClock {
    spent: std::cell::RefCell<Vec<(&'static str, Duration)>>,
}

impl ExportClock {
    fn time<T>(&self, work: &'static str, run: impl FnOnce() -> T) -> T {
        let started = Instant::now();
        let result = run();
        let elapsed = started.elapsed();
        let mut spent = self.spent.borrow_mut();
        match spent.iter_mut().find(|(name, _)| *name == work) {
            Some((_, total)) => *total += elapsed,
            None => spent.push((work, elapsed)),
        }
        result
    }

    fn record(&self) {
        let mut spent = self.spent.borrow().clone();
        spent.sort_by_key(|(_, total)| std::cmp::Reverse(*total));
        let listed = spent
            .iter()
            .map(|(name, total)| format!("{name} {:.3}", total.as_secs_f64()))
            .collect::<Vec<_>>()
            .join(", ");
        calibraw_core::diagnostics::record(format!("Export thread time in seconds: {listed}"));
    }
}

/// What every export tile is rendered from, for the Glow map prepass and the
/// final tiles alike.
pub(super) struct ExportTileInputs<'a> {
    pub(super) pipeline: &'a RawGpuPipeline,
    pub(super) device: &'a wgpu::Device,
    pub(super) queue: &'a wgpu::Queue,
    raw: &'a LoadedRaw,
    exposure: &'a ExposureParams,
    masks: &'a MaskStack,
    remove: &'a RemoveEditState,
    look: Option<&'a ResolvedLook>,
    geometry: GeometryTransform,
    /// Whether the tone prepass kept every tile's RAW-stage scene: the RAW
    /// stage, the only reader of the RAW tile's textures, then does not run
    /// again, so the tile is not uploaded.
    raw_stage_kept: bool,
    clock: &'a ExportClock,
    /// The binary16 coverage of the layer being uploaded, kept from tile to
    /// tile so its allocation is made once (`upload_mask_atlas`).
    mask_bits: &'a RefCell<Vec<u16>>,
}

impl ExportTileInputs<'_> {
    /// Uploads the RAW pixels (unless the RAW-stage scene is kept) and masks
    /// of `tile`, the `index`th, through `scratch` and returns its parameters.
    pub(super) fn upload(
        &self,
        tile: ExportTile,
        index: usize,
        scratch: &mut LoadedRaw,
    ) -> Result<GpuParams> {
        let (raw, masks) = (self.raw, self.masks);
        // The tile is extracted either way: its parameters come from it.
        self.clock.time("tile extraction", || {
            extract_padded_tile_into(raw, tile, scratch);
        });
        if !self.raw_stage_kept {
            self.clock
                .time("RAW upload", || {
                    self.pipeline.upload_raw_tile(self.queue, scratch)
                })
                .with_context(|| format!("upload export tile {}", index + 1))?;
        }
        let mask_region = tile_mask_source_region(
            masks,
            tile.global_origin_x,
            tile.global_origin_y,
            tile.padded_width,
            tile.padded_height,
            raw.width,
            raw.height,
        );
        let mask_extent = mask_region_texture_extent(mask_region, self.pipeline.mask_atlas_edge());
        let params = GpuParams::new_for_tile(
            self.exposure,
            masks,
            scratch,
            tile.global_origin_x,
            tile.global_origin_y,
            raw.width,
            raw.height,
        )
        .with_vignette_geometry(self.geometry)
        .with_mask_uv_rect_and_extent(
            mask_source_region_uv(mask_region, raw.width, raw.height),
            mask_extent,
        )
        .with_look_lut(self.look);
        self.clock.time("mask upload", || {
            upload_mask_atlas(
                self.pipeline,
                self.queue,
                masks,
                &params,
                raw.width,
                raw.height,
                mask_region,
                raw.lens_geometry.as_deref(),
                &mut self.mask_bits.borrow_mut(),
            )
        })?;
        Ok(params)
    }

    /// Puts the RAW-stage scene of `tile`, the `index`th, in the pipeline:
    /// the one the tone prepass kept in `store`, or the RAW stage run again
    /// with Remove.
    pub(super) fn load_scene(
        &self,
        tile: ExportTile,
        index: usize,
        params: &GpuParams,
        store: Option<&RawStageStore<'_>>,
    ) -> Result<()> {
        match store {
            Some(store) => self
                .clock
                .time("kept scene upload", || {
                    store.write_tile(self.pipeline, self.queue, tile)
                })
                .with_context(|| format!("upload the RAW-stage scene of tile {}", index + 1)),
            None => self
                .clock
                .time("RAW stage", || {
                    self.pipeline.dispatch_stage_with_remove(
                        self.queue,
                        self.device,
                        params,
                        ProcessingStage::Raw,
                        RemoveSceneContext::new(
                            self.remove,
                            self.raw,
                            self.exposure,
                            [tile.global_origin_x as f32, tile.global_origin_y as f32],
                            [tile.padded_width as f32, tile.padded_height as f32],
                        ),
                    )
                })
                .with_context(|| format!("apply Remove to export tile {}", index + 1)),
        }
    }
}

/// Notes in the diagnostics file that `stage` starts tile `index`, with the
/// padded size the GPU renders it at. Written for every tile (`trace`, not
/// `record`), so after a crash the last one shows what was running.
fn trace_export_tile(
    stage: &str,
    index: usize,
    plan: &TilePlan,
    tile: crate::pipeline::ExportTile,
) {
    calibraw_core::diagnostics::trace(format!(
        "Export step: {stage} tile {}/{} (padded {}x{} px)",
        index + 1,
        plan.tile_count(),
        tile.padded_width,
        tile.padded_height
    ));
}

/// Stores the RAW-stage texels of tile `index`, which `readback` reads.
fn store_raw_stage_tile(
    store: &mut RawStageStore<'_>,
    device: &wgpu::Device,
    watch_gpu: &mut impl FnMut(Duration) -> bool,
    tile: crate::pipeline::ExportTile,
    index: usize,
    readback: crate::pipeline::PendingSceneReadback,
) -> Result<()> {
    // The first tile's scene is summarized for the diagnostics: an export that
    // comes out black shows there whether the RAW stage already left nothing.
    let rgba32 = store.texel_bytes() == 16;
    let mut summary = ValueSummary::default();
    readback
        .finish(device, GpuWait::Watched(watch_gpu), |row, texels| {
            if index == 0 && rgba32 {
                for texel in texels.chunks_exact(16) {
                    summary.extend(texel[..12].chunks_exact(4).map(|channel| {
                        f32::from_le_bytes([channel[0], channel[1], channel[2], channel[3]])
                    }));
                }
            }
            store.store_row(tile, row, texels);
        })
        .with_context(|| format!("read the RAW-stage scene of tile {}", index + 1))?;
    if index == 0 && rgba32 {
        calibraw_core::diagnostics::record(format!(
            "Export RAW-stage scene of tile 1: RGB {summary}"
        ));
    }
    Ok(())
}

/// Records the whole-image tone statistics of the tone prepass, which every
/// tile's tone response reads: degenerate ones render the whole export black.
fn record_tone_statistics(pipeline: &RawGpuPipeline, device: &wgpu::Device, queue: &wgpu::Queue) {
    match pipeline.read_tone_statistics(device, queue) {
        Ok([p005, p05, p50, p95, p995, spread, count, _]) => {
            calibraw_core::diagnostics::record(format!(
                "Export tone statistics: EV percentiles 0.5/5/50/95/99.5 = {p005:?}/{p05:?}/{p50:?}/{p95:?}/{p995:?} spread={spread:?} pixels={count:?}"
            ))
        }
        Err(error) => calibraw_core::diagnostics::record(format!(
            "Export tone statistics could not be read: {error:#}"
        )),
    }
}

/// The range, mean and non-finite count of some channel values, for the
/// diagnostics.
#[derive(Default)]
struct ValueSummary {
    finite: u64,
    nonfinite: u64,
    minimum: f32,
    maximum: f32,
    sum: f64,
}

impl ValueSummary {
    fn of(values: impl Iterator<Item = f32>) -> Self {
        let mut summary = Self::default();
        summary.extend(values);
        summary
    }

    fn extend(&mut self, values: impl Iterator<Item = f32>) {
        for value in values {
            if !value.is_finite() {
                self.nonfinite += 1;
                continue;
            }
            if self.finite == 0 {
                (self.minimum, self.maximum) = (value, value);
            } else {
                self.minimum = self.minimum.min(value);
                self.maximum = self.maximum.max(value);
            }
            self.finite += 1;
            self.sum += f64::from(value);
        }
    }
}

impl std::fmt::Display for ValueSummary {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mean = if self.finite == 0 {
            f64::NAN
        } else {
            self.sum / self.finite as f64
        };
        write!(
            formatter,
            "min={:?} max={:?} mean={mean:.6} values={} nonfinite={}",
            self.minimum, self.maximum, self.finite, self.nonfinite
        )
    }
}

/// Watches an export's GPU readbacks: they wait as long as the export is not cancelled, and
/// each further second past [`SLOW_GPU_WAIT`] is reported.
pub(super) fn export_gpu_watcher<'a>(
    events: &'a mpsc::Sender<ExportEvent>,
    cancellation: &'a AtomicBool,
) -> impl FnMut(Duration) -> bool + 'a {
    let mut reported_wait_seconds = 0;
    move |waited| {
        if waited >= SLOW_GPU_WAIT && waited.as_secs() != reported_wait_seconds {
            reported_wait_seconds = waited.as_secs();
            let _ = events.send(ExportEvent::WaitingForGpu { waited });
        }
        !cancellation.load(Ordering::Acquire)
    }
}

fn report_completed_export_tile(
    events: &mpsc::Sender<ExportEvent>,
    completed_tiles: &mut usize,
    total_tiles: usize,
    first_progress_logged: &mut bool,
    export_started: Instant,
    rgb: &[f32],
) {
    *completed_tiles += 1;
    if !*first_progress_logged {
        *first_progress_logged = true;
        calibraw_core::diagnostics::record(format!(
            "First export tile completed after {:.3}s; pipelined GPU readback is active; display-linear RGB {}",
            export_started.elapsed().as_secs_f64(),
            ValueSummary::of(rgb.iter().copied()),
        ));
    }
    let _ = events.send(ExportEvent::Progress {
        completed_tiles: *completed_tiles,
        total_tiles,
    });
}

/// The mask atlas edge of an export tile `padded` pixels across in a photo of
/// `image` pixels: the tile and the masks' raster margin around it, or the
/// default edge without masks.
pub(super) fn export_tile_mask_atlas_edge(
    masks: &MaskStack,
    image: [u32; 2],
    padded: [u32; 2],
) -> u32 {
    if masks.masks.is_empty() {
        return mask_atlas_edge();
    }
    let margin = masks.raster_margin_pixels(image[0], image[1]);
    export_mask_atlas_edge(
        padded[0].saturating_add(margin.saturating_mul(2)),
        padded[1].saturating_add(margin.saturating_mul(2)),
    )
}

/// Largest edge of an export tile with its halo, which bounds the GPU memory
/// of one tile: a 1024 px core with a 768 px halo, or a larger core with a
/// smaller halo.
pub(super) const MAX_PADDED_TILE_EDGE: u32 = 1024 + 2 * 768;

/// Largest export tile core: the one the narrowest halo leaves.
const MAX_TILE_CORE: u32 = MAX_PADDED_TILE_EDGE - 2 * MIN_EXPORT_TILE_HALO;

/// Bytes of the source band an export holds: a fixed amount on a computer, and
/// on a phone or tablet the 64 MiB the 384 MiB GPU floor allows, scaled like
/// the export tile whose rows it holds.
fn max_export_band_bytes() -> u64 {
    crate::pipeline::scale_with_mobile_export_tile(MAX_EXPORT_BAND_BYTES)
}

/// Most source rows, aligned to the tone-guide grid, a band of a
/// `source_width` px wide photo may hold.
pub(super) fn max_export_band_rows(source_width: u32) -> Result<u32> {
    let bytes_per_source_row = u64::from(source_width)
        .checked_mul(3)
        .and_then(|value| value.checked_mul(std::mem::size_of::<f32>() as u64))
        .context("export source-band row size overflow")?;
    anyhow::ensure!(bytes_per_source_row > 0, "export source width is zero");
    let maximum_rows =
        (max_export_band_bytes() / bytes_per_source_row).min(u64::from(u32::MAX)) as u32;
    Ok(maximum_rows - maximum_rows % TONE_GUIDE_CELL_SIZE)
}

fn validate_tile_spec(spec: TileSpec) -> Result<()> {
    let maximum_core = MAX_TILE_CORE;
    let scale = TONE_GUIDE_CELL_SIZE;
    anyhow::ensure!(
        (64..=maximum_core).contains(&spec.core_edge),
        "export tile core must be between 64 and {maximum_core} pixels"
    );
    anyhow::ensure!(
        (MIN_EXPORT_TILE_HALO..=EXPORT_TILE_HALO).contains(&spec.halo),
        "export halo must be between {MIN_EXPORT_TILE_HALO} and {EXPORT_TILE_HALO} pixels"
    );
    anyhow::ensure!(
        spec.core_edge.is_multiple_of(scale) && spec.halo.is_multiple_of(scale),
        "export tile core and halo must align to the global tone-guide grid"
    );
    let padded_edge = spec
        .core_edge
        .checked_add(spec.halo.checked_mul(2).context("export halo overflow")?)
        .context("padded export tile overflow")?;
    anyhow::ensure!(
        padded_edge <= MAX_PADDED_TILE_EDGE,
        "export tile must be at most {MAX_PADDED_TILE_EDGE} pixels across with its halo"
    );
    Ok(())
}

pub(super) fn bounded_tile_spec(mut spec: TileSpec, source_width: u32) -> Result<TileSpec> {
    // A wide halo takes its room from the core, so one tile never needs more
    // GPU memory than MAX_PADDED_TILE_EDGE allows.
    let core_room = MAX_PADDED_TILE_EDGE.saturating_sub(spec.halo.saturating_mul(2));
    spec.core_edge = spec
        .core_edge
        .min(core_room - core_room % TONE_GUIDE_CELL_SIZE);
    validate_tile_spec(spec)?;
    let aligned_rows = max_export_band_rows(source_width)?.min(spec.core_edge);
    anyhow::ensure!(
        aligned_rows >= 64,
        "the source image is too wide for the bounded export memory budget"
    );
    spec.core_edge = spec.core_edge.min(aligned_rows);
    validate_tile_spec(spec)?;
    Ok(spec)
}

pub(super) fn tile_mask_source_region(
    masks: &MaskStack,
    tile_origin_x: i32,
    tile_origin_y: i32,
    tile_width: u32,
    tile_height: u32,
    full_width: u32,
    full_height: u32,
) -> [u32; 4] {
    let full_width = full_width.max(1);
    let full_height = full_height.max(1);
    let margin = i64::from(masks.raster_margin_pixels(full_width, full_height));
    let x0 = (i64::from(tile_origin_x) - margin).clamp(0, i64::from(full_width - 1));
    let y0 = (i64::from(tile_origin_y) - margin).clamp(0, i64::from(full_height - 1));
    let x1 = (i64::from(tile_origin_x) + i64::from(tile_width) + margin)
        .clamp(x0 + 1, i64::from(full_width));
    let y1 = (i64::from(tile_origin_y) + i64::from(tile_height) + margin)
        .clamp(y0 + 1, i64::from(full_height));
    [x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32]
}

/// Rasterizes `region` of the masks a render of `params` reads
/// ([`GpuParams::reads_mask_layer`]) and uploads them to the mask atlas.
/// Each layer is rasterized into `bits`, which the caller keeps for its next
/// tile: a new buffer of this size costs a phone its page faults, more than
/// a radial shape takes to draw.
#[allow(clippy::too_many_arguments)]
pub(super) fn upload_mask_atlas(
    pipeline: &RawGpuPipeline,
    queue: &wgpu::Queue,
    masks: &MaskStack,
    params: &GpuParams,
    image_width: u32,
    image_height: u32,
    region: [u32; 4],
    lens_geometry: Option<&LensGeometryMap>,
    bits: &mut Vec<u16>,
) -> Result<()> {
    let edge = pipeline.mask_atlas_edge();
    let extent = mask_region_texture_extent(region, edge);
    let layers = masks.masks.len().min(MAX_LOCAL_MASKS);
    if !(0..layers).any(|layer| params.reads_mask_layer(layer)) {
        return Ok(());
    }
    // Cropped once for every layer.
    let cropped = masks.region(region, [image_width, image_height]);
    for layer in (0..layers).filter(|layer| params.reads_mask_layer(*layer)) {
        cropped.rasterize_layer_f16_into(layer, extent, lens_geometry, bits);
        pipeline
            .update_mask_layer_region(queue, layer, extent[0], extent[1], bits)
            .with_context(|| format!("upload local-mask layer {}", layer + 1))?;
    }
    Ok(())
}
