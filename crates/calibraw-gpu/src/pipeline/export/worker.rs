//! The export worker thread: spawning, diagnostics, tile planning and program reuse.

use super::*;

pub fn spawn_tiled_export(
    format: ExportFormat,
    job: TiledExportJob,
) -> mpsc::Receiver<ExportEvent> {
    let (sender, receiver) = mpsc::channel();
    let worker_sender = sender.clone();
    let worker_name = format.worker_name();

    let spawn_result = std::thread::Builder::new()
        .name(worker_name.to_owned())
        .spawn(move || {
            let worker_started = Instant::now();
            record_export_worker_started(format, &job);
            let result = run_export_worker(format, &job, &worker_sender);
            record_export_worker_finished(format, worker_started, &result);
            let _ = worker_sender.send(ExportEvent::Finished(
                result.map_err(|error| format!("{error:#}")),
            ));
        });

    if let Err(error) = spawn_result {
        let _ = sender.send(ExportEvent::Finished(Err(format!(
            "{}: {error}",
            format.worker_spawn_error()
        ))));
    }
    receiver
}

fn record_export_worker_started(format: ExportFormat, job: &TiledExportJob) {
    match format {
        ExportFormat::Png => calibraw_core::diagnostics::record(format!(
            "PNG export worker started: source={}x{} cfa={:?} requested_tile_core={} halo={} exposure={:.3} white_balance={:?} demosaic={:?} highlight={:?}",
            job.raw.width,
            job.raw.height,
            job.raw.cfa_kind,
            job.tile_spec.core_edge,
            job.tile_spec.halo,
            job.exposure.exposure,
            job.exposure.white_balance,
            job.exposure.demosaic_mode,
            job.exposure.highlight_method,
        )),
        ExportFormat::Jpeg => calibraw_core::diagnostics::record(format!(
            "JPEG export worker started: source={}x{} quality={} cfa={:?} requested_tile_core={} halo={}",
            job.raw.width,
            job.raw.height,
            job.settings.jpeg_quality,
            job.raw.cfa_kind,
            job.tile_spec.core_edge,
            job.tile_spec.halo,
        )),
        ExportFormat::Tiff | ExportFormat::JpegXl => {}
    }
}

fn record_export_worker_finished(format: ExportFormat, started: Instant, result: &Result<PathBuf>) {
    let format_name = match format {
        ExportFormat::Png => "PNG",
        ExportFormat::Jpeg => "JPEG",
        ExportFormat::Tiff | ExportFormat::JpegXl => return,
    };
    match result {
        Ok(_) => calibraw_core::diagnostics::record(format!(
            "{format_name} export worker finished successfully in {:.3}s",
            started.elapsed().as_secs_f64()
        )),
        Err(error) => calibraw_core::diagnostics::record(format!(
            "{format_name} export worker failed after {:.3}s: {error:#}",
            started.elapsed().as_secs_f64()
        )),
    }
}

