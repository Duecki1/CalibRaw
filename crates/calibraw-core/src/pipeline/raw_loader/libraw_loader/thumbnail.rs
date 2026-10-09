//! Embedded and processed RAW thumbnails, validated before decoding.

use super::*;

#[cfg(target_os = "android")]
pub(super) const MAX_EMBEDDED_THUMBNAIL_BYTES: usize = 64 * 1024 * 1024;
#[cfg(not(target_os = "android"))]
pub(super) const MAX_EMBEDDED_THUMBNAIL_BYTES: usize = 128 * 1024 * 1024;
#[cfg(target_os = "android")]
const MAX_THUMBNAIL_SOURCE_EDGE: u32 = 8_192;
#[cfg(not(target_os = "android"))]
const MAX_THUMBNAIL_SOURCE_EDGE: u32 = 65_535;
#[cfg(target_os = "android")]
const MAX_THUMBNAIL_DECODE_BYTES: u64 = 64 * 1024 * 1024;
#[cfg(not(target_os = "android"))]
const MAX_THUMBNAIL_DECODE_BYTES: u64 = 256 * 1024 * 1024;
#[cfg(target_os = "android")]
const MAX_ANDROID_THUMBNAIL_FALLBACK_SENSOR_PIXELS: u64 = MAX_SENSOR_PIXELS;

pub(in crate::pipeline::raw_loader) fn load_raw_embedded_thumbnail(
    path: &Path,
    maximum_edge: u32,
) -> Result<RawThumbnail> {
    validate_input_file(path, MAX_RAW_FILE_BYTES, "embedded RAW thumbnail input")?;
    anyhow::ensure!(maximum_edge > 0, "thumbnail edge must be non-zero");
    load_embedded_thumbnail(path, maximum_edge)
}

pub(in crate::pipeline::raw_loader) fn load_raw_thumbnail(
    path: &Path,
    maximum_edge: u32,
) -> Result<RawThumbnail> {
    validate_input_file(path, MAX_RAW_FILE_BYTES, "RAW thumbnail input")?;
    anyhow::ensure!(maximum_edge > 0, "thumbnail edge must be non-zero");

    match load_embedded_thumbnail(path, maximum_edge) {
        Ok(thumbnail) => Ok(thumbnail),
        Err(embedded_error) => load_processed_thumbnail(path, maximum_edge)
            .with_context(|| format!("embedded RAW preview was unavailable ({embedded_error:#})")),
    }
}

fn load_embedded_thumbnail(path: &Path, maximum_edge: u32) -> Result<RawThumbnail> {
    // File reads end with the in-memory copy; decoding runs outside the read gate.
    let (image, orientation) = crate::serialized_reads::run(|| -> Result<_> {
        let ctx = open_libraw(path)?;
        // LibRaw 0.22 reports the preview's format and channel count only once
        // it is unpacked; its declared length is known after open and bounds the
        // allocation that unpacking makes.
        validate_embedded_thumbnail_length(unsafe { (*ctx.raw).thumbnail.tlength })?;
        check_libraw(
            unsafe { ffi::libraw_unpack_thumb(ctx.raw) },
            "unpack RAW thumbnail",
        )?;
        validate_embedded_thumbnail_header(&ctx)?;
        let orientation = embedded_thumbnail_orientation(&ctx);

        let mut error = 0;
        let image = unsafe { ffi::libraw_dcraw_make_mem_thumb(ctx.raw, &mut error) };
        // The copy is malloc'd independently of the context and freed by the
        // context-free `libraw_dcraw_clear_mem`, so it outlives `ctx`.
        let image = ProcessedImage::new(image, error, "make in-memory RAW thumbnail")?;
        Ok((image, orientation))
    })?;
    unsafe { thumbnail_from_processed(&image, maximum_edge, orientation) }
}

fn validate_embedded_thumbnail_length(length: u32) -> Result<usize> {
    let length = usize::try_from(length).context("embedded RAW preview length overflow")?;
    anyhow::ensure!(
        length > 0 && length <= MAX_EMBEDDED_THUMBNAIL_BYTES,
        "embedded RAW preview payload size {length} is outside the safe range"
    );
    Ok(length)
}

