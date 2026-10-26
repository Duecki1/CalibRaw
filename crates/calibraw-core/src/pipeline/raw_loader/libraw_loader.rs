// SPDX-License-Identifier: GPL-3.0-or-later
// Camera-matrix normalization follows darktable 5.6.0 and the related
// Ansel/dcraw implementations; temperature and tint follow the DNG SDK.
// Copyright (C) 2010-2026 darktable developers and the Ansel/dcraw authors.
// Copyright (C) 2026 CalibRaw contributors (Rust adaptation).

use super::super::chromatic_aberration::LateralChromaticAberration;
use super::super::noise::NoiseProfile;
use super::{
    validate_raw_dimensions, CameraColorModel, CameraProfile, CameraProfileCandidate,
    CameraProfileMode, CameraWhiteBalanceModel, CfaKind, CompactPixelMap, DngColorEndpoint,
    LoadedRaw, RawThumbnail, SensorSaturation, WhiteBalanceRanges, MAX_RAW_FILE_BYTES,
    MAX_SENSOR_EDGE, MAX_SENSOR_PIXELS,
};
use crate::matrix;
use crate::pipeline::basicadj::{
    temperature_kelvin_from_offset, white_balance_tint_from_offset, MAX_TEMPERATURE_KELVIN,
    MAX_WHITE_BALANCE_TINT, MIN_TEMPERATURE_KELVIN, MIN_WHITE_BALANCE_TINT,
};
use crate::pipeline::color_profile::{DcpMatrixSet, DcpProfile};
use anyhow::{anyhow, Context, Result};
use rayon::prelude::*;
use std::collections::HashMap;
use std::ffi::CStr;
#[cfg(not(windows))]
use std::ffi::CString;
use std::fs;
use std::io::Cursor;
use std::ops::RangeInclusive;
use std::os::raw::c_char;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Instant, SystemTime};

#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;

mod camera_matrix;
mod cct;
mod dcp_index;
mod profile_color_source;
mod sensor;
mod temperature_tint;
mod thumbnail;
pub(super) use camera_matrix::*;
use cct::*;
pub(super) use dcp_index::*;
use profile_color_source::LibRawColorInputs;
pub use profile_color_source::ProfileColorSource;
pub(super) use profile_color_source::{
    read_embedded_signature, CameraProfileSelection, DecoderColor, EmbeddedSignature,
    RawlerColorInputs,
};
pub(super) use sensor::*;
pub(super) use temperature_tint::*;
pub(super) use thumbnail::*;

const MAX_DCP_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MISSING_BASELINE_EXPOSURE_FALLBACK_EV: f32 = 1.25;

fn valid_baseline_exposure(value: f32) -> Option<f32> {
    (value.is_finite() && value > -999.0).then_some(value)
}

fn resolve_default_exposure_ev(baseline_exposure: Option<f32>, profile_offset_ev: f32) -> f32 {
    let baseline = baseline_exposure.unwrap_or(MISSING_BASELINE_EXPOSURE_FALLBACK_EV);
    (baseline + profile_offset_ev).clamp(-5.0, 5.0)
}

pub(super) fn apply_resolved_default_exposure(
    camera_profile: &mut CameraProfile,
    baseline_exposure: Option<f32>,
) {
    camera_profile.default_exposure_ev =
        resolve_default_exposure_ev(baseline_exposure, camera_profile.profile_exposure_offset_ev);
}

/// The camera profile a decode of the RAW at `path` applies, as
/// [`select_camera_profiles`] chooses it, with the embedded signature it was
/// matched against.
pub(super) fn resolve_camera_profiles(
    path: &Path,
    mode: CameraProfileMode,
    profile_folder: Option<&Path>,
    selected_profile: Option<&Path>,
    camera_make: &str,
    camera_model: &str,
) -> Result<(EmbeddedSignature, CameraProfileSelection)> {
    let signature = read_embedded_signature(path, mode)?;
    let raw_camera_signature = match &signature {
        EmbeddedSignature::Read(signature) => signature.clone(),
        EmbeddedSignature::Unread => None,
    };
    let selection = select_camera_profiles(
        mode,
        profile_folder,
        selected_profile,
        camera_make,
        camera_model,
        raw_camera_signature,
    );
    Ok((signature, selection))
}

