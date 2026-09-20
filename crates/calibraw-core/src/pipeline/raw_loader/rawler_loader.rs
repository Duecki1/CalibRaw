// SPDX-License-Identifier: GPL-3.0-or-later
//! Isolated DNG backend. Rawler owns container and pixel decoding;
//! CalibRaw owns validation and adapts the result to its existing colour pipeline.

use super::libraw_loader as shared;
use super::{
    validate_raw_dimensions, CameraProfile, CameraProfileCandidate, CameraProfileMode,
    CaptureMetadata, CfaKind, CompactPixelMap, LoadedRaw, RawDisplayMetadata, RawThumbnail,
    MAX_RAW_FILE_BYTES, MAX_SENSOR_EDGE, MAX_SENSOR_PIXELS,
};
use crate::pipeline::{color_profile::DcpProfile, noise::NoiseProfile};
use anyhow::{anyhow, bail, ensure, Context, Result};
use rawler::decoders::{
    Decoder, FormatHint, Orientation, RawDecodeParams, RawMetadata, WellKnownIFD,
};
use rawler::formats::tiff::{Entry, IFD};
use rawler::imgop::develop::RawDevelop;
use rawler::rawimage::{RawImage, RawImageData, RawPhotometricInterpretation};
use rawler::rawsource::RawSource;
use rawler::tags::{DngTag, TiffCommonTag};
use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    path::Path,
    rc::Rc,
    sync::Arc,
};

const TIFF_TAG_PLANAR_CONFIGURATION: u16 = 284;
const MAX_RAWLER_THUMBNAIL_FALLBACK_EDGE: usize = 2048;

fn rawler_thumbnail_fallback_allowed(width: usize, height: usize) -> bool {
    width <= MAX_RAWLER_THUMBNAIL_FALLBACK_EDGE && height <= MAX_RAWLER_THUMBNAIL_FALLBACK_EDGE
}

fn guarded<T>(operation: impl FnOnce() -> Result<T>) -> Result<T> {
    catch_unwind(AssertUnwindSafe(operation))
        .map_err(|_| anyhow!("Rawler encountered malformed or unsupported RAW data"))?
}

fn entry(ifd: &IFD, tag: u16) -> Option<&Entry> {
    ifd.entries.get(&tag)
}

fn number(ifd: &IFD, tag: u16) -> Result<Option<usize>> {
    entry(ifd, tag)
        .map(|e| {
            ensure!(
                e.value.count() == 1,
                "invalid value count for RAW tag {tag}"
            );
            Ok(e.force_usize(0))
        })
        .transpose()
}

fn values<const N: usize>(raw: &IFD, root: &IFD, tag: DngTag) -> Result<Option<[f32; N]>> {
    entry(raw, tag as u16)
        .or_else(|| entry(root, tag as u16))
        .map(|e| {
            ensure!(
                e.value.count() == N,
                "unsupported {:?} component count",
                tag
            );
            let result = std::array::from_fn(|i| e.force_f32(i));
            ensure!(result.iter().all(|v| v.is_finite()), "non-finite {:?}", tag);
            Ok(result)
        })
        .transpose()
}

fn sensor_dimensions(width: usize, height: usize) -> Result<usize> {
    ensure!(
        width > 0
            && height > 0
            && width <= MAX_SENSOR_EDGE as usize
            && height <= MAX_SENSOR_EDGE as usize,
        "Rawler sensor dimensions {width}x{height} exceed the sensor edge limit"
    );
    let pixels = width
        .checked_mul(height)
        .context("sensor pixel count overflow")?;
    ensure!(
        pixels as u64 <= MAX_SENSOR_PIXELS,
        "Rawler sensor dimensions exceed the {MAX_SENSOR_PIXELS}-pixel limit"
    );
    Ok(pixels)
}

#[derive(Clone, Copy)]
struct Geometry {
    sensor_width: usize,
    sensor_height: usize,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    orientation: Orientation,
}

