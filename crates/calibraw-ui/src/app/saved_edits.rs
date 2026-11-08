//! Rendering photos with the edits saved in their sidecars, away from the
//! editor: the library's edited thumbnails and Develop's reference photo.
//! A photo renders as the editor's fitted preview shows it, on the editor's
//! GPU device with the programs the editor compiled, reduced to a small size.

use super::{
    fitted_pipeline_options, masks_have_missing_range_sources, reconstruct_canonical_mask_source,
    CalibRawApp, ErrorKind,
};
use crate::i18n::tr_fmt;
use crate::luts::{LutStore, ResolvedLook};
use crate::pipeline::{
    apply_lens_correction, build_proxy, load_raw_file_with_profile_selection,
    transform_thumbnail_geometry_with_lens, CameraProfileMode, GpuParams, GpuProgramPrewarm,
    LoadedRaw, MaskStack, ProxySpec, RawGpuPipeline, RawGpuProgramTemplate, RawThumbnail,
    RemoveSceneContext,
};
use crate::platform::Platform;
use crate::sidecar::{EditState, SidecarError};
use eframe::wgpu;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};

/// The editor's GPU device, which saved edits render on, and the programs
/// they reuse.
#[derive(Clone)]
pub(crate) struct SavedEditsGpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// The editor's preview programs, published by the startup prewarm.
    prewarmed: Arc<GpuProgramPrewarm>,
    /// Programs compiled here for photos the prewarmed ones do not fit, such
    /// as another sensor layout, kept for the next such photo.
    compiled: Arc<Mutex<Vec<RawGpuProgramTemplate>>>,
}

