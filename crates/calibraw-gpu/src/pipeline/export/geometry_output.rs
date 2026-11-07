//! Output through a geometry transform: the rendered photo is resampled on the GPU through the
//! crop, turns, flips, rotation, perspective and lens map (`resample_geometry`).

use super::*;

pub(super) fn render_geometry_output<W: Write>(
    context: ExportContext<'_>,
    request: ExportRequest<'_>,
    output: &mut W,
    row_format: ExportRowFormat,
) -> Result<()> {
    let (source_width, source_height) = (request.raw.width, request.raw.height);
    let source_bytes = checked_rgb_len(source_width, source_height)? as u64 * 4;
    let mut linear = IntermediateWriter::new(
        request.staging_output()?,
        source_bytes,
        context.memory,
        "linear raster",
    )?;
    stream_tiled_linear_rows(context, request, |_source_y, row| {
        linear
            .write_all(bytemuck::cast_slice(row))
            .context("write geometry linear source row")
    })?;
    let linear = linear.finish()?;
    let source = validate_linear_rgb_raster(&linear, source_width, source_height)?;

    let window = request.window;
    let map = GeometryInverseMap::new_with_lens(
        request.geometry,
        request.raw.lens_geometry.as_deref(),
        source_width,
        source_height,
        window.raster_width,
        window.raster_height,
    );
    let (geometry_width, geometry_height) = request
        .geometry
        .crop_pixel_dimensions(source_width, source_height);
    let mut output_sharpen = FinalSizeOutputSharpen::new(
        geometry_width,
        geometry_height,
        window.raster_width,
        window.raster_height,
    )
    .with_row_width(window.width)
    .with_passthrough(row_format == ExportRowFormat::RgbF32Le)
    .with_watermark(PreparedWatermark::for_request(&request)?);
    let output_transform = request.color.transform.as_ref();
    let finalize_started = Instant::now();
    let mut watch_gpu = export_gpu_watcher(context.events, context.cancellation);
    resample_geometry(
        context.device,
        context.queue,
        GeometryResampleJob {
            map,
            source,
            window: [window.left, window.top, window.width, window.height],
        },
        GpuWait::Watched(&mut watch_gpu),
        |row| {
            ensure_export_not_cancelled(context.cancellation)?;
            output_sharpen.push_row(row, output_transform, row_format, output)
        },
    )?;
    output_sharpen.finish(output_transform, row_format, output)?;
    calibraw_core::diagnostics::record(format!(
        "Export geometry, final sharpening and output encoding finished in {:.3}s: {}x{}",
        finalize_started.elapsed().as_secs_f64(),
        window.width,
        window.height,
    ));
    Ok(())
}
