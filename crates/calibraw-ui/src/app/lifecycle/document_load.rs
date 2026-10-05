//! Worker-thread half of opening a RAW document: read the sidecar, decode,
//! restore saved lens correction and AI-denoise results, and render the first
//! GPU preview. The UI-thread half lives in `documents.rs`.

use super::*;
use std::sync::RwLock;

/// A RAW file to open in Develop.
pub(crate) struct DocumentSource {
    pub(crate) path: PathBuf,
    pub(crate) label: String,
    pub(crate) sidecar_target: crate::sidecar::SidecarTarget,
    /// `path` is a private copy (an Android import) to delete once decoded.
    pub(crate) delete_after_decode: bool,
    /// Keeps a descriptor-backed `path` readable until decoding finishes.
    pub(crate) raw_fd_guard: Option<std::fs::File>,
}

impl DocumentSource {
    pub(crate) fn desktop(path: PathBuf, label: String) -> Self {
        Self {
            sidecar_target: crate::sidecar::SidecarTarget::Desktop {
                raw_path: path.clone(),
            },
            path,
            label,
            delete_after_decode: false,
            raw_fd_guard: None,
        }
    }

    /// The decoded file to delete afterwards, when `path` is a disposable copy.
    pub(super) fn disposable_copy(&self) -> Option<&Path> {
        (self.delete_after_decode && self.raw_fd_guard.is_none()).then_some(&self.path)
    }

    /// A path that stays valid after decoding, for later reloads.
    pub(super) fn persistent_path(&self) -> Option<PathBuf> {
        (!self.delete_after_decode && self.raw_fd_guard.is_none()).then(|| self.path.clone())
    }
}

/// Reopens the current document with a different camera profile while keeping
/// its in-memory edits instead of the saved sidecar.
pub(crate) struct ProfileReload {
    /// `None` requests automatic profile selection.
    pub(crate) camera_profile: Option<PathBuf>,
    pub(crate) edits: SidecarEditState,
}

pub(super) struct CameraProfileSettings {
    pub(super) mode: CameraProfileMode,
    pub(super) folder: Option<PathBuf>,
    pub(super) last_used: Option<PathBuf>,
}

/// GPU programs that can be reused instead of compiling the preview anew.
pub(super) struct PreviewProgramSources {
    pub(super) previous_pipeline: Option<RawGpuPipeline>,
    pub(super) retained_template: Option<RawGpuProgramTemplate>,
    pub(super) startup_prewarm: Option<mpsc::Receiver<Result<RawGpuPipeline, String>>>,
}

/// Everything the decode worker needs, captured on the UI thread.
pub(super) struct DocumentLoadJob {
    pub(super) source: DocumentSource,
    pub(super) profile_reload: Option<ProfileReload>,
    /// Editing time carried over when a profile reload replaces the sidecar
    /// edits of the document that is already open.
    pub(super) editing_time_override_ms: Option<u64>,
    pub(super) raw_cache_key: String,
    pub(super) cached_original_raw: Option<Arc<LoadedRaw>>,
    pub(super) decode_gate: Arc<RwLock<()>>,
    pub(super) document_generation: u64,
    pub(super) initial_exposure: ExposureParams,
    pub(super) preview_quality: PreviewQuality,
    pub(super) viewport_pixels: [u32; 2],
    pub(super) camera_profiles: CameraProfileSettings,
    pub(super) ai_denoise_result_path: Option<PathBuf>,
    pub(super) device: wgpu::Device,
    pub(super) queue: wgpu::Queue,
    pub(super) programs: PreviewProgramSources,
    #[cfg(target_os = "android")]
    pub(super) android_app: calibraw_ffi::AndroidApp,
    #[cfg(target_os = "android")]
    pub(super) export_active: bool,
}

type SidecarLookup = Result<Option<crate::sidecar::LoadedSidecar>, crate::sidecar::SidecarError>;