/// The DCP `mode` applies to a camera: `selected_profile` when it matches the
/// camera, otherwise the best match in DCP mode, and none for the photo's own
/// colour data. Matching DCPs take `raw_camera_signature`.
pub(super) fn select_camera_profiles(
    mode: CameraProfileMode,
    profile_folder: Option<&Path>,
    selected_profile: Option<&Path>,
    camera_make: &str,
    camera_model: &str,
    raw_camera_signature: Option<String>,
) -> CameraProfileSelection {
    let mut matches = profile_folder
        .map(|folder| find_matching_dcp_profiles(folder, camera_make, camera_model))
        .transpose()
        .unwrap_or_else(|error| {
            if let Some(folder) = profile_folder {
                log::warn!(
                    "could not search DCP profile folder {}: {error:#}",
                    folder.display()
                );
            }
            None
        })
        .unwrap_or_default();
    let candidates = matches
        .iter()
        .map(|candidate| CameraProfileCandidate {
            path: candidate.path.clone(),
            name: candidate.name.clone(),
        })
        .collect::<Vec<_>>();
    let explicitly_selected = selected_profile.and_then(|requested| {
        matches
            .iter()
            .position(|candidate| candidate.path == requested)
            .map(|index| matches.remove(index))
    });
    let external_profile = if mode == CameraProfileMode::MatrixOnly {
        None
    } else {
        explicitly_selected.or_else(|| {
            (mode.prefers_external_dcp() && !matches.is_empty()).then(|| matches.remove(0))
        })
    };
    let (source, profile) = external_profile
        .map(|mut candidate| {
            candidate.profile.camera_calibration_signature = raw_camera_signature;
            (Some(candidate.path), Some(candidate.profile))
        })
        .unwrap_or((None, None));
    CameraProfileSelection {
        profile,
        source,
        candidates,
    }
}

use crate::color_math::{bradford_adaptation, D65_XYZ, DNG_PCS_D50_XYZ};
const XYZ_TO_REC2020: [[f32; 3]; 3] = [
    [1.7166512, -0.3556708, -0.2533663],
    [-0.6666844, 1.6164812, 0.0157685],
    [0.0176399, -0.0427706, 0.9421031],
];

#[allow(
    clippy::upper_case_acronyms,
    clippy::ptr_offset_with_cast,
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    unnecessary_transmutes
)]
mod ffi {
    include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
}

pub(super) fn load_raw_file_with_profile_selection(
    path: &Path,
    mode: CameraProfileMode,
    profile_folder: Option<&Path>,
    selected_profile: Option<&Path>,
) -> Result<LoadedRaw> {
    validate_input_file(path, MAX_RAW_FILE_BYTES, "RAW input")?;
    // The RAW is read in two gated sections, header and payload, so DCP
    // profile resolution in between does not hold the read gate.
    let (source_metadata, ctx) = crate::serialized_reads::run(|| -> Result<_> {
        let source_metadata = read_exif_capture_metadata_or_default(path);

        let ctx = LibRawContext::new()?;
        let identify_started = Instant::now();
        open_libraw_file(&ctx, path, "open RAW file")?;
        crate::diagnostics::record(format!(
            "LibRaw identify/open_file finished in {:.3}s",
            identify_started.elapsed().as_secs_f64()
        ));
        Ok((source_metadata, ctx))
    })?;
    validate_opened_raw_geometry(&ctx)?;

    let iparams = &ctx.data().rawdata.iparams;
    let (camera_make, camera_model) = (
        c_array_to_string(&iparams.make),
        c_array_to_string(&iparams.model),
    );

    let profile_started = Instant::now();
    let (signature, selection) = resolve_camera_profiles(
        path,
        mode,
        profile_folder,
        selected_profile,
        &camera_make,
        &camera_model,
    )?;
    crate::diagnostics::record(format!(
        "Camera-profile resolution finished in {:.3}s",
        profile_started.elapsed().as_secs_f64()
    ));

    let unpack_started = Instant::now();
    // SAFETY: `ctx` is a live context with a file opened (type invariant).
    crate::serialized_reads::run(|| {
        check_libraw(unsafe { ffi::libraw_unpack(ctx.raw) }, "unpack RAW file")
    })?;
    crate::diagnostics::record(format!(
        "LibRaw sensor unpack finished in {:.3}s",
        unpack_started.elapsed().as_secs_f64()
    ));
    let materialize_started = Instant::now();
    let mut loaded = loaded_raw_from_context(&ctx, signature, selection.profile, selection.source)?;
    merge_exif_capture_metadata(&mut loaded.capture_metadata, source_metadata);
    crate::diagnostics::record(format!(
        "Decoded mosaic materialization finished in {:.3}s",
        materialize_started.elapsed().as_secs_f64()
    ));
    loaded.available_camera_profiles = selection.candidates;
    Ok(loaded)
}

pub(super) fn load_raw_file_with_dcp(path: &Path, profile_path: &Path) -> Result<LoadedRaw> {
    validate_input_file(path, MAX_RAW_FILE_BYTES, "RAW input")?;
    let (selected, candidate, signature) = read_selected_dcp_profile(path, profile_path)?;
    let mut loaded = load_raw_file_with_selected_profile(
        path,
        signature,
        Some(selected),
        Some(candidate.path.clone()),
    )?;
    loaded.available_camera_profiles = vec![candidate];
    Ok(loaded)
}

