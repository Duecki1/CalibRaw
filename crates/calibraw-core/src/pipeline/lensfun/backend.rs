//! Lens correction through the native Lensfun library.

use super::*;
use anyhow::Context;
use rayon::prelude::*;
use std::ffi::{c_char, c_int, CStr, CString};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::Arc;

mod cfa_sampling;
mod correction;
mod database;
mod matching;
use cfa_sampling::*;
use correction::*;
use database::*;
use matching::*;

const LENSFUN_BUILD_VERSION: &str = env!("CALIBRAW_LENSFUN_BUILD_VERSION");

mod ffi {
    #![allow(
        dead_code,
        non_camel_case_types,
        non_snake_case,
        non_upper_case_globals
    )]
    include!(concat!(env!("OUT_DIR"), "/lensfun_bindings.rs"));
}

use ffi::{
    lfCamera, lfDatabase, lfLens, lfModifier, lf_db_destroy, lf_db_find_cameras,
    lf_db_find_cameras_ext, lf_db_find_lenses_hd, lf_db_get_lenses, lf_db_load, lf_db_load_file,
    lf_db_new, lf_free, lf_mlstr_get, lf_modifier_add_coord_callback_scale,
    lf_modifier_apply_color_modification, lf_modifier_apply_subpixel_geometry_distortion,
    lf_modifier_destroy, lf_modifier_get_auto_scale, lf_modifier_initialize, lf_modifier_new,
};

const LF_NO_ERROR: ffi::lfError = ffi::LF_NO_ERROR;
const LF_SEARCH_LOOSE: c_int = ffi::LF_SEARCH_LOOSE as c_int;
const LF_SEARCH_SORT_AND_UNIQUIFY: c_int = ffi::LF_SEARCH_SORT_AND_UNIQUIFY as c_int;
const LF_MODIFY_TCA: c_int = ffi::LF_MODIFY_TCA as c_int;
const LF_MODIFY_VIGNETTING: c_int = ffi::LF_MODIFY_VIGNETTING as c_int;
const LF_MODIFY_DISTORTION: c_int = ffi::LF_MODIFY_DISTORTION as c_int;
const LF_MODIFY_GEOMETRY: c_int = ffi::LF_MODIFY_GEOMETRY as c_int;
const LF_MODIFY_SCALE: c_int = ffi::LF_MODIFY_SCALE as c_int;
const LF_CR_UNKNOWN: c_int = ffi::LF_CR_UNKNOWN as c_int;
const LF_CR_RED: c_int = ffi::LF_CR_RED as c_int;
const LF_CR_GREEN: c_int = ffi::LF_CR_GREEN as c_int;
const LF_CR_BLUE: c_int = ffi::LF_CR_BLUE as c_int;
const LF_CR_RGBA: c_int =
    LF_CR_RED | (LF_CR_GREEN << 4) | (LF_CR_BLUE << 8) | (LF_CR_UNKNOWN << 12);

pub(super) fn catalog(raw: &LoadedRaw) -> LensfunCatalog {
    match catalog_result(raw) {
        Ok(catalog) => catalog,
        Err(error) => LensfunCatalog {
            available: false,
            status: format!("Lensfun: {error:#}"),
            ..LensfunCatalog::default()
        },
    }
}

fn catalog_result(raw: &LoadedRaw) -> Result<LensfunCatalog> {
    let database = Database::load()?;
    let camera = find_camera(&database, &raw.camera_make, &raw.camera_model);
    let camera_label = camera
        .and_then(camera_name)
        .map(|(maker, model)| format!("{maker} {model}").trim().to_owned())
        .unwrap_or_else(|| {
            let reported = format!("{} {}", raw.camera_make, raw.camera_model)
                .trim()
                .to_owned();
            if reported.is_empty() {
                "Not reported".to_owned()
            } else {
                format!("{reported} (no Lensfun camera match)")
            }
        });

    let mut lenses = camera
        .map(|camera| compatible_lenses(&database, camera))
        .unwrap_or_else(|| all_lenses(&database));
    sort_and_deduplicate_lenses(&mut lenses);

    let auto_match = find_auto_lens(&database, camera, raw);
    let message = if let Some(found) = &auto_match {
        format!("Auto-detected {} from RAW metadata", found.label())
    } else if raw.lens_model.trim().is_empty() {
        "The RAW file does not identify a lens. Select one manually.".to_owned()
    } else if camera.is_none() {
        format!(
            "No camera profile matched, and the lens ‘{}’ was not unambiguous. Select a profile manually.",
            raw.lens_model
        )
    } else {
        format!(
            "No Lensfun profile matched ‘{}’. Select one manually.",
            raw.lens_model
        )
    };
    let status = format!("Lensfun {LENSFUN_BUILD_VERSION}: {message}");

    Ok(LensfunCatalog {
        available: true,
        camera_label,
        lenses,
        auto_match,
        status,
    })
}