impl Geometry {
    fn from_ifds(raw: &IFD, root: &IFD) -> Result<Self> {
        let sensor_width =
            number(raw, TiffCommonTag::ImageWidth as u16)?.context("missing RAW width")?;
        let sensor_height =
            number(raw, TiffCommonTag::ImageLength as u16)?.context("missing RAW height")?;
        sensor_dimensions(sensor_width, sensor_height)?;
        let integer = |v: f32| -> Result<usize> {
            ensure!(
                v >= 0.0 && v.fract() == 0.0 && v <= MAX_SENSOR_EDGE as f32,
                "unsupported fractional or invalid DNG crop"
            );
            Ok(v as usize)
        };
        let [top, left, bottom, right] = values::<4>(raw, raw, DngTag::ActiveArea)?.unwrap_or([
            0.0,
            0.0,
            sensor_height as f32,
            sensor_width as f32,
        ]);
        let (top, left, bottom, right) = (
            integer(top)?,
            integer(left)?,
            integer(bottom)?,
            integer(right)?,
        );
        ensure!(
            left < right && top < bottom && right <= sensor_width && bottom <= sensor_height,
            "DNG active area is outside sensor bounds"
        );
        let origin = values::<2>(raw, raw, DngTag::DefaultCropOrigin)?.unwrap_or([0.0; 2]);
        let size = values::<2>(raw, raw, DngTag::DefaultCropSize)?
            .unwrap_or([(right - left) as f32, (bottom - top) as f32]);
        let (x, y, width, height) = (
            left + integer(origin[0])?,
            top + integer(origin[1])?,
            integer(size[0])?,
            integer(size[1])?,
        );
        ensure!(
            x.checked_add(width).is_some_and(|v| v <= right)
                && y.checked_add(height).is_some_and(|v| v <= bottom),
            "DNG default crop is outside the active area"
        );
        validate_raw_dimensions(width as u32, height as u32)?;
        let orientation = number(root, TiffCommonTag::Orientation as u16)?.unwrap_or(1);
        ensure!(
            (1..=8).contains(&orientation),
            "unsupported DNG orientation {orientation}"
        );
        Ok(Self {
            sensor_width,
            sensor_height,
            x,
            y,
            width,
            height,
            orientation: Orientation::from_u16(orientation as u16),
        })
    }

    fn dimensions(self) -> [u32; 2] {
        if self.orientation.to_flips().0 {
            [self.height as u32, self.width as u32]
        } else {
            [self.width as u32, self.height as u32]
        }
    }

    // Output -> uncropped sensor coordinates. Using the same mapping for pixels,
    // CFA indices and black levels preserves phase after odd crops and rotations.
    fn source(self, x: usize, y: usize) -> (usize, usize) {
        let (transpose, horizontal, vertical) = self.orientation.to_flips();
        let (x, y) = if transpose { (y, x) } else { (x, y) };
        (
            self.x + if horizontal { self.width - 1 - x } else { x },
            self.y + if vertical { self.height - 1 - y } else { y },
        )
    }
}

struct Input {
    source: RawSource,
    decoder: Box<dyn Decoder>,
    raw: Rc<IFD>,
    root: Rc<IFD>,
    geometry: Geometry,
}

fn open(path: &Path) -> Result<Input> {
    shared::validate_input_file(path, MAX_RAW_FILE_BYTES, "Rawler input")?;
    let source = RawSource::new(path).context("open Rawler source")?;
    let decoder = rawler::get_decoder(&source).context("identify Rawler input")?;
    // Proprietary decoders do not consistently expose pre-decode sensor bounds
    // through this API. Leave those formats to LibRaw rather than bypass limits.
    ensure!(
        decoder.format_hint() == FormatHint::DNG,
        "Rawler DNG backend supports DNG containers only"
    );
    let raw = decoder
        .ifd(WellKnownIFD::Raw)?
        .context("Rawler has no RAW image header")?;
    let root = decoder
        .ifd(WellKnownIFD::Root)?
        .context("Rawler has no root image header")?;
    let geometry = Geometry::from_ifds(&raw, &root)?;
    Ok(Input {
        source,
        decoder,
        raw,
        root,
        geometry,
    })
}