/// Reads the DCP explicitly chosen for the proprietary RAW at `raw_path`, for
/// either backend. A profile embedded in the RAW supplies the camera
/// calibration signature that the DCP's calibration matrices are matched to;
/// it is returned too.
pub(super) fn read_selected_dcp_profile(
    raw_path: &Path,
    profile_path: &Path,
) -> Result<(DcpProfile, CameraProfileCandidate, EmbeddedSignature)> {
    validate_input_file(profile_path, MAX_DCP_FILE_BYTES, "DCP profile")?;
    let mut selected = DcpProfile::from_path(profile_path)
        .with_context(|| format!("read DCP profile {}", profile_path.display()))?
        .ok_or_else(|| anyhow!("{} is not a DNG camera profile", profile_path.display()))?;

    let raw_signature = read_optional_profile(raw_path)?
        .map(|raw_profile| raw_profile.camera_calibration_signature);
    if let Some(signature) = &raw_signature {
        selected.camera_calibration_signature = signature.clone();
    }
    let candidate = CameraProfileCandidate {
        path: profile_path.to_path_buf(),
        name: dcp_profile_display_name(selected.name.as_deref(), profile_path),
    };
    Ok((
        selected,
        candidate,
        EmbeddedSignature::Read(raw_signature.flatten()),
    ))
}

pub(super) fn load_raw_display_metadata(path: &Path) -> Result<super::RawDisplayMetadata> {
    validate_input_file(path, MAX_RAW_FILE_BYTES, "RAW dimension input")?;
    let ctx = open_libraw(path)?;
    display_metadata_from_context(&ctx)
}

/// Reads the oriented dimensions and capture settings LibRaw parsed when the
/// file was opened; it does not touch the file again.
fn display_metadata_from_context(ctx: &LibRawContext) -> Result<super::RawDisplayMetadata> {
    let sizes = &ctx.data().rawdata.sizes;
    let other = &ctx.data().other;
    let width = u32::from(sizes.width);
    let height = u32::from(sizes.height);
    anyhow::ensure!(
        width > 0 && height > 0,
        "LibRaw header reports empty active dimensions"
    );

    let dimensions = match sizes.flip {
        5 | 6 => [height, width],
        _ => [width, height],
    };
    Ok(super::RawDisplayMetadata {
        aperture: finite_positive_or_zero(other.aperture),
        dimensions,
        iso_speed: finite_positive_or_zero(other.iso_speed),
        shutter_seconds: finite_positive_or_zero(other.shutter),
        focal_length: finite_positive_or_zero(other.focal_len),
    })
}

fn open_libraw(path: &Path) -> Result<LibRawContext> {
    let ctx = LibRawContext::new()?;
    open_libraw_file(&ctx, path, "open RAW thumbnail")?;
    validate_opened_thumbnail_geometry(&ctx)?;
    Ok(ctx)
}

pub(super) fn validate_input_file(path: &Path, maximum_bytes: u64, label: &str) -> Result<()> {
    let source =
        fs::metadata(path).with_context(|| format!("inspect {label} {}", path.display()))?;
    anyhow::ensure!(source.is_file(), "{label} is not a regular file");
    anyhow::ensure!(source.len() > 0, "{label} is empty");
    anyhow::ensure!(
        source.len() <= maximum_bytes,
        "{label} is {} bytes; the safe input limit is {maximum_bytes}",
        source.len()
    );
    Ok(())
}

pub(super) fn read_optional_profile(path: &Path) -> Result<Option<DcpProfile>> {
    DcpProfile::from_path(path)
        .with_context(|| format!("read embedded DCP profile {}", path.display()))
}

fn load_raw_file_with_selected_profile(
    path: &Path,
    signature: EmbeddedSignature,
    dcp_profile: Option<DcpProfile>,
    profile_source: Option<PathBuf>,
) -> Result<LoadedRaw> {
    validate_input_file(path, MAX_RAW_FILE_BYTES, "RAW input")?;
    // Every file read happens here; unpacking ends with the payload in memory.
    let (source_metadata, ctx) = crate::serialized_reads::run(|| -> Result<_> {
        let source_metadata = read_exif_capture_metadata_or_default(path);

        let ctx = LibRawContext::new()?;

        open_libraw_file(&ctx, path, "open RAW file")?;
        validate_opened_raw_geometry(&ctx)?;
        // SAFETY: `ctx` is a live context with a file opened (type invariant).
        check_libraw(unsafe { ffi::libraw_unpack(ctx.raw) }, "unpack RAW file")?;
        Ok((source_metadata, ctx))
    })?;

    let mut loaded = loaded_raw_from_context(&ctx, signature, dcp_profile, profile_source)?;
    merge_exif_capture_metadata(&mut loaded.capture_metadata, source_metadata);
    Ok(loaded)
}