pub(super) fn run_document_load(job: DocumentLoadJob) -> Result<LoadedPreview, LoadFailure> {
    let open_started = Instant::now();
    let DocumentLoadJob {
        source,
        profile_reload,
        editing_time_override_ms,
        raw_cache_key,
        cached_original_raw,
        decode_gate,
        document_generation,
        initial_exposure,
        preview_quality,
        viewport_pixels,
        camera_profiles,
        ai_denoise_result_path,
        device,
        queue,
        programs,
        #[cfg(target_os = "android")]
        android_app,
        #[cfg(target_os = "android")]
        export_active,
    } = job;
    // Android cannot afford two preview pipelines alongside a running export.
    #[cfg(target_os = "android")]
    let programs = if export_active {
        calibraw_core::diagnostics::record(
            "Released the previous Android preview before concurrent RAW open",
        );
        PreviewProgramSources {
            previous_pipeline: None,
            ..programs
        }
    } else {
        programs
    };

    let sidecar_started = Instant::now();
    let loaded_sidecar = load_sidecar_for_target(
        &source.sidecar_target,
        #[cfg(target_os = "android")]
        &android_app,
    );
    calibraw_core::diagnostics::record(format!(
        "RAW sidecar lookup finished in {:.3}s",
        sidecar_started.elapsed().as_secs_f64()
    ));
    let review = loaded_sidecar
        .as_ref()
        .ok()
        .and_then(Option::as_ref)
        .map(|loaded| loaded.review)
        .unwrap_or_default();
    let (reload_profile, reload_edits) = match profile_reload {
        Some(reload) => (Some(reload.camera_profile), Some(reload.edits)),
        None => (None, None),
    };
    let profile_request =
        CameraProfileRequest::resolve(reload_profile, &loaded_sidecar, &camera_profiles);

    let decode_was_cached = cached_original_raw.is_some();
    let decode_started = Instant::now();
    let decoded = decode_raw(
        &source.path,
        cached_original_raw,
        &decode_gate,
        &profile_request,
        &camera_profiles,
    );
    let decode_is_unsupported = decoded.as_ref().err().is_some_and(is_unsupported_raw_error);
    match &decoded {
        Ok(raw) => {
            calibraw_core::diagnostics::record(format!(
                "RAW decode finished in {:.3}s (cached={decode_was_cached})",
                decode_started.elapsed().as_secs_f64()
            ));
            calibraw_core::diagnostics::record_raw("Decoded RAW", raw);
        }
        Err(error) => calibraw_core::diagnostics::record(format!(
            "RAW decode failed after {:.3}s: {error:#}",
            decode_started.elapsed().as_secs_f64()
        )),
    }
    let source_path = source.persistent_path();
    if let Some(copy) = source.disposable_copy() {
        remove_temporary_raw(copy);
    }
    let DocumentSource {
        label,
        sidecar_target,
        raw_fd_guard,
        ..
    } = source;
    drop(raw_fd_guard);

    let edits = InitialEdits::resolve(
        reload_edits,
        editing_time_override_ms,
        loaded_sidecar,
        initial_exposure,
    );
    let result = (|| {
        let original_raw = decoded.map_err(|error| format!("{error:#}"))?;
        let InitialEdits {
            mut exposure,
            mut masks,
            remove,
            saved_lens,
            ai_masks_need_update,
            geometry,
            editing_time_ms,
            mut sidecar_warning,
            mut sidecar_needs_rewrite,
            use_adaptive_detail_defaults,
        } = edits;
        if use_adaptive_detail_defaults {
            original_raw.apply_adaptive_detail_defaults(&mut exposure);
        }
        record_edit_state(&exposure, &masks);

        let selected_camera_profile =
            profile_request.applied_profile(&camera_profiles, &original_raw);
        if profile_request.from_sidecar
            && profile_request.path.is_some()
            && selected_camera_profile.is_none()
        {
            sidecar_needs_rewrite = true;
            append_notice(
                &mut sidecar_warning,
                "The saved camera profile was not found or did not match this camera; \
                 automatic profile selection was used instead.",
            );
        }

        let (lens_correction, full_raw) =
            apply_saved_lens_correction(&original_raw, saved_lens, &mut sidecar_warning);
        restore_ai_denoise_result(&full_raw, &exposure, ai_denoise_result_path.as_deref())?;
        if full_raw.uses_opposed_chroma(&exposure) {
            let highlight_started = Instant::now();
            full_raw.inpaint_opposed_chroma_for_exposure(&exposure);
            calibraw_core::diagnostics::record(format!(
                "Full-resolution highlight analysis finished in {:.3}s",
                highlight_started.elapsed().as_secs_f64()
            ));
        }

        let preview_raw =
            build_preview_proxy(&full_raw, preview_quality, viewport_pixels, geometry);
        let initial_params =
            GpuParams::new(&exposure, &masks, &preview_raw).with_vignette_geometry(geometry);
        let pipeline =
            create_preview_pipeline(&device, &queue, &preview_raw, &initial_params, programs)?;
        let mask_source = if needs_canonical_mask_source(&masks) {
            Some(reconstruct_canonical_mask_source(
                &pipeline,
                &device,
                &queue,
                &full_raw,
                &preview_raw,
                &mut masks,
            )?)
        } else {
            None
        };
        let mask_upload_started = Instant::now();
        CalibRawApp::upload_preview_masks(&pipeline, &queue, &masks, &preview_raw)?;
        calibraw_core::diagnostics::record(format!(
            "Preview masks rasterized/uploaded in {:.3}s",
            mask_upload_started.elapsed().as_secs_f64()
        ));
        let params =
            GpuParams::new(&exposure, &masks, &preview_raw).with_vignette_geometry(geometry);
        let render_started = Instant::now();
        pipeline
            .recompute_with_remove(
                &queue,
                &device,
                &params,
                RemoveSceneContext::full_frame(&remove, &full_raw, &exposure),
            )
            .map_err(|error| format!("initial Remove scene integration failed: {error:#}"))?;
        calibraw_core::diagnostics::record(format!(
            "Initial GPU preview dispatch submitted in {:.3}s",
            render_started.elapsed().as_secs_f64()
        ));
        calibraw_core::diagnostics::record(format!(
            "RAW open worker finished in {:.3}s",
            open_started.elapsed().as_secs_f64()
        ));

        Ok(LoadedPreview {
            source_path,
            raw_cache_key,
            label: label.clone(),
            original_raw,
            full_raw,
            preview_raw,
            pipeline,
            review,
            rendered_exposure: exposure,
            rendered_masks: masks,
            remove,
            ai_masks_need_update,
            mask_source,
            lens_correction,
            sidecar_target,
            document_generation,
            sidecar_warning,
            sidecar_needs_rewrite,
            editing_time_ms,
            selected_camera_profile,
            geometry,
        })
    })();

    result.map_err(|message| {
        calibraw_core::diagnostics::record(format!(
            "RAW open worker failed after {:.3}s: {message}",
            open_started.elapsed().as_secs_f64()
        ));
        LoadFailure {
            label,
            message,
            unsupported: decode_is_unsupported,
        }
    })
}

