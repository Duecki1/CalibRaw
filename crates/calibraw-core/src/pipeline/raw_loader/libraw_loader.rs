// SPDX-License-Identifier: GPL-3.0-or-later
// Camera-matrix normalization and temperature/tint handling follow
// darktable 5.6.0 and the related Ansel/dcraw implementations.
// Copyright (C) 2010-2026 darktable developers and the Ansel/dcraw authors.
// Copyright (C) 2026 CalibRaw contributors (Rust adaptation).

use super::super::noise::NoiseProfile;
use super::{
    validate_raw_dimensions, CameraColorModel, CameraProfile, CameraProfileCandidate,
    CameraProfileMode, CameraWhiteBalanceModel, CfaKind, CompactPixelMap, DngColorEndpoint,
    LoadedRaw, RawThumbnail, MAX_RAW_FILE_BYTES, MAX_SENSOR_EDGE, MAX_SENSOR_PIXELS,
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
mod sensor;
mod temperature_tint;
mod thumbnail;
pub(super) use camera_matrix::*;
use cct::*;
pub(super) use dcp_index::*;
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

pub(super) fn resolve_camera_profiles(
    path: &Path,
    mode: CameraProfileMode,
    profile_folder: Option<&Path>,
    selected_profile: Option<&Path>,
    camera_make: &str,
    camera_model: &str,
) -> Result<(
    Option<DcpProfile>,
    Option<PathBuf>,
    Vec<CameraProfileCandidate>,
)> {
    let raw_camera_signature = match mode {
        CameraProfileMode::Automatic | CameraProfileMode::DcpProfiles => {
            read_optional_profile(path)?.and_then(|profile| profile.camera_calibration_signature)
        }
        CameraProfileMode::MatrixOnly => None,
    };
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
    let available_camera_profiles = matches
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
    let (selected_profile_path, selected_profile) = external_profile
        .map(|mut candidate| {
            candidate.profile.camera_calibration_signature = raw_camera_signature;
            (Some(candidate.path), Some(candidate.profile))
        })
        .unwrap_or((None, None));
    Ok((
        selected_profile,
        selected_profile_path,
        available_camera_profiles,
    ))
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
    let source_metadata = read_exif_capture_metadata_or_default(path);

    let ctx = LibRawContext::new()?;
    let identify_started = Instant::now();
    open_libraw_file(&ctx, path, "open RAW file")?;
    crate::diagnostics::record(format!(
        "LibRaw identify/open_file finished in {:.3}s",
        identify_started.elapsed().as_secs_f64()
    ));
    unsafe { validate_opened_raw_geometry(&ctx) }?;

    let (camera_make, camera_model) = unsafe {
        let iparams = &(*ctx.raw).rawdata.iparams;
        (
            c_array_to_string(&iparams.make),
            c_array_to_string(&iparams.model),
        )
    };

    let profile_started = Instant::now();
    let (selected_profile, selected_profile_path, available_camera_profiles) =
        resolve_camera_profiles(
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
    check_libraw(unsafe { ffi::libraw_unpack(ctx.raw) }, "unpack RAW file")?;
    crate::diagnostics::record(format!(
        "LibRaw sensor unpack finished in {:.3}s",
        unpack_started.elapsed().as_secs_f64()
    ));
    let materialize_started = Instant::now();
    let mut loaded = unsafe { loaded_raw_from_context(&ctx, selected_profile) }?;
    loaded.capture_metadata.flash = source_metadata.flash.or(loaded.capture_metadata.flash);
    for (tag, value) in source_metadata.exif_dates {
        loaded
            .capture_metadata
            .exif_dates
            .retain(|(existing, _)| *existing != tag);
        loaded.capture_metadata.exif_dates.push((tag, value));
    }
    crate::diagnostics::record(format!(
        "Decoded mosaic materialization finished in {:.3}s",
        materialize_started.elapsed().as_secs_f64()
    ));
    loaded.camera_profile_source = selected_profile_path;
    loaded.available_camera_profiles = available_camera_profiles;
    Ok(loaded)
}

pub(super) fn load_raw_file_with_dcp(path: &Path, profile_path: &Path) -> Result<LoadedRaw> {
    validate_input_file(path, MAX_RAW_FILE_BYTES, "RAW input")?;
    validate_input_file(profile_path, MAX_DCP_FILE_BYTES, "DCP profile")?;
    let mut selected = DcpProfile::from_path(profile_path)
        .with_context(|| format!("read DCP profile {}", profile_path.display()))?
        .ok_or_else(|| anyhow!("{} is not a DNG camera profile", profile_path.display()))?;

    if let Some(raw_profile) = read_optional_profile(path)? {
        selected.camera_calibration_signature = raw_profile.camera_calibration_signature;
    }
    let display_name = dcp_profile_display_name(selected.name.as_deref(), profile_path);
    let mut loaded = load_raw_file_with_selected_profile(path, Some(selected))?;
    loaded.camera_profile_source = Some(profile_path.to_path_buf());
    loaded.available_camera_profiles = vec![CameraProfileCandidate {
        path: profile_path.to_path_buf(),
        name: display_name,
    }];
    Ok(loaded)
}

pub(super) fn load_raw_display_metadata(path: &Path) -> Result<super::RawDisplayMetadata> {
    validate_input_file(path, MAX_RAW_FILE_BYTES, "RAW dimension input")?;
    let ctx = open_libraw(path)?;

    let sizes = unsafe { &(*ctx.raw).rawdata.sizes };
    let other = unsafe { &(*ctx.raw).other };
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
    unsafe { validate_opened_thumbnail_geometry(&ctx) }?;
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
    dcp_profile: Option<DcpProfile>,
) -> Result<LoadedRaw> {
    validate_input_file(path, MAX_RAW_FILE_BYTES, "RAW input")?;
    // Every file read happens here; unpacking ends with the payload in memory.
    let (source_metadata, ctx) = crate::serialized_reads::run(|| -> Result<_> {
        let source_metadata = read_exif_capture_metadata_or_default(path);

        let ctx = LibRawContext::new()?;

        open_libraw_file(&ctx, path, "open RAW file")?;
        unsafe { validate_opened_raw_geometry(&ctx) }?;
        check_libraw(unsafe { ffi::libraw_unpack(ctx.raw) }, "unpack RAW file")?;
        Ok((source_metadata, ctx))
    })?;

    let mut loaded = unsafe { loaded_raw_from_context(&ctx, dcp_profile) }?;
    loaded.capture_metadata.flash = source_metadata.flash.or(loaded.capture_metadata.flash);
    for (tag, value) in source_metadata.exif_dates {
        loaded
            .capture_metadata
            .exif_dates
            .retain(|(existing, _)| *existing != tag);
        loaded.capture_metadata.exif_dates.push((tag, value));
    }
    Ok(loaded)
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
        unsafe { ffi::libraw_open_wfile(ctx.raw, wide_path.as_ptr()) }
    };
    #[cfg(not(windows))]
    let result = {
        let c_path = path_to_libraw_cstring(path)?;
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

struct LibRawContext {
    raw: *mut ffi::libraw_data_t,
}

impl LibRawContext {
    fn new() -> Result<Self> {
        let raw = unsafe { ffi::libraw_init(0) };
        if raw.is_null() {
            Err(anyhow!("libraw_init returned null"))
        } else {
            Ok(Self { raw })
        }
    }
}

impl Drop for LibRawContext {
    fn drop(&mut self) {
        unsafe {
            ffi::libraw_close(self.raw);
        }
    }
}

unsafe fn validate_opened_raw_geometry(ctx: &LibRawContext) -> Result<()> {
    let raw = &*ctx.raw;
    let sizes = &raw.rawdata.sizes;
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

unsafe fn loaded_raw_from_context(
    ctx: &LibRawContext,
    dcp_profile: Option<DcpProfile>,
) -> Result<LoadedRaw> {
    let raw = &*ctx.raw;
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
    let (width, height, raw_pixels, color_indices, black_levels_per_pixel) =
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
        })?;
    let physical_wb = white_balance(color.cam_mul, cdesc);
    let wb_coeffs = canonicalize_f32x4(physical_wb, cfa_map);
    let calibration_compatible = dcp_profile
        .as_ref()
        .is_none_or(DcpProfile::calibration_is_compatible);
    let (cam_to_srgb, profile_weight, white_balance_model) = camera_to_working_matrix(
        color,
        physical_wb,
        cdesc,
        dcp_profile.as_ref(),
        calibration_compatible,
    )?;
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

    let mut camera_profile = dcp_profile
        .map(|profile| CameraProfile::from_dcp(profile, profile_weight))
        .unwrap_or_default();
    let baseline_exposure = valid_baseline_exposure(color.dng_levels.baseline_exposure);
    apply_resolved_default_exposure(&mut camera_profile, baseline_exposure);
    if !color.profile.is_null() && color.profile_length > 0 {
        let length = usize::try_from(color.profile_length).unwrap_or(0);
        if length <= 16 * 1024 * 1024 {
            let source = std::slice::from_raw_parts(color.profile as *const u8, length);
            let mut profile = Vec::new();
            profile
                .try_reserve_exact(length)
                .context("reserve embedded camera ICC profile")?;
            profile.extend_from_slice(source);
            camera_profile.embedded_camera_icc = Some(profile);
        } else {
            log::warn!("ignoring embedded camera ICC profile larger than 16 MiB");
        }
    }

    Ok(LoadedRaw {
        width,
        height,
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
        raw_pixels,
        scene_linear_raster: None,
        color_indices,
        wb_coeffs,
        cam_to_srgb,
        black_levels,
        black_levels_per_pixel,
        white_levels,
        noise_profile,
        camera_profile,
        camera_profile_source: None,
        available_camera_profiles: Vec::new(),
        white_balance_model,
        lens_geometry: None,
        ai_denoised: Arc::new(std::sync::RwLock::new(None)),
        opposed_chroma_cache: Default::default(),
        opposed_chroma_source_identity: Default::default(),
        opposed_chroma_reference_source: true,
    })
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
    use super::{
        adjusted_white_balance_coefficients, apply_resolved_default_exposure, black_levels,
        cam_to_working, canonical_cfa_map, canonicalize_f32x4, cfa_kind_from_filters,
        daylight_white_balance, effective_black_level, identity_4x4, identity_fallback_4x4,
        matching_thumbnail_orientation, oriented_source_pos, resolve_default_exposure_ev,
        saturation_adjusted_white_levels, smaller_covering_preview_index, valid_baseline_exposure,
        validate_embedded_thumbnail_metadata, white_balance, white_levels, CameraColorModel,
        CameraProfile, CameraWhiteBalanceModel, CfaKind, DcpMatrixSet, DcpProfile,
        DngColorEndpoint, EmbeddedPreview, MAX_EMBEDDED_THUMBNAIL_BYTES,
        MISSING_BASELINE_EXPOSURE_FALLBACK_EV,
    };
    use crate::matrix;

    #[test]
    fn capture_timestamp_preserves_camera_calendar_time() {
        for (timestamp, date) in [
            (1, "1970:01:01 00:00:01"),
            (951_827_696, "2000:02:29 12:34:56"),
            (1_704_067_200, "2024:01:01 00:00:00"),
        ] {
            assert_eq!(
                super::capture_dates_from_timestamp(timestamp),
                vec![(0x9003, date.to_owned()), (0x9004, date.to_owned())]
            );
        }
        assert!(super::capture_dates_from_timestamp(0).is_empty());
    }

    const RGBG: [u8; 4] = *b"RGBG";

    fn white_balance_test_model() -> CameraWhiteBalanceModel {
        // Off-diagonal terms are essential: an identity matrix never exposes
        // the negative camera responses responsible for the magenta-end reset.
        let mut model = CameraWhiteBalanceModel {
            base_wb: [1.0; 4],
            cdesc: RGBG,
            base_cct: 5_000.0,
            color: CameraColorModel::Matrix {
                xyz_to_camera: [
                    [0.65, -0.2, -0.1],
                    [-0.2, 1.1, 0.1],
                    [0.05, -0.3, 0.9],
                    [0.0; 3],
                ],
            },
        };
        model.base_wb = super::temperature_tint_to_coefficients(&model, 5_000.0, 1.0).unwrap();
        model
    }

    #[test]
    fn white_balance_magenta_endpoint_stays_valid_instead_of_resetting() {
        let model = white_balance_test_model();
        assert!(super::temperature_tint_to_coefficients(
            &model,
            5_000.0,
            super::MIN_WHITE_BALANCE_TINT,
        )
        .is_none());
        let range = super::white_balance_tint_range(&model, 5_000.0).unwrap();
        assert!(*range.start() > super::MIN_WHITE_BALANCE_TINT);
        let magenta =
            super::temperature_tint_to_coefficients(&model, 5_000.0, *range.start()).unwrap();
        let neutral = super::temperature_tint_to_coefficients(&model, 5_000.0, 1.0).unwrap();
        let green = super::temperature_tint_to_coefficients(&model, 5_000.0, *range.end()).unwrap();
        for channel in [0, 2] {
            assert!(magenta[channel] > neutral[channel]);
            assert!(green[channel] < neutral[channel]);
        }
        let (base_temperature, base_tint) =
            super::temperature_tint_from_coefficients(&model, model.base_wb).unwrap();
        let actual = adjusted_white_balance_coefficients(
            &model,
            crate::pipeline::temperature_offset_from_kelvin(base_temperature, 5_000.0),
            crate::pipeline::white_balance_tint_offset(base_tint, super::MIN_WHITE_BALANCE_TINT),
        )
        .expect("old out-of-range edits must clamp instead of losing WB");
        for channel in 0..4 {
            assert!((actual[channel] - magenta[channel]).abs() < 0.01);
        }
    }

    #[test]
    fn white_balance_tint_ranges_are_valid_and_monotonic_throughout() {
        let model = white_balance_test_model();
        for temperature in [
            1_901.0, 2_222.0, 3_999.0, 4_000.0, 5_000.0, 7_000.0, 25_000.0,
        ] {
            let range = super::white_balance_tint_range(&model, temperature).unwrap();
            let mut previous = [f32::INFINITY; 4];
            for tick in ((*range.start() * 1_000.0).round() as i32)
                ..=((*range.end() * 1_000.0).round() as i32)
            {
                let tint = tick as f32 / 1_000.0;
                let wb = super::temperature_tint_to_coefficients(&model, temperature, tint)
                    .unwrap_or_else(|| panic!("invalid {temperature} K / {tint}"));
                assert!(wb.iter().all(|gain| gain.is_finite() && *gain > 0.0));
                for channel in [0, 2] {
                    assert!(
                        wb[channel] < previous[channel],
                        "dead/reversed tint at {temperature} / {tint}"
                    );
                }
                previous = wb;
            }
        }
    }

    #[test]
    fn white_balance_temperature_ranges_stop_before_invalid_responses() {
        let model = white_balance_test_model();
        for tint in [0.4, 0.8, 1.0, 2.326] {
            let range = super::white_balance_temperature_range(&model, 5_000.0, tint).unwrap();
            if tint == 0.4 {
                assert!(*range.start() > super::MIN_TEMPERATURE_KELVIN);
                assert!(*range.end() < super::MAX_TEMPERATURE_KELVIN);
            }
            let mut previous = None;
            for kelvin in (*range.start() as u32)..=(*range.end() as u32) {
                let wb = super::temperature_tint_to_coefficients(&model, kelvin as f32, tint)
                    .unwrap_or_else(|| panic!("invalid {kelvin} K / {tint}"));
                assert!(wb.iter().all(|gain| gain.is_finite() && *gain > 0.0));
                assert_ne!(Some(wb), previous, "dead temperature step at {kelvin} K");
                previous = Some(wb);
            }
            for outside in [range.start() - 1.0, range.end() + 1.0] {
                if (super::MIN_TEMPERATURE_KELVIN..=super::MAX_TEMPERATURE_KELVIN)
                    .contains(&outside)
                {
                    assert!(
                        super::temperature_tint_to_coefficients(&model, outside, tint).is_none()
                    );
                }
            }
        }
    }

    #[test]
    fn white_balance_limits_also_apply_to_dng_matrices() {
        let mut model = white_balance_test_model();
        let CameraColorModel::Matrix { xyz_to_camera } = model.color else {
            unreachable!();
        };
        let matrix_range = super::white_balance_tint_range(&model, 5_000.0).unwrap();
        let endpoint = DngColorEndpoint {
            cct: Some(6_504.0),
            color_matrix: xyz_to_camera,
            calibration: identity_4x4(),
            forward_matrix: None,
        };
        model.color = CameraColorModel::Dng {
            endpoints: Box::new([endpoint; 2]),
            analog_balance: identity_4x4(),
        };
        assert_eq!(
            super::white_balance_tint_range(&model, 5_000.0).unwrap(),
            matrix_range
        );
    }

    #[test]
    fn white_balance_green_endpoint_is_limited_when_its_response_goes_negative() {
        let mut model = white_balance_test_model();
        let CameraColorModel::Matrix { xyz_to_camera } = &mut model.color else {
            unreachable!();
        };
        xyz_to_camera[1] = [-0.4, 1.0, -0.4];
        assert!(super::temperature_tint_to_coefficients(&model, 5_000.0, 2.326).is_none());
        let range = super::white_balance_tint_range(&model, 5_000.0).unwrap();
        assert!(*range.end() < super::MAX_WHITE_BALANCE_TINT);
        let clamped = super::clamp_white_balance_temperature_tint(&model, 5_000.0, 2.326).unwrap();
        assert_eq!(clamped.1, *range.end());
        assert!(super::temperature_tint_to_coefficients(&model, clamped.0, clamped.1).is_some());
    }

    #[test]
    fn white_balance_limits_keep_the_full_supported_range_for_positive_matrices() {
        let mut model = white_balance_test_model();
        model.color = CameraColorModel::Matrix {
            xyz_to_camera: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.0; 3]],
        };
        for temperature in [
            super::MIN_TEMPERATURE_KELVIN,
            5_000.0,
            super::MAX_TEMPERATURE_KELVIN,
        ] {
            assert_eq!(
                super::white_balance_tint_range(&model, temperature).unwrap(),
                super::MIN_WHITE_BALANCE_TINT..=super::MAX_WHITE_BALANCE_TINT,
            );
        }
        for tint in [
            super::MIN_WHITE_BALANCE_TINT,
            1.0,
            super::MAX_WHITE_BALANCE_TINT,
        ] {
            assert_eq!(
                super::white_balance_temperature_range(&model, 5_000.0, tint).unwrap(),
                super::MIN_TEMPERATURE_KELVIN..=super::MAX_TEMPERATURE_KELVIN,
            );
        }
    }

    #[test]
    fn white_balance_invalid_second_green_is_not_treated_as_missing() {
        let mut model = white_balance_test_model();
        let CameraColorModel::Matrix { xyz_to_camera } = &mut model.color else {
            unreachable!();
        };
        xyz_to_camera[3] = [1.0, -0.5, 0.0];
        assert!(super::temperature_tint_to_coefficients(&model, 5_000.0, 0.45).is_none());
        assert!(
            *super::white_balance_tint_range(&model, 5_000.0)
                .unwrap()
                .start()
                > 0.5
        );
    }

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
    fn default_render_exposure_prefers_dng_baseline_and_combines_profile_offset_once() {
        assert_eq!(valid_baseline_exposure(-1000.0), None);
        assert_eq!(valid_baseline_exposure(f32::NAN), None);
        assert_eq!(valid_baseline_exposure(-0.35), Some(-0.35));
        assert!((resolve_default_exposure_ev(Some(-0.35), 0.20) + 0.15).abs() < 1e-6);
        assert!(
            (resolve_default_exposure_ev(None, 0.0) - MISSING_BASELINE_EXPOSURE_FALLBACK_EV).abs()
                < 1e-6
        );
        assert!(
            (resolve_default_exposure_ev(None, 0.10)
                - (MISSING_BASELINE_EXPOSURE_FALLBACK_EV + 0.10))
                .abs()
                < 1e-6
        );
    }

    #[test]
    fn resolved_default_exposure_updates_the_profile_without_accumulating() {
        let mut profile = CameraProfile {
            profile_exposure_offset_ev: 0.2,
            ..CameraProfile::default()
        };

        apply_resolved_default_exposure(&mut profile, Some(-0.35));
        assert!((profile.default_exposure_ev + 0.15).abs() < 1e-6);

        apply_resolved_default_exposure(&mut profile, Some(-0.35));
        assert!((profile.default_exposure_ev + 0.15).abs() < 1e-6);
    }

    #[test]
    fn embedded_thumbnail_orientation_uses_the_matching_preview_metadata() {
        let selected = (1600, 1067, 412_345);
        let candidates = [
            (160, 107, 12_345, 3),
            (1600, 1067, 412_345, u16::MAX),
            (1600, 1067, 412_344, 5),
            (1600, 1067, 412_345, 6),
        ];

        assert_eq!(
            matching_thumbnail_orientation(selected, candidates),
            Some(6)
        );
        assert_eq!(
            matching_thumbnail_orientation(selected, [(1600, 1067, 412_345, u16::MAX)]),
            None
        );
    }

    #[test]
    fn embedded_thumbnail_metadata_is_bounded() {
        assert!(validate_embedded_thumbnail_metadata(
            super::ffi::LibRaw_thumbnail_formats_LIBRAW_THUMBNAIL_JPEG,
            1600,
            1067,
            2_000_000,
            3,
        )
        .is_ok());
        assert!(validate_embedded_thumbnail_metadata(
            super::ffi::LibRaw_thumbnail_formats_LIBRAW_THUMBNAIL_JPEG,
            1600,
            1067,
            u32::try_from(MAX_EMBEDDED_THUMBNAIL_BYTES + 1).unwrap(),
            3,
        )
        .is_err());
        assert!(validate_embedded_thumbnail_metadata(
            super::ffi::LibRaw_thumbnail_formats_LIBRAW_THUMBNAIL_BITMAP,
            1000,
            1000,
            1_000_000,
            3,
        )
        .is_err());
    }

    #[test]
    fn the_smallest_matching_preview_that_covers_the_edge_is_chosen() {
        let jpeg = |width, height| EmbeddedPreview {
            jpeg: true,
            dimensions: [width, height],
            length: 100_000,
        };
        // Sony ARW: medium preview, tiny thumbnail and full-size JPEG.
        let sony = [jpeg(1616, 1080), jpeg(160, 120), jpeg(7008, 4672)];
        assert_eq!(
            smaller_covering_preview_index([7008, 4672], sony, 512),
            Some(0)
        );
        // Nothing smaller covers a larger request, so LibRaw's default stays.
        assert_eq!(
            smaller_covering_preview_index([7008, 4672], sony, 2048),
            None
        );
        // Rotated dimensions still match the default's aspect ratio.
        assert_eq!(
            smaller_covering_preview_index([7008, 4672], [jpeg(1080, 1616)], 512),
            Some(0)
        );
        // A letterboxed 4:3 preview of a 3:2 image is never chosen.
        assert_eq!(
            smaller_covering_preview_index([6000, 4000], [jpeg(1024, 768)], 512),
            None
        );
        // Non-JPEG, empty and unknown-size previews are skipped.
        let bitmap = EmbeddedPreview {
            jpeg: false,
            ..jpeg(1500, 1000)
        };
        let empty = EmbeddedPreview {
            length: 0,
            ..jpeg(1500, 1000)
        };
        assert_eq!(
            smaller_covering_preview_index(
                [6000, 4000],
                [bitmap, empty, jpeg(0, 0), jpeg(900, 600)],
                512
            ),
            Some(3)
        );
        // Unknown default dimensions keep LibRaw's choice.
        assert_eq!(
            smaller_covering_preview_index([0, 0], [jpeg(1500, 1000)], 512),
            None
        );
    }

    #[test]
    fn global_wb_changes_coefficients_without_reinterpolating_camera_data() {
        let endpoint = |cct, red_scale, blue_scale| DngColorEndpoint {
            cct: Some(cct),
            color_matrix: [
                [red_scale, 0.0, 0.0],
                [0.0, 0.5, 0.0],
                [0.0, 0.0, blue_scale],
                [0.0, 0.5, 0.0],
            ],
            calibration: identity_4x4(),
            forward_matrix: None,
        };
        let model = CameraWhiteBalanceModel {
            base_wb: [2.0, 1.0, 1.5, 1.0],
            cdesc: RGBG,
            base_cct: 5000.0,
            color: CameraColorModel::Dng {
                endpoints: Box::new([endpoint(2856.0, 1.2, 0.8), endpoint(6504.0, 0.9, 1.1)]),
                analog_balance: identity_4x4(),
            },
        };

        let cooler = adjusted_white_balance_coefficients(&model, -20.0, 0.0).unwrap();
        let warmer = adjusted_white_balance_coefficients(&model, 20.0, 0.0).unwrap();
        assert!(warmer.iter().all(|value| value.is_finite()));
        assert_ne!(cooler, warmer);

        let tinted = adjusted_white_balance_coefficients(&model, 20.0, 20.0).unwrap();
        assert_ne!(warmer, tinted);
    }

    #[test]
    fn rawnind_daylight_white_balance_uses_d65_camera_response() {
        let model = CameraWhiteBalanceModel {
            base_wb: [2.0, 1.0, 1.5, 1.0],
            cdesc: RGBG,
            base_cct: 4_500.0,
            color: CameraColorModel::Matrix {
                xyz_to_camera: [
                    [1.0, 0.0, 0.0],
                    [0.0, 1.0, 0.0],
                    [0.0, 0.0, 1.0],
                    [0.0, 0.0, 0.0],
                ],
            },
        };
        let wb = daylight_white_balance(&model).unwrap();
        assert!((wb[0] - 1.0 / super::D65_XYZ[0]).abs() < 1e-6);
        assert_eq!(wb[1], 1.0);
        assert!((wb[2] - 1.0 / super::D65_XYZ[2]).abs() < 1e-6);
    }

    #[test]
    fn libraw_filter_codes_select_the_demosaic_family() {
        assert_eq!(cfa_kind_from_filters(9).unwrap(), CfaKind::XTrans);
        assert_eq!(cfa_kind_from_filters(0x9494_9494).unwrap(), CfaKind::Bayer);
        assert!(cfa_kind_from_filters(0).is_err());
        assert!(cfa_kind_from_filters(1).is_err());
    }

    #[test]
    fn documented_libraw_rotations_map_output_to_source_coordinates() {
        assert_eq!(oriented_source_pos(0, 0, 3, 2, 5), (2, 0));
        assert_eq!(oriented_source_pos(1, 2, 3, 2, 5), (0, 1));
        assert_eq!(oriented_source_pos(0, 0, 3, 2, 6), (0, 1));
        assert_eq!(oriented_source_pos(1, 2, 3, 2, 6), (2, 0));
    }

    #[test]
    fn camera_neutral_maps_to_rec2020_neutral() {
        let matrix = cam_to_working(
            [
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
                [0.0, 0.0, 0.0],
            ],
            RGBG,
        );

        for (channel, row) in matrix.iter().enumerate() {
            let mapped_neutral = row[0] + row[1] + row[2];
            assert!(
                (mapped_neutral - 1.0).abs() < 1e-5,
                "camera neutral mapped to {mapped_neutral} in working channel {channel}"
            );
        }
    }

    fn assert_white_balanced_neutral_maps_to_unit_white(matrix: [[f32; 4]; 3]) {
        // The shader feeds raw * wb, so a neutral camera response becomes ones.
        for (channel, row) in matrix.iter().enumerate() {
            let mapped = row[0] + row[1] + row[2];
            assert!(
                (mapped - 1.0).abs() < 1e-5,
                "neutral mapped to {mapped} in working channel {channel}"
            );
        }
    }

    #[test]
    fn dng_color_and_forward_matrix_paths_map_neutral_to_unit_white() {
        let wb = [2.1, 1.0, 1.4, 1.0];
        // A realistic, non-normalized D65 ColorMatrix: camera white luminance is not 1.
        let color_matrix_only = super::InterpolatedDngProfile {
            color_matrix: [
                [0.7374, -0.2389, -0.0551],
                [-0.5435, 1.3162, 0.2519],
                [-0.1006, 0.1795, 0.6552],
                [0.0, 0.0, 0.0],
            ],
            calibration: identity_4x4(),
            forward_matrix: None,
            weight: 0.0,
        };
        assert_white_balanced_neutral_maps_to_unit_white(
            super::dng_camera_to_working(color_matrix_only, identity_4x4(), wb, wb, RGBG).unwrap(),
        );

        // Rows that do not sum to the PCS white, as in some third-party profiles.
        let unnormalized_forward = super::InterpolatedDngProfile {
            forward_matrix: Some([
                [0.61, 0.29, 0.09, 0.0],
                [0.26, 0.69, 0.06, 0.0],
                [0.02, 0.11, 0.73, 0.0],
            ]),
            ..color_matrix_only
        };
        assert_white_balanced_neutral_maps_to_unit_white(
            super::dng_camera_to_working(unnormalized_forward, identity_4x4(), wb, wb, RGBG)
                .unwrap(),
        );
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

    fn dcp_with_matrix(red: f32, blue: f32) -> DcpProfile {
        let first = DcpMatrixSet {
            illuminant: Some(21),
            color_matrix: Some([
                [red, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, blue],
                [0.0, 0.0, 0.0],
            ]),
            camera_calibration: Some(identity_4x4()),
            forward_matrix: Some([
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
            ]),
        };
        let second = DcpMatrixSet {
            illuminant: Some(17),
            color_matrix: Some([
                [blue, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, red],
                [0.0, 0.0, 0.0],
            ]),
            camera_calibration: first.camera_calibration,
            forward_matrix: first.forward_matrix,
        };
        DcpProfile {
            matrices: [first, second],
            ..DcpProfile::default()
        }
    }

    #[test]
    fn decoder_neutral_interpolation_responds_to_white_balance() {
        let profile = dcp_with_matrix(1.2, 0.8);
        let cold = super::interpolated_parsed_dng_profile_with_fallbacks(
            &profile,
            [3.0, 1.0, 0.8, 1.0],
            identity_4x4(),
            true,
            [identity_4x4(); 2],
            None,
        )
        .unwrap();
        let warm = super::interpolated_parsed_dng_profile_with_fallbacks(
            &profile,
            [0.8, 1.0, 3.0, 1.0],
            identity_4x4(),
            true,
            [identity_4x4(); 2],
            None,
        )
        .unwrap();
        assert_ne!(cold.weight, warm.weight);
    }

    #[test]
    fn decoder_neutral_color_helper_uses_selected_dcp_and_preserves_wb_model() {
        let embedded = dcp_with_matrix(1.0, 1.0);
        let selected = dcp_with_matrix(2.0, 3.0);
        let (matrix, weight, model) = super::camera_to_working_matrix_from_profiles(
            [
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
                [0.0, 0.0, 0.0],
            ],
            [2.0, 1.0, 1.5, 1.0],
            RGBG,
            Some(&embedded),
            Some(&selected),
            identity_4x4(),
        )
        .unwrap();
        assert_eq!(weight, 0.0);
        assert!(matrix.iter().flatten().all(|value| value.is_finite()));
        let CameraColorModel::Dng { endpoints, .. } = model.unwrap().color else {
            panic!("DCP metadata should produce a DNG color model")
        };
        assert_eq!(endpoints[0].color_matrix[0][0], 2.0);
    }

    #[test]
    fn decoder_neutral_color_helper_rejects_singular_camera_metadata() {
        let result = super::camera_to_working_matrix_from_profiles(
            [[0.0; 3]; 4],
            [1.0, 1.0, 1.0, 1.0],
            RGBG,
            None,
            None,
            identity_4x4(),
        );
        assert!(result.is_err());
    }

    #[test]
    fn cfa_planes_are_canonicalized_without_merging_greens() {
        let map = canonical_cfa_map(*b"GRGB").unwrap();
        assert_eq!(map, [1, 0, 3, 2]);
        assert_eq!(
            canonicalize_f32x4([10.0, 20.0, 30.0, 40.0], map),
            [20.0, 10.0, 40.0, 30.0]
        );
    }

    #[test]
    fn non_rgb_cfa_is_rejected_instead_of_silently_miscolored() {
        assert!(canonical_cfa_map(*b"GMCY").is_err());
        assert!(canonical_cfa_map(*b"RGBG").is_ok());
    }

    #[test]
    fn three_channel_dng_calibration_completes_the_unused_fourth_plane() {
        let matrix = [
            [1.1, 0.0, 0.0, 0.0],
            [0.0, 0.9, 0.0, 0.0],
            [0.0, 0.0, 1.05, 0.0],
            [0.0, 0.0, 0.0, 0.0],
        ];
        let completed = identity_fallback_4x4(matrix);
        assert_eq!(completed[3][3], 1.0);
        assert!(matrix::invert(completed).is_some());
        assert_eq!(completed[0][0], 1.1);
        assert_eq!(completed[1][1], 0.9);
        assert_eq!(completed[2][2], 1.05);
    }

    #[test]
    fn calibration_keeps_both_green_planes_distinct() {
        assert_eq!(black_levels(64, &[1, 2, 3, 4]), [65.0, 66.0, 67.0, 68.0]);
        assert_eq!(
            white_levels(4095, [4000, 4010, 4020, 4030], [64.0; 4]),
            [4000.0, 4010.0, 4020.0, 4030.0]
        );
    }

    #[test]
    fn invalid_linear_max_falls_back_to_decoded_white_level() {
        assert_eq!(
            white_levels(4095, [10, 4000, 5000, 0], [64.0; 4]),
            [4095.0, 4000.0, 4095.0, 4095.0]
        );
    }

    /// A 12-bit mosaic with a smooth ramp up to `ramp_top` plus `piled` pixels at `pile_code`.
    fn ramp_with_pile_up(ramp_top: u16, pile_code: u16, piled: usize) -> Vec<u16> {
        let mut pixels: Vec<u16> = (0..200_000).map(|i| 67 + (i % 3_800) as u16).collect();
        pixels.retain(|&value| value <= ramp_top);
        pixels.extend(std::iter::repeat_n(pile_code, piled));
        pixels
    }

    #[test]
    fn saturation_pile_up_below_nominal_white_lowers_all_channels() {
        // Olympus XZ-1: LibRaw reports 4095 but the sensor saturates at 3972.
        let pixels = ramp_with_pile_up(3_867, 3_972, 50_000);
        assert_eq!(
            saturation_adjusted_white_levels([4095.0; 4], [67.0; 4], &pixels),
            [3972.0; 4]
        );
    }

    #[test]
    fn natural_highlight_tail_keeps_nominal_white() {
        let pixels = ramp_with_pile_up(3_867, 3_867, 0);
        assert_eq!(
            saturation_adjusted_white_levels([4095.0; 4], [67.0; 4], &pixels),
            [4095.0; 4]
        );
    }

    #[test]
    fn saturation_detection_ignores_hot_pixels_and_low_pile_ups() {
        // A few hot pixels above a real pile-up do not hide it.
        let mut pixels = ramp_with_pile_up(3_867, 3_972, 50_000);
        pixels.extend([4_050, 4_050, 4_090]);
        assert_eq!(
            saturation_adjusted_white_levels([4095.0; 4], [67.0; 4], &pixels),
            [3972.0; 4]
        );
        // Data reaching the nominal white already agrees with the metadata.
        let at_white = ramp_with_pile_up(3_867, 4_095, 50_000);
        assert_eq!(
            saturation_adjusted_white_levels([4095.0; 4], [67.0; 4], &at_white),
            [4095.0; 4]
        );
        // A uniform bright patch in the lower part of the range is not saturation.
        let mid = ramp_with_pile_up(2_000, 2_500, 50_000);
        assert_eq!(
            saturation_adjusted_white_levels([4095.0; 4], [67.0; 4], &mid),
            [4095.0; 4]
        );
    }

    #[test]
    fn repeating_black_pattern_uses_active_area_coordinates() {
        let cblack = [1, 2, 3, 4, 2, 3, 10, 20, 30, 40, 50, 60];
        assert_eq!(effective_black_level(64, &cblack, 2, 0, 0), 77.0);
        assert_eq!(effective_black_level(64, &cblack, 2, 4, 3), 117.0);
    }

    #[test]
    fn malformed_black_pattern_is_ignored_without_out_of_bounds_access() {
        let cblack = [1, 2, 3, 4, 99, 99];
        assert_eq!(effective_black_level(64, &cblack, 1, 500, 500), 66.0);
    }

    #[test]
    fn white_balance_uses_the_average_green_reference() {
        let wb = white_balance([2.0, 1.0, 1.5, 1.2], RGBG);
        let green_mean = 0.5 * (wb[1] + wb[3]);
        assert!((green_mean - 1.0).abs() < 1e-6);
        assert!((wb[1] - wb[3]).abs() > 1e-3);
    }
}
