use crate::file_ops::write_bytes_atomically;
use crate::pipeline::RawThumbnail;
use image::codecs::jpeg::JpegEncoder;
use image::ImageFormat;
use rayon::iter::{IndexedParallelIterator, ParallelIterator};
use rayon::slice::ParallelSliceMut;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::UNIX_EPOCH;

/// Generation of the desktop thumbnail stamp for sensor RAWs; bump it to drop every cached
/// RAW thumbnail.
const RAW_STAMP_GENERATION: u32 = 1;
/// Generation for TIFFs, which render through the float raster path. Generation 3 drops
/// thumbnails whose float downscale lifted black.
const TIFF_STAMP_GENERATION: u32 = 3;
const RAW_THUMBNAIL_SUFFIX: &str = ".calibraw-raw-thumb.jpg";
const RAW_THUMBNAIL_FINGERPRINT_SUFFIX: &str = ".calibraw-raw-thumb.fingerprint";
pub const DESKTOP_THUMBNAIL_CACHE_DIR: &str = "library-thumbnails";
pub const THUMBNAIL_JPEG_QUALITY: u8 = 88;
const MAX_CACHED_THUMBNAIL_EDGE: u32 = 8192;
const MAX_CACHED_THUMBNAIL_BYTES: u64 = 128 * 1024 * 1024;
const MAX_CACHED_THUMBNAIL_DECODE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_CACHED_THUMBNAIL_PIXELS: u64 = MAX_CACHED_THUMBNAIL_DECODE_BYTES / 4;

struct RenderedThumbnailLimiter {
    state: Mutex<RenderedThumbnailLimiterState>,
    available: Condvar,
}

struct RenderedThumbnailLimiterState {
    limit: usize,
    active: usize,
}

pub struct RenderedThumbnailPermit {
    limiter: &'static RenderedThumbnailLimiter,
}

static RENDERED_THUMBNAIL_LIMITER: OnceLock<RenderedThumbnailLimiter> = OnceLock::new();

fn rendered_thumbnail_limiter() -> &'static RenderedThumbnailLimiter {
    RENDERED_THUMBNAIL_LIMITER.get_or_init(|| RenderedThumbnailLimiter {
        state: Mutex::new(RenderedThumbnailLimiterState {
            limit: 1,
            active: 0,
        }),
        available: Condvar::new(),
    })
}

pub fn set_rendered_thumbnail_worker_limit(limit: usize) {
    let limiter = rendered_thumbnail_limiter();
    let mut state = limiter
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    state.limit = limit.max(1);
    limiter.available.notify_all();
}

pub fn acquire_rendered_thumbnail_worker() -> RenderedThumbnailPermit {
    let limiter = rendered_thumbnail_limiter();
    let mut state = limiter
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    while state.active >= state.limit {
        state = limiter
            .available
            .wait(state)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
    }
    state.active += 1;
    RenderedThumbnailPermit { limiter }
}

impl Drop for RenderedThumbnailPermit {
    fn drop(&mut self) {
        let mut state = self
            .limiter
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.active = state.active.saturating_sub(1);
        self.limiter.available.notify_one();
    }
}

pub fn downscale_to_fit(image: image::DynamicImage, maximum_edge: u32) -> image::DynamicImage {
    if image.width() <= maximum_edge && image.height() <= maximum_edge {
        return image;
    }
    // `imageops::thumbnail` rounds each block sum by adding half the block's
    // pixel count, which lifts every float sample by 0.5, and `resize` clamps
    // floats to [0, 1]. Float rasters are scene-linear and may exceed 1, so
    // they take a plain area average instead.
    match image {
        image::DynamicImage::ImageRgb32F(buffer) => {
            image::DynamicImage::ImageRgb32F(area_average_f32(&buffer, maximum_edge))
        }
        image::DynamicImage::ImageRgba32F(buffer) => {
            image::DynamicImage::ImageRgba32F(area_average_f32(&buffer, maximum_edge))
        }
        image => image.thumbnail(maximum_edge, maximum_edge),
    }
}

