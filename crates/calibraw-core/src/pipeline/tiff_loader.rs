use super::raw_loader::{validate_raw_dimensions, LoadedRaw, RawThumbnail};
use anyhow::{anyhow, Context, Result};
use image::{ColorType, ImageFormat};
use rayon::prelude::*;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use super::display_raster::{
    encoded_rgb_to_scene_linear_rec2020, ensure_finite, scene_linear_thumbnail, EncodedColorSpace,
};

#[cfg(target_os = "android")]
const MAX_TIFF_DECODE_BYTES: u64 = 768 * 1024 * 1024;
// The image crate holds the decoded image twice while decoding (its output and
// the TIFF decoder's buffer) plus one compressed strip or tile. Budget for the
// largest accepted image at 16 bytes per pixel (32-bit float RGBA) and strips
// of up to 256 MiB.
#[cfg(not(target_os = "android"))]
const MAX_TIFF_DECODE_BYTES: u64 = super::raw_loader::MAX_RAW_PIXELS * 16 * 2 + 256 * 1024 * 1024;
const MAX_TIFF_IFDS: usize = 32;
const MAX_TIFF_IFD_ENTRIES: u64 = 4096;
const MAX_TIFF_SUBIFDS: usize = 64;
const MIN_TIFF_ICC_BYTES: u64 = 132;

// TIFF and DNG tag codes read by the container probes.
const TAG_BITS_PER_SAMPLE: u16 = 258;
const TAG_COMPRESSION: u16 = 259;
const TAG_PHOTOMETRIC: u16 = 262;
const TAG_SUB_IFDS: u16 = 330;
/// TIFF/EP CFARepeatPatternDim and CFAPattern: only sensor data carries them.
const TAG_CFA_REPEAT_PATTERN_DIM: u16 = 33421;
const TAG_CFA_PATTERN: u16 = 33422;
const TAG_ICC_PROFILE: u16 = 34675;
/// Four-byte DNG version tuple; present in every DNG.
const TAG_DNG_VERSION: u16 = 50706;
/// Photometric interpretations of sensor data: colour filter array and linear raw.
pub(super) const PHOTOMETRIC_CFA: u64 = 32803;
pub(super) const PHOTOMETRIC_LINEAR_RAW: u64 = 34892;
const MAX_TIFF_ICC_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TiffContainerKind {
    Sensor,
    Raster,
}

#[derive(Clone, Copy)]
enum ByteOrder {
    Little,
    Big,
}

impl ByteOrder {
    fn u16(self, bytes: [u8; 2]) -> u16 {
        match self {
            Self::Little => u16::from_le_bytes(bytes),
            Self::Big => u16::from_be_bytes(bytes),
        }
    }

    fn u32(self, bytes: [u8; 4]) -> u32 {
        match self {
            Self::Little => u32::from_le_bytes(bytes),
            Self::Big => u32::from_be_bytes(bytes),
        }
    }

    fn u64(self, bytes: [u8; 8]) -> u64 {
        match self {
            Self::Little => u64::from_le_bytes(bytes),
            Self::Big => u64::from_be_bytes(bytes),
        }
    }
}

struct TiffHeader {
    order: ByteOrder,
    big_tiff: bool,
    first_ifd: u64,
    file_len: u64,
}

fn open_tiff(path: &Path) -> Result<(File, TiffHeader)> {
    let mut file = File::open(path).with_context(|| format!("open TIFF {}", path.display()))?;
    let file_len = file
        .metadata()
        .with_context(|| format!("inspect TIFF {}", path.display()))?
        .len();
    let mut bytes = [0u8; 16];
    file.read_exact(&mut bytes[..8])
        .with_context(|| format!("read TIFF header {}", path.display()))?;
    let order = match &bytes[..2] {
        b"II" => ByteOrder::Little,
        b"MM" => ByteOrder::Big,
        _ => return Err(anyhow!("{} is not a TIFF container", path.display())),
    };
    let (big_tiff, first_ifd) = match order.u16([bytes[2], bytes[3]]) {
        42 => (false, u64::from(order.u32(bytes[4..8].try_into()?))),
        43 => {
            file.read_exact(&mut bytes[8..16])
                .with_context(|| format!("read BigTIFF header {}", path.display()))?;
            anyhow::ensure!(
                order.u16([bytes[4], bytes[5]]) == 8 && order.u16([bytes[6], bytes[7]]) == 0,
                "unsupported BigTIFF header in {}",
                path.display()
            );
            (true, order.u64(bytes[8..16].try_into()?))
        }
        _ => return Err(anyhow!("{} has an invalid TIFF magic", path.display())),
    };
    Ok((
        file,
        TiffHeader {
            order,
            big_tiff,
            first_ifd,
            file_len,
        },
    ))
}