fn validate_layout(input: &Input) -> Result<()> {
    let raw = &*input.raw;
    let cpp =
        number(raw, TiffCommonTag::SamplesPerPixel as u16)?.context("missing sample count")?;
    let photometric = number(raw, TiffCommonTag::PhotometricInt as u16)?
        .context("missing photometric interpretation")?;
    ensure!(
        (photometric == 32803 && cpp == 1) || (photometric == 34892 && cpp == 3),
        "unsupported DNG photometric interpretation {photometric} with {cpp} channels"
    );
    ensure!(
        number(raw, TIFF_TAG_PLANAR_CONFIGURATION)?.unwrap_or(1) == 1,
        "planar DNG samples are unsupported"
    );
    ensure!(
        number(raw, DngTag::CFALayout as u16)?.unwrap_or(1) == 1,
        "non-rectangular DNG CFA layout is unsupported"
    );
    if let Some(colors) = values::<3>(raw, raw, DngTag::CFAPlaneColor)? {
        ensure!(
            colors == [0.0, 1.0, 2.0],
            "non-RGB DNG CFA planes are unsupported"
        );
    }
    if let Some(scale) = values::<2>(raw, raw, DngTag::DefaultScale)? {
        ensure!(
            scale[0] > 0.0 && scale[1] > 0.0 && (scale[0] - scale[1]).abs() < 1e-6,
            "non-square DNG pixels are unsupported"
        );
    }
    // Rawler 0.8 applies LinearizationTable and supported interleave factors,
    // but it does not apply DNG opcode lists. Samsung Expert RAW / phone DNGs
    // commonly carry lens-shading GainMap corrections in OpcodeList2. Treat the
    // missing correction as a documented rendering limitation rather than a
    // decode blocker, otherwise valid LinearRaw JPEG-XL images cannot open.
    if entry(raw, DngTag::OpcodeList2 as u16).is_some() {
        log::warn!(
            "DNG OpcodeList2 is present but Rawler 0.8 does not apply DNG opcodes; decoding without the stage-2 correction"
        );
        crate::diagnostics::record(
            "DNG OpcodeList2 present; Rawler decoded pixels without applying the stage-2 opcode list",
        );
    }
    // Keep other unimplemented raw-stage corrections strict. Ignoring these
    // could change the interpretation of the sensor samples in less predictable
    // ways, and CalibRaw deliberately does not maintain a private opcode parser.
    for tag in [DngTag::OpcodeList1, DngTag::OpcodeList3] {
        ensure!(
            entry(raw, tag as u16).is_none(),
            "Rawler cannot apply DNG correction {:?}",
            tag
        );
    }
    for tag in [DngTag::BlackLevelDeltaH, DngTag::BlackLevelDeltaV] {
        ensure!(
            entry(raw, tag as u16).is_none(),
            "Rawler cannot apply DNG black-level correction {:?}",
            tag
        );
    }
    let width = input.geometry.sensor_width;
    let height = input.geometry.sensor_height;
    let (offset_tag, count_tag, count) = if entry(raw, TiffCommonTag::TileOffsets as u16).is_some()
    {
        let tw = number(raw, TiffCommonTag::TileWidth as u16)?.context("missing tile width")?;
        let th = number(raw, TiffCommonTag::TileLength as u16)?.context("missing tile height")?;
        sensor_dimensions(tw, th)?;
        let columns = width.div_ceil(tw);
        let rows = height.div_ceil(th);
        sensor_dimensions(
            columns.checked_mul(tw).context("tile width overflow")?,
            rows.checked_mul(th).context("tile height overflow")?,
        )?;
        (
            TiffCommonTag::TileOffsets,
            TiffCommonTag::TileByteCounts,
            columns * rows,
        )
    } else {
        let rows = number(raw, TiffCommonTag::RowsPerStrip as u16)?.unwrap_or(height);
        ensure!(
            rows > 0 && rows <= MAX_SENSOR_EDGE as usize,
            "invalid DNG strip height"
        );
        (
            TiffCommonTag::StripOffsets,
            TiffCommonTag::StripByteCounts,
            height.div_ceil(rows),
        )
    };
    let offsets = raw.get_entry(offset_tag).context("missing pixel offsets")?;
    let counts = raw
        .get_entry(count_tag)
        .context("missing pixel byte counts")?;
    ensure!(
        offsets.value.count() == count && counts.value.count() == count,
        "invalid DNG strip/tile count"
    );
    for i in 0..count {
        let offset = offsets.force_usize(i);
        let bytes = counts.force_usize(i);
        ensure!(
            bytes > 0
                && offset
                    .checked_add(bytes)
                    .is_some_and(|end| end <= input.source.buf().len()),
            "DNG pixel payload is outside the file"
        );
    }
    Ok(())
}