/// Unpacks the stored samples of a DNG's main image for a payload Rawler
/// cannot decode, without any LibRaw colour processing. Returns `cpp` samples
/// per pixel (1 for CFA, 3 for LinearRaw) in file order over the full
/// `[width, height]` image, before black subtraction, cropping and
/// orientation. A LinearizationTable is already applied, as Rawler does.
pub(super) fn unpack_dng_samples(
    path: &Path,
    [width, height]: [usize; 2],
    cpp: usize,
) -> Result<Vec<u16>> {
    validate_input_file(path, MAX_RAW_FILE_BYTES, "RAW input")?;
    let ctx = crate::serialized_reads::run(|| -> Result<_> {
        let ctx = LibRawContext::new()?;
        open_libraw_file(&ctx, path, "open DNG")?;
        validate_opened_raw_geometry(&ctx)?;
        // SAFETY: `ctx` is a live context with a file opened (type invariant).
        check_libraw(unsafe { ffi::libraw_unpack(ctx.raw) }, "unpack DNG")?;
        Ok(ctx)
    })?;
    let rawdata = &ctx.data().rawdata;
    let sizes = &rawdata.sizes;
    anyhow::ensure!(
        usize::from(sizes.raw_width) == width && usize::from(sizes.raw_height) == height,
        "LibRaw unpacked {}x{} DNG samples; expected {width}x{height}",
        sizes.raw_width,
        sizes.raw_height
    );
    // LibRaw keeps one sample per pixel in `raw_image`, and three-sample
    // images in `color3_image` or, padded to four, `color4_image`.
    let (base, stride) = match cpp {
        1 => (rawdata.raw_image, 1),
        3 if !rawdata.color3_image.is_null() => (rawdata.color3_image.cast::<u16>(), 3),
        3 => (rawdata.color4_image.cast::<u16>(), 4),
        _ => anyhow::bail!("unsupported {cpp}-sample DNG"),
    };
    anyhow::ensure!(
        !base.is_null(),
        "LibRaw did not unpack {cpp}-sample DNG pixels"
    );
    let row_samples = width * stride;
    let pitch = match sizes.raw_pitch as usize {
        0 => row_samples,
        bytes => bytes / std::mem::size_of::<u16>(),
    };
    anyhow::ensure!(
        pitch >= row_samples,
        "LibRaw raw_pitch is smaller than one unpacked row"
    );
    // SAFETY: a non-null pixel buffer is the one `libraw_unpack` allocated:
    // `raw_height` rows of `raw_pitch` bytes (`raw_width * stride` samples when
    // the pitch is 0), owned by `ctx`, which outlives this borrow. The length
    // ends at the last row's final sample, inside that allocation.
    let buffer = unsafe { std::slice::from_raw_parts(base, (height - 1) * pitch + row_samples) };
    let mut samples = Vec::new();
    samples
        .try_reserve_exact(width * height * cpp)
        .context("reserve unpacked DNG samples")?;
    for row in buffer.chunks(pitch) {
        for pixel in row[..row_samples].chunks_exact(stride) {
            samples.extend_from_slice(&pixel[..cpp]);
        }
    }
    Ok(samples)
}

pub(super) fn read_exif_capture_metadata(path: &Path) -> Result<super::CaptureMetadata> {
    // LibRaw and Rawler report the numeric capture fields themselves; EXIF
    // only supplies the verbatim dates and the flash state.
    let capture = crate::pipeline::exif_metadata::ExifSummary::read(path)?.capture;
    Ok(super::CaptureMetadata {
        exif_dates: capture.exif_dates,
        flash: capture.flash,
        ..Default::default()
    })
}

/// Lets the EXIF flash state and dates read by `read_exif_capture_metadata`
/// take precedence over LibRaw's; an EXIF date replaces LibRaw's for its tag.
fn merge_exif_capture_metadata(capture: &mut super::CaptureMetadata, exif: super::CaptureMetadata) {
    capture.flash = exif.flash.or(capture.flash);
    for (tag, value) in exif.exif_dates {
        capture.exif_dates.retain(|(existing, _)| *existing != tag);
        capture.exif_dates.push((tag, value));
    }
}

pub(super) fn read_exif_capture_metadata_or_default(path: &Path) -> super::CaptureMetadata {
    read_exif_capture_metadata(path).unwrap_or_else(|error| {
        log::warn!(
            "could not read EXIF metadata from {}: {error:#}",
            path.display()
        );
        crate::diagnostics::record(format!(
            "EXIF metadata fallback used for {}: {error:#}",
            path.display()
        ));
        Default::default()
    })
}