/// Compressions only camera RAW containers use: old-style JPEG (Canon CR2, Kodak),
/// Sony ARW, Samsung and Epson packing, Nikon NEF, Kodak and Pentax.
const SENSOR_COMPRESSIONS: &[u64] = &[6, 32767, 32769, 32770, 34713, 65000, 65535];

/// Whether `path` is decoded as a raster TIFF rather than by the sensor decoders.
///
/// A `.tif` or `.tiff` path is one unless it carries sensor data. Android opens
/// documents as `/proc/self/fd/<n>`, which loses the name, so an extensionless path is
/// judged by its content. Many camera RAW formats are TIFF containers as well, so such
/// a file must also lack every mark of one: CFA data, Canon's CR2 signature, a
/// compression only cameras use or a sensor bit depth such as 12 or 14 bits.
pub(super) fn is_raster_tiff(path: &Path) -> Result<bool> {
    if is_tiff_path(path) {
        return Ok(inspect_tiff_container(path)? == TiffContainerKind::Raster);
    }
    if path.extension().is_some() {
        return Ok(false);
    }
    // Anything that does not read as a raster TIFF stays with the sensor decoders.
    Ok(!has_camera_raw_marks(path).unwrap_or(true)
        && matches!(inspect_tiff_container(path), Ok(TiffContainerKind::Raster)))
}

fn has_camera_raw_marks(path: &Path) -> Result<bool> {
    let mut header = [0u8; 10];
    File::open(path)?.read_exact(&mut header)?;
    if &header[8..10] == b"CR" {
        return Ok(true);
    }
    let marked = walk_tiff_fields(path, |file, tag, field| {
        let sensor = match tag {
            TAG_BITS_PER_SAMPLE => integer_values(file, field, 4)?
                .into_iter()
                .any(|bits| !matches!(bits, 1 | 8 | 16 | 32 | 64)),
            TAG_COMPRESSION => scalar_u64(file, field)?
                .is_some_and(|compression| SENSOR_COMPRESSIONS.contains(&compression)),
            _ => false,
        };
        Ok(sensor.then_some(()))
    })?;
    Ok(marked.is_some())
}

pub(crate) fn is_tiff_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("tif") || extension.eq_ignore_ascii_case("tiff")
        })
}

pub(super) fn has_dng_version(path: &Path) -> Result<bool> {
    Ok(walk_tiff_fields(path, |_file, tag, field| {
        // DNGVersion is a four-byte version tuple. Reuse the bounded TIFF
        // walker so extensionless Android descriptor paths can be identified.
        Ok((tag == TAG_DNG_VERSION && field.field_type == 1 && field.count == 4).then_some(true))
    })?
    .unwrap_or(false))
}

fn inspect_tiff_container(path: &Path) -> Result<TiffContainerKind> {
    Ok(walk_tiff_fields(path, |file, tag, field| {
        if matches!(tag, TAG_CFA_REPEAT_PATTERN_DIM | TAG_CFA_PATTERN) {
            return Ok(Some(TiffContainerKind::Sensor));
        }
        if tag == TAG_PHOTOMETRIC && field.count == 1 {
            if let Some(value) = scalar_u64(file, field)? {
                if matches!(value, PHOTOMETRIC_CFA | PHOTOMETRIC_LINEAR_RAW) {
                    return Ok(Some(TiffContainerKind::Sensor));
                }
            }
        }
        Ok(None)
    })?
    .unwrap_or(TiffContainerKind::Raster))
}