fn embedded_profile(path: &Path, input: &Input) -> Result<DcpProfile> {
    let mut profile = shared::read_optional_profile(path)?.unwrap_or_default();
    for (i, (color, calibration, forward, illuminant)) in [
        (
            DngTag::ColorMatrix1,
            DngTag::CameraCalibration1,
            DngTag::ForwardMatrix1,
            DngTag::CalibrationIlluminant1,
        ),
        (
            DngTag::ColorMatrix2,
            DngTag::CameraCalibration2,
            DngTag::ForwardMatrix2,
            DngTag::CalibrationIlluminant2,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let set = &mut profile.matrices[i];
        if let Some(v) = values::<9>(&input.raw, &input.root, color)? {
            set.color_matrix = Some([
                v[0..3].try_into()?,
                v[3..6].try_into()?,
                v[6..9].try_into()?,
                [0.0; 3],
            ]);
        }
        if let Some(v) = values::<9>(&input.raw, &input.root, calibration)? {
            let mut matrix = identity();
            for row in 0..3 {
                matrix[row][..3].copy_from_slice(&v[row * 3..row * 3 + 3]);
            }
            set.camera_calibration = Some(matrix);
        }
        if let Some(v) = values::<9>(&input.raw, &input.root, forward)? {
            set.forward_matrix = Some(std::array::from_fn(|r| {
                [v[r * 3], v[r * 3 + 1], v[r * 3 + 2], 0.0]
            }));
        }
        if let Some(v) = values::<1>(&input.raw, &input.root, illuminant)? {
            set.illuminant = Some(v[0] as u16);
        }
    }
    ensure!(
        profile.matrices.iter().any(|s| s.color_matrix.is_some()),
        "DNG has no supported RGB camera color matrix"
    );
    Ok(profile)
}

fn identity() -> [[f32; 4]; 4] {
    std::array::from_fn(|r| std::array::from_fn(|c| if r == c { 1.0 } else { 0.0 }))
}
fn positive(value: Option<rawler::formats::tiff::Rational>) -> f32 {
    value
        .map(|v| v.as_f32())
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(0.0)
}

fn capture_metadata(path: &Path, md: &RawMetadata) -> CaptureMetadata {
    let exif = &md.exif;
    let mut capture = shared::read_exif_capture_metadata_or_default(path);
    if capture.exif_dates.is_empty() {
        capture.exif_dates = [
            (0x9003, &exif.date_time_original),
            (0x9004, &exif.create_date),
            (0x0132, &exif.modify_date),
        ]
        .into_iter()
        .filter_map(|(tag, value)| value.clone().map(|v| (tag, v)))
        .collect();
    }
    capture.iso_speed = exif
        .iso_speed
        .or(exif.iso_speed_ratings.map(u32::from))
        .unwrap_or(0) as f32;
    capture.shutter_seconds = positive(exif.exposure_time);
    capture.flash = capture.flash.or(exif.flash);
    capture.artist = exif.artist.clone().unwrap_or_default();
    capture
}

fn adapt(
    image: RawImage,
    input: &Input,
    path: &Path,
    selected: Option<DcpProfile>,
) -> Result<LoadedRaw> {
    let geometry = input.geometry;
    ensure!(
        image.width == geometry.sensor_width && image.height == geometry.sensor_height,
        "Rawler decoded dimensions differ from the validated header"
    );
    ensure!(
        image.fuji_rotation_width.is_none(),
        "rotated sensor geometry is unsupported"
    );
    let sensor_pixels = sensor_dimensions(image.width, image.height)?;
    let expected = sensor_pixels
        .checked_mul(image.cpp)
        .context("RAW sample count overflow")?;
    let sample_len = match &image.data {
        RawImageData::Integer(v) => v.len(),
        RawImageData::Float(v) => v.len(),
    };
    ensure!(
        sample_len == expected,
        "Rawler sample count differs from its dimensions"
    );
    let black = &image.blacklevel;
    ensure!(
        black.cpp == image.cpp
            && black.width > 0
            && black.height > 0
            && black.width <= image.width
            && black.height <= image.height,
        "invalid DNG black-level pattern"
    );
    ensure!(
        black
            .width
            .checked_mul(black.height)
            .and_then(|n| n.checked_mul(black.cpp))
            == Some(black.levels.len()),
        "invalid black-level sample count"
    );
    let black_values = black.as_vec();
    ensure!(
        black_values.iter().all(|v| v.is_finite()),
        "invalid DNG black levels"
    );
    let white = image.whitelevel.as_vec();
    ensure!(
        white.len() == image.cpp && white.iter().all(|v| v.is_finite() && *v > 0.0),
        "unsupported DNG white levels"
    );
    for (i, b) in black_values.iter().enumerate() {
        ensure!(
            white[i % image.cpp] > *b,
            "DNG white level must exceed black level"
        );
    }
    let black_at = |x: usize, y: usize, channel: usize| {
        black_values[((y % black.height) * black.width + x % black.width) * image.cpp + channel]
    };

    let embedded = embedded_profile(path, input)?;
    let xyz_to_cam = embedded
        .matrices
        .iter()
        .find_map(|s| s.color_matrix)
        .context("missing color matrix")?;
    let mut wb = image.wb_coeffs;
    if !wb[..3].iter().all(|v| v.is_finite() && *v > 0.0) {
        // Missing as-shot WB uses the embedded matrix's daylight neutral.
        ensure!(
            entry(&input.root, DngTag::AsShotNeutral as u16)
                .or_else(|| entry(&input.raw, DngTag::AsShotNeutral as u16))
                .is_none(),
            "invalid DNG as-shot neutral"
        );
        for c in 0..3 {
            wb[c] = 1.0
                / (xyz_to_cam[c][0] * 0.9504559 + xyz_to_cam[c][1] + xyz_to_cam[c][2] * 1.0890578);
        }
        ensure!(
            wb[..3].iter().all(|v| v.is_finite() && *v > 0.0),
            "cannot derive a valid DNG white balance"
        );
    }
    let wb = shared::white_balance(wb, *b"RGB\0");
    let wb_coeffs = [wb[0], wb[1], wb[2], wb[1]];
    let mut analog = identity();
    if let Some(v) = values::<3>(&input.raw, &input.root, DngTag::AnalogBalance)? {
        ensure!(v.iter().all(|v| *v > 0.0), "invalid DNG analog balance");
        for c in 0..3 {
            analog[c][c] = v[c];
        }
    }
    let (cam_to_srgb, weight, white_balance_model) =
        shared::camera_to_working_matrix_from_profiles(
            xyz_to_cam,
            wb_coeffs,
            *b"RGB\0",
            Some(&embedded),
            selected.as_ref(),
            analog,
        )?;
    let mut camera_profile = selected
        .map(|p| CameraProfile::from_dcp(p, weight))
        .unwrap_or_default();
    let baseline = values::<1>(&input.raw, &input.root, DngTag::BaselineExposure)?.map(|v| v[0]);
    shared::apply_resolved_default_exposure(&mut camera_profile, baseline);
    let md = input
        .decoder
        .raw_metadata(&input.source, &RawDecodeParams::default())
        .unwrap_or_else(|error| {
            log::warn!("Rawler capture metadata unavailable: {error}");
            RawMetadata::default()
        });
    let capture_metadata = capture_metadata(path, &md);
    let [width, height] = geometry.dimensions();
    let pixels = validate_raw_dimensions(width, height)?;
    let mut loaded = LoadedRaw {
        width,
        height,
        camera_make: image.make.clone(),
        camera_model: image.model.clone(),
        lens_make: md.exif.lens_make.clone().unwrap_or_default(),
        lens_model: md.exif.lens_model.clone().unwrap_or_default(),
        focal_length: positive(md.exif.focal_length),
        aperture: positive(md.exif.fnumber),
        focus_distance: positive(md.exif.subject_distance),
        capture_metadata,
        cfa_kind: CfaKind::Bayer,
        raw_pixels: Vec::new(),
        scene_linear_raster: None,
        color_indices: CompactPixelMap::repeating(width, height, 1, 1, vec![1]),
        wb_coeffs,
        cam_to_srgb,
        black_levels: [0.0; 4],
        black_levels_per_pixel: CompactPixelMap::repeating(width, height, 1, 1, vec![0.0]),
        white_levels: [1.0; 4],
        noise_profile: NoiseProfile::default(),
        camera_profile,
        camera_profile_source: None,
        available_camera_profiles: Vec::new(),
        white_balance_model,
        lens_geometry: None,
        ai_denoised: Default::default(),
        opposed_chroma_cache: Default::default(),
        opposed_chroma_source_identity: Default::default(),
        opposed_chroma_reference_source: true,
    };
    match &image.photometric {
        RawPhotometricInterpretation::Cfa(config) => {
            ensure!(image.cpp == 1, "unsupported multi-channel CFA");
            let RawImageData::Integer(data) = &image.data else {
                bail!("floating-point CFA DNG is not supported by the integer GPU sensor path");
            };
            let cfa = &config.cfa;
            let period = match (cfa.width, cfa.height) {
                (2, 2) => 2,
                (6, 6) => 6,
                _ => bail!("unsupported DNG CFA pattern"),
            };
            let mut counts = [0; 3];
            for y in 0..period {
                for x in 0..period {
                    let c = cfa.color_at(y, x);
                    ensure!(c < 3, "non-RGB CFA colors are unsupported");
                    counts[c] += 1;
                }
            }
            ensure!(
                (period == 2 && counts == [1, 2, 1]) || (period == 6 && counts == [8, 20, 8]),
                "unsupported Bayer/X-Trans pattern"
            );
            if period == 2 {
                ensure!(
                    cfa.color_at(0, 0) != cfa.color_at(0, 1)
                        && cfa.color_at(0, 0) != cfa.color_at(1, 0),
                    "non-Bayer 2x2 CFA"
                );
            }
            loaded.cfa_kind = if period == 2 {
                CfaKind::Bayer
            } else {
                CfaKind::XTrans
            };
            let green_position = (0..period * period)
                .find(|i| cfa.color_at(i / period, i % period) == 1)
                .unwrap();
            let channel_at = |x: usize, y: usize| -> u8 {
                let c = cfa.color_at(y, x);
                if period == 2 && c == 1 && (y % 2) * 2 + x % 2 != green_position {
                    3
                } else {
                    c as u8
                }
            };
            let (tile_w, tile_h) = (period.min(width as usize), period.min(height as usize));
            let mut indices = Vec::with_capacity(tile_w * tile_h);
            for y in 0..tile_h {
                for x in 0..tile_w {
                    let (sx, sy) = geometry.source(x, y);
                    indices.push(channel_at(sx, sy));
                }
            }
            loaded.color_indices =
                CompactPixelMap::repeating(width, height, tile_w as u32, tile_h as u32, indices);
            let (bw, bh) = if geometry.orientation.to_flips().0 {
                (black.height, black.width)
            } else {
                (black.width, black.height)
            };
            let (bw, bh) = (bw.min(width as usize), bh.min(height as usize));
            let mut levels = Vec::new();
            levels.try_reserve_exact(bw * bh)?;
            for y in 0..bh {
                for x in 0..bw {
                    let (sx, sy) = geometry.source(x, y);
                    levels.push(black_at(sx, sy, 0));
                }
            }
            loaded.black_levels_per_pixel =
                CompactPixelMap::repeating(width, height, bw as u32, bh as u32, levels);
            let mut sums = [0.0; 4];
            let mut n = [0usize; 4];
            for y in 0..black.height.max(period) {
                for x in 0..black.width.max(period) {
                    let c = channel_at(x, y) as usize;
                    sums[c] += black_at(x, y, 0);
                    n[c] += 1;
                }
            }
            loaded.black_levels = std::array::from_fn(|i| {
                if n[i] > 0 {
                    sums[i] / n[i] as f32
                } else {
                    sums[1] / n[1].max(1) as f32
                }
            });
            loaded.white_levels = [white[0]; 4];
            loaded.raw_pixels.try_reserve_exact(pixels)?;
            for y in 0..height as usize {
                for x in 0..width as usize {
                    let (sx, sy) = geometry.source(x, y);
                    loaded.raw_pixels.push(data[sy * image.width + sx]);
                }
            }
            loaded.noise_profile = NoiseProfile::estimate(
                width,
                height,
                &loaded.raw_pixels,
                &loaded.color_indices,
                &loaded.black_levels_per_pixel,
                loaded.white_levels,
                loaded.capture_metadata.iso_speed,
                period as u32,
            );
        }
        RawPhotometricInterpretation::LinearRaw => {
            ensure!(
                image.cpp == 3,
                "only three-channel RGB LinearRaw is supported"
            );
            let mut rgb = Vec::new();
            rgb.try_reserve_exact(pixels.checked_mul(3).context("RGB size overflow")?)?;
            for y in 0..height as usize {
                for x in 0..width as usize {
                    let (sx, sy) = geometry.source(x, y);
                    let index = (sy * image.width + sx) * 3;
                    for (c, w) in white.iter().enumerate() {
                        let sample = match &image.data {
                            RawImageData::Integer(v) => v[index + c] as f32,
                            RawImageData::Float(v) => v[index + c],
                        };
                        let b = black_at(sx, sy, c);
                        let value = (sample - b) / (w - b);
                        ensure!(value.is_finite(), "non-finite LinearRaw sample");
                        rgb.push(value.max(0.0));
                    }
                }
            }
            loaded.scene_linear_raster = Some(Arc::from(rgb));
        }
        _ => bail!("unsupported Rawler photometric interpretation"),
    }
    Ok(loaded)
}

pub(super) fn load_raw_file_with_profile_selection(
    path: &Path,
    mode: CameraProfileMode,
    folder: Option<&Path>,
    selected: Option<&Path>,
) -> Result<LoadedRaw> {
    guarded(|| {
        let input = open(path)?;
        validate_layout(&input)?;
        let image = input
            .decoder
            .raw_image(&input.source, &RawDecodeParams::default(), false)
            .context("decode DNG with Rawler")?;
        let (profile, source, candidates) = shared::resolve_camera_profiles(
            path,
            mode,
            folder,
            selected,
            &image.make,
            &image.model,
        )?;
        let mut loaded = adapt(image, &input, path, profile)?;
        loaded.camera_profile_source = source;
        loaded.available_camera_profiles = candidates;
        crate::diagnostics::record("RAW decode completed through Rawler backend");
        Ok(loaded)
    })
}

pub(super) fn load_raw_file_with_dcp(path: &Path, profile_path: &Path) -> Result<LoadedRaw> {
    guarded(|| {
        shared::validate_input_file(profile_path, 64 * 1024 * 1024, "DCP profile")?;
        let mut profile =
            DcpProfile::from_path(profile_path)?.context("not a DNG camera profile")?;
        let input = open(path)?;
        validate_layout(&input)?;
        profile.camera_calibration_signature =
            embedded_profile(path, &input)?.camera_calibration_signature;
        let name = profile.name.clone().unwrap_or_else(|| {
            profile_path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });
        let image = input
            .decoder
            .raw_image(&input.source, &RawDecodeParams::default(), false)?;
        let mut loaded = adapt(image, &input, path, Some(profile))?;
        loaded.camera_profile_source = Some(profile_path.to_owned());
        loaded.available_camera_profiles = vec![CameraProfileCandidate {
            path: profile_path.to_owned(),
            name,
        }];
        Ok(loaded)
    })
}

pub(super) fn load_raw_display_metadata(path: &Path) -> Result<RawDisplayMetadata> {
    guarded(|| {
        let input = open(path)?;
        let md = input
            .decoder
            .raw_metadata(&input.source, &RawDecodeParams::default())
            .unwrap_or_default();
        Ok(RawDisplayMetadata {
            dimensions: input.geometry.dimensions(),
            aperture: positive(md.exif.fnumber),
            focal_length: positive(md.exif.focal_length),
            iso_speed: md
                .exif
                .iso_speed
                .or(md.exif.iso_speed_ratings.map(u32::from))
                .unwrap_or(0) as f32,
            shutter_seconds: positive(md.exif.exposure_time),
        })
    })
}

fn thumbnail_from_dynamic_image(
    preview: image::DynamicImage,
    maximum_edge: u32,
    orientation: Orientation,
) -> Result<RawThumbnail> {
    ensure!(maximum_edge > 0, "thumbnail edge must be non-zero");
    let resized = preview
        .thumbnail(maximum_edge.min(8192), maximum_edge.min(8192))
        .to_rgba8();
    ensure!(
        resized.width() > 0 && resized.height() > 0,
        "DNG preview has invalid dimensions"
    );
    let geometry = Geometry {
        sensor_width: resized.width() as usize,
        sensor_height: resized.height() as usize,
        x: 0,
        y: 0,
        width: resized.width() as usize,
        height: resized.height() as usize,
        orientation,
    };
    let [width, height] = geometry.dimensions();
    let byte_len = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .context("DNG thumbnail allocation overflow")?;
    let mut rgba = Vec::new();
    rgba.try_reserve_exact(byte_len)?;
    for y in 0..height as usize {
        for x in 0..width as usize {
            let (sx, sy) = geometry.source(x, y);
            rgba.extend_from_slice(&resized.get_pixel(sx as u32, sy as u32).0);
        }
    }
    Ok(RawThumbnail {
        width,
        height,
        rgba,
    })
}

fn embedded_thumbnail(input: &Input) -> Result<image::DynamicImage> {
    let params = RawDecodeParams::default();
    let mut failures = Vec::new();

    match input.decoder.thumbnail_image(&input.source, &params) {
        Ok(Some(image)) => return Ok(image),
        Ok(None) => {}
        Err(error) => failures.push(format!("thumbnail: {error}")),
    }
    match input.decoder.preview_image(&input.source, &params) {
        Ok(Some(image)) => return Ok(image),
        Ok(None) => {}
        Err(error) => failures.push(format!("preview: {error}")),
    }
    if failures.is_empty() {
        bail!("DNG has no embedded thumbnail or preview")
    } else {
        bail!(
            "DNG has no usable embedded thumbnail or preview ({})",
            failures.join("; ")
        )
    }
}

pub(super) fn load_raw_embedded_thumbnail(path: &Path, maximum_edge: u32) -> Result<RawThumbnail> {
    guarded(|| {
        ensure!(maximum_edge > 0, "thumbnail edge must be non-zero");
        let input = open(path)?;
        let orientation = input.geometry.orientation;
        let preview = embedded_thumbnail(&input)?;
        thumbnail_from_dynamic_image(preview, maximum_edge, orientation)
    })
}

pub(super) fn load_raw_thumbnail(path: &Path, maximum_edge: u32) -> Result<RawThumbnail> {
    guarded(|| {
        ensure!(maximum_edge > 0, "thumbnail edge must be non-zero");
        let input = open(path)?;
        let orientation = input.geometry.orientation;

        match embedded_thumbnail(&input) {
            Ok(preview) => thumbnail_from_dynamic_image(preview, maximum_edge, orientation),
            Err(embedded_error) => {
                // Rawler has no scaled-develop API. Only use its full intermediate
                // for genuinely small sources; larger files return an error so the
                // shared router can use LibRaw's half-size fallback instead.
                ensure!(
                    rawler_thumbnail_fallback_allowed(
                        input.geometry.sensor_width,
                        input.geometry.sensor_height
                    ),
                    "DNG has no embedded preview and is too large for the bounded Rawler thumbnail fallback"
                );
                let _render_permit = crate::thumbnail_cache::acquire_rendered_thumbnail_worker();
                validate_layout(&input)?;
                let raw = input
                    .decoder
                    .raw_image(&input.source, &RawDecodeParams::default(), false)
                    .context("decode small DNG for thumbnail fallback")?;
                let developed = RawDevelop::default()
                    .develop_intermediate(&raw)
                    .context("develop small DNG thumbnail fallback")?
                    .to_dynamic_image()
                    .context("Rawler could not convert developed DNG thumbnail to an image")?;
                thumbnail_from_dynamic_image(developed, maximum_edge, orientation).with_context(
                    || format!("embedded DNG preview failed first: {embedded_error:#}"),
                )
            }
        }
    })
}

#[cfg(test)]
mod tests;
