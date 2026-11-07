//! Rendering tiles and streaming them to the encoder in row bands.

use super::*;

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
        program_template,
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
        raw.inpaint_opposed_chroma_for_exposure(exposure);
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
    let tile_pipeline = if let Some(template) = program_template {
        match RawGpuPipeline::new(
            device,
            queue,
            &first_raw,
            &first_params,
            PipelineOptions::new(ProcessingQuality::High)
                .mask_atlas_edge(export_mask_edge)
                .programs(template),
        ) {
            Ok(pipeline) => {
                calibraw_core::diagnostics::record(
                    "Full-quality export reused startup-precompiled GPU programs",
                );
                pipeline
            }
            Err(reuse_error) => {
                calibraw_core::diagnostics::record(format!(
                    "Full-quality export program reuse unavailable ({reuse_error:#}); compiling programs"
                ));
                RawGpuPipeline::new(
                    device,
                    queue,
                    &first_raw,
                    &first_params,
                    PipelineOptions::new(ProcessingQuality::High).mask_atlas_edge(export_mask_edge),
                )
                .context("create reusable full-quality export pipeline")?
            }
        }
    } else {
        RawGpuPipeline::new(
            device,
            queue,
            &first_raw,
            &first_params,
            PipelineOptions::new(ProcessingQuality::High).mask_atlas_edge(export_mask_edge),
        )
        .context("create reusable full-quality export pipeline")?
    };
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
    let mut tile_scratch = first_raw;
    tile_pipeline.begin_export_tone_analysis(queue, device);
    for (index, tile) in plan.tiles.iter().copied().enumerate() {
        ensure_export_not_cancelled(cancellation)?;
        if index != 0 {
            extract_padded_tile_into(raw, tile, &mut tile_scratch);
        }
        tile_pipeline
            .upload_raw_tile(queue, &tile_scratch)
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
        tile_pipeline
            .accumulate_export_tone_tile_with_remove(
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
                store_raw_stage_tile(
                    store,
                    device,
                    &mut watch_gpu,
                    previous,
                    previous_index,
                    readback,
                )?;
            }
        }
    }
    if let (Some(store), Some((tile, index, readback))) =
        (&mut raw_stage_store, pending_scene.take())
    {
        store_raw_stage_tile(store, device, &mut watch_gpu, tile, index, readback)?;
    }
    tile_pipeline.finish_export_tone_analysis(queue, device);
    calibraw_core::diagnostics::record(format!(
        "Exact full-resolution tone-analysis prepass queued in {:.3}s across {} tiles",
        tone_analysis_started.elapsed().as_secs_f64(),
        plan.tile_count()
    ));

    if let Some(timing) = &gpu_timing {
        timing.phase("final tiles");
    }
    let total_tiles = plan.tile_count();
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
        band.resize(band_values, 0.0f32);
        let mut pending_readback = None;
        for (absolute_index, tile) in plan.tiles[band_start..tile_index]
            .iter()
            .copied()
            .enumerate()
        {
            ensure_export_not_cancelled(cancellation)?;
            let global_index = band_start + absolute_index;
            extract_padded_tile_into(raw, tile, &mut tile_scratch);
            tile_pipeline
                .upload_raw_tile(queue, &tile_scratch)
                .with_context(|| format!("upload export tile {}", global_index + 1))?;
            let mask_region = tile_mask_source_region(
                masks,
                tile.global_origin_x,
                tile.global_origin_y,
                tile.padded_width,
                tile.padded_height,
                raw.width,
                raw.height,
            );
            let mask_extent =
                mask_region_texture_extent(mask_region, tile_pipeline.mask_atlas_edge());
            upload_mask_atlas(
                &tile_pipeline,
                queue,
                masks,
                raw.width,
                raw.height,
                mask_region,
                raw.lens_geometry.as_deref(),
            )?;

            let params = GpuParams::new_for_tile(
                exposure,
                masks,
                &tile_scratch,
                tile.global_origin_x,
                tile.global_origin_y,
                raw.width,
                raw.height,
            )
            .with_vignette_geometry(geometry)
            .with_mask_uv_rect_and_extent(
                mask_source_region_uv(mask_region, raw.width, raw.height),
                mask_extent,
            )
            .with_look_lut(look);
            match &raw_stage_store {
                Some(store) => {
                    store
                        .write_tile(&tile_pipeline, queue, tile)
                        .with_context(|| {
                            format!("upload the RAW-stage scene of tile {}", global_index + 1)
                        })?;
                    tile_pipeline.dispatch_export_tile_from_scene(queue, device, &params);
                }
                None => tile_pipeline
                    .dispatch_export_tile_with_remove(
                        queue,
                        device,
                        &params,
                        RemoveSceneContext::new(
                            remove,
                            raw,
                            exposure,
                            [tile.global_origin_x as f32, tile.global_origin_y as f32],
                            [tile.padded_width as f32, tile.padded_height as f32],
                        ),
                    )
                    .with_context(|| format!("apply Remove to export tile {}", global_index + 1))?,
            }
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

            let previous = pending_readback.replace((tile, global_index, readback));
            if let Some((previous_tile, previous_index, previous_readback)) = previous {
                let rgb = previous_readback
                    .finish(device, GpuWait::Watched(&mut watch_gpu))
                    .with_context(|| format!("read export tile {}", previous_index + 1))?;
                stitch_linear_tile_into_band(&mut band, raw.width, band_y, previous_tile, &rgb)?;
                report_completed_export_tile(
                    events,
                    &mut completed_tiles,
                    total_tiles,
                    &mut first_progress_logged,
                    export_started,
                );
            }
        }

        if let Some((last_tile, last_index, last_readback)) = pending_readback.take() {
            let rgb = last_readback
                .finish(device, GpuWait::Watched(&mut watch_gpu))
                .with_context(|| format!("read export tile {}", last_index + 1))?;
            stitch_linear_tile_into_band(&mut band, raw.width, band_y, last_tile, &rgb)?;
            report_completed_export_tile(
                events,
                &mut completed_tiles,
                total_tiles,
                &mut first_progress_logged,
                export_started,
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
            row_sink(source_y, &band[start..end])?;
        }
    }
    if let Some(timing) = gpu_timing {
        timing.finish();
    }
    Ok(())
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
    readback
        .finish(device, GpuWait::Watched(watch_gpu), |row, texels| {
            store.store_row(tile, row, texels);
        })
        .with_context(|| format!("read the RAW-stage scene of tile {}", index + 1))
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
) {
    *completed_tiles += 1;
    if !*first_progress_logged {
        *first_progress_logged = true;
        calibraw_core::diagnostics::record(format!(
            "First export tile completed after {:.3}s; pipelined GPU readback is active",
            export_started.elapsed().as_secs_f64()
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

pub(super) fn upload_mask_atlas(
    pipeline: &RawGpuPipeline,
    queue: &wgpu::Queue,
    masks: &MaskStack,
    image_width: u32,
    image_height: u32,
    region: [u32; 4],
    lens_geometry: Option<&LensGeometryMap>,
) -> Result<()> {
    let edge = pipeline.mask_atlas_edge();
    let extent = mask_region_texture_extent(region, edge);
    for layer in 0..masks.masks.len().min(MAX_LOCAL_MASKS) {
        let bytes = masks.rasterize_layer_region_f16(
            layer,
            extent,
            region,
            [image_width, image_height],
            lens_geometry,
        );
        pipeline
            .update_mask_layer_region(queue, layer, extent[0], extent[1], &bytes)
            .with_context(|| format!("upload local-mask layer {}", layer + 1))?;
    }
    Ok(())
}