fn walk_tiff_fields<T>(
    path: &Path,
    mut visit: impl FnMut(&mut File, u16, TiffFieldRef) -> Result<Option<T>>,
) -> Result<Option<T>> {
    let (mut file, header) = open_tiff(path)?;
    let TiffHeader {
        order,
        big_tiff,
        first_ifd,
        file_len,
    } = header;
    let mut pending = vec![first_ifd];
    let mut visited = std::collections::HashSet::new();
    let mut inspected = 0usize;
    while let Some(ifd_offset) = pending.pop() {
        if ifd_offset == 0 || !visited.insert(ifd_offset) {
            continue;
        }
        anyhow::ensure!(inspected < MAX_TIFF_IFDS, "TIFF contains too many IFDs");
        inspected += 1;
        anyhow::ensure!(ifd_offset < file_len, "TIFF IFD offset is outside the file");
        file.seek(SeekFrom::Start(ifd_offset))?;
        let entry_count = if big_tiff {
            read_u64(&mut file, order)?
        } else {
            u64::from(read_u16(&mut file, order)?)
        };
        anyhow::ensure!(
            entry_count <= MAX_TIFF_IFD_ENTRIES,
            "TIFF IFD contains too many entries"
        );
        let entry_size = if big_tiff { 20u64 } else { 12u64 };
        let count_size = if big_tiff { 8u64 } else { 2u64 };
        let entries_start = ifd_offset
            .checked_add(count_size)
            .context("TIFF IFD offset overflow")?;
        let entries_bytes = entry_count
            .checked_mul(entry_size)
            .context("TIFF IFD size overflow")?;
        let next_pos = entries_start
            .checked_add(entries_bytes)
            .context("TIFF IFD size overflow")?;
        let next_width = if big_tiff { 8u64 } else { 4u64 };
        anyhow::ensure!(
            next_pos
                .checked_add(next_width)
                .is_some_and(|end| end <= file_len),
            "TIFF IFD extends outside the file"
        );
        for entry_index in 0..entry_count {
            let entry_offset = entries_start + entry_index * entry_size;
            file.seek(SeekFrom::Start(entry_offset))?;
            let tag = read_u16(&mut file, order)?;
            let field_type = read_u16(&mut file, order)?;
            let count = if big_tiff {
                read_u64(&mut file, order)?
            } else {
                u64::from(read_u32(&mut file, order)?)
            };
            let value_field_offset = file.stream_position()?;
            let value_or_offset = if big_tiff {
                read_u64(&mut file, order)?
            } else {
                u64::from(read_u32(&mut file, order)?)
            };
            let field = TiffFieldRef {
                order,
                big_tiff,
                field_type,
                count,
                value_field_offset,
                value_or_offset,
                file_len,
            };
            if let Some(value) = visit(&mut file, tag, field)? {
                return Ok(Some(value));
            }
            if tag == TAG_SUB_IFDS && pending.len() < MAX_TIFF_SUBIFDS {
                let offsets = integer_values(&mut file, field, MAX_TIFF_SUBIFDS - pending.len())?;
                pending.extend(offsets.into_iter().filter(|offset| *offset != 0));
            }
        }
        file.seek(SeekFrom::Start(next_pos))?;
        let next_ifd = if big_tiff {
            read_u64(&mut file, order)?
        } else {
            u64::from(read_u32(&mut file, order)?)
        };
        if next_ifd != 0 {
            pending.push(next_ifd);
        }
    }
    Ok(None)
}

fn read_u16(reader: &mut File, order: ByteOrder) -> Result<u16> {
    let mut bytes = [0u8; 2];
    reader.read_exact(&mut bytes)?;
    Ok(order.u16(bytes))
}

fn read_u32(reader: &mut File, order: ByteOrder) -> Result<u32> {
    let mut bytes = [0u8; 4];
    reader.read_exact(&mut bytes)?;
    Ok(order.u32(bytes))
}

fn read_u64(reader: &mut File, order: ByteOrder) -> Result<u64> {
    let mut bytes = [0u8; 8];
    reader.read_exact(&mut bytes)?;
    Ok(order.u64(bytes))
}

fn field_width(field_type: u16) -> Option<u64> {
    match field_type {
        1 | 2 | 6 | 7 => Some(1),
        3 | 8 => Some(2),
        4 | 9 | 11 | 13 => Some(4),
        5 | 10 | 12 | 16 | 17 | 18 => Some(8),
        _ => None,
    }
}

