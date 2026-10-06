//! Rendering edited (developed) thumbnails on a shared headless GPU device.

use super::*;

pub(in crate::ui::library) type ThumbnailLoader = Arc<
    dyn Fn(&LibraryAsset, ThumbnailLoadStage) -> Result<LoadedLibraryThumbnail, String>
        + Send
        + Sync
        + 'static,
>;

#[cfg(not(target_os = "android"))]
pub(in crate::ui::library) struct DevelopedThumbnailGpu {
    pub(super) device: eframe::wgpu::Device,
    pub(super) queue: eframe::wgpu::Queue,
}

#[cfg(not(target_os = "android"))]
static DEVELOPED_THUMBNAIL_GPU: OnceLock<Result<Mutex<DevelopedThumbnailGpu>, String>> =
    OnceLock::new();

#[cfg(not(target_os = "android"))]
pub(in crate::ui::library) fn developed_thumbnail_gpu(
) -> Result<&'static Mutex<DevelopedThumbnailGpu>, String> {
    let initialized = DEVELOPED_THUMBNAIL_GPU.get_or_init(|| {
        let gpu = calibraw_gpu::request_headless_device(&calibraw_gpu::HeadlessDeviceRequest {
            label: "calibraw library edited-thumbnail device",
            power_preference: eframe::wgpu::PowerPreference::LowPower,
            texture_dimension: DEVELOPED_THUMBNAIL_PROXY_EDGE.max(mask_atlas_edge()),
        })
        .map_err(|error| match error {
            calibraw_gpu::HeadlessDeviceError::NoAdapter(error) => {
                format!("could not find a GPU for edited thumbnails: {error}")
            }
            calibraw_gpu::HeadlessDeviceError::TextureTooLarge {
                required, supported, ..
            } => format!(
                "edited thumbnails require a {required}-pixel GPU texture, but this adapter supports {supported}"
            ),
            calibraw_gpu::HeadlessDeviceError::Device(error) => {
                format!("could not create the edited-thumbnail GPU device: {error}")
            }
        })?;
        let (device, queue) = (gpu.device, gpu.queue);
        calibraw_gpu::install_uncaptured_gpu_error_handler(&device);
        Ok(Mutex::new(DevelopedThumbnailGpu { device, queue }))
    });

    match initialized {
        Ok(gpu) => Ok(gpu),
        Err(error) => Err(error.clone()),
    }
}