fn run_export_worker(
    format: ExportFormat,
    job: &TiledExportJob,
    events: &mpsc::Sender<ExportEvent>,
) -> Result<PathBuf> {
    let program_template = (job.raw.cfa_kind == CfaKind::Bayer)
        .then(|| await_export_program_template(job.program_prewarm.as_deref()))
        .flatten();
    let geometry = job.geometry.sanitized();
    let (targets, window) = match &job.files {
        ExportFiles::Single(target) => {
            let (geometry_width, geometry_height) =
                geometry.crop_pixel_dimensions(job.raw.width, job.raw.height);
            let (width, height) = job
                .settings
                .checked_output_dimensions(geometry_width, geometry_height)?;
            (
                std::slice::from_ref(target),
                OutputWindow::whole(width, height),
            )
        }
        ExportFiles::Carousel { carousel, slides } => {
            anyhow::ensure!(
                slides.len() == usize::from(carousel.slides()),
                "a carousel needs one target per slide"
            );
            let layout = carousel.layout(geometry, job.raw.width, job.raw.height, &job.settings)?;
            (slides.as_slice(), layout.window())
        }
    };
    let tile_spec = resolved_export_tile_spec(job.tile_spec, &job.exposure, &job.masks, &job.raw)?;

    let color_settings = match format {
        ExportFormat::Jpeg => {
            let mut settings = job.settings.clone();
            settings.bit_depth = ExportBitDepth::Eight;
            Cow::Owned(settings)
        }
        ExportFormat::Png | ExportFormat::Tiff | ExportFormat::JpegXl => {
            Cow::Borrowed(&job.settings)
        }
    };
    let color = resolve_export_color(color_settings.as_ref())?;
    let bit_depth = if format == ExportFormat::Jpeg {
        ExportBitDepth::Eight
    } else {
        job.settings.bit_depth
    };

    let memory = IntermediateMemory::for_this_device();
    export_to_destinations(targets, &job.cancellation, |outputs| {
        let context = ExportContext {
            device: &job.device,
            queue: &job.queue,
            events,
            cancellation: &job.cancellation,
            program_template: program_template.as_deref(),
            memory: &memory,
        };
        let request = ExportRequest {
            raw: &job.raw,
            exposure: &job.exposure,
            masks: &job.masks,
            remove: &job.remove,
            look: job.look.as_ref(),
            outputs,
            tile_spec,
            window,
            keep_metadata: job.settings.keep_metadata,
            metadata: &job.metadata,
            geometry,
            bit_depth,
            color: &color,
            watermark: job.settings.watermark.as_ref(),
        };
        match format {
            ExportFormat::Png => export_tiled_png(context, request),
            ExportFormat::Jpeg => export_tiled_jpeg(context, request, job.settings.jpeg_quality),
            ExportFormat::Tiff => export_tiled_tiff(context, request),
            ExportFormat::JpegXl => export_tiled_jxl(context, request),
        }
    })
}

pub(super) fn resolved_export_tile_spec(
    mut tile_spec: TileSpec,
    exposure: &ExposureParams,
    masks: &MaskStack,
    raw: &LoadedRaw,
) -> Result<TileSpec> {
    let required_halo = required_export_tile_halo(exposure, masks);
    tile_spec.halo = if tile_spec.halo == EXPORT_TILE_HALO {
        required_halo
    } else {
        tile_spec.halo.max(required_halo)
    };
    if tile_spec.core_edge == TileSpec::default().core_edge {
        tile_spec.core_edge = device_export_core_edge(tile_spec, exposure, masks, raw)?;
    }
    bounded_tile_spec(tile_spec, raw.width)
}

/// The core of an export tile of the default size, `spec.core_edge`. A phone
/// or tablet, whose GPU working set is limited, takes the largest core whose
/// graph fits the share of the limit previews leave for an export tile
/// (`mobile_export_padded_edge`), never less than the default, and then the
/// smallest core that covers the photo in as many tiles. A computer keeps the
/// default.
fn device_export_core_edge(
    spec: TileSpec,
    exposure: &ExposureParams,
    masks: &MaskStack,
    raw: &LoadedRaw,
) -> Result<u32> {
    let default_core = spec.core_edge;
    let halo_edges = spec.halo.saturating_mul(2);
    // A larger tile must not shrink its mask region into the atlas, which
    // would rasterize the masks coarser than the default tile does.
    let mut maximum_padded = MAX_PADDED_TILE_EDGE;
    if !masks.masks.is_empty() {
        let margin = masks.raster_margin_pixels(raw.width, raw.height);
        maximum_padded = maximum_padded
            .min(export_mask_atlas_edge_limit().saturating_sub(margin.saturating_mul(2)));
    }
    let shape = ExportTileShape {
        cfa_kind: raw.cfa_kind,
        layout: raw.pixel_layout(),
        ai_cfa: exposure.ai_denoise_enabled,
        mask_layers: masks.masks.len(),
        profile_bytes: PREVIEWED_PROFILE_BYTES,
    };
    let Some(padded) = RawGpuPipeline::mobile_export_padded_edge(maximum_padded, shape, |edge| {
        export_tile_mask_atlas_edge(masks, [raw.width, raw.height], [edge, edge])
    }) else {
        return Ok(default_core);
    };
    let fitted = padded.saturating_sub(halo_edges);
    let fitted = (fitted - fitted % TONE_GUIDE_CELL_SIZE)
        .min(max_export_band_rows(raw.width)?)
        .min(MAX_PADDED_TILE_EDGE.saturating_sub(halo_edges));
    if fitted <= default_core {
        return Ok(default_core);
    }
    // Tiles are square and the last ones overhang the photo, so the fewest
    // tiles of `fitted` are as well covered by a smaller core.
    let columns = raw.width.div_ceil(fitted).max(1);
    let rows = raw.height.div_ceil(fitted).max(1);
    let covering = raw
        .width
        .div_ceil(columns)
        .max(raw.height.div_ceil(rows))
        .next_multiple_of(TONE_GUIDE_CELL_SIZE)
        .clamp(64, fitted);
    calibraw_core::diagnostics::record(format!(
        "Export tile core {covering} px sized for the GPU working set (largest fitting {fitted} px, halo {})",
        spec.halo
    ));
    Ok(covering)
}