fn open_libraw_file(ctx: &LibRawContext, path: &Path, action: &str) -> Result<()> {
    // Windows' narrow API uses the current code page, not UTF-8. Pass the
    // native UTF-16 path so every LibRaw caller can open Unicode filenames.
    #[cfg(windows)]
    let result = {
        let wide_path = path_to_libraw_wide(path)?;
        // SAFETY: `ctx` is a live context (type invariant) and `wide_path` is
        // NUL-terminated and outlives the call.
        unsafe { ffi::libraw_open_wfile(ctx.raw, wide_path.as_ptr()) }
    };
    #[cfg(not(windows))]
    let result = {
        let c_path = path_to_libraw_cstring(path)?;
        // SAFETY: `ctx` is a live context (type invariant) and `c_path` is
        // NUL-terminated and outlives the call.
        unsafe { ffi::libraw_open_file(ctx.raw, c_path.as_ptr()) }
    };
    check_libraw(result, action)
}

#[cfg(windows)]
fn path_to_libraw_wide(path: &Path) -> Result<Vec<u16>> {
    let mut wide_path: Vec<u16> = path.as_os_str().encode_wide().collect();
    if wide_path.contains(&0) {
        return Err(anyhow!(
            "RAW path contains an interior NUL code unit: {}",
            path.display()
        ));
    }
    wide_path.push(0);
    Ok(wide_path)
}

#[cfg(unix)]
fn path_to_libraw_cstring(path: &Path) -> Result<CString> {
    CString::new(path.as_os_str().as_bytes())
        .with_context(|| format!("RAW path contains an interior NUL byte: {}", path.display()))
}

#[cfg(not(any(unix, windows)))]
fn path_to_libraw_cstring(path: &Path) -> Result<CString> {
    let utf8 = path.to_str().with_context(|| {
        format!(
            "LibRaw requires a Unicode path on this platform: {}",
            path.display()
        )
    })?;
    CString::new(utf8.as_bytes())
        .with_context(|| format!("RAW path contains an interior NUL byte: {}", path.display()))
}

/// One LibRaw processor. `raw` is the non-null context `libraw_init` returned,
/// owned exclusively by this value and closed when it drops, so it is valid
/// for every LibRaw call made through it until then. The type is neither
/// `Send` nor `Sync`, so only the creating thread uses it.
struct LibRawContext {
    raw: *mut ffi::libraw_data_t,
}

impl LibRawContext {
    fn new() -> Result<Self> {
        // SAFETY: `libraw_init` has no preconditions; a null result is handled.
        let raw = unsafe { ffi::libraw_init(0) };
        if raw.is_null() {
            Err(anyhow!("libraw_init returned null"))
        } else {
            Ok(Self { raw })
        }
    }

    /// The fields LibRaw has filled in so far. Do not hold the reference
    /// across a LibRaw call that writes the context, such as open or unpack.
    fn data(&self) -> &ffi::libraw_data_t {
        // SAFETY: `raw` is valid and initialized for the lifetime of `self`
        // (type invariant).
        unsafe { &*self.raw }
    }

    /// Mutable access, for processing parameters set before a LibRaw call.
    fn data_mut(&mut self) -> &mut ffi::libraw_data_t {
        // SAFETY: as in `data`; `&mut self` makes this the only reference.
        unsafe { &mut *self.raw }
    }
}

impl Drop for LibRawContext {
    fn drop(&mut self) {
        // SAFETY: `raw` came from `libraw_init` and is closed exactly once.
        unsafe {
            ffi::libraw_close(self.raw);
        }
    }
}

fn validate_opened_raw_geometry(ctx: &LibRawContext) -> Result<()> {
    let sizes = &ctx.data().rawdata.sizes;
    let active_width = sizes.width as u32;
    let active_height = sizes.height as u32;
    validate_raw_dimensions(active_width, active_height)
        .context("LibRaw header reports an image too large to unpack safely")?;

    let sensor_width = sizes.raw_width as u32;
    let sensor_height = sizes.raw_height as u32;
    anyhow::ensure!(
        sensor_width > 0 && sensor_height > 0,
        "LibRaw header reports empty sensor dimensions"
    );
    anyhow::ensure!(
            sensor_width <= MAX_SENSOR_EDGE && sensor_height <= MAX_SENSOR_EDGE,
            "LibRaw sensor dimensions {sensor_width}x{sensor_height} exceed the {MAX_SENSOR_EDGE}-pixel edge limit"
        );
    let sensor_pixels = u64::from(sensor_width)
        .checked_mul(u64::from(sensor_height))
        .context("LibRaw sensor pixel count overflow")?;
    anyhow::ensure!(
            sensor_pixels <= MAX_SENSOR_PIXELS,
            "LibRaw sensor dimensions {sensor_width}x{sensor_height} contain {sensor_pixels} pixels; the safe unpack limit is {MAX_SENSOR_PIXELS}"
        );
    let minimum_pitch = u64::from(sensor_width)
        .checked_mul(std::mem::size_of::<u16>() as u64)
        .context("LibRaw sensor pitch overflow")?;
    let raw_pitch = u64::from(sizes.raw_pitch);
    anyhow::ensure!(
        raw_pitch == 0 || (raw_pitch >= minimum_pitch && raw_pitch <= 1_073_741_824),
        "LibRaw header reports invalid raw pitch {raw_pitch} for width {sensor_width}"
    );
    Ok(())
}