#[cfg(not(target_os = "android"))]
pub(in crate::ui::library) fn render_uncached_developed_thumbnail(
    path: &Path,
    maximum_edge: u32,
) -> Result<Option<RawThumbnail>, String> {
    let loaded_sidecar = match crate::sidecar::load_desktop(path) {
        Ok(Some(sidecar)) => sidecar,
        Ok(None) => return Ok(None),
        Err(crate::sidecar::SidecarError::Invalid(error)) => {
            log::warn!(
                "ignoring invalid sidecar while rendering library thumbnail for {}: {error}",
                path.display()
            );
            return Ok(None);
        }
        Err(error) => {
            return Err(format!(
                "could not load edits for {}: {error}",
                path.display()
            ))
        }
    };
    if !crate::sidecar::edit_state_has_adjustments(&loaded_sidecar.edits) {
        return Ok(None);
    }
    let sidecar_fingerprint = crate::sidecar::desktop_sidecar_fingerprint(path)?
        .ok_or_else(|| "edit sidecar disappeared before thumbnail rendering".to_owned())?;

    let _render_permit = calibraw_core::thumbnail_cache::acquire_rendered_thumbnail_worker();

    if let Some(thumbnail) = crate::sidecar::load_developed_thumbnail_cache(path, maximum_edge)? {
        let cached_edge = thumbnail.width.max(thumbnail.height);
        let minimum_edge = maximum_edge.saturating_mul(3) / 4;
        if maximum_edge <= THUMBNAIL_EDGE || cached_edge >= minimum_edge {
            return Ok(Some(thumbnail));
        }
    }

    let performance =
        crate::performance_settings::load(crate::performance_settings::desktop_path().as_deref());
    let mut camera_profile_folder = performance.camera_profile_folder;
    if performance.camera_profile_auto_detect
        && camera_profile_folder
            .as_ref()
            .is_none_or(|folder| !folder.is_dir())
    {
        camera_profile_folder = crate::performance_settings::detected_camera_profile_folder();
    }
    let requested_camera_profile =
        loaded_sidecar
            .edits
            .camera_profile
            .as_ref()
            .and_then(|relative| {
                camera_profile_folder
                    .as_ref()
                    .map(|root| root.join(relative))
            });
    let full_raw = load_raw_file_with_profile_selection(
        path,
        performance.camera_profile_mode,
        camera_profile_folder.as_deref(),
        requested_camera_profile.as_deref(),
    )
    .map_err(|error| format!("could not decode edited RAW {}: {error:#}", path.display()))?;
    let edits = loaded_sidecar.edits;
    if full_raw.uses_opposed_chroma(&edits.exposure) {
        full_raw.inpaint_opposed_chroma_for_exposure(&edits.exposure);
    }
    let render_proxy_edge = DEVELOPED_THUMBNAIL_PROXY_EDGE.max(maximum_edge);
    let mut preview_raw = if full_raw.width.max(full_raw.height) > render_proxy_edge {
        build_proxy(
            &full_raw,
            ProxySpec {
                max_edge: render_proxy_edge,
            },
        )
    } else {
        full_raw
    };

    let geometry = edits.geometry;
    if edits.lens.enabled {
        let catalog = lensfun_catalog(&preview_raw);
        let selected = catalog
            .lenses
            .iter()
            .find(|lens| lens.maker == edits.lens.maker && lens.model == edits.lens.model)
            .cloned()
            .or_else(|| {
                (!edits.lens.maker.is_empty() || !edits.lens.model.is_empty()).then(|| {
                    LensfunLens {
                        maker: edits.lens.maker.clone(),
                        model: edits.lens.model.clone(),
                        ..LensfunLens::default()
                    }
                })
            })
            .or(catalog.auto_match)
            .map(|lens| LensfunLens {
                corrections: edits.lens.corrections(),
                ..lens
            });
        if let Some(selected) = selected {
            match apply_lensfun_correction(&preview_raw, &selected) {
                Ok(corrected) => preview_raw = corrected,
                Err(error) => log::warn!(
                    "could not apply saved lens correction to library thumbnail {}: {error:#}",
                    path.display()
                ),
            }
        }
    }

    let mut masks = Arc::unwrap_or_clone(edits.masks);
    let initial_params =
        GpuParams::new(&edits.exposure, &masks, &preview_raw).with_vignette_geometry(geometry);
    let gpu = developed_thumbnail_gpu()?;
    let gpu = gpu
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let pipeline = RawGpuPipeline::new(
        &gpu.device,
        &gpu.queue,
        &preview_raw,
        &initial_params,
        PipelineOptions::new(ProcessingQuality::Preview),
    )
    .map_err(|error| format!("could not prepare edited thumbnail rendering: {error:#}"))?;
    if crate::app::masks_have_missing_range_sources(&masks) {
        let neutral_exposure = crate::pipeline::ExposureParams::scene_referred_default();
        let neutral_masks = MaskStack::default();
        let neutral_params = GpuParams::new(&neutral_exposure, &neutral_masks, &preview_raw);
        pipeline.recompute(&gpu.queue, &gpu.device, &neutral_params);
        let rgba = pipeline
            .read_output_region_blocking(
                &gpu.device,
                &gpu.queue,
                0,
                0,
                preview_raw.width,
                preview_raw.height,
            )
            .map_err(|error| format!("could not build range-mask thumbnail source: {error:#}"))?;
        let source = MaskRgbImage::new(preview_raw.width, preview_raw.height, rgba)
            .ok_or_else(|| "range-mask thumbnail source has invalid dimensions".to_owned())?;
        crate::app::install_missing_range_sources(&mut masks, &source);
    }

    for layer in 0..masks.masks.len().min(MAX_LOCAL_MASKS) {
        let edge = pipeline.mask_atlas_edge();
        let values = masks.rasterize_layer_region_f16(
            layer,
            [edge, edge],
            [0, 0, preview_raw.width, preview_raw.height],
            [preview_raw.width, preview_raw.height],
            preview_raw.lens_geometry.as_deref(),
        );
        pipeline
            .update_mask_layer(&gpu.queue, layer, &values)
            .map_err(|error| format!("could not apply thumbnail local mask: {error:#}"))?;
    }
    pipeline
        .update_light_rays_mask_layers(
            &gpu.queue,
            &masks,
            preview_raw.width,
            preview_raw.height,
            preview_raw.lens_geometry.as_deref(),
        )
        .map_err(|error| format!("could not apply thumbnail Light Rays mask: {error:#}"))?;
    let params =
        GpuParams::new(&edits.exposure, &masks, &preview_raw).with_vignette_geometry(geometry);
    pipeline.recompute(&gpu.queue, &gpu.device, &params);
    let thumbnail = pipeline
        .output_snapshot(&gpu.device, &gpu.queue)
        .read_thumbnail_blocking(&gpu.device, &gpu.queue, maximum_edge)
        .map_err(|error| format!("could not read edited thumbnail pixels: {error:#}"))?;
    let thumbnail = crate::pipeline::transform_thumbnail_geometry_with_lens(
        &thumbnail,
        geometry,
        preview_raw.lens_geometry.as_deref(),
    );
    crate::sidecar::save_developed_thumbnail_cache(path, &thumbnail, sidecar_fingerprint)?;
    Ok(Some(thumbnail))
}