/// The camera profile to request from the decoder.
struct CameraProfileRequest {
    path: Option<PathBuf>,
    /// The request came from the document's sidecar, so a mismatch is reported.
    from_sidecar: bool,
}

impl CameraProfileRequest {
    fn resolve(
        reload_profile: Option<Option<PathBuf>>,
        sidecar: &SidecarLookup,
        settings: &CameraProfileSettings,
    ) -> Self {
        if let Some(path) = reload_profile {
            return Self {
                path,
                from_sidecar: false,
            };
        }
        match sidecar {
            Ok(Some(loaded)) if crate::sidecar::edit_state_has_adjustments(&loaded.edits) => Self {
                // Sidecars store profiles relative to the profile folder; "." is the
                // folder itself, which selects the embedded camera matrix.
                path: loaded.edits.camera_profile.as_ref().and_then(|relative| {
                    settings.folder.as_ref().map(|root| {
                        if relative == Path::new(".") {
                            root.clone()
                        } else {
                            root.join(relative)
                        }
                    })
                }),
                from_sidecar: loaded.edits.camera_profile.is_some(),
            },
            // No sidecar, or one without adjustments: start like a first import.
            Ok(_) => Self {
                path: settings
                    .last_used
                    .as_ref()
                    .and_then(|relative| settings.folder.as_ref().map(|root| root.join(relative))),
                from_sidecar: false,
            },
            Err(_) => Self {
                path: None,
                from_sidecar: false,
            },
        }
    }

