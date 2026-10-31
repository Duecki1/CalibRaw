//! PNG, JPEG and JPEG XL writers.

use super::*;

pub(super) fn export_tiled_png(
    context: ExportContext<'_>,
    request: ExportRequest<'_>,
) -> Result<()> {
    validate_export_dimensions(request.window.width, request.window.height)?;
    anyhow::ensure!(
        !request.bit_depth.is_float(),
        "PNG export supports 8-bit or 16-bit integer output; use TIFF for a float/linear master"
    );
    let row_format = match request.bit_depth {
        ExportBitDepth::Eight => ExportRowFormat::Rgba8,
        ExportBitDepth::Sixteen => ExportRowFormat::Rgba16Be,
        ExportBitDepth::Float32Linear => unreachable!("float PNG rejected above"),
    };
    let mut writers = request
        .outputs
        .iter()
        .map(|&output| write_png_header(request, output))
        .collect::<Result<Vec<_>>>()?;
    let mut streams = writers
        .iter_mut()
        .map(|writer| {
            writer
                .stream_writer_with_size(64 * 1024)
                .context("create streaming PNG writer")
        })
        .collect::<Result<Vec<_>>>()?;
    let mut rows = SlideWriter::new(&mut streams, request.slide_width(), row_format);
    render_export_output(context, request, &mut rows, row_format)?;
    for stream in streams {
        stream.finish().context("finish streaming PNG data")?;
    }
    for writer in writers {
        writer.finish().context("finish PNG file")?;
    }
    Ok(())
}

/// Starts the PNG file of one slide, up to its pixel data.
fn write_png_header(
    request: ExportRequest<'_>,
    output: ExportOutput<'_>,
) -> Result<png::Writer<BufWriter<fs::File>>> {
    let (width, height) = (request.slide_width(), request.window.height);
    let file = open_export_destination(output)
        .with_context(|| format!("create export {}", output.path.display()))?;
    let mut info = png::Info::with_size(width, height);
    info.color_type = png::ColorType::Rgba;
    info.bit_depth = match request.bit_depth {
        ExportBitDepth::Eight => png::BitDepth::Eight,
        ExportBitDepth::Sixteen => png::BitDepth::Sixteen,
        ExportBitDepth::Float32Linear => unreachable!("float PNG rejected by the caller"),
    };
    if let Some(profile) = request.color.embedded_icc.as_ref() {
        info.icc_profile = Some(Cow::Owned(profile.clone()));
    }
    if request.keep_metadata {
        info.exif_metadata = Some(Cow::Owned(build_exif_payload(
            request.metadata,
            width,
            height,
        )));
    }
    let mut encoder =
        png::Encoder::with_info(BufWriter::new(file), info).context("configure PNG encoder")?;
    // Lossless pixel values are identical; avoid spending most of an export
    // searching for a slightly smaller DEFLATE stream.
    encoder.set_compression(png::Compression::Fast);
    if request.color.srgb {
        encoder.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
    }
    if request.keep_metadata {
        add_png_text_metadata(&mut encoder, request.metadata, width, height)?;
    }
    encoder
        .write_header()
        .with_context(|| format!("write PNG header for {}", output.path.display()))
}