fn validate_embedded_thumbnail_header(ctx: &LibRawContext) -> Result<()> {
    let thumbnail = unsafe { &(*ctx.raw).thumbnail };
    validate_embedded_thumbnail_metadata(
        thumbnail.tformat,
        thumbnail.twidth,
        thumbnail.theight,
        thumbnail.tlength,
        thumbnail.tcolors,
    )
}

pub(super) fn validate_embedded_thumbnail_metadata(
    format: ffi::LibRaw_thumbnail_formats,
    width: u16,
    height: u16,
    length: u32,
    colors: i32,
) -> Result<()> {
    let length = validate_embedded_thumbnail_length(length)?;

    match format {
        ffi::LibRaw_thumbnail_formats_LIBRAW_THUMBNAIL_JPEG => {
            if width != 0 || height != 0 {
                anyhow::ensure!(
                    width > 0
                        && height > 0
                        && u32::from(width) <= MAX_THUMBNAIL_SOURCE_EDGE
                        && u32::from(height) <= MAX_THUMBNAIL_SOURCE_EDGE,
                    "embedded JPEG preview {width}x{height} is outside the safe dimension range"
                );
            }
        }
        ffi::LibRaw_thumbnail_formats_LIBRAW_THUMBNAIL_BITMAP => {
            anyhow::ensure!(
                width > 0
                    && height > 0
                    && u32::from(width) <= MAX_THUMBNAIL_SOURCE_EDGE
                    && u32::from(height) <= MAX_THUMBNAIL_SOURCE_EDGE,
                "embedded bitmap preview {width}x{height} is outside the safe dimension range"
            );
            anyhow::ensure!(
                matches!(colors, 1 | 3),
                "unsupported {colors}-channel embedded bitmap preview"
            );
            let expected = usize::from(width)
                .checked_mul(usize::from(height))
                .and_then(|pixels| pixels.checked_mul(colors as usize))
                .context("embedded bitmap preview byte count overflow")?;
            anyhow::ensure!(
                expected <= length
                    && u64::try_from(expected).unwrap_or(u64::MAX) <= MAX_THUMBNAIL_DECODE_BYTES,
                "embedded bitmap preview metadata requires {expected} bytes but declares {length}"
            );
        }
        _ => {
            return Err(anyhow!("unsupported embedded RAW preview format {format}"));
        }
    }
    Ok(())
}

fn load_processed_thumbnail(path: &Path, maximum_edge: u32) -> Result<RawThumbnail> {
    let _render_permit = crate::thumbnail_cache::acquire_rendered_thumbnail_worker();
    // Unpacking reads the sensor payload; processing runs outside the read gate.
    let ctx = crate::serialized_reads::run(|| -> Result<_> {
        let ctx = open_libraw(path)?;
        #[cfg(target_os = "android")]
        {
            let sizes = unsafe { &(*ctx.raw).rawdata.sizes };
            let sensor_pixels = u64::from(sizes.raw_width)
                .checked_mul(u64::from(sizes.raw_height))
                .context("RAW thumbnail fallback sensor dimensions overflow")?;
            anyhow::ensure!(
                sensor_pixels <= MAX_ANDROID_THUMBNAIL_FALLBACK_SENSOR_PIXELS,
                "embedded preview is unavailable and the {sensor_pixels}-pixel sensor exceeds the Android sensor safety limit"
            );
        }
        unsafe {
            (*ctx.raw).params.half_size = 1;
            (*ctx.raw).params.use_camera_wb = 1;
            (*ctx.raw).params.output_color = 1;
            (*ctx.raw).params.output_bps = 8;
            (*ctx.raw).params.user_flip = -1;
        }
        check_libraw(
            unsafe { ffi::libraw_unpack(ctx.raw) },
            "unpack RAW thumbnail fallback",
        )?;
        Ok(ctx)
    })?;
    check_libraw(
        unsafe { ffi::libraw_dcraw_process(ctx.raw) },
        "process RAW thumbnail fallback",
    )?;

    let mut error = 0;
    let image = unsafe { ffi::libraw_dcraw_make_mem_image(ctx.raw, &mut error) };
    let image = ProcessedImage::new(image, error, "make fallback RAW thumbnail")?;
    unsafe { thumbnail_from_processed(&image, maximum_edge, 0) }
}