    fn selects_embedded_matrix(&self, settings: &CameraProfileSettings) -> bool {
        self.path
            .as_ref()
            .zip(settings.folder.as_ref())
            .is_some_and(|(selected, root)| selected == root)
    }

    /// The profile the decoder actually used, or `None` when it fell back to
    /// automatic selection.
    fn applied_profile(
        &self,
        settings: &CameraProfileSettings,
        raw: &LoadedRaw,
    ) -> Option<PathBuf> {
        if self.selects_embedded_matrix(settings) {
            return settings.folder.clone();
        }
        self.path
            .clone()
            .filter(|requested| raw.camera_profile_source.as_ref() == Some(requested))
    }
}

fn decode_raw(
    path: &Path,
    cached: Option<Arc<LoadedRaw>>,
    decode_gate: &RwLock<()>,
    profile: &CameraProfileRequest,
    settings: &CameraProfileSettings,
) -> anyhow::Result<Arc<LoadedRaw>> {
    if let Some(raw) = cached {
        return Ok(raw);
    }
    let embedded_matrix = profile.selects_embedded_matrix(settings);
    let mode = if embedded_matrix {
        CameraProfileMode::MatrixOnly
    } else {
        settings.mode
    };
    let _decode_guard = decode_gate
        .write()
        .map_err(|_| anyhow::anyhow!("RAW decode gate was poisoned"))?;
    load_raw_file_with_profile_selection(
        path,
        mode,
        settings.folder.as_deref(),
        profile.path.as_deref().filter(|_| !embedded_matrix),
    )
    .map(Arc::new)
}

/// The edit state a document opens with: a profile reload's in-memory edits,
/// the saved sidecar, or defaults.
struct InitialEdits {
    exposure: ExposureParams,
    masks: MaskStack,
    remove: RemoveEditState,
    saved_lens: Option<crate::sidecar::LensEditState>,
    ai_masks_need_update: bool,
    geometry: GeometryTransform,
    editing_time_ms: u64,
    sidecar_warning: Option<String>,
    sidecar_needs_rewrite: bool,
    /// Documents without saved edits adopt camera-specific detail defaults.
    use_adaptive_detail_defaults: bool,
}

impl InitialEdits {
    fn resolve(
        reload_edits: Option<SidecarEditState>,
        editing_time_override_ms: Option<u64>,
        sidecar: SidecarLookup,
        default_exposure: ExposureParams,
    ) -> Self {
        if let Some(edits) = reload_edits {
            return Self::from_edits(edits, editing_time_override_ms.unwrap_or(0), None, true);
        }
        match sidecar {
            // A sidecar without adjustments (written by a rating or a reset)
            // opens like a first import, so per-camera defaults still apply.
            Ok(Some(loaded)) if !crate::sidecar::edit_state_has_adjustments(&loaded.edits) => {
                Self {
                    editing_time_ms: loaded.editing_time_ms,
                    sidecar_needs_rewrite: loaded.migrated,
                    ..Self::defaults(default_exposure, None)
                }
            }
            Ok(Some(loaded)) => {
                let warning = loaded.migrated.then(|| {
                    "Loaded edits were migrated to the current sidecar format.".to_owned()
                });
                Self::from_edits(
                    loaded.edits,
                    loaded.editing_time_ms,
                    warning,
                    loaded.migrated,
                )
            }
            Ok(None) => Self::defaults(default_exposure, None),
            Err(error) => Self::defaults(
                default_exposure,
                Some(format!(
                    "Could not load this RAW's sidecar; using default edits: {error}"
                )),
            ),
        }
    }

    fn from_edits(
        edits: SidecarEditState,
        editing_time_ms: u64,
        sidecar_warning: Option<String>,
        sidecar_needs_rewrite: bool,
    ) -> Self {
        Self {
            exposure: edits.exposure,
            masks: Arc::unwrap_or_clone(edits.masks),
            remove: Arc::unwrap_or_clone(edits.remove),
            saved_lens: Some(edits.lens),
            ai_masks_need_update: edits.ai_masks_need_update,
            geometry: edits.geometry.sanitized(),
            editing_time_ms,
            sidecar_warning,
            sidecar_needs_rewrite,
            use_adaptive_detail_defaults: false,
        }
    }