#[derive(Clone, Copy)]
struct TiffFieldRef {
    order: ByteOrder,
    big_tiff: bool,
    field_type: u16,
    count: u64,
    value_field_offset: u64,
    value_or_offset: u64,
    file_len: u64,
}

impl TiffFieldRef {
    fn data_offset(self, total_bytes: u64) -> u64 {
        let inline_bytes = if self.big_tiff { 8 } else { 4 };
        if total_bytes <= inline_bytes {
            self.value_field_offset
        } else {
            self.value_or_offset
        }
    }
}

fn scalar_u64(file: &mut File, field: TiffFieldRef) -> Result<Option<u64>> {
    Ok(integer_values(file, field, 1)?.into_iter().next())
}

fn integer_values(file: &mut File, field: TiffFieldRef, limit: usize) -> Result<Vec<u64>> {
    let TiffFieldRef {
        order,
        field_type,
        count,
        file_len,
        ..
    } = field;

    let Some(width) = field_width(field_type) else {
        return Ok(Vec::new());
    };
    if !matches!(field_type, 1 | 3 | 4 | 13 | 16 | 18) || count == 0 {
        return Ok(Vec::new());
    }
    let total = width.checked_mul(count).context("TIFF tag size overflow")?;
    let data_offset = field.data_offset(total);
    anyhow::ensure!(
        data_offset
            .checked_add(total)
            .is_some_and(|end| end <= file_len),
        "TIFF tag data extends outside the file"
    );
    let current = file.stream_position()?;
    file.seek(SeekFrom::Start(data_offset))?;
    let value_count = usize::try_from(count).unwrap_or(usize::MAX).min(limit);
    let mut values = Vec::with_capacity(value_count);
    for _ in 0..value_count {
        let value = match field_type {
            1 => {
                let mut byte = [0u8; 1];
                file.read_exact(&mut byte)?;
                u64::from(byte[0])
            }
            3 => u64::from(read_u16(file, order)?),
            4 | 13 => u64::from(read_u32(file, order)?),
            16 | 18 => read_u64(file, order)?,
            _ => unreachable!(),
        };
        values.push(value);
    }
    file.seek(SeekFrom::Start(current))?;
    Ok(values)
}

pub(super) fn load_raster_tiff(path: &Path) -> Result<LoadedRaw> {
    let (width, height, rgb, source_is_float) = decode_scene_linear_rec2020(path)?;
    let mut raw = LoadedRaw::from_scene_linear_rec2020(width, height, rgb)?;
    // Floating-point TIFFs hold scene radiance (HDR and focus-stack masters, linear
    // exports); integer TIFFs are developed photos.
    raw.scene_referred_raster = source_is_float;
    Ok(raw)
}

/// Decoded linear Rec.2020 pixels and whether the file stores floating-point samples.
fn decode_scene_linear_rec2020(path: &Path) -> Result<(u32, u32, Vec<f32>, bool)> {
    let image = decode_tiff(path)?;
    let width = image.width();
    let height = image.height();
    validate_raw_dimensions(width, height)?;
    let source_color = image.color();
    let source_is_float = matches!(source_color, ColorType::Rgb32F | ColorType::Rgba32F);
    let mut rgb = image.into_rgb32f().into_raw();
    if rgb.par_iter().any(|value| !value.is_finite()) {
        return Err(anyhow!("TIFF contains NaN or infinity"));
    }

    if let Some(icc) = read_embedded_icc_profile(path)? {
        encoded_rgb_to_scene_linear_rec2020(&mut rgb, EncodedColorSpace::Icc(&icc))
            .with_context(|| format!("apply embedded TIFF ICC profile from {}", path.display()))?;
    } else if !source_is_float {
        encoded_rgb_to_scene_linear_rec2020(&mut rgb, EncodedColorSpace::SRGB)?;
    }
    ensure_finite(&rgb)?;

    Ok((width, height, rgb, source_is_float))
}