/// Box-filters `source` down to the size `DynamicImage::thumbnail` picks for
/// a `maximum_edge` square. Each output pixel averages the source pixels whose
/// index range maps onto it, so every source pixel contributes exactly once.
/// Callers only downscale, which keeps every span at least one pixel wide.
fn area_average_f32<P>(
    source: &image::ImageBuffer<P, Vec<f32>>,
    maximum_edge: u32,
) -> image::ImageBuffer<P, Vec<f32>>
where
    P: image::Pixel<Subpixel = f32>,
{
    let (width, height) = source.dimensions();
    let ratio = (f64::from(maximum_edge) / f64::from(width))
        .min(f64::from(maximum_edge) / f64::from(height));
    let output_width = ((f64::from(width) * ratio).round() as u32).clamp(1, width);
    let output_height = ((f64::from(height) * ratio).round() as u32).clamp(1, height);
    let spans = |source_len: u32, output_len: u32| -> Vec<(usize, usize)> {
        (0..u64::from(output_len))
            .map(|index| {
                let start = index * u64::from(source_len) / u64::from(output_len);
                let end = (index + 1) * u64::from(source_len) / u64::from(output_len);
                (start as usize, end as usize)
            })
            .collect()
    };
    let column_spans = spans(width, output_width);
    let row_spans = spans(height, output_height);
    let channels = usize::from(P::CHANNEL_COUNT);
    let source_stride = width as usize * channels;
    let samples = source.as_raw();

    let mut output = vec![0.0f32; output_width as usize * output_height as usize * channels];
    output
        .par_chunks_exact_mut(output_width as usize * channels)
        .zip(&row_spans)
        .for_each(|(output_row, &(top, bottom))| {
            let mut column_sums = vec![0.0f64; source_stride];
            for row in
                samples[top * source_stride..bottom * source_stride].chunks_exact(source_stride)
            {
                for (sum, &value) in column_sums.iter_mut().zip(row) {
                    *sum += f64::from(value);
                }
            }
            for (output_pixel, &(left, right)) in
                output_row.chunks_exact_mut(channels).zip(&column_spans)
            {
                let count = ((bottom - top) * (right - left)) as f64;
                for (channel, value) in output_pixel.iter_mut().enumerate() {
                    let sum: f64 = (left..right)
                        .map(|column| column_sums[column * channels + channel])
                        .sum();
                    *value = (sum / count) as f32;
                }
            }
        });
    image::ImageBuffer::from_raw(output_width, output_height, output)
        .expect("area average output length matches its dimensions")
}

fn discard_invalid_cache_file(path: &Path) {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => log::warn!(
            "could not remove invalid thumbnail cache {}: {error}",
            path.display()
        ),
    }
}

fn accepted_thumbnail_layout(width: u32, height: u32) -> Result<(u64, u64, u64), String> {
    if width == 0 || height == 0 {
        return Err("thumbnail dimensions must be non-zero".to_owned());
    }
    if width > MAX_CACHED_THUMBNAIL_EDGE || height > MAX_CACHED_THUMBNAIL_EDGE {
        return Err(format!(
            "thumbnail dimensions {width}x{height} are outside the cache safety limit"
        ));
    }
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or_else(|| "thumbnail pixel count overflow".to_owned())?;
    if pixels > MAX_CACHED_THUMBNAIL_PIXELS {
        return Err(format!(
            "thumbnail {width}x{height} contains {pixels} pixels, above the {MAX_CACHED_THUMBNAIL_PIXELS} pixel cache limit"
        ));
    }
    let row_bytes = u64::from(width)
        .checked_mul(4)
        .ok_or_else(|| "thumbnail RGBA row byte count overflow".to_owned())?;
    let decoded_bytes = row_bytes
        .checked_mul(u64::from(height))
        .ok_or_else(|| "thumbnail decoded byte count overflow".to_owned())?;
    if decoded_bytes > MAX_CACHED_THUMBNAIL_DECODE_BYTES {
        return Err(format!(
            "thumbnail {width}x{height} requires {decoded_bytes} decoded bytes, above the {} byte cache limit",
            MAX_CACHED_THUMBNAIL_DECODE_BYTES
        ));
    }
    Ok((pixels, row_bytes, decoded_bytes))
}

pub fn load_jpeg(path: &Path, maximum_edge: u32) -> Result<Option<RawThumbnail>, String> {
    if maximum_edge == 0 {
        return Err("thumbnail edge must be non-zero".to_owned());
    }
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "could not inspect thumbnail cache {}: {error}",
                path.display()
            ))
        }
    };
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_CACHED_THUMBNAIL_BYTES {
        discard_invalid_cache_file(path);
        return Ok(None);
    }

    let dimensions = image::ImageReader::with_format(
        match fs::File::open(path) {
            Ok(file) => std::io::BufReader::new(file),
            Err(error) => {
                return Err(format!(
                    "could not open thumbnail cache {}: {error}",
                    path.display()
                ))
            }
        },
        ImageFormat::Jpeg,
    )
    .into_dimensions();
    let (source_width, source_height) = match dimensions {
        Ok(dimensions) => dimensions,
        Err(_) => {
            discard_invalid_cache_file(path);
            return Ok(None);
        }
    };
    if accepted_thumbnail_layout(source_width, source_height).is_err() {
        discard_invalid_cache_file(path);
        return Ok(None);
    }

    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) => {
            return Err(format!(
                "could not reopen thumbnail cache {}: {error}",
                path.display()
            ))
        }
    };
    let mut reader =
        image::ImageReader::with_format(std::io::BufReader::new(file), ImageFormat::Jpeg);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_CACHED_THUMBNAIL_EDGE);
    limits.max_image_height = Some(MAX_CACHED_THUMBNAIL_EDGE);
    limits.max_alloc = Some(MAX_CACHED_THUMBNAIL_DECODE_BYTES);
    reader.limits(limits);
    let image = match reader.decode() {
        Ok(image) => image,
        Err(_) => {
            discard_invalid_cache_file(path);
            return Ok(None);
        }
    };
    let image = downscale_to_fit(image, maximum_edge).to_rgba8();
    let (width, height) = image.dimensions();
    let (_, _, decoded_bytes) = match accepted_thumbnail_layout(width, height) {
        Ok(layout) => layout,
        Err(_) => {
            discard_invalid_cache_file(path);
            return Ok(None);
        }
    };
    let expected = match usize::try_from(decoded_bytes) {
        Ok(expected) => expected,
        Err(_) => {
            discard_invalid_cache_file(path);
            return Ok(None);
        }
    };
    let rgba = image.into_raw();
    if rgba.len() != expected {
        discard_invalid_cache_file(path);
        return Ok(None);
    }
    Ok(Some(RawThumbnail {
        width,
        height,
        rgba,
    }))
}

