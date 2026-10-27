//! Worker-thread half of opening a RAW document: read the sidecar, decode,
//! restore saved lens correction and AI-denoise results, and render the first
//! GPU preview. The UI-thread half lives in `documents.rs`.

use super::*;
use std::sync::RwLock;

/// The load notice for edits migrated to the current sidecar format. It is
/// informational; every other load warning reports a problem.
pub(super) const SIDECAR_MIGRATED_NOTICE: &str =
    "Loaded edits were migrated to the current sidecar format.";

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
    /// The view the reopened document is shown at; the UI thread keeps it
    /// and the decode worker ignores it.
    pub(crate) view: Option<KeptView>,
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
    pub(super) decoded_raws: DecodedRawCache,
    pub(super) decode_gate: Arc<RwLock<()>>,
    pub(super) document_generation: u64,
    pub(super) initial_exposure: ExposureParams,
    pub(super) preview_quality: PreviewQuality,
    pub(super) fitted_viewport_pixels: [u32; 2],
    pub(super) camera_profiles: CameraProfileSettings,
    /// Reads the edit's LUT; `None` without a LUT library.
    pub(super) luts: Option<crate::luts::LutStore>,
    pub(super) automatic_lens: AutomaticLensCorrection,
    pub(super) ai_denoise_result_path: Option<PathBuf>,
    pub(super) people_analysis_path: Option<PathBuf>,
    pub(super) device: wgpu::Device,
    pub(super) queue: wgpu::Queue,
    pub(super) programs: PreviewProgramSources,
    pub(super) platform: Arc<dyn Platform>,
    /// Release the previous preview pipeline before decoding: set where
    /// memory is constrained and an export is running.
    pub(super) release_previous_preview: bool,
}

type SidecarLookup = Result<Option<crate::sidecar::LoadedSidecar>, crate::sidecar::SidecarError>;