/// Builds the `LoadedRaw` from a context that has been unpacked, with
/// `dcp_profile`, read from `profile_source` and matched against
/// `signature`, or the RAW's own colour data.
fn loaded_raw_from_context(
    ctx: &LibRawContext,
    signature: EmbeddedSignature,
    dcp_profile: Option<DcpProfile>,
    profile_source: Option<PathBuf>,
) -> Result<LoadedRaw> {
    let raw = ctx.data();
    let rawdata = &raw.rawdata;
    let sizes = &rawdata.sizes;
    let color = &rawdata.color;
    let iparams = &rawdata.iparams;
    let lens = &raw.lens;
    let other = &raw.other;

    if rawdata.raw_image.is_null() {
        return Err(anyhow!(
            "LibRaw did not expose a single-channel raw_image buffer"
        ));
    }

    let raw_width = sizes.raw_width as u32;
    let raw_height = sizes.raw_height as u32;
    let crop_x = sizes.left_margin as u32;
    let crop_y = sizes.top_margin as u32;
    let width = sizes.width as u32;
    let height = sizes.height as u32;
    validate_raw_dimensions(width, height)
        .context("LibRaw reported an image too large to process safely")?;
    if !sizes.pixel_aspect.is_finite() || sizes.pixel_aspect <= 0.0 {
        return Err(anyhow!(
            "LibRaw reported invalid pixel aspect ratio {}",
            sizes.pixel_aspect
        ));
    }
    if (sizes.pixel_aspect - 1.0).abs() > 1e-6 {
        return Err(anyhow!(
                "non-square RAW pixels (aspect {}) require a geometry-resampling stage that CalibRaw does not implement yet",
                sizes.pixel_aspect
            ));
    }
    if !matches!(sizes.flip, 0 | 3 | 5 | 6) {
        return Err(anyhow!(
            "unsupported LibRaw orientation code {}; expected 0, 3, 5, or 6",
            sizes.flip
        ));
    }

    let crop_right = crop_x
        .checked_add(width)
        .ok_or_else(|| anyhow!("LibRaw horizontal crop overflow"))?;
    let crop_bottom = crop_y
        .checked_add(height)
        .ok_or_else(|| anyhow!("LibRaw vertical crop overflow"))?;
    if crop_right > raw_width || crop_bottom > raw_height {
        return Err(anyhow!(
            "LibRaw crop is outside RAW bounds: crop {}x{} at {},{} in {}x{}",
            width,
            height,
            crop_x,
            crop_y,
            raw_width,
            raw_height
        ));
    }

    let cfa_kind = cfa_kind_from_filters(iparams.filters)?;
    let cdesc = cdesc4(iparams);
    let cfa_map = canonical_cfa_map(cdesc)?;
    let physical_black_levels = black_levels(color.black, &color.cblack);
    // SAFETY: a non-null `raw_image` is the buffer `libraw_unpack` allocated:
    // `raw_height` rows of `raw_pitch` bytes (or `raw_width` samples when the
    // pitch is 0), owned by `ctx`, which outlives the copy. `raw` belongs to it.
    let (width, height, raw_pixels, color_indices, black_levels_per_pixel) = unsafe {
        copy_active_pixels(ActivePixelCopy {
            raw: ctx.raw,
            raw_image: rawdata.raw_image,
            raw_dimensions: [raw_width, raw_height],
            crop_origin: [crop_x, crop_y],
            dimensions: [width, height],
            raw_pitch: sizes.raw_pitch as usize,
            flip: sizes.flip,
            cfa_kind,
            cdesc,
            cfa_map,
            shared_black: color.black,
            cblack: &color.cblack,
        })
    }?;
    let physical_wb = white_balance(color.cam_mul, cdesc);
    let wb_coeffs = canonicalize_f32x4(physical_wb, cfa_map);
    let color_source = ProfileColorSource::new(
        &c_array_to_string(&iparams.make),
        &c_array_to_string(&iparams.model),
        signature,
        DecoderColor::LibRaw(Box::new(LibRawColorInputs {
            color: LibRawColor::from_libraw(color),
            physical_wb,
            cdesc,
            baseline_exposure: valid_baseline_exposure(color.dng_levels.baseline_exposure),
            embedded_icc: embedded_camera_icc(color)?,
        })),
    );
    let profile_color = color_source.resolve(dcp_profile, profile_source)?;
    let black_levels = canonicalize_f32x4(physical_black_levels, cfa_map);
    let linear_max = color.linear_max.map(normalize_libraw_linear_max);
    let white_levels = saturation_adjusted_white_levels(
        canonicalize_f32x4(
            white_levels(color.maximum, linear_max, physical_black_levels),
            cfa_map,
        ),
        black_levels,
        &raw_pixels,
    );
    validate_sensor_levels(white_levels, black_levels, &raw_pixels)?;
    let sensor_saturation =
        sensor_saturation(white_levels, black_levels, color.maximum, &raw_pixels);
    let noise_profile = NoiseProfile::estimate(
        width,
        height,
        &raw_pixels,
        &color_indices,
        &black_levels_per_pixel,
        white_levels,
        finite_positive_or_zero(other.iso_speed),
        match cfa_kind {
            CfaKind::Bayer => 2,
            CfaKind::XTrans => 6,
        },
    );

    let mut loaded = LoadedRaw {
        width,
        height,
        source_extent: [width, height],
        camera_make: c_array_to_string(&iparams.make),
        camera_model: c_array_to_string(&iparams.model),
        lens_make: c_array_to_string(&lens.LensMake),
        lens_model: c_array_to_string(&lens.Lens),
        focal_length: finite_positive_or_zero(other.focal_len),
        aperture: finite_positive_or_zero(other.aperture),
        focus_distance: 0.0,
        capture_metadata: super::CaptureMetadata {
            // `time_t` is `c_long`, so its width is platform-dependent: 32-bit on
            // Windows/LLP32 targets, 64-bit on LP64. The cast normalises it to i64
            // on every platform, so it cannot be dropped even when it is a no-op.
            #[allow(clippy::unnecessary_cast)] // Width-normalising cast, not a numeric conversion.
            exif_dates: capture_dates_from_timestamp(other.timestamp as i64),
            iso_speed: finite_positive_or_zero(other.iso_speed),
            shutter_seconds: finite_positive_or_zero(other.shutter),
            flash: (color.flash_used.is_finite() && color.flash_used > 0.0).then_some(1),
            description: c_array_to_string(&other.desc),
            artist: c_array_to_string(&other.artist),
        },
        cfa_kind,
        raw_pixels: raw_pixels.into(),
        scene_linear_raster: None,
        scene_referred_raster: false,
        color_indices,
        wb_coeffs,
        black_levels,
        black_levels_per_pixel,
        white_levels,
        sensor_saturation,
        noise_profile,
        profile_color: Arc::new(profile_color),
        color_source: Some(Arc::new(color_source)),
        available_camera_profiles: Vec::new(),
        lens_geometry: None,
        lens_shading: None,
        lateral_chromatic_aberration: LateralChromaticAberration::NONE,
        ai_denoised: Arc::new(std::sync::RwLock::new(None)),
        opposed_chroma_cache: Default::default(),
        opposed_chroma_source_identity: Default::default(),
        opposed_chroma_reference_source: true,
    };
    loaded.lateral_chromatic_aberration = LateralChromaticAberration::estimate(&loaded);
    Ok(loaded)
}