fn embedded_thumbnail_orientation(ctx: &LibRawContext) -> i32 {
    unsafe {
        let raw = &*ctx.raw;
        let selected = &raw.thumbnail;
        let thumbnail_list = &raw.thumbs_list;
        let count = usize::try_from(thumbnail_list.thumbcount)
            .unwrap_or(0)
            .min(thumbnail_list.thumblist.len());
        matching_thumbnail_orientation(
            (selected.twidth, selected.theight, selected.tlength),
            thumbnail_list.thumblist[..count].iter().map(|thumbnail| {
                (
                    thumbnail.twidth,
                    thumbnail.theight,
                    thumbnail.tlength,
                    thumbnail.tflip,
                )
            }),
        )
        .unwrap_or(raw.rawdata.sizes.flip)
    }
}

pub(super) fn matching_thumbnail_orientation(
    selected: (u16, u16, u32),
    candidates: impl IntoIterator<Item = (u16, u16, u32, u16)>,
) -> Option<i32> {
    candidates.into_iter().find_map(|candidate| {
        let (width, height, length, flip) = candidate;
        (flip != u16::MAX && (width, height, length) == selected).then_some(i32::from(flip))
    })
}

struct ProcessedImage(pub(super) *mut ffi::libraw_processed_image_t);

impl ProcessedImage {
    pub(super) fn new(
        image: *mut ffi::libraw_processed_image_t,
        error: i32,
        action: &str,
    ) -> Result<Self> {
        if image.is_null() {
            check_libraw(error, action)?;
            return Err(anyhow!("LibRaw failed to {action}: no image was returned"));
        }
        Ok(Self(image))
    }
}

impl Drop for ProcessedImage {
    fn drop(&mut self) {
        unsafe { ffi::libraw_dcraw_clear_mem(self.0) };
    }
}