    fn defaults(exposure: ExposureParams, sidecar_warning: Option<String>) -> Self {
        Self {
            exposure,
            masks: MaskStack::default(),
            remove: RemoveEditState::default(),
            saved_lens: None,
            ai_masks_need_update: false,
            geometry: GeometryTransform::default(),
            editing_time_ms: 0,
            sidecar_warning,
            sidecar_needs_rewrite: false,
            use_adaptive_detail_defaults: true,
        }
    }
}

fn record_edit_state(exposure: &ExposureParams, masks: &MaskStack) {
    calibraw_core::diagnostics::record(format!(
        "Edit state: exposure={:.3} temperature={:.3} tint={:.3} saturation={:.3} \
         vibrance={:.3} luminance_nr={:.1} color_nr={:.1} demosaic={:?} highlight={:?} masks={}",
        exposure.exposure,
        exposure.temperature,
        exposure.tint,
        exposure.saturation,
        exposure.vibrance,
        exposure.luminance_denoise,
        exposure.chroma_denoise * 100.0,
        exposure.demosaic_mode,
        exposure.highlight_method,
        masks.masks.len(),
    ));
}

/// Restores the sidecar's lens selection and applies it at full resolution.
/// Returns the lens state and the RAW to develop, which is the original when
/// correction is off or fails.
fn apply_saved_lens_correction(
    original_raw: &Arc<LoadedRaw>,
    saved_lens: Option<crate::sidecar::LensEditState>,
    sidecar_warning: &mut Option<String>,
) -> (LensCorrectionState, Arc<LoadedRaw>) {
    let lens_started = Instant::now();
    let mut lens_correction = LensCorrectionState::from_catalog(lensfun_catalog(original_raw));
    calibraw_core::diagnostics::record(format!(
        "Lensfun catalog lookup finished in {:.3}s",
        lens_started.elapsed().as_secs_f64()
    ));
    if let Some(saved) = saved_lens {
        lens_correction.selected_maker = saved.maker;
        lens_correction.selected_model = saved.model;
        lens_correction.enabled = saved.enabled && lens_correction.catalog.available;
        if saved.enabled && !lens_correction.catalog.available {
            append_notice(
                sidecar_warning,
                "The saved lens correction is unavailable in this build.",
            );
        }
    }
    let selection = lens_correction
        .enabled
        .then(|| lens_correction.selected_lens())
        .flatten();
    let full_raw = match selection {
        None => {
            lens_correction.enabled = false;
            Arc::clone(original_raw)
        }
        Some(selection) => {
            let apply_started = Instant::now();
            match apply_lensfun_correction(original_raw, &selection) {
                Ok(corrected) => {
                    calibraw_core::diagnostics::record(format!(
                        "Lensfun full-resolution correction applied in {:.3}s",
                        apply_started.elapsed().as_secs_f64()
                    ));
                    lens_correction.applied = true;
                    lens_correction.catalog.status = format!(
                        "Automatically applied {} from RAW metadata",
                        selection.label()
                    );
                    Arc::new(corrected)
                }
                Err(error) => {
                    calibraw_core::diagnostics::record(format!(
                        "Lensfun full-resolution correction failed after {:.3}s",
                        apply_started.elapsed().as_secs_f64()
                    ));
                    lens_correction.enabled = false;
                    lens_correction.applied = false;
                    lens_correction.catalog.status = format!(
                        "Matched {}, but correction failed: {error:#}",
                        selection.label()
                    );
                    append_notice(
                        sidecar_warning,
                        "The saved lens correction failed; the original geometry is shown.",
                    );
                    Arc::clone(original_raw)
                }
            }
        }
    };
    calibraw_core::diagnostics::record(format!(
        "Lensfun catalog/correction prepared in {:.3}s",
        lens_started.elapsed().as_secs_f64()
    ));
    (lens_correction, full_raw)
}