/// Renders `request` into one staged raster per slide, then hands `encode` each slide's output
/// and pixels, removing each raster once it is encoded.
fn render_staged_slides(
    context: ExportContext<'_>,
    request: ExportRequest<'_>,
    row_format: ExportRowFormat,
    mut encode: impl FnMut(ExportOutput<'_>, &[u8]) -> Result<()>,
) -> Result<()> {
    let staged = request
        .outputs
        .iter()
        .map(|&output| StagedFile::for_output(output))
        .collect::<Result<Vec<_>>>()?;
    {
        let mut writers = staged
            .iter()
            .map(|raster| {
                let file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(raster.path())
                    .with_context(|| {
                        format!("create staged RGB raster {}", raster.path().display())
                    })?;
                Ok(BufWriter::new(file))
            })
            .collect::<Result<Vec<_>>>()?;
        let mut rows = SlideWriter::new(&mut writers, request.slide_width(), row_format);
        render_export_output(context, request, &mut rows, row_format)?;
        rows.flush().context("flush staged RGB raster")?;
    }

    for (raster, &output) in staged.into_iter().zip(request.outputs) {
        ensure_export_not_cancelled(context.cancellation)?;
        let file = fs::File::open(raster.path())
            .with_context(|| format!("open staged RGB raster {}", raster.path().display()))?;
        // SAFETY: the raster is a private staging file with a unique name, finished above and
        // not written again; the mapping is dropped before the file is removed.
        let mapped = unsafe { memmap2::MmapOptions::new().map(&file) }
            .with_context(|| format!("map staged RGB raster {}", raster.path().display()))?;
        encode(output, &mapped)?;
    }
    Ok(())
}

pub(super) fn export_tiled_jpeg(
    context: ExportContext<'_>,
    request: ExportRequest<'_>,
    quality: u8,
) -> Result<()> {
    let quality = quality.clamp(1, 100);
    render_staged_slides(context, request, ExportRowFormat::Rgb8, |output, rgb| {
        encode_jpeg_rgb(JpegEncodeRequest {
            rgb,
            output,
            width: request.slide_width(),
            height: request.window.height,
            quality,
            keep_metadata: request.keep_metadata,
            metadata: request.metadata,
            icc_profile: request.color.embedded_icc.as_deref(),
        })
    })
}

pub(super) fn export_tiled_jxl(
    context: ExportContext<'_>,
    request: ExportRequest<'_>,
) -> Result<()> {
    anyhow::ensure!(
        !request.bit_depth.is_float(),
        "JPEG XL export supports 8-bit or 16-bit integer output"
    );
    let (width, height) = (request.slide_width(), request.window.height);
    anyhow::ensure!(
        width > 1 && height > 1,
        "JPEG XL encoder requires width and height greater than one pixel"
    );
    let row_format = match request.bit_depth {
        ExportBitDepth::Eight => ExportRowFormat::Rgb8,
        ExportBitDepth::Sixteen => ExportRowFormat::Rgb16Le,
        ExportBitDepth::Float32Linear => unreachable!(),
    };
    render_staged_slides(context, request, row_format, |output, mapped| {
        let bytes_per_sample = if request.bit_depth == ExportBitDepth::Eight {
            1
        } else {
            2
        };
        let expected = checked_rgb_len(width, height)?
            .checked_mul(bytes_per_sample)
            .context("JPEG XL raster length overflow")?;
        anyhow::ensure!(
            mapped.len() == expected,
            "JPEG XL RGB raster has unexpected size"
        );
        let depth = if bytes_per_sample == 1 {
            zune_core::bit_depth::BitDepth::Eight
        } else {
            zune_core::bit_depth::BitDepth::Sixteen
        };
        let options = zune_core::options::EncoderOptions::new(
            width as usize,
            height as usize,
            zune_core::colorspace::ColorSpace::RGB,
            depth,
        );
        let encoder = zune_jpegxl::JxlSimpleEncoder::new(mapped, options);
        if request.keep_metadata {
            with_staging_file(output, |staged_jxl| {
                let encoded_file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(staged_jxl)
                    .context("create staged JPEG XL codestream")?;
                let mut encoded_writer = BufWriter::new(encoded_file);
                encoder
                    .encode(&mut encoded_writer)
                    .map_err(|error| anyhow::anyhow!("encode JPEG XL: {error}"))?;
                encoded_writer.flush().context("flush JPEG XL codestream")?;
                drop(encoded_writer);
                let mut encoded_file =
                    fs::File::open(staged_jxl).context("open staged JPEG XL codestream")?;
                let codestream_len = encoded_file.metadata()?.len();
                let output_file = open_export_destination(output)
                    .with_context(|| format!("create JPEG XL {}", output.path.display()))?;
                let mut writer = BufWriter::new(output_file);
                write_jxl_container(
                    &mut writer,
                    &mut encoded_file,
                    codestream_len,
                    &build_exif_payload(request.metadata, width, height),
                )?;
                writer.flush().context("flush JPEG XL container")
            })?;
        } else {
            let output_file = open_export_destination(output)
                .with_context(|| format!("create JPEG XL {}", output.path.display()))?;
            let mut writer = BufWriter::new(output_file);
            encoder
                .encode(&mut writer)
                .map_err(|error| anyhow::anyhow!("encode JPEG XL: {error}"))?;
            writer.flush().context("flush JPEG XL export")?;
        }
        Ok(())
    })
}

pub(super) fn write_jxl_container<W: Write>(
    output: &mut W,
    codestream: &mut impl std::io::Read,
    codestream_len: u64,
    exif: &[u8],
) -> Result<()> {
    // JPEG XL container signature and file type boxes.
    output.write_all(&[0, 0, 0, 12, b'J', b'X', b'L', b' ', 13, 10, 0x87, 10])?;
    output.write_all(&[
        0, 0, 0, 20, b'f', b't', b'y', b'p', b'j', b'x', b'l', b' ', 0, 0, 0, 0, b'j', b'x', b'l',
        b' ',
    ])?;
    let exif_box_size = u32::try_from(
        exif.len()
            .checked_add(12)
            .context("JPEG XL EXIF size overflow")?,
    )
    .context("JPEG XL EXIF box too large")?;
    output.write_all(&exif_box_size.to_be_bytes())?;
    output.write_all(b"Exif")?;
    output.write_all(&0u32.to_be_bytes())?; // TIFF header starts at byte zero.
    output.write_all(exif)?;
    let box_size = codestream_len
        .checked_add(8)
        .context("JPEG XL codestream size overflow")?;
    if box_size <= u32::MAX as u64 {
        output.write_all(&(box_size as u32).to_be_bytes())?;
        output.write_all(b"jxlc")?;
    } else {
        output.write_all(&1u32.to_be_bytes())?;
        output.write_all(b"jxlc")?;
        output.write_all(
            &codestream_len
                .checked_add(16)
                .context("JPEG XL box size overflow")?
                .to_be_bytes(),
        )?;
    }
    std::io::copy(codestream, output).context("write JPEG XL codestream")?;
    Ok(())
}

pub(super) struct JpegEncodeRequest<'a> {
    pub(super) rgb: &'a [u8],
    pub(super) output: ExportOutput<'a>,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) quality: u8,
    pub(super) keep_metadata: bool,
    pub(super) metadata: &'a ExportMetadata,
    pub(super) icc_profile: Option<&'a [u8]>,
}