pub fn save_jpeg(path: &Path, thumbnail: &RawThumbnail) -> Result<(), String> {
    let (_, _, decoded_bytes) = accepted_thumbnail_layout(thumbnail.width, thumbnail.height)?;
    let expected =
        usize::try_from(decoded_bytes).map_err(|_| "thumbnail byte count overflow".to_owned())?;
    if thumbnail.rgba.len() != expected {
        return Err("thumbnail has an invalid RGBA byte count".to_owned());
    }

    let image =
        image::RgbaImage::from_raw(thumbnail.width, thumbnail.height, thumbnail.rgba.clone())
            .ok_or_else(|| "thumbnail has an invalid RGBA layout".to_owned())?;
    let image = image::DynamicImage::ImageRgba8(image).to_rgb8();
    let image = image::DynamicImage::ImageRgb8(image);
    let mut encoded = Vec::new();
    JpegEncoder::new_with_quality(&mut encoded, THUMBNAIL_JPEG_QUALITY)
        .encode_image(&image)
        .map_err(|error| format!("could not encode thumbnail cache: {error}"))?;
    if u64::try_from(encoded.len()).unwrap_or(u64::MAX) > MAX_CACHED_THUMBNAIL_BYTES {
        return Err("encoded thumbnail exceeds the cache size limit".to_owned());
    }
    write_bytes_atomically(path, &encoded).map_err(|error| {
        format!(
            "could not write thumbnail cache {}: {error}",
            path.display()
        )
    })
}

pub fn fnv1a64(bytes: &[u8]) -> u64 {
    fnv1a64_bytes(bytes.iter().copied())
}

fn fnv1a64_bytes(bytes: impl IntoIterator<Item = u8>) -> u64 {
    bytes
        .into_iter()
        .fold(crate::fnv::OFFSET_BASIS, crate::fnv::step)
}

pub fn load_desktop_raw_thumbnail(
    raw_path: &Path,
    maximum_edge: u32,
) -> Result<Option<RawThumbnail>, String> {
    let cache_path = desktop_raw_thumbnail_path(raw_path);
    let fingerprint_path = desktop_raw_thumbnail_fingerprint_path(raw_path);
    if !cache_path.is_file() || !fingerprint_path.is_file() {
        return Ok(None);
    }
    let expected = desktop_raw_stamp(raw_path)?;
    let cached = fs::read_to_string(&fingerprint_path).map_err(|error| {
        format!(
            "could not read RAW thumbnail fingerprint {}: {error}",
            fingerprint_path.display()
        )
    })?;
    if cached.trim() != expected {
        let _ = fs::remove_file(&cache_path);
        let _ = fs::remove_file(&fingerprint_path);
        return Ok(None);
    }
    load_jpeg(&cache_path, maximum_edge)
}

pub fn save_desktop_raw_thumbnail(raw_path: &Path, thumbnail: &RawThumbnail) -> Result<(), String> {
    let expected = desktop_raw_stamp(raw_path)?;
    let cache_path = desktop_raw_thumbnail_path(raw_path);
    let fingerprint_path = desktop_raw_thumbnail_fingerprint_path(raw_path);
    save_jpeg(&cache_path, thumbnail)?;
    write_bytes_atomically(&fingerprint_path, format!("{expected}\n").as_bytes()).map_err(
        |error| {
            format!(
                "could not write RAW thumbnail fingerprint {}: {error}",
                fingerprint_path.display()
            )
        },
    )?;
    if desktop_raw_stamp(raw_path)? != expected {
        let _ = fs::remove_file(&cache_path);
        let _ = fs::remove_file(&fingerprint_path);
        return Err("RAW changed while its thumbnail was being cached".to_owned());
    }
    Ok(())
}