/// A copy of the camera ICC profile LibRaw read from the RAW, if any.
fn embedded_camera_icc(color: &ffi::libraw_colordata_t) -> Result<Option<Vec<u8>>> {
    if color.profile.is_null() || color.profile_length == 0 {
        return Ok(None);
    }
    let length = usize::try_from(color.profile_length).unwrap_or(0);
    if length > 16 * 1024 * 1024 {
        log::warn!("ignoring embedded camera ICC profile larger than 16 MiB");
        return Ok(None);
    }
    // SAFETY: a non-null `profile` is LibRaw's copy of the embedded ICC
    // profile, `profile_length` bytes owned by the context `color` belongs
    // to, which outlives this borrow.
    let source = unsafe { std::slice::from_raw_parts(color.profile as *const u8, length) };
    let mut profile = Vec::new();
    profile
        .try_reserve_exact(length)
        .context("reserve embedded camera ICC profile")?;
    profile.extend_from_slice(source);
    Ok(Some(profile))
}

fn c_array_to_string(value: &[c_char]) -> String {
    let bytes: Vec<u8> = value
        .iter()
        .copied()
        .take_while(|value| *value != 0)
        .map(c_char_as_u8)
        .collect();
    String::from_utf8_lossy(&bytes).trim().to_owned()
}

#[cfg(target_os = "android")]
fn c_char_as_u8(value: c_char) -> u8 {
    value
}

#[cfg(not(target_os = "android"))]
fn c_char_as_u8(value: c_char) -> u8 {
    value as u8
}

fn finite_positive_or_zero(value: f32) -> f32 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        0.0
    }
}

