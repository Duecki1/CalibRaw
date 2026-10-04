//! Rendering tiles and streaming them to the encoder in row bands.

use super::*;

pub(super) fn render_export_output<W: Write>(
    context: ExportContext<'_>,
    request: ExportRequest<'_>,
    output: &mut W,
    row_format: ExportRowFormat,
) -> Result<()> {
    validate_export_dimensions(request.output_width, request.output_height)?;
    if !request.geometry.is_identity() || request.raw.lens_geometry.is_some() {
        return render_geometry_output(context, request, output, row_format);
    }

    let output_transform = request.color.transform.as_ref();
    let mut resizer = LinearLightResizer::new_with_format(
        request.raw.width,
        request.raw.height,
        request.output_width,
        request.output_height,
        row_format,
    )?;
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
    } = context;
    let ExportRequest {
        raw,
        exposure,
        masks,
        remove,
        output: _,
        tile_spec,
        output_width,
        output_height,
        keep_metadata: _,
        metadata: _,
        geometry,
        bit_depth: _,
        color: _,
    } = request;
    let export_started = Instant::now();
    ensure_export_not_cancelled(cancellation)?;
    anyhow::ensure!(
        !exposure.ai_denoise_enabled || raw.ai_denoised_image().is_some(),
        "AI denoise is enabled but its full-resolution RawNIND result is not ready"
    );
    validate_export_dimensions(output_width, output_height)?;
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
        output_width,
        output_height,
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
    ));
    let pipeline_started = Instant::now();
    let export_mask_edge = if masks.masks.is_empty() {
        mask_atlas_edge()
    } else {
        let margin = masks.raster_margin_pixels(raw.width, raw.height);
        export_mask_atlas_edge(
            first.padded_width.saturating_add(margin.saturating_mul(2)),
            first.padded_height.saturating_add(margin.saturating_mul(2)),
        )
    };
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
    calibraw_core::diagnostics::record(format!(
        "Full-quality export pipeline prepared in {:.3}s; padded_tile={}x{} viewport-local mask_atlas={}x{} R16F",
        pipeline_started.elapsed().as_secs_f64(),
        first_raw.width,
        first_raw.height,
        export_mask_edge,
        export_mask_edge
    ));
    tile_pipeline
        .update_light_rays_mask_layers(
            queue,
            masks,
            raw.width,
            raw.height,
            raw.lens_geometry.as_deref(),
        )
        .context("upload full-image Light Rays emission masks")?;

    let tone_analysis_started = Instant::now();
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
    }
    tile_pipeline.finish_export_tone_analysis(queue, device);
    calibraw_core::diagnostics::record(format!(
        "Exact full-resolution tone-analysis prepass queued in {:.3}s across {} tiles",
        tone_analysis_started.elapsed().as_secs_f64(),
        plan.tile_count()
    ));

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
            );
            tile_pipeline
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
                .with_context(|| format!("apply Remove to export tile {}", global_index + 1))?;
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
                    .finish(device)
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
                .finish(device)
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
    Ok(())
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

fn validate_tile_spec(spec: TileSpec) -> Result<()> {
    let maximum_core = 1024;
    let maximum_halo = 768;
    let scale = TONE_GUIDE_CELL_SIZE;
    anyhow::ensure!(
        (64..=maximum_core).contains(&spec.core_edge),
        "export tile core must be between 64 and {maximum_core} pixels"
    );
    anyhow::ensure!(
        (MIN_EXPORT_TILE_HALO..=maximum_halo).contains(&spec.halo),
        "export halo must be between {MIN_EXPORT_TILE_HALO} and {maximum_halo} pixels"
    );
    anyhow::ensure!(
        spec.core_edge.is_multiple_of(scale) && spec.halo.is_multiple_of(scale),
        "export tile core and halo must align to the global tone-guide grid"
    );
    spec.core_edge
        .checked_add(spec.halo.checked_mul(2).context("export halo overflow")?)
        .context("padded export tile overflow")?;
    Ok(())
}

pub(super) fn bounded_tile_spec(mut spec: TileSpec, source_width: u32) -> Result<TileSpec> {
    validate_tile_spec(spec)?;
    let bytes_per_source_row = u64::from(source_width)
        .checked_mul(3)
        .and_then(|value| value.checked_mul(std::mem::size_of::<f32>() as u64))
        .context("export source-band row size overflow")?;
    anyhow::ensure!(bytes_per_source_row > 0, "export source width is zero");
    let alignment = TONE_GUIDE_CELL_SIZE;
    let maximum_rows =
        (MAX_EXPORT_BAND_BYTES / bytes_per_source_row).min(u64::from(spec.core_edge)) as u32;
    let aligned_rows = maximum_rows - maximum_rows % alignment;
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