fn desktop_raw_thumbnail_path(raw_path: &Path) -> PathBuf {
    desktop_cache_path_for_raw(raw_path, RAW_THUMBNAIL_SUFFIX)
}

fn desktop_raw_thumbnail_fingerprint_path(raw_path: &Path) -> PathBuf {
    desktop_cache_path_for_raw(raw_path, RAW_THUMBNAIL_FINGERPRINT_SUFFIX)
}

pub fn desktop_app_cache_root() -> PathBuf {
    desktop_platform_cache_root().join("calibraw")
}

pub fn desktop_thumbnail_cache_root() -> PathBuf {
    desktop_app_cache_root().join(DESKTOP_THUMBNAIL_CACHE_DIR)
}

pub fn clear_desktop_thumbnail_cache() -> Result<(), String> {
    let root = desktop_thumbnail_cache_root();
    match fs::remove_dir_all(&root) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "could not clear thumbnail cache {}: {error}",
            root.display()
        )),
    }
}

pub fn desktop_thumbnail_cache_size_bytes() -> Result<u64, String> {
    directory_size_bytes(&desktop_thumbnail_cache_root())
}

fn directory_size_bytes(root: &Path) -> Result<u64, String> {
    let mut directories = vec![root.to_path_buf()];
    let mut total = 0_u64;
    while let Some(directory) = directories.pop() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "could not inspect thumbnail cache {}: {error}",
                    directory.display()
                ))
            }
        };
        for entry in entries {
            let entry = entry.map_err(|error| {
                format!(
                    "could not read thumbnail cache entry in {}: {error}",
                    directory.display()
                )
            })?;
            let file_type = entry.file_type().map_err(|error| {
                format!(
                    "could not inspect thumbnail cache entry {}: {error}",
                    entry.path().display()
                )
            })?;
            if file_type.is_dir() {
                directories.push(entry.path());
            } else if file_type.is_file() {
                let bytes = entry
                    .metadata()
                    .map_err(|error| {
                        format!(
                            "could not inspect thumbnail cache entry {}: {error}",
                            entry.path().display()
                        )
                    })?
                    .len();
                total = total.saturating_add(bytes);
            }
        }
    }
    Ok(total)
}

#[cfg(windows)]
fn desktop_platform_cache_root() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .map(PathBuf::from)
                .map(|home| home.join("AppData").join("Local"))
        })
        .unwrap_or_else(std::env::temp_dir)
}

#[cfg(target_os = "macos")]
fn desktop_platform_cache_root() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join("Library").join("Caches"))
        .unwrap_or_else(std::env::temp_dir)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn desktop_platform_cache_root() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
}

#[cfg(not(any(unix, windows)))]
fn desktop_platform_cache_root() -> PathBuf {
    std::env::temp_dir()
}

pub fn desktop_cache_path_for_raw(raw_path: &Path, suffix: &str) -> PathBuf {
    let identity = fs::canonicalize(raw_path).unwrap_or_else(|_| {
        if raw_path.is_absolute() {
            raw_path.to_path_buf()
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(raw_path)
        }
    });
    let key = desktop_path_fingerprint(&identity);
    desktop_thumbnail_cache_root()
        .join(format!("{:02x}", key >> 56))
        .join(format!("{key:016x}{suffix}"))
}

#[cfg(unix)]
fn desktop_path_fingerprint(path: &Path) -> u64 {
    use std::os::unix::ffi::OsStrExt;
    fnv1a64(path.as_os_str().as_bytes())
}

#[cfg(windows)]
fn desktop_path_fingerprint(path: &Path) -> u64 {
    use std::os::windows::ffi::OsStrExt;

    fnv1a64_bytes(
        path.as_os_str()
            .encode_wide()
            .flat_map(|word| word.to_le_bytes()),
    )
}

#[cfg(not(any(unix, windows)))]
fn desktop_path_fingerprint(path: &Path) -> u64 {
    fnv1a64(path.to_string_lossy().as_bytes())
}

fn desktop_raw_stamp(raw_path: &Path) -> Result<String, String> {
    let metadata = fs::metadata(raw_path)
        .map_err(|error| format!("could not inspect RAW {}: {error}", raw_path.display()))?;
    if !metadata.is_file() {
        return Err(format!("RAW {} is not a regular file", raw_path.display()));
    }
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let generation = if crate::pipeline::is_tiff_path(raw_path) {
        TIFF_STAMP_GENERATION
    } else {
        RAW_STAMP_GENERATION
    };
    Ok(format!("v{generation}:{}:{modified}", metadata.len()))
}