fn check_libraw(err: i32, action: &str) -> Result<()> {
    const LIBRAW_UNSUPPORTED_FILE: i32 = -2;

    if err == 0 {
        return Ok(());
    }

    // SAFETY: `libraw_strerror` returns null or a static NUL-terminated string.
    let message = unsafe {
        let ptr = ffi::libraw_strerror(err);
        if ptr.is_null() {
            "unknown LibRaw error".into()
        } else {
            CStr::from_ptr(ptr).to_string_lossy().into_owned()
        }
    };

    let detail = format!("LibRaw failed to {action}: {message} ({err})");
    if err == LIBRAW_UNSUPPORTED_FILE {
        Err(anyhow::Error::new(super::UnsupportedRawFormat { detail }))
    } else {
        Err(anyhow!("{detail}"))
    }
}

/// LibRaw represents camera wall-clock time as seconds from the Unix epoch.
/// Convert without applying the exporting computer's timezone.
fn capture_dates_from_timestamp(timestamp: i64) -> Vec<(u16, String)> {
    if timestamp <= 0 || timestamp > 253_402_300_799 {
        return Vec::new();
    }
    // Gregorian civil date from days since 1970-01-01.
    let days = timestamp / 86_400 + 719_468;
    let era = days / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = month_index + if month_index < 10 { 3 } else { -9 };
    let year = year + i64::from(month <= 2);
    let hour = timestamp % 86_400 / 3600;
    let minute = timestamp % 3600 / 60;
    let second = timestamp % 60;
    let date = format!("{year:04}:{month:02}:{day:02} {hour:02}:{minute:02}:{second:02}");
    vec![(0x9003, date.clone()), (0x9004, date)]
}

#[cfg(test)]
mod tests {

    #[test]
    fn libraw_opens_paths_with_spaces_and_unicode() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("Thorge Claußen ä ü ö 日本語 📷");
        std::fs::create_dir(&directory).unwrap();
        for name in ["IMG 5501.CR2", "Straße ÄÖÜ é 中文 🌄.CR2"] {
            let path = directory.join(name);
            // An existing non-RAW file must reach format identification rather
            // than fail to open. This exercises native file I/O without a large
            // camera fixture and catches code-page conversion on Windows.
            std::fs::write(&path, [0u8; 4096]).unwrap();
            let ctx = super::LibRawContext::new().unwrap();
            let error = super::open_libraw_file(&ctx, &path, "open RAW file").unwrap_err();
            assert!(
                error.is::<super::super::UnsupportedRawFormat>(),
                "{}: {error:#}",
                path.display()
            );
            drop(ctx);
            std::fs::remove_file(&path).unwrap();
            let missing_ctx = super::LibRawContext::new().unwrap();
            let missing_error =
                super::open_libraw_file(&missing_ctx, &path, "open RAW file").unwrap_err();
            assert!(!missing_error.is::<super::super::UnsupportedRawFormat>());
        }
    }

    #[cfg(windows)]
    #[test]
    fn libraw_wide_paths_preserve_native_code_units_and_reject_nul() {
        use std::ffi::OsString;
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        use std::path::Path;

        let path = Path::new(r"C:\Users\Thorge Claußen\ä ü ö 日本語 📷.CR2");
        let expected: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        assert_eq!(super::path_to_libraw_wide(path).unwrap(), expected);

        // Windows paths can contain unpaired surrogates; don't round-trip
        // through a Rust UTF-8 string and lose these native code units.
        let native_path = OsString::from_wide(&[0x61, 0xd800, 0x62]);
        assert_eq!(
            super::path_to_libraw_wide(Path::new(&native_path)).unwrap(),
            [0x61, 0xd800, 0x62, 0]
        );
        let invalid_path = OsString::from_wide(&[0x61, 0, 0x62]);
        assert!(super::path_to_libraw_wide(Path::new(&invalid_path)).is_err());
    }

    #[test]
    fn robertson_cct_matches_reference_illuminants() {
        for ([x, y], expected) in [
            ([0.3127, 0.3290], 6504.0),
            ([0.447_57, 0.407_45], 2856.0),
            ([0.3457, 0.3585], 5003.0),
        ] {
            let cct = super::xyz_to_cct([x / y, 1.0, (1.0 - x - y) / y]).unwrap();
            assert!((cct - expected).abs() < 15.0, "xy {x},{y}: {cct} K");
        }
    }

    #[test]
    fn reference_illuminants_have_lightroom_temperature_and_tint() {
        // D65 lies above the locus (greener), illuminant A on it.
        for ([x, y], expected_kelvin, expected_tint) in [
            ([0.312_72, 0.329_03], 6_504.0, 9.6),
            ([0.447_57, 0.407_45], 2_856.0, 0.0),
        ] {
            let xyz = [x / y, 1.0, (1.0 - x - y) / y];
            let (kelvin, tint) =
                super::uv_temperature_tint(super::xyz_to_uv(xyz).unwrap()).unwrap();
            assert!(
                (kelvin - expected_kelvin).abs() < 15.0,
                "xy {x},{y}: {kelvin} K"
            );
            assert!(
                (tint - expected_tint).abs() < 1.0,
                "xy {x},{y}: tint {tint}"
            );
        }
    }
}