pub(super) fn apply(raw: &LoadedRaw, selection: &LensfunLens) -> Result<LoadedRaw> {
    let database = Database::load()?;
    let camera = find_camera(&database, &raw.camera_make, &raw.camera_model);
    let lens = find_lens(&database, camera, selection)
        .ok_or_else(|| anyhow!("Lensfun has no profile for {}", selection.label()))?;

    let lens_fields =
        lens_fields(lens).ok_or_else(|| anyhow!("Lensfun returned a null lens profile"))?;
    let crop = camera
        .and_then(camera_crop_factor)
        .and_then(positive)
        .or_else(|| positive(lens_fields.crop_factor))
        .unwrap_or(1.0);
    let focal = positive(raw.focal_length).unwrap_or_else(|| {
        let min = lens_fields.min_focal;
        let max = lens_fields.max_focal;
        if min.is_finite() && max.is_finite() && min > 0.0 && max >= min {
            0.5 * (min + max)
        } else {
            min.max(1.0)
        }
    });
    let width = c_int::try_from(raw.width).context("RAW width does not fit Lensfun")?;
    let height = c_int::try_from(raw.height).context("RAW height does not fit Lensfun")?;
    let aperture = positive(raw.aperture).unwrap_or(8.0);
    let distance = positive(raw.focus_distance).unwrap_or(1000.0);

    let LensfunCorrections {
        geometry: geometry_requested,
        vignetting: vignetting_requested,
    } = selection.corrections;
    if !geometry_requested && !vignetting_requested {
        return Err(anyhow!("no lens correction type is selected"));
    }
    let config = |requested_flags| ModifierConfig {
        crop,
        dimensions: [width, height],
        focal,
        aperture,
        distance,
        requested_flags,
    };

    // Vignetting and TCA act on the mosaic; distortion becomes a geometry map
    // applied later. TCA is part of the geometry option.
    let mut early_requested = 0;
    if geometry_requested {
        early_requested |= LF_MODIFY_TCA;
    }
    if vignetting_requested {
        early_requested |= LF_MODIFY_VIGNETTING;
    }
    let (early_modifier, early_flags) = initialize_modifier(lens, config(early_requested))?;

    let mut geometry_flags = 0;
    let mut geometry_modifier = None;
    if geometry_requested {
        let (modifier, flags) = initialize_modifier(lens, config(LF_MODIFY_DISTORTION))?;
        geometry_flags = flags;

        let (scale_probe_modifier, scale_probe_flags) =
            initialize_modifier(lens, config(LF_MODIFY_TCA | LF_MODIFY_DISTORTION))?;
        if scale_probe_flags & (LF_MODIFY_DISTORTION | LF_MODIFY_GEOMETRY | LF_MODIFY_TCA) != 0 {
            let scale = unsafe { lf_modifier_get_auto_scale(scale_probe_modifier.0, 0) };
            if scale.is_finite() && scale > 0.0 {
                let scaling_added =
                    unsafe { lf_modifier_add_coord_callback_scale(modifier.0, scale, 0) };
                if scaling_added != 0 {
                    geometry_flags |= LF_MODIFY_SCALE;
                }
            }
        }
        geometry_modifier = Some(modifier);
    }

    if early_flags & (LF_MODIFY_TCA | LF_MODIFY_VIGNETTING) == 0
        && geometry_flags & (LF_MODIFY_DISTORTION | LF_MODIFY_GEOMETRY | LF_MODIFY_SCALE) == 0
    {
        return Err(anyhow!(
            "the selected Lensfun profile contains no applicable data for the chosen corrections"
        ));
    }

    let lens_geometry = match &geometry_modifier {
        Some(modifier) => build_lens_geometry_map(raw, modifier, geometry_flags)?,
        None => None,
    };
    if early_flags & (LF_MODIFY_TCA | LF_MODIFY_VIGNETTING) == 0 {
        let mut corrected = raw.clone();
        corrected.lens_geometry = lens_geometry;
        return Ok(corrected);
    }
    correct_mosaic(raw, &early_modifier, early_flags, lens_geometry)
}

#[cfg(test)]
mod ffi_boundary_tests {
    use super::*;

    #[test]
    fn null_native_results_are_normal_wrapper_misses() {
        assert!(camera_name(ptr::null()).is_none());
        assert!(camera_crop_factor(ptr::null()).is_none());
        assert!(lens_fields(ptr::null()).is_none());
        assert!(lens_name(ptr::null()).is_none());
        assert!(pointer_list::<lfLens>(ptr::null()).is_empty());

        let list = OwnedPointerList::<lfLens> {
            pointer: ptr::null_mut(),
        };
        assert!(list.first().is_none());
        assert!(list.values().is_empty());
    }

    #[test]
    fn native_owners_remain_raii_guards() {
        assert!(std::mem::needs_drop::<Database>());
        assert!(std::mem::needs_drop::<Modifier>());
        assert!(std::mem::needs_drop::<OwnedPointerList<lfLens>>());
    }
}
