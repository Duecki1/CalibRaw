use super::*;
use crate::i18n::{tr, tr_fmt};
use document_load::{prepare_ahead_of_open, AheadOfOpen};
use document_load::{
    run_document_load, sidecar_migrated_notice, CameraProfileSettings, DocumentLoadJob,
    PreviewProgramSources,
};
pub(crate) use document_load::{DocumentSource, ProfileReload};
pub(crate) use prefetch::NeighbourPrefetch;
pub(in crate::app) use profile_preview::reserves_profile_preview;
pub(crate) use profile_preview::{CameraProfileChoice, CameraProfilePreview};

pub(super) fn remove_temporary_raw(path: &std::path::Path) {
    if let Err(error) = std::fs::remove_file(path) {
        log::warn!(
            "could not remove imported Android RAW cache file {}: {error}",
            path.display()
        );
    }
}

pub(super) fn load_sidecar_for_target(
    target: &crate::sidecar::SidecarTarget,
    platform: &dyn Platform,
) -> Result<Option<crate::sidecar::LoadedSidecar>, crate::sidecar::SidecarError> {
    match target {
        crate::sidecar::SidecarTarget::Desktop { raw_path } => {
            crate::sidecar::load_desktop(raw_path)
        }
        crate::sidecar::SidecarTarget::Android {
            raw_uri,
            display_name,
        } => platform.load_sidecar(raw_uri, display_name),
    }
}

pub(super) fn raw_cache_key_for_target(target: &crate::sidecar::SidecarTarget) -> String {
    match target {
        crate::sidecar::SidecarTarget::Desktop { raw_path } => {
            let metadata = std::fs::metadata(raw_path).ok();
            let bytes = metadata
                .as_ref()
                .map(std::fs::Metadata::len)
                .unwrap_or_default();
            let modified = metadata
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_nanos())
                .unwrap_or_default();
            format!("desktop:{}:{bytes}:{modified}", raw_path.display())
        }
        crate::sidecar::SidecarTarget::Android { raw_uri, .. } => {
            format!("android:{raw_uri}")
        }
    }
}

pub(super) fn prewarm_dcp_profile_folder(folder: Option<std::path::PathBuf>) {
    let Some(folder) = folder.filter(|folder| folder.is_dir()) else {
        return;
    };
    let _ = std::thread::Builder::new()
        .name("calibraw-dcp-prewarm".to_owned())
        .spawn(move || {
            let started = Instant::now();
            crate::pipeline::prewarm_dcp_profile_index(&folder);
            calibraw_core::diagnostics::record(format!(
                "DCP profile index prewarmed in {:.3}s",
                started.elapsed().as_secs_f64()
            ));
        });
}

pub(super) fn selected_picker_directory(path: &std::path::Path) -> Option<std::path::PathBuf> {
    if path.is_dir() {
        Some(path.to_path_buf())
    } else {
        path.parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(std::path::Path::to_path_buf)
    }
}

pub(super) fn append_notice(notice: &mut Option<String>, message: &str) {
    match notice {
        Some(existing) => {
            existing.push(' ');
            existing.push_str(message);
        }
        None => *notice = Some(message.to_owned()),
    }
}

pub(in crate::app) fn masks_have_missing_range_sources(masks: &MaskStack) -> bool {
    masks.masks.iter().any(|mask| {
        mask.components.iter().any(|component| {
            matches!(
                &component.geometry,
                MaskGeometry::LuminanceRange { source: None, .. }
                    | MaskGeometry::ColorRange { source: None, .. }
            )
        })
    })
}

pub(in crate::app) fn needs_canonical_mask_source(masks: &MaskStack) -> bool {
    masks_have_missing_range_sources(masks)
        || masks.masks.iter().any(|mask| {
            mask.components
                .iter()
                .any(|component| matches!(&component.geometry, MaskGeometry::Object { .. }))
        })
}

fn install_missing_range_sources(masks: &mut MaskStack, source: &MaskRgbImage) {
    for mask in &mut masks.masks {
        for component in &mut mask.components {
            match &mut component.geometry {
                MaskGeometry::LuminanceRange { source: target, .. }
                | MaskGeometry::ColorRange { source: target, .. }
                    if target.is_none() =>
                {
                    *target = Some(source.clone());
                }
                _ => {}
            }
        }
    }
}

/// Range masks sample a canonical (default-edit) rendering of the photo. For
/// masks whose sidecar lacks it, `pipeline`, built for `pipeline_raw`, renders
/// it, and it is installed into them. `full_raw` is the whole photo, whose
/// highlight analysis the rendering reads.
pub(in crate::app) fn reconstruct_canonical_mask_source(
    pipeline: &RawGpuPipeline,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    full_raw: &LoadedRaw,
    pipeline_raw: &LoadedRaw,
    exposure: &ExposureParams,
    masks: &mut MaskStack,
) -> Result<MaskRgbImage, String> {
    let started = Instant::now();
    let reference_exposure = exposure.source_reference();
    if full_raw.uses_highlight_model(&reference_exposure) {
        full_raw.prime_highlight_model(&reference_exposure);
    }
    let reference_masks = MaskStack::default();
    let reference_params = GpuParams::new(&reference_exposure, &reference_masks, pipeline_raw);
    pipeline.recompute(queue, device, &reference_params);
    let rgba = pipeline
        .read_output_region_blocking(device, queue, 0, 0, pipeline.width, pipeline.height)
        .map_err(|error| {
            tr_fmt(
                "range-mask source readback failed: {error}",
                &[("error", &format!("{error:#}"))],
            )
        })?;
    let source = MaskRgbImage::new(pipeline.width, pipeline.height, rgba)
        .ok_or_else(|| tr("range-mask source dimensions are invalid").to_owned())?;
    install_missing_range_sources(masks, &source);
    calibraw_core::diagnostics::record(format!(
        "Canonical mask source reconstructed in {:.3}s",
        started.elapsed().as_secs_f64()
    ));
    Ok(source)
}

mod cache;
mod document_load;
mod documents;
mod pickers;
mod prefetch;
mod profile_preview;
mod profiles;
mod settings;
mod startup;