pub(super) fn ensure_export_not_cancelled(cancellation: &AtomicBool) -> Result<()> {
    anyhow::ensure!(!cancellation.load(Ordering::Acquire), "export cancelled");
    Ok(())
}

fn await_export_program_template(
    prewarm: Option<&GpuProgramPrewarm>,
) -> Option<Arc<RawGpuProgramTemplate>> {
    let prewarm = prewarm?;
    let wait_started = Instant::now();
    match prewarm.wait() {
        Ok(template) => {
            calibraw_core::diagnostics::record(format!(
                "Full-quality export program prewarm available after {:.3}s wait",
                wait_started.elapsed().as_secs_f64()
            ));
            Some(template)
        }
        Err(error) => {
            calibraw_core::diagnostics::record(format!(
                "Full-quality export program prewarm unavailable: {error}"
            ));
            None
        }
    }
}

/// The edits an export renders, for [`compile_export_programs`].
pub struct ExportProgramEdits<'a> {
    pub exposure: &'a ExposureParams,
    pub masks: &'a MaskStack,
    pub geometry: GeometryTransform,
    pub look: Option<&'a ResolvedLook>,
}

/// Compiles into `template` the programs exporting `raw` with `edits` uses (its effects,
/// colour denoise and demosaic variant), so the export reusing the template does not wait for
/// them, and saves them to the template's persistent pipeline cache. It renders nothing: it
/// builds a 64 px tile's graph from the template, whose construction compiles what the edits
/// select. Blocks for as long as compiling takes; call it off the UI thread.
pub fn compile_export_programs(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    template: &RawGpuProgramTemplate,
    raw: &LoadedRaw,
    edits: ExportProgramEdits<'_>,
) -> Result<()> {
    let ExportProgramEdits {
        exposure,
        masks,
        geometry,
        look,
    } = edits;
    let spec = TileSpec {
        core_edge: 64,
        halo: required_export_tile_halo(exposure, masks),
    };
    let tile = *TilePlan::new(raw.width, raw.height, spec)
        .tiles
        .first()
        .context("cannot compile the programs of an empty RAW image")?;
    let tile_raw = extract_padded_tile(raw, tile);
    let params = GpuParams::new_for_tile(
        exposure,
        masks,
        &tile_raw,
        tile.global_origin_x,
        tile.global_origin_y,
        raw.width,
        raw.height,
    )
    .with_vignette_geometry(geometry)
    .with_look_lut(look);
    RawGpuPipeline::new(
        device,
        queue,
        &tile_raw,
        &params,
        PipelineOptions::new(ProcessingQuality::High)
            .mask_atlas_edge(64)
            .programs(template),
    )
    .context("compile the export programs")?;
    template.persist_pipeline_cache()?;
    Ok(())
}