impl SavedEditsGpu {
    pub(crate) fn new(
        device: wgpu::Device,
        queue: wgpu::Queue,
        prewarmed: Arc<GpuProgramPrewarm>,
    ) -> Self {
        Self {
            device,
            queue,
            prewarmed,
            compiled: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// The fitted preview's pipeline for `raw`, built from programs compiled
    /// before where they fit. Waits for the startup prewarm.
    fn pipeline(
        &self,
        raw: &LoadedRaw,
        params: &GpuParams,
        mask_count: usize,
    ) -> Result<RawGpuPipeline, String> {
        let options = fitted_pipeline_options(mask_count);
        let quality = options.quality;
        // A failed prewarm leaves the programs to be compiled here.
        let prewarmed = self.prewarmed.wait().ok();
        let template = match prewarmed.filter(|template| template.fits(raw, quality)) {
            Some(template) => Some(RawGpuProgramTemplate::clone(&template)),
            None => self
                .compiled
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .iter()
                .find(|template| template.fits(raw, quality))
                .cloned(),
        };
        let options = match &template {
            Some(template) => options.programs(template),
            None => options,
        };
        let pipeline = RawGpuPipeline::new(&self.device, &self.queue, raw, params, options)
            .map_err(|error| {
                tr_fmt(
                    "could not prepare edited thumbnail rendering: {error}",
                    &[("error", &format!("{error:#}"))],
                )
            })?;
        if template.is_none() {
            // Renders running side by side may have compiled the same programs.
            let mut compiled = self.compiled.lock().unwrap_or_else(PoisonError::into_inner);
            if !compiled.iter().any(|template| template.fits(raw, quality)) {
                compiled.push(pipeline.program_template());
            }
        }
        Ok(pipeline)
    }
}

/// What rendering saved edits takes from the settings: the camera profiles a
/// sidecar may name and the LUTs it may apply.
#[derive(Clone)]
pub(crate) struct SavedEditsSettings {
    camera_profile_mode: CameraProfileMode,
    camera_profile_folder: Option<PathBuf>,
    luts: Option<LutStore>,
}

impl SavedEditsSettings {
    /// The edit's LUT, or `None` when it has none or the library does not
    /// hold it.
    fn look(&self, edits: &EditState) -> Option<ResolvedLook> {
        let look = edits.look_lut.as_ref()?;
        let lut = self.luts.as_ref()?.load(look.id).ok()?;
        Some(ResolvedLook {
            lut,
            amount: look.amount,
        })
    }
}

/// Where a photo, its sidecar and its developed thumbnail live.
#[derive(Clone, Debug)]
pub(crate) enum EditedPhoto {
    /// A file with its sidecar beside it and its developed thumbnail in the
    /// desktop cache.
    File(PathBuf),
    /// A media-library photo, reached through the platform.
    MediaLibrary { uri: String, display_name: String },
}

impl EditedPhoto {
    fn label(&self) -> String {
        match self {
            Self::File(path) => path.display().to_string(),
            Self::MediaLibrary { display_name, .. } => display_name.clone(),
        }
    }

    /// The saved edits and the sidecar's render fingerprint; `None` without a
    /// sidecar, or with one the editor would ignore as invalid.
    fn load_edits(&self, platform: &dyn Platform) -> Result<Option<(EditState, u64)>, String> {
        let loaded = match self {
            Self::File(path) => crate::sidecar::load_desktop_for_render(path),
            Self::MediaLibrary { uri, display_name } => {
                platform.load_sidecar_for_render(uri, display_name)
            }
        };
        match loaded {
            Ok(loaded) => Ok(loaded.map(|(sidecar, fingerprint)| (sidecar.edits, fingerprint))),
            Err(SidecarError::Invalid(error)) => {
                log::warn!(
                    "ignoring the invalid sidecar of {} while rendering its edits: {error}",
                    self.label()
                );
                Ok(None)
            }
            Err(error) => Err(tr_fmt(
                "could not load edits for {path}: {error}",
                &[("path", &self.label()), ("error", &error)],
            )),
        }
    }

    /// Decodes the photo with the camera profile `edits` name.
    fn decode(
        &self,
        platform: &dyn Platform,
        settings: &SavedEditsSettings,
        edits: &EditState,
    ) -> Result<LoadedRaw, String> {
        let profile_folder = settings.camera_profile_folder.as_deref();
        let requested_profile = edits
            .camera_profile
            .as_ref()
            .and_then(|relative| profile_folder.map(|root| root.join(relative)));
        let decode = |path: &Path| {
            load_raw_file_with_profile_selection(
                path,
                settings.camera_profile_mode,
                profile_folder,
                requested_profile.as_deref(),
            )
            .map_err(|error| {
                tr_fmt(
                    "could not decode edited RAW {path}: {error}",
                    &[("path", &self.label()), ("error", &format!("{error:#}"))],
                )
            })
        };
        match self {
            Self::File(path) => decode(path),
            Self::MediaLibrary { uri, display_name } => {
                let mut decoded = None;
                platform.read_library_photo(uri, display_name, &mut |path| {
                    decoded = Some(decode(path)?);
                    Ok(())
                })?;
                decoded.ok_or_else(|| format!("the platform returned no read of {display_name}"))
            }
        }
    }

    /// Keeps `thumbnail` as the photo's developed thumbnail while the sidecar
    /// still has `fingerprint`; false, and nothing kept, once it changed.
    fn save_developed_thumbnail(
        &self,
        platform: &dyn Platform,
        thumbnail: &RawThumbnail,
        fingerprint: u64,
    ) -> Result<bool, String> {
        match self {
            Self::File(path) => {
                match crate::sidecar::save_developed_thumbnail_cache(path, thumbnail, fingerprint) {
                    Ok(_) => Ok(true),
                    // The cache refuses a thumbnail whose sidecar changed.
                    Err(_)
                        if crate::sidecar::desktop_sidecar_fingerprint(path)?
                            != Some(fingerprint) =>
                    {
                        Ok(false)
                    }
                    Err(error) => Err(error),
                }
            }
            Self::MediaLibrary { uri, display_name } => platform.save_rendered_developed_thumbnail(
                uri,
                display_name,
                thumbnail,
                fingerprint,
            ),
        }
    }
}

/// A finished render of a photo's saved edits.
pub(crate) enum EditedRender {
    /// The edits, rendered. `current` is false when the sidecar changed while
    /// they rendered: the thumbnail then shows older edits and is not kept.
    Rendered {
        thumbnail: RawThumbnail,
        current: bool,
    },
    /// The sidecar holds no edits that change the photo.
    Unedited,
    /// Stopped before the photo was decoded or rendered.
    Cancelled,
}

/// One photo to render with its saved edits.
pub(crate) struct EditedRenderJob<'a> {
    pub(crate) photo: &'a EditedPhoto,
    /// The long edge the result is fitted into.
    pub(crate) maximum_edge: u32,
    /// Whether the saved lens correction applies. It resamples the whole
    /// photo, so library thumbnails, too small to show it, leave it out.
    pub(crate) lens_correction: bool,
    pub(crate) gpu: &'a SavedEditsGpu,
    pub(crate) settings: &'a SavedEditsSettings,
    pub(crate) platform: &'a dyn Platform,
    /// Read while the photo decodes, so an interactive decode goes first.
    pub(crate) decode_gate: &'a RwLock<()>,
    /// Checked before the photo is decoded and before it renders.
    pub(crate) cancel: &'a AtomicBool,
}

impl EditedRenderJob<'_> {
    /// Renders the photo as its sidecar saved it and keeps the result as its
    /// developed thumbnail. A render without the edit's LUT, which the
    /// library does not hold, is not kept, so the LUT shows once imported.
    pub(crate) fn run(&self) -> Result<EditedRender, String> {
        let Some((edits, fingerprint)) = self.photo.load_edits(self.platform)? else {
            return Ok(EditedRender::Unedited);
        };
        if !crate::sidecar::edit_state_has_adjustments(&edits) {
            return Ok(EditedRender::Unedited);
        }
        if self.cancelled() {
            return Ok(EditedRender::Cancelled);
        }
        let raw = {
            // The gate guards no data, so a poisoned one is usable.
            let _decoding = self
                .decode_gate
                .read()
                .unwrap_or_else(PoisonError::into_inner);
            self.photo.decode(self.platform, self.settings, &edits)?
        };
        if self.cancelled() {
            return Ok(EditedRender::Cancelled);
        }
        let look = self.settings.look(&edits);
        let complete = edits.look_lut.is_none() || look.is_some();
        let thumbnail = render_saved_edits(
            self.gpu,
            raw,
            edits,
            look.as_ref(),
            self.maximum_edge,
            self.lens_correction,
        )?;
        let current = !complete
            || self
                .photo
                .save_developed_thumbnail(self.platform, &thumbnail, fingerprint)?;
        Ok(EditedRender::Rendered { thumbnail, current })
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }
}