/// Installs the RawNIND result saved next to the RAW when AI denoise is
/// enabled, or drops any installed result when it is not. A result that no
/// longer matches the RAW is deleted; after the open the user is asked before
/// the model runs again.
fn restore_ai_denoise_result(
    raw: &LoadedRaw,
    exposure: &ExposureParams,
    result_path: Option<&Path>,
) -> Result<(), String> {
    if !exposure.ai_denoise_enabled {
        raw.clear_ai_denoised_image();
        return Ok(());
    }
    if raw.ai_denoised_image().is_some() {
        return Ok(());
    }
    let Some(result_path) = result_path else {
        return Ok(());
    };
    let restore_started = Instant::now();
    match calibraw_ai::ai_denoise::load_saved_result(result_path, raw) {
        Ok(Some(image)) => {
            raw.set_ai_denoised_image(image)
                .map_err(|error| format!("could not install saved AI-denoise result: {error:#}"))?;
            calibraw_core::diagnostics::record(format!(
                "AI-denoise result restored in {:.3}s from {}",
                restore_started.elapsed().as_secs_f64(),
                result_path.display()
            ));
        }
        Ok(None) => calibraw_core::diagnostics::record(
            "No saved AI-denoise result; the user is asked after open",
        ),
        Err(error) => {
            log::warn!(
                "discarding invalid AI-denoise result {}: {error:#}",
                result_path.display()
            );
            calibraw_core::diagnostics::record(format!("AI-denoise result rejected: {error:#}"));
            match std::fs::remove_file(result_path) {
                Err(remove_error) if remove_error.kind() != std::io::ErrorKind::NotFound => {
                    log::warn!(
                        "could not remove invalid AI-denoise result {}: {remove_error}",
                        result_path.display()
                    );
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn build_preview_proxy(
    full_raw: &Arc<LoadedRaw>,
    quality: PreviewQuality,
    viewport_pixels: [u32; 2],
    geometry: GeometryTransform,
) -> Arc<LoadedRaw> {
    let spec = ProxySpec {
        max_edge: quality.proxy_edge_for_fitted_source(
            viewport_pixels,
            full_raw.width,
            full_raw.height,
            geometry,
        ),
    };
    let proxy_started = Instant::now();
    let preview_raw = if full_raw.width.max(full_raw.height) <= spec.max_edge {
        Arc::clone(full_raw)
    } else {
        Arc::new(build_proxy(full_raw, spec))
    };
    calibraw_core::diagnostics::record(format!(
        "Preview proxy prepared in {:.3}s: {}x{} -> {}x{}",
        proxy_started.elapsed().as_secs_f64(),
        full_raw.width,
        full_raw.height,
        preview_raw.width,
        preview_raw.height
    ));
    calibraw_core::diagnostics::record_raw("Preview proxy", &preview_raw);
    preview_raw
}

/// Builds the preview pipeline, reusing compiled programs from the previous
/// document or the startup prewarm when available.
fn create_preview_pipeline(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    preview_raw: &LoadedRaw,
    params: &GpuParams,
    programs: PreviewProgramSources,
) -> Result<RawGpuPipeline, String> {
    let PreviewProgramSources {
        previous_pipeline,
        retained_template,
        startup_prewarm,
    } = programs;
    // Only wait for the startup prewarm when nothing else can provide programs.
    let prewarmed_pipeline = startup_prewarm
        .filter(|_| previous_pipeline.is_none() && retained_template.is_none())
        .and_then(|receiver| {
            let wait_started = Instant::now();
            match receiver.recv() {
                Ok(Ok(pipeline)) => {
                    calibraw_core::diagnostics::record(format!(
                        "GPU preview startup prewarm available after {:.3}s wait",
                        wait_started.elapsed().as_secs_f64()
                    ));
                    Some(pipeline)
                }
                Ok(Err(error)) => {
                    calibraw_core::diagnostics::record(error);
                    None
                }
                Err(error) => {
                    calibraw_core::diagnostics::record(format!(
                        "GPU preview startup prewarm unavailable: {error}"
                    ));
                    None
                }
            }
        });
    let template = previous_pipeline
        .as_ref()
        .map(RawGpuPipeline::program_template)
        .or(retained_template)
        .or_else(|| {
            prewarmed_pipeline
                .as_ref()
                .map(RawGpuPipeline::program_template)
        });

    let quality = ProcessingQuality::Preview;
    let compile = || {
        RawGpuPipeline::new(
            device,
            queue,
            preview_raw,
            params,
            PipelineOptions::new(quality),
        )
        .map_err(|error| format!("GPU preview setup failed: {error:#}"))
    };
    let pipeline_started = Instant::now();
    let pipeline = match template {
        Some(template) => match RawGpuPipeline::new(
            device,
            queue,
            preview_raw,
            params,
            PipelineOptions::new(quality).programs(&template),
        ) {
            Ok(pipeline) => {
                calibraw_core::diagnostics::record("GPU preview reused precompiled programs");
                pipeline
            }
            Err(reuse_error) => {
                calibraw_core::diagnostics::record(format!(
                    "GPU preview program reuse unavailable ({reuse_error:#}); compiling programs"
                ));
                compile()?
            }
        },
        None => compile()?,
    };
    calibraw_core::diagnostics::record(format!(
        "GPU preview pipeline created in {:.3}s",
        pipeline_started.elapsed().as_secs_f64()
    ));
    Ok(pipeline)
}

/// Range masks sample a canonical (default-edit) rendering of the image. When
/// a sidecar lacks that source, render it with the new pipeline and install it
/// into the masks that need it.
fn reconstruct_canonical_mask_source(
    pipeline: &RawGpuPipeline,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    full_raw: &LoadedRaw,
    preview_raw: &LoadedRaw,
    masks: &mut MaskStack,
) -> Result<MaskRgbImage, String> {
    let started = Instant::now();
    let reference_exposure = ExposureParams::scene_referred_default();
    if full_raw.uses_opposed_chroma(&reference_exposure) {
        full_raw.inpaint_opposed_chroma_for_exposure(&reference_exposure);
    }
    let reference_masks = MaskStack::default();
    let reference_params = GpuParams::new(&reference_exposure, &reference_masks, preview_raw);
    pipeline.recompute(queue, device, &reference_params);
    let rgba = pipeline
        .read_output_region_blocking(device, queue, 0, 0, pipeline.width, pipeline.height)
        .map_err(|error| format!("range-mask source readback failed: {error:#}"))?;
    let source = MaskRgbImage::new(pipeline.width, pipeline.height, rgba)
        .ok_or_else(|| "range-mask source dimensions are invalid".to_owned())?;
    install_missing_range_sources(masks, &source);
    calibraw_core::diagnostics::record(format!(
        "Canonical mask source reconstructed with the preview pipeline in {:.3}s",
        started.elapsed().as_secs_f64()
    ));
    Ok(source)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sidecar::{default_edit_state, LoadedSidecar, PhotoReview, SidecarError};

    fn settings() -> CameraProfileSettings {
        CameraProfileSettings {
            mode: CameraProfileMode::default(),
            folder: Some(PathBuf::from("/profiles")),
            last_used: Some(PathBuf::from("Last.dcp")),
        }
    }

    fn sidecar_with_profile(camera_profile: Option<&str>) -> SidecarLookup {
        let mut edits = default_edit_state();
        edits.camera_profile = camera_profile.map(PathBuf::from);
        Ok(Some(LoadedSidecar {
            edits,
            review: PhotoReview::default(),
            editing_time_ms: 42,
            migrated: false,
        }))
    }

    #[test]
    fn only_unguarded_imports_are_disposable_and_only_plain_paths_persist() {
        let mut source = DocumentSource::desktop(PathBuf::from("/raw/a.nef"), "a".to_owned());
        assert_eq!(source.disposable_copy(), None);
        assert_eq!(source.persistent_path(), Some(PathBuf::from("/raw/a.nef")));

        source.delete_after_decode = true;
        assert_eq!(source.disposable_copy(), Some(Path::new("/raw/a.nef")));
        assert_eq!(source.persistent_path(), None);

        source.raw_fd_guard = Some(std::fs::File::open(std::env::current_exe().unwrap()).unwrap());
        assert_eq!(source.disposable_copy(), None);
        assert_eq!(source.persistent_path(), None);
    }

    #[test]
    fn camera_profile_request_prefers_reload_then_sidecar_then_last_used() {
        let settings = settings();
        let reload = CameraProfileRequest::resolve(
            Some(None),
            &sidecar_with_profile(Some("Saved.dcp")),
            &settings,
        );
        assert_eq!(reload.path, None);
        assert!(!reload.from_sidecar);

        let saved = CameraProfileRequest::resolve(
            None,
            &sidecar_with_profile(Some("Saved.dcp")),
            &settings,
        );
        assert_eq!(saved.path, Some(PathBuf::from("/profiles/Saved.dcp")));
        assert!(saved.from_sidecar);
        assert!(!saved.selects_embedded_matrix(&settings));

        let embedded =
            CameraProfileRequest::resolve(None, &sidecar_with_profile(Some(".")), &settings);
        assert_eq!(embedded.path, Some(PathBuf::from("/profiles")));
        assert!(embedded.selects_embedded_matrix(&settings));

        let unsaved = CameraProfileRequest::resolve(None, &Ok(None), &settings);
        assert_eq!(unsaved.path, Some(PathBuf::from("/profiles/Last.dcp")));
        assert!(!unsaved.from_sidecar);

        // A sidecar without adjustments selects a profile like a first import.
        let unadjusted =
            CameraProfileRequest::resolve(None, &sidecar_with_profile(None), &settings);
        assert_eq!(unadjusted.path, Some(PathBuf::from("/profiles/Last.dcp")));
        assert!(!unadjusted.from_sidecar);

        let unreadable = CameraProfileRequest::resolve(
            None,
            &Err(SidecarError::Invalid("corrupt".to_owned())),
            &settings,
        );
        assert_eq!(unreadable.path, None);
    }

    #[test]
    fn initial_edits_come_from_reload_sidecar_or_defaults() {
        let defaults = ExposureParams::scene_referred_default();

        let reloaded =
            InitialEdits::resolve(Some(default_edit_state()), Some(7), Ok(None), defaults);
        assert!(reloaded.sidecar_needs_rewrite);
        assert_eq!(reloaded.editing_time_ms, 7);
        assert!(!reloaded.use_adaptive_detail_defaults);

        let mut edited = sidecar_with_profile(None);
        if let Ok(Some(loaded)) = &mut edited {
            loaded.edits.exposure.exposure = 0.5;
        }
        let saved = InitialEdits::resolve(None, None, edited, defaults);
        assert!(!saved.sidecar_needs_rewrite);
        assert_eq!(saved.editing_time_ms, 42);
        assert!(saved.saved_lens.is_some());
        assert_eq!(saved.exposure.exposure, 0.5);
        assert!(!saved.use_adaptive_detail_defaults);

        // A rating or a reset leaves a sidecar without adjustments; it opens like
        // a first import (camera defaults apply) and keeps its editing time.
        let mut sticky = defaults;
        sticky.demosaic_mode = crate::pipeline::DemosaicMode::Dual;
        let unadjusted = InitialEdits::resolve(None, None, sidecar_with_profile(None), sticky);
        assert!(unadjusted.use_adaptive_detail_defaults);
        assert_eq!(unadjusted.editing_time_ms, 42);
        assert!(unadjusted.saved_lens.is_none());
        assert_eq!(unadjusted.exposure, sticky);
        assert!(unadjusted.sidecar_warning.is_none());

        let fresh = InitialEdits::resolve(None, None, Ok(None), defaults);
        assert!(fresh.use_adaptive_detail_defaults);
        assert!(fresh.sidecar_warning.is_none());

        let unreadable = InitialEdits::resolve(
            None,
            None,
            Err(SidecarError::Invalid("corrupt".to_owned())),
            defaults,
        );
        assert!(unreadable.use_adaptive_detail_defaults);
        assert!(unreadable
            .sidecar_warning
            .is_some_and(|warning| warning.contains("using default edits")));
    }
}
