//! The export worker thread: spawning, diagnostics, tile planning and program reuse.

use super::*;

pub fn spawn_tiled_export(
    format: ExportFormat,
    job: TiledExportJob,
) -> mpsc::Receiver<ExportEvent> {
    let (sender, receiver) = mpsc::channel();
    let worker_sender = sender.clone();
    let worker_path = job.target.path().to_path_buf();
    let worker_name = format.worker_name();

    let spawn_result = std::thread::Builder::new()
        .name(worker_name.to_owned())
        .spawn(move || {
            let worker_started = Instant::now();
            record_export_worker_started(format, &job);
            let result = run_export_worker(format, &job, &worker_sender);
            record_export_worker_finished(format, worker_started, &result);
            let _ = worker_sender.send(ExportEvent::Finished(
                result
                    .map(|_| worker_path)
                    .map_err(|error| format!("{error:#}")),
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
            "PNG export worker started: source={}x{} cfa={:?} requested_tile_core={} halo={} exposure={:.3} temperature={:.3} tint={:.3} demosaic={:?} highlight={:?}",
            job.raw.width,
            job.raw.height,
            job.raw.cfa_kind,
            job.tile_spec.core_edge,
            job.tile_spec.halo,
            job.exposure.exposure,
            job.exposure.temperature,
            job.exposure.tint,
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

fn record_export_worker_finished(format: ExportFormat, started: Instant, result: &Result<()>) {
    let format_name = match format {
        ExportFormat::Png => "PNG",
        ExportFormat::Jpeg => "JPEG",
        ExportFormat::Tiff | ExportFormat::JpegXl => return,
    };
    match result {
        Ok(()) => calibraw_core::diagnostics::record(format!(
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
) -> Result<()> {
    let program_template = (job.raw.cfa_kind == CfaKind::Bayer)
        .then(|| await_export_program_template(job.program_prewarm.as_deref()))
        .flatten();
    let geometry = job.geometry.sanitized();
    let (geometry_width, geometry_height) =
        geometry.crop_pixel_dimensions(job.raw.width, job.raw.height);
    let (output_width, output_height) = job
        .settings
        .checked_output_dimensions(geometry_width, geometry_height)?;
    let tile_spec =
        resolved_export_tile_spec(job.tile_spec, &job.exposure, &job.masks, job.raw.width)?;

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

    export_to_destination(&job.target, &job.cancellation, |output| {
        let context = ExportContext {
            device: &job.device,
            queue: &job.queue,
            events,
            cancellation: &job.cancellation,
            program_template: program_template.as_deref(),
        };
        let request = ExportRequest {
            raw: &job.raw,
            exposure: &job.exposure,
            masks: &job.masks,
            remove: &job.remove,
            output,
            tile_spec,
            output_width,
            output_height,
            keep_metadata: job.settings.keep_metadata,
            metadata: &job.metadata,
            geometry,
            bit_depth,
            color: &color,
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
    source_width: u32,
) -> Result<TileSpec> {
    let required_halo = required_export_tile_halo(exposure, masks);
    tile_spec.halo = if tile_spec.halo == EXPORT_TILE_HALO {
        required_halo
    } else {
        tile_spec.halo.max(required_halo)
    };
    bounded_tile_spec(tile_spec, source_width)
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