pub(super) fn run_document_load(job: DocumentLoadJob) -> Result<LoadedPreview, LoadFailure> {
    let open_started = Instant::now();
    let DocumentLoadJob {
        source,
        profile_reload,
        editing_time_override_ms,
        decoded_raws,
        decode_gate,
        document_generation,
        initial_exposure,
        preview_quality,
        fitted_viewport_pixels,
        camera_profiles,
        luts,
        automatic_lens,
        ai_denoise_result_path,
        people_analysis_path,
        device,
        queue,
        programs,
        platform,
        release_previous_preview,
    } = job;
    // Constrained memory cannot afford two preview pipelines alongside a running export.
    let programs = if release_previous_preview {
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
    let loaded_sidecar = load_sidecar_for_target(&source.sidecar_target, &*platform);
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

    let decode_started = Instant::now();
    let decoded = decode_cached(
        &decoded_raws,
        &source,
        decode_request(&profile_request, &camera_profiles),
        &decode_gate,
        DecodePriority::Interactive,
    );
    let decode_is_unsupported = decoded.as_ref().err().is_some_and(is_unsupported_raw_error);
    match &decoded {
        Ok(decoded) => {
            calibraw_core::diagnostics::record(format!(
                "RAW decode ready in {:.3}s",
                decode_started.elapsed().as_secs_f64()
            ));
            calibraw_core::diagnostics::record_raw("Decoded RAW", &decoded.original);
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
        let decoded = decoded.map_err(|error| format!("{error:#}"))?;
        let original_raw = Arc::clone(&decoded.original);
        let InitialEdits {
            mut exposure,
            mut masks,
            remove,
            look_lut,
            saved_lens,
            ai_masks_need_update,
            geometry,
            editing_time_ms,
            mut sidecar_warning,
            mut sidecar_needs_rewrite,
            use_adaptive_detail_defaults,
            starts_unedited,
        } = edits;
        if use_adaptive_detail_defaults {
            original_raw.apply_adaptive_detail_defaults(&mut exposure);
        }
        // The form this photo keeps (`LoadedRaw::resolve_white_balance`): as
        // shot without a white-balance model, where older versions left pasted
        // shifts hidden, and a custom white balance limited to this camera.
        // Saved again when it changes.
        // migration: remove in v2.0.0 the conversion of offsets saved up to
        // v1.3 that also happens here (`WhiteBalance::LegacyOffset`).
        let white_balance = original_raw.resolve_white_balance(exposure.white_balance);
        if white_balance != exposure.white_balance {
            exposure.white_balance = white_balance;
            sidecar_needs_rewrite = true;
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

        // A LUT the library does not hold stays in the edit, so it applies
        // again once the LUT is imported; the photo renders without it.
        let look_lut_table = look_lut.as_ref().and_then(|look| {
            match luts.as_ref().map(|store| store.load(look.id)) {
                Some(Ok(table)) => Some(table),
                Some(Err(crate::luts::LutLibraryError::NotFound(_))) | None => {
                    append_notice(&mut sidecar_warning, &missing_look_lut_notice(look));
                    None
                }
                Some(Err(error)) => {
                    append_notice(
                        &mut sidecar_warning,
                        &format!("Could not read the LUT “{}”: {error}", look.name),
                    );
                    None
                }
            }
        });
        let look = look_lut
            .as_ref()
            .zip(look_lut_table.as_ref())
            .map(|(look, table)| crate::luts::ResolvedLook {
                lut: Arc::clone(table),
                amount: look.amount,
            });

        let lens_state_saved = saved_lens.is_some();
        let (lens_correction, full_raw) =
            apply_saved_lens_correction(&decoded, saved_lens, automatic_lens, &mut sidecar_warning);
        let lens_profile_missing = !lens_state_saved
            && automatic_lens.enabled
            && lens_correction.catalog.available
            && lens_correction.catalog.auto_match.is_none()
            && !original_raw.is_display_referred_raster();
        restore_ai_denoise_result(&full_raw, &exposure, ai_denoise_result_path.as_deref())?;
        let people_analysis =
            load_people_analysis(people_analysis_path.as_deref(), &masks, &full_raw, &remove);
        prepare_highlight_analysis(&full_raw, &exposure);

        let preview_raw = build_preview_proxy(
            &full_raw,
            preview_quality,
            fitted_viewport_pixels,
            geometry,
            masks.masks.len(),
        );
        let initial_params = GpuParams::new(&exposure, &masks, &preview_raw)
            .with_vignette_geometry(geometry)
            .with_look_lut(look.as_ref());
        let pipeline = create_preview_pipeline(
            &device,
            &queue,
            &preview_raw,
            &initial_params,
            programs,
            masks.masks.len(),
        )?;
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
        let params = GpuParams::new(&exposure, &masks, &preview_raw)
            .with_vignette_geometry(geometry)
            .with_look_lut(look.as_ref());
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
            people_analysis,
            lens_correction,
            lens_profile_missing,
            sidecar_target,
            document_generation,
            sidecar_warning,
            sidecar_needs_rewrite,
            editing_time_ms,
            selected_camera_profile,
            look_lut,
            look_lut_table,
            geometry,
            starts_unedited,
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

/// What preparing a document ahead of its open needs, captured on the UI thread.
pub(super) struct AheadOfOpen {
    pub(super) platform: Arc<dyn Platform>,
    pub(super) decoded_raws: DecodedRawCache,
    pub(super) decode_gate: Arc<RwLock<()>>,
    pub(super) camera_profiles: CameraProfileSettings,
    pub(super) automatic_lens: AutomaticLensCorrection,
    pub(super) initial_exposure: ExposureParams,
    pub(super) program_template: Option<RawGpuProgramTemplate>,
}

/// Runs the work opening the photo at `path` would wait for and the caches
/// keep: the decode, lens correction, highlight analysis and the GPU program
/// its edits select. It follows [`run_document_load`] step by step, so the
/// later open finds exactly what it asks for. `cancelled` is checked between
/// steps.
pub(super) fn prepare_ahead_of_open(
    context: &AheadOfOpen,
    path: &Path,
    cancelled: impl Fn() -> bool,
) -> anyhow::Result<()> {
    let source = DocumentSource::desktop(path.to_owned(), String::new());
    let loaded_sidecar = load_sidecar_for_target(&source.sidecar_target, &*context.platform);
    let profile_request =
        CameraProfileRequest::resolve(None, &loaded_sidecar, &context.camera_profiles);
    let decoded = decode_cached(
        &context.decoded_raws,
        &source,
        decode_request(&profile_request, &context.camera_profiles),
        &context.decode_gate,
        DecodePriority::Background,
    )?;
    if cancelled() {
        return Ok(());
    }

    let InitialEdits {
        mut exposure,
        saved_lens,
        use_adaptive_detail_defaults,
        ..
    } = InitialEdits::resolve(None, None, loaded_sidecar, context.initial_exposure);
    if use_adaptive_detail_defaults {
        decoded
            .original
            .apply_adaptive_detail_defaults(&mut exposure);
    }
    let (_, full_raw) =
        apply_saved_lens_correction(&decoded, saved_lens, context.automatic_lens, &mut None);
    if cancelled() {
        return Ok(());
    }

    // With AI denoise the analysis reads the denoised image, which only the
    // open installs.
    if !exposure.ai_denoise_enabled {
        prepare_highlight_analysis(&full_raw, &exposure);
    }
    if let Some(template) = &context.program_template {
        template.compile_for_params(&GpuParams::new(&exposure, &MaskStack::default(), &full_raw));
    }
    Ok(())
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
            .filter(|requested| raw.profile_color.camera_profile_source.as_ref() == Some(requested))
    }
}

/// The decoder inputs that choosing `selection` in the Camera profile card
/// resolves to, as a reload for it requests them.
pub(super) fn selection_decode_request(
    selection: Option<PathBuf>,
    settings: &CameraProfileSettings,
) -> DecodeRequest {
    decode_request(
        &CameraProfileRequest {
            path: selection,
            from_sidecar: false,
        },
        settings,
    )
}

/// The decoder inputs that a camera-profile request resolves to.
fn decode_request(
    profile: &CameraProfileRequest,
    settings: &CameraProfileSettings,
) -> DecodeRequest {
    let embedded_matrix = profile.selects_embedded_matrix(settings);
    DecodeRequest {
        mode: if embedded_matrix {
            CameraProfileMode::MatrixOnly
        } else {
            settings.mode
        },
        folder: settings.folder.clone(),
        profile: profile.path.clone().filter(|_| !embedded_matrix),
    }
}

/// A decode of the document and the RAW it opens with: the decode with the
/// camera profile the document's request chooses.
struct RequestedDecode {
    decoded: Arc<DecodedRaw>,
    request: DecodeRequest,
    original: Arc<LoadedRaw>,
}

/// Whether a decode is the one the user waits for, or background work.
#[derive(Clone, Copy)]
enum DecodePriority {
    /// Runs alone under the decode gate.
    Interactive,
    /// Shares the decode gate with thumbnail decodes and yields to interactive ones.
    Background,
}

/// The decoded RAW for `source` and `request`, from the cache or decoded now.
/// A cached decode serves another camera-profile request without decoding
/// again, as long as it can match DCPs when `request` needs that.
fn decode_cached(
    cache: &DecodedRawCache,
    source: &DocumentSource,
    request: DecodeRequest,
    decode_gate: &RwLock<()>,
    priority: DecodePriority,
) -> anyhow::Result<RequestedDecode> {
    let key = DecodeKey::new(raw_cache_key_for_target(&source.sidecar_target), &request);
    // A decode that can match DCPs also serves the embedded matrix.
    let matching = DecodeKey {
        matches_dcps: true,
        ..key.clone()
    };
    let ready = (!key.matches_dcps)
        .then(|| cache.get_ready(&matching))
        .flatten();
    let decoded = match ready {
        Some(decoded) => decoded,
        None => cache.get_or_decode(&key, &request, || {
            let decode = || {
                let started = Instant::now();
                let raw = load_raw_file_with_profile_selection(
                    &source.path,
                    request.mode,
                    request.folder.as_deref(),
                    request.profile.as_deref(),
                );
                calibraw_core::diagnostics::record(format!(
                    "RAW decoded in {:.3}s",
                    started.elapsed().as_secs_f64()
                ));
                raw
            };
            let poisoned = || anyhow::anyhow!("RAW decode gate was poisoned");
            match priority {
                DecodePriority::Interactive => {
                    let _exclusive = decode_gate.write().map_err(|_| poisoned())?;
                    decode()
                }
                DecodePriority::Background => {
                    let _shared = decode_gate.read().map_err(|_| poisoned())?;
                    decode()
                }
            }
        })?,
    };
    let original = decoded.original_for(&request)?;
    Ok(RequestedDecode {
        decoded,
        request,
        original,
    })
}

/// Runs the full-resolution highlight analysis `exposure` needs. Its result is
/// cached in `full_raw`, so preparing it ahead of an open is reused there.
fn prepare_highlight_analysis(full_raw: &LoadedRaw, exposure: &ExposureParams) {
    if full_raw.uses_opposed_chroma(exposure) {
        let highlight_started = Instant::now();
        full_raw.inpaint_opposed_chroma_for_exposure(exposure);
        calibraw_core::diagnostics::record(format!(
            "Full-resolution highlight analysis finished in {:.3}s",
            highlight_started.elapsed().as_secs_f64()
        ));
    }
}

/// The edit state a document opens with: a profile reload's in-memory edits,
/// the saved sidecar, or defaults.
struct InitialEdits {
    exposure: ExposureParams,
    masks: MaskStack,
    remove: RemoveEditState,
    look_lut: Option<crate::luts::LookLut>,
    saved_lens: Option<crate::sidecar::LensEditState>,
    ai_masks_need_update: bool,
    geometry: GeometryTransform,
    editing_time_ms: u64,
    sidecar_warning: Option<String>,
    sidecar_needs_rewrite: bool,
    /// Documents without saved edits adopt camera-specific detail defaults.
    use_adaptive_detail_defaults: bool,
    starts_unedited: bool,
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
                let warning = loaded.migrated.then(|| SIDECAR_MIGRATED_NOTICE.to_owned());
                Self::from_edits(
                    loaded.edits,
                    loaded.editing_time_ms,
                    warning,
                    loaded.migrated,
                )
            }
            Ok(None) => Self::defaults(default_exposure, None),
            Err(error) => Self {
                starts_unedited: false,
                ..Self::defaults(
                    default_exposure,
                    Some(format!(
                        "Could not load this RAW's sidecar; using default edits: {error}"
                    )),
                )
            },
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
            look_lut: edits.look_lut,
            saved_lens: Some(edits.lens),
            ai_masks_need_update: edits.ai_masks_need_update,
            geometry: edits.geometry.sanitized(),
            editing_time_ms,
            sidecar_warning,
            sidecar_needs_rewrite,
            use_adaptive_detail_defaults: false,
            starts_unedited: false,
        }
    }

    fn defaults(exposure: ExposureParams, sidecar_warning: Option<String>) -> Self {
        Self {
            exposure,
            masks: MaskStack::default(),
            remove: RemoveEditState::default(),
            look_lut: None,
            saved_lens: None,
            ai_masks_need_update: false,
            geometry: GeometryTransform::default(),
            editing_time_ms: 0,
            sidecar_warning,
            sidecar_needs_rewrite: false,
            use_adaptive_detail_defaults: true,
            starts_unedited: true,
        }
    }
}

fn record_edit_state(exposure: &ExposureParams, masks: &MaskStack) {
    calibraw_core::diagnostics::record(format!(
        "Edit state: exposure={:.3} white_balance={:?} saturation={:.3} \
         vibrance={:.3} luminance_nr={:.1} color_nr={:.1} demosaic={:?} highlight={:?} masks={}",
        exposure.exposure,
        exposure.white_balance,
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
/// Photos without a saved lens state follow the automatic-correction setting.
/// Returns the lens state and the RAW to develop, which is the original when
/// correction is off or fails.
fn apply_saved_lens_correction(
    decoded: &RequestedDecode,
    saved_lens: Option<crate::sidecar::LensEditState>,
    automatic_lens: AutomaticLensCorrection,
    sidecar_warning: &mut Option<String>,
) -> (LensCorrectionState, Arc<LoadedRaw>) {
    let lens_started = Instant::now();
    let original_raw = &decoded.original;
    let mut lens_correction =
        LensCorrectionState::automatic(decoded.decoded.lens_catalog(), automatic_lens);
    calibraw_core::diagnostics::record(format!(
        "Lensfun catalog lookup finished in {:.3}s",
        lens_started.elapsed().as_secs_f64()
    ));
    if let Some(saved) = saved_lens {
        lens_correction.corrections = saved.corrections();
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
    let selection = lens_correction.requested_correction();
    let full_raw = match selection {
        None => {
            lens_correction.enabled = false;
            Arc::clone(original_raw)
        }
        Some(selection) => {
            let apply_started = Instant::now();
            match decoded
                .decoded
                .lens_corrected_for(&decoded.request, &selection, |raw| {
                    apply_lens_correction(raw, &selection)
                }) {
                Ok(corrected) => {
                    calibraw_core::diagnostics::record(format!(
                        "Lensfun full-resolution correction ready in {:.3}s",
                        apply_started.elapsed().as_secs_f64()
                    ));
                    lens_correction.applied = true;
                    lens_correction.catalog.status = format!(
                        "Automatically applied {} from RAW metadata",
                        selection.label()
                    );
                    corrected
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
    match calibraw_ai::models::denoise::load_saved_result(result_path, raw) {
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

/// The people analysis kept beside the photo, when its edit has a people
/// mask and the analysis is of the content the photo has now. One that cannot
/// be read is only a cache: the people are found again when a mask needs them.
fn load_people_analysis(
    path: Option<&Path>,
    masks: &MaskStack,
    raw: &LoadedRaw,
    remove: &RemoveEditState,
) -> Option<PeopleAnalysis> {
    let path = path.filter(|path| path.is_file())?;
    let has_people = masks
        .masks
        .iter()
        .flat_map(|mask| &mask.components)
        .any(|component| component.kind == MaskKind::Person);
    if !has_people {
        return None;
    }
    let started = Instant::now();
    let loaded = calibraw_ai::models::people::content_fingerprint(raw, remove)
        .and_then(|content| PeopleAnalysis::load(path, content));
    match loaded {
        Ok(Some(analysis)) => {
            calibraw_core::diagnostics::record(format!(
                "People analysis restored in {:.3}s from {}",
                started.elapsed().as_secs_f64(),
                path.display()
            ));
            Some(analysis)
        }
        Ok(None) => {
            calibraw_core::diagnostics::record(
                "The saved people analysis is of other content; people are found again",
            );
            None
        }
        Err(error) => {
            log::warn!("ignoring the people analysis {}: {error:#}", path.display());
            calibraw_core::diagnostics::record(format!("People analysis rejected: {error:#}"));
            None
        }
    }
}

fn build_preview_proxy(
    full_raw: &Arc<LoadedRaw>,
    quality: PreviewQuality,
    fitted_viewport_pixels: [u32; 2],
    geometry: GeometryTransform,
    mask_count: usize,
) -> Arc<LoadedRaw> {
    let spec = ProxySpec {
        max_edge: quality.proxy_edge_for_fitted_source(
            fitted_viewport_pixels,
            full_raw.width,
            full_raw.height,
            geometry,
            full_raw.cfa_kind,
            mask_count,
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
    mask_count: usize,
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

    let options =
        fitted_pipeline_options(mask_count).profile_preview(reserves_profile_preview(preview_raw));
    let compile = || {
        RawGpuPipeline::new(device, queue, preview_raw, params, options)
            .map_err(|error| format!("GPU preview setup failed: {error:#}"))
    };
    let pipeline_started = Instant::now();
    let pipeline = match template {
        Some(template) => match RawGpuPipeline::new(
            device,
            queue,
            preview_raw,
            params,
            options.programs(&template),
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