pub(super) fn load_raster_tiff_dimensions(path: &Path) -> Result<[u32; 2]> {
    let file = File::open(path).with_context(|| format!("open TIFF {}", path.display()))?;
    let mut reader = image::ImageReader::with_format(BufReader::new(file), ImageFormat::Tiff);
    configure_limits(&mut reader);
    let (width, height) = reader
        .into_dimensions()
        .with_context(|| format!("inspect TIFF dimensions {}", path.display()))?;
    validate_raw_dimensions(width, height)?;
    Ok([width, height])
}

pub(super) fn load_raster_tiff_thumbnail(path: &Path, maximum_edge: u32) -> Result<RawThumbnail> {
    anyhow::ensure!(maximum_edge > 0, "thumbnail edge must be non-zero");
    let (width, height, rgb, _) = decode_scene_linear_rec2020(path)?;
    let image = image::DynamicImage::ImageRgb32F(
        image::Rgb32FImage::from_raw(width, height, rgb)
            .ok_or_else(|| anyhow!("TIFF RGB buffer size does not match its dimensions"))?,
    );
    let image = crate::thumbnail_cache::downscale_to_fit(image, maximum_edge);
    let (width, height) = (image.width(), image.height());
    Ok(scene_linear_thumbnail(
        width,
        height,
        &image.into_rgb32f().into_raw(),
    ))
}

fn read_embedded_icc_profile(path: &Path) -> Result<Option<Vec<u8>>> {
    walk_tiff_fields(path, |file, tag, field| {
        if tag != TAG_ICC_PROFILE {
            return Ok(None);
        }
        anyhow::ensure!(
            matches!(field.field_type, 1 | 7),
            "TIFF ICC profile tag has unsupported field type {}",
            field.field_type
        );
        anyhow::ensure!(
            (MIN_TIFF_ICC_BYTES..=MAX_TIFF_ICC_BYTES).contains(&field.count),
            "TIFF ICC profile has an invalid size"
        );
        let data_offset = field.data_offset(field.count);
        anyhow::ensure!(
            data_offset
                .checked_add(field.count)
                .is_some_and(|end| end <= field.file_len),
            "TIFF ICC profile extends outside the file"
        );
        file.seek(SeekFrom::Start(data_offset))?;
        let mut bytes =
            vec![0u8; usize::try_from(field.count).context("TIFF ICC profile size overflow")?];
        file.read_exact(&mut bytes)?;
        Ok(Some(normalize_icc_profile(bytes)?))
    })
}

fn normalize_icc_profile(mut bytes: Vec<u8>) -> Result<Vec<u8>> {
    anyhow::ensure!(
        bytes.len() >= MIN_TIFF_ICC_BYTES as usize && &bytes[36..40] == b"acsp",
        "TIFF ICC profile has an invalid header"
    );
    let declared = u32::from_be_bytes(
        bytes[0..4]
            .try_into()
            .map_err(|_| anyhow!("TIFF ICC profile has an invalid size field"))?,
    ) as usize;
    anyhow::ensure!(
        (MIN_TIFF_ICC_BYTES as usize..=bytes.len()).contains(&declared),
        "TIFF ICC profile declares an invalid size"
    );
    bytes.truncate(declared);
    Ok(bytes)
}

fn decode_tiff(path: &Path) -> Result<image::DynamicImage> {
    let file = File::open(path).with_context(|| format!("open TIFF {}", path.display()))?;
    let mut reader = image::ImageReader::with_format(BufReader::new(file), ImageFormat::Tiff);
    configure_limits(&mut reader);
    let image = reader
        .decode()
        .with_context(|| format!("decode TIFF {} in process", path.display()))?;
    let pixels = validate_raw_dimensions(image.width(), image.height())? as u64;
    anyhow::ensure!(
        pixels.saturating_mul(16) <= MAX_TIFF_DECODE_BYTES,
        "TIFF {} requires too much decoded memory for this platform",
        path.display()
    );
    Ok(image)
}

fn configure_limits<R: std::io::BufRead + Seek>(reader: &mut image::ImageReader<R>) {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(super::raw_loader::MAX_RAW_EDGE);
    limits.max_image_height = Some(super::raw_loader::MAX_RAW_EDGE);
    limits.max_alloc = Some(MAX_TIFF_DECODE_BYTES);
    reader.limits(limits);
}