pub(super) fn encode_jpeg_rgb(request: JpegEncodeRequest<'_>) -> Result<()> {
    let JpegEncodeRequest {
        rgb,
        output,
        width: output_width,
        height: output_height,
        quality,
        keep_metadata,
        metadata,
        icc_profile,
    } = request;
    validate_rgb_raster_len(rgb, output_width, output_height)?;
    let width = u16::try_from(output_width).context("JPEG width exceeds baseline limit")?;
    let height = u16::try_from(output_height).context("JPEG height exceeds baseline limit")?;
    let file = open_export_destination(output)
        .with_context(|| format!("create JPEG {}", output.path.display()))?;
    let mut writer = BufWriter::with_capacity(256 * 1024, file);
    let encode_started = Instant::now();
    let mut encoder = jpeg_encoder::Encoder::new(&mut writer, quality.clamp(1, 100));
    encoder.set_sampling_factor(jpeg_encoder::SamplingFactor::F_1_1);
    if let Some(profile) = icc_profile {
        encoder
            .add_icc_profile(profile)
            .context("embed JPEG ICC profile")?;
    }
    if keep_metadata {
        encoder
            .add_exif_metadata(&build_exif_payload(metadata, output_width, output_height))
            .context("embed JPEG EXIF metadata")?;
    }
    encoder
        .encode(rgb, width, height, jpeg_encoder::ColorType::Rgb)
        .with_context(|| format!("encode JPEG {}", output.path.display()))?;
    writer.flush().context("flush JPEG export")?;
    calibraw_core::diagnostics::record(format!(
        "JPEG compression finished in {:.3}s: {}x{} quality={}",
        encode_started.elapsed().as_secs_f64(),
        output_width,
        output_height,
        quality,
    ));
    Ok(())
}