unsafe fn thumbnail_from_processed(
    image: &ProcessedImage,
    maximum_edge: u32,
    orientation: i32,
) -> Result<RawThumbnail> {
    let processed = &*image.0;
    let data_size = processed.data_size as usize;
    anyhow::ensure!(
        data_size > 0 && data_size <= MAX_EMBEDDED_THUMBNAIL_BYTES,
        "LibRaw thumbnail payload size {data_size} is outside the safe range"
    );
    let data = std::slice::from_raw_parts(processed.data.as_ptr(), data_size);

    let decoded = match processed.type_ {
        ffi::LibRaw_image_formats_LIBRAW_IMAGE_JPEG => {
            let dimensions_reader =
                image::ImageReader::with_format(Cursor::new(data), image::ImageFormat::Jpeg);
            let (width, height) = dimensions_reader
                .into_dimensions()
                .context("inspect embedded JPEG thumbnail")?;
            anyhow::ensure!(
                width > 0
                    && height > 0
                    && width <= MAX_THUMBNAIL_SOURCE_EDGE
                    && height <= MAX_THUMBNAIL_SOURCE_EDGE,
                "embedded JPEG thumbnail {width}x{height} is outside the safe dimension range"
            );
            let decoded_bytes = u64::from(width)
                .checked_mul(u64::from(height))
                .and_then(|pixels| pixels.checked_mul(3))
                .context("embedded JPEG thumbnail byte count overflow")?;
            anyhow::ensure!(
                decoded_bytes <= MAX_THUMBNAIL_DECODE_BYTES,
                "embedded JPEG thumbnail requires at least {decoded_bytes} decoded bytes, exceeding the safe allocation limit"
            );
            let mut reader =
                image::ImageReader::with_format(Cursor::new(data), image::ImageFormat::Jpeg);
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(MAX_THUMBNAIL_SOURCE_EDGE);
            limits.max_image_height = Some(MAX_THUMBNAIL_SOURCE_EDGE);
            limits.max_alloc = Some(MAX_THUMBNAIL_DECODE_BYTES);
            reader.limits(limits);
            reader.decode().context("decode embedded JPEG thumbnail")?
        }
        ffi::LibRaw_image_formats_LIBRAW_IMAGE_BITMAP => {
            anyhow::ensure!(
                processed.bits == 8,
                "unsupported {}-bit RAW thumbnail",
                processed.bits
            );
            let width = u32::from(processed.width);
            let height = u32::from(processed.height);
            let colors = usize::from(processed.colors);
            anyhow::ensure!(
                width > 0 && height > 0,
                "LibRaw returned an empty bitmap thumbnail"
            );
            anyhow::ensure!(
                width <= MAX_THUMBNAIL_SOURCE_EDGE && height <= MAX_THUMBNAIL_SOURCE_EDGE,
                "LibRaw bitmap thumbnail {width}x{height} exceeds the safe edge limit"
            );
            anyhow::ensure!(
                matches!(colors, 1 | 3),
                "unsupported {colors}-channel RAW thumbnail"
            );
            let pixels = usize::try_from(width)
                .ok()
                .and_then(|width| {
                    usize::try_from(height)
                        .ok()
                        .and_then(|height| width.checked_mul(height))
                })
                .context("RAW thumbnail dimensions overflow")?;
            let expected = pixels
                .checked_mul(colors)
                .context("RAW thumbnail byte count overflow")?;
            anyhow::ensure!(
                u64::try_from(expected).unwrap_or(u64::MAX) <= MAX_THUMBNAIL_DECODE_BYTES,
                "LibRaw bitmap thumbnail requires {expected} decoded bytes, exceeding the safe allocation limit"
            );
            anyhow::ensure!(data.len() >= expected, "truncated bitmap RAW thumbnail");
            if colors == 3 {
                let buffer = image::RgbImage::from_raw(width, height, data[..expected].to_vec())
                    .context("invalid RGB RAW thumbnail buffer")?;
                image::DynamicImage::ImageRgb8(buffer)
            } else {
                let buffer = image::GrayImage::from_raw(width, height, data[..expected].to_vec())
                    .context("invalid grayscale RAW thumbnail buffer")?;
                image::DynamicImage::ImageLuma8(buffer)
            }
        }
        format => return Err(anyhow!("unsupported LibRaw thumbnail format {format}")),
    };

    let mut oriented = crate::thumbnail_cache::downscale_to_fit(decoded, maximum_edge);
    let transform = match orientation {
        0 => image::metadata::Orientation::NoTransforms,
        1 => image::metadata::Orientation::FlipHorizontal,
        2 => image::metadata::Orientation::FlipVertical,
        3 => image::metadata::Orientation::Rotate180,
        4 => image::metadata::Orientation::Rotate90FlipH,
        5 => image::metadata::Orientation::Rotate270,
        6 => image::metadata::Orientation::Rotate90,
        7 => image::metadata::Orientation::Rotate270FlipH,
        _ => image::metadata::Orientation::NoTransforms,
    };
    oriented.apply_orientation(transform);
    let thumbnail = oriented.to_rgba8();
    let (width, height) = thumbnail.dimensions();
    Ok(RawThumbnail {
        width,
        height,
        rgba: thumbnail.into_raw(),
    })
}

pub(super) unsafe fn validate_opened_thumbnail_geometry(ctx: &LibRawContext) -> Result<()> {
    let raw = &*ctx.raw;
    let sizes = &raw.rawdata.sizes;
    let active_width = u32::from(sizes.width);
    let active_height = u32::from(sizes.height);
    if active_width != 0 || active_height != 0 {
        anyhow::ensure!(
            active_width > 0
                && active_height > 0
                && active_width <= MAX_SENSOR_EDGE
                && active_height <= MAX_SENSOR_EDGE,
            "LibRaw header reports active dimensions {active_width}x{active_height} outside the thumbnail safety limit"
        );
        active_width
            .checked_mul(active_height)
            .context("RAW thumbnail active pixel count overflow")?;
    }

    let sensor_width = u32::from(sizes.raw_width);
    let sensor_height = u32::from(sizes.raw_height);
    if sensor_width != 0 || sensor_height != 0 {
        anyhow::ensure!(
            sensor_width > 0
                && sensor_height > 0
                && sensor_width <= MAX_SENSOR_EDGE
                && sensor_height <= MAX_SENSOR_EDGE,
            "LibRaw header reports sensor dimensions {sensor_width}x{sensor_height} outside the thumbnail safety limit"
        );
        sensor_width
            .checked_mul(sensor_height)
            .context("RAW thumbnail header pixel count overflow")?;
    }
    Ok(())
}