/// `full_raw` rendered with `edits` as the editor's fitted preview shows them,
/// fitted into `maximum_edge`, and without their lens correction unless
/// `lens_correction`. `look` is the edit's LUT, `None` when the library does
/// not hold it.
fn render_saved_edits(
    gpu: &SavedEditsGpu,
    full_raw: LoadedRaw,
    edits: EditState,
    look: Option<&ResolvedLook>,
    maximum_edge: u32,
    lens_correction: bool,
) -> Result<RawThumbnail, String> {
    let EditState {
        mut exposure,
        geometry,
        masks,
        remove,
        lens,
        ..
    } = edits;
    // As the editor opens a photo: lens correction resamples the whole
    // mosaic, which a reduced proxy no longer holds, so it comes first.
    let requested = lens_correction
        .then(|| lens.requested_correction(&full_raw))
        .flatten();
    let full_raw = match requested {
        Some(correction) => match apply_lens_correction(&full_raw, &correction) {
            Ok(corrected) => corrected,
            // The editor shows the photo uncorrected too when this fails.
            Err(error) => {
                log::warn!("could not apply the saved lens correction: {error:#}");
                full_raw
            }
        },
        None => full_raw,
    };
    // A film negative the Library marked renders as its estimate; opening the
    // photo saves one.
    exposure.settle_film(&full_raw, geometry);
    if full_raw.uses_opposed_chroma(&exposure) {
        full_raw.inpaint_opposed_chroma_for_exposure(&exposure);
    }
    let full_raw = Arc::new(full_raw);
    // The photo renders at the size it is shown at; effects that reach
    // across it scale with its full size (`source_extent`), so they look as
    // they do in the editor.
    let preview_raw = if full_raw.width.max(full_raw.height) > maximum_edge {
        Arc::new(build_proxy(
            &full_raw,
            ProxySpec {
                max_edge: maximum_edge,
            },
        ))
    } else {
        Arc::clone(&full_raw)
    };

    let mut masks = Arc::unwrap_or_clone(masks);
    let params = |masks: &MaskStack| {
        GpuParams::new(&exposure, masks, &preview_raw)
            .with_vignette_geometry(geometry)
            .with_look_lut(look)
    };
    let pipeline = gpu.pipeline(&preview_raw, &params(&masks), masks.masks.len())?;
    if masks_have_missing_range_sources(&masks) {
        reconstruct_canonical_mask_source(
            &pipeline,
            &gpu.device,
            &gpu.queue,
            &full_raw,
            &preview_raw,
            &exposure,
            &mut masks,
        )?;
    }
    CalibRawApp::upload_preview_masks(&pipeline, &gpu.queue, &masks, &preview_raw)?;
    pipeline
        .recompute_with_remove(
            &gpu.queue,
            &gpu.device,
            &params(&masks),
            RemoveSceneContext::full_frame(&remove, &full_raw, &exposure),
        )
        .map_err(|error| {
            tr_fmt(
                "Remove could not be applied: {error}",
                &[("error", &format!("{error:#}"))],
            )
        })?;
    let thumbnail = pipeline
        .output_snapshot(&gpu.device, &gpu.queue)
        .read_thumbnail_blocking(&gpu.device, &gpu.queue, maximum_edge)
        .map_err(|error| {
            tr_fmt(
                "could not read edited thumbnail pixels: {error}",
                &[("error", &format!("{error:#}"))],
            )
        })?;
    Ok(transform_thumbnail_geometry_with_lens(
        &thumbnail,
        geometry,
        preview_raw.lens_geometry.as_deref(),
    ))
}

impl CalibRawApp {
    pub(crate) fn saved_edits_settings(&self) -> SavedEditsSettings {
        SavedEditsSettings {
            camera_profile_mode: self.preferences.camera_profile_mode,
            camera_profile_folder: self.preferences.camera_profile_folder.clone(),
            luts: self.luts.store(),
        }
    }

    /// Starts the library's next edited-thumbnail renders, and reports the
    /// renders that failed once none is left.
    pub(crate) fn run_library_developed_renders(&mut self) {
        if self.library.developed_render_due() {
            let settings = self.saved_edits_settings();
            self.library
                .start_developed_renders(&settings, &self.egui_ctx);
        }
        if let Some(message) = self.library.take_developed_render_failures() {
            self.report_error(ErrorKind::EditedThumbnails, message);
        }
    }
}
