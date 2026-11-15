//! Camera match: the look fitted so the photo renders as its camera's own
//! JPEG preview does (`calibraw_core::pipeline::camera_look`).
//!
//! A look is fitted to one photo's own embedded preview and never moves to
//! another photo ([`CameraMatch`]). The setting is on unless the user turned
//! it off. A photo that opens without a sidecar while it is on is fitted on
//! the document-load worker, before the new-photo preset applies and before
//! the photo's start settles, so the look is part of how the photo starts.
//!
//! A reset, and a paste with the Camera match category, set only whether
//! Camera match is on and its amount: a photo they turn on without a look of
//! its own waits for a fit ([`CameraMatch::Pending`]), which runs as a
//! foreground job while the photo is open and as it opens otherwise. The fit
//! fills in the pending match everywhere it is recorded, the open photo and
//! its edit history, as the photo's own fact rather than a new step.
//! Changing the camera profile fits the look again. The Camera match tile
//! fits it by hand.
//!
//! A fit the guard rejects, or a photo without a usable embedded preview,
//! leaves a pending match off with a diagnostics line; a fit started by hand
//! reports it in the error dialog. A fit that cannot run is reported either
//! way, and a pending match then stays pending.

use super::*;
use crate::i18n::{tr, tr_fmt};
use crate::pipeline::camera_look::{
    fit_camera_look, CameraLook, CameraLookFit, CameraLookFitError, CameraMatch, FitStage,
    FitVerdict,
};
use crate::pipeline::{load_raw_embedded_thumbnail, RawThumbnail};

/// The title of the Camera match card, which its preview eye is keyed by.
pub(crate) const CAMERA_MATCH_CARD_TITLE: &str = "Camera match";
/// The long edge of the embedded preview a fit compares with.
const PREVIEW_EDGE: u32 = 512;
/// The long edge of the graph a fit started by hand renders on.
const MANUAL_FIT_EDGE: u32 = 512;

/// The embedded preview of the RAW at `path` for a fit; `None`, with a
/// diagnostics line, when it has none that can be read.
pub(in crate::app) fn read_camera_preview(path: &Path) -> Option<Arc<RawThumbnail>> {
    match load_raw_embedded_thumbnail(path, PREVIEW_EDGE) {
        Ok(preview) => Some(Arc::new(preview)),
        Err(error) => {
            calibraw_core::diagnostics::record(format!(
                "Camera match: the embedded preview could not be read: {error:#}"
            ));
            None
        }
    }
}

/// Whether `raw`, read as `exposure` says, can be matched to its camera: a
/// camera RAW, not a rendered file, developed as it is rather than as a film
/// negative.
pub(in crate::app) fn camera_match_applies(raw: &LoadedRaw, exposure: &ExposureParams) -> bool {
    !raw.is_display_referred_raster() && exposure.film.is_positive()
}

/// Fits the camera look of `preview`, rendering the canonical edit of `raw`
/// (`ExposureParams::source_reference` of `exposure`) on `pipeline`, a graph
/// built for `raw`. Leaves the pipeline holding a canonical render, which the
/// caller replaces.
pub(in crate::app) fn fit_on_pipeline(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &RawGpuPipeline,
    raw: &LoadedRaw,
    exposure: &ExposureParams,
    preview: &RawThumbnail,
) -> Result<CameraLookFit, CameraLookFitError> {
    let reference = exposure.source_reference();
    let masks = MaskStack::default();
    fit_camera_look(preview, |exposure_ev| {
        let params =
            GpuParams::new(&reference, &masks, raw).with_profile_exposure_offset(exposure_ev);
        pipeline.recompute(queue, device, &params);
        let rgba = pipeline.read_output_region_blocking(
            device,
            queue,
            0,
            0,
            pipeline.width,
            pipeline.height,
        )?;
        Ok(RawThumbnail {
            width: pipeline.width,
            height: pipeline.height,
            rgba,
        })
    })
}

/// The look an automatic fit leaves, `None` when it failed or the guard
/// rejected it, with a diagnostics line saying how it went.
pub(in crate::app) fn automatic_look(
    label: &str,
    result: Result<CameraLookFit, CameraLookFitError>,
) -> Option<CameraLook> {
    match result {
        Ok(fit) => {
            calibraw_core::diagnostics::record(format!(
                "Camera match for {label}: {}",
                fit_summary(&fit)
            ));
            fit.look
        }
        Err(error) => {
            calibraw_core::diagnostics::record(format!(
                "Camera match for {label} not fitted: {error}"
            ));
            None
        }
    }
}

/// The verdict and held-out CIEDE2000 of a fit, for diagnostics.
fn fit_summary(fit: &CameraLookFit) -> String {
    let mean = |stage| fit.score(stage).map_or(f32::NAN, |score| score.mean);
    format!(
        "{:?}; held-out CIEDE2000 {:.2} canonical, {:.2} curve and matrix, {:.2} with residual; \
         exposure {:+.2} EV",
        fit.verdict,
        mean(FitStage::Canonical),
        mean(FitStage::BakedMatrix),
        mean(FitStage::BakedResidual),
        fit.full_look.exposure_ev(),
    )
}

/// Why a fit runs on the open photo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CameraMatchRequest {
    /// The Camera match tile: every reason it leaves no look is reported.
    ByHand,
    /// A match a reset or a paste turned on ([`CameraMatch::Pending`]): a
    /// photo without a look to match turns it off without a report, as a
    /// first open would leave it.
    Pending,
}

/// How a fit on the open photo ended.
pub(in crate::app) enum OpenPhotoFit {
    Fitted(CameraLook),
    /// The photo has no look to match (no usable embedded preview, a preview
    /// of another picture, or a fit the guard rejects), with the message a
    /// fit by hand shows.
    NotMatched(String),
    /// The fit could not run.
    Failed(String),
}

/// Renders on a graph of its own and fits, for a fit on the open photo.
fn run_open_photo_fit(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    full_raw: &LoadedRaw,
    exposure: &ExposureParams,
    preview: &RawThumbnail,
    template: Option<&RawGpuProgramTemplate>,
) -> OpenPhotoFit {
    let reference = exposure.source_reference();
    if full_raw.uses_highlight_model(&reference) {
        full_raw.prime_highlight_model(&reference);
    }
    let proxy;
    let raw = if full_raw.width.max(full_raw.height) > MANUAL_FIT_EDGE {
        proxy = build_proxy(
            full_raw,
            ProxySpec {
                max_edge: MANUAL_FIT_EDGE,
            },
        );
        &proxy
    } else {
        full_raw
    };
    let quality = ProcessingQuality::Preview;
    let mut options = PipelineOptions::new(quality).mask_atlas_edge(64);
    if let Some(template) = template.filter(|template| template.fits(raw, quality)) {
        options = options.programs(template);
    }
    let params = GpuParams::new(&reference, &MaskStack::default(), raw);
    let pipeline = match RawGpuPipeline::new(device, queue, raw, &params, options) {
        Ok(pipeline) => pipeline,
        Err(error) => return OpenPhotoFit::Failed(prepare_failed_message(&format!("{error:#}"))),
    };
    let fit = match fit_on_pipeline(device, queue, &pipeline, raw, exposure, preview) {
        Ok(fit) => fit,
        Err(error) => return fit_error_outcome(&error),
    };
    calibraw_core::diagnostics::record(format!(
        "Camera match of the open photo: {}",
        fit_summary(&fit)
    ));
    match (fit.verdict, fit.look) {
        (FitVerdict::Rejected, _) | (_, None) => OpenPhotoFit::NotMatched(not_matched_message()),
        (_, Some(look)) => OpenPhotoFit::Fitted(look),
    }
}

fn fit_error_outcome(error: &CameraLookFitError) -> OpenPhotoFit {
    match error {
        CameraLookFitError::PreviewTooSmall { .. } => {
            OpenPhotoFit::NotMatched(no_preview_message())
        }
        CameraLookFitError::Alignment { .. } | CameraLookFitError::TooFewSamples { .. } => {
            OpenPhotoFit::NotMatched(
                tr("The photo's embedded preview does not show the same picture as the RAW.")
                    .to_owned(),
            )
        }
        CameraLookFitError::Render(error) => OpenPhotoFit::Failed(prepare_failed_message(error)),
        CameraLookFitError::Solve => OpenPhotoFit::NotMatched(not_matched_message()),
    }
}

fn no_preview_message() -> String {
    tr("The photo has no embedded preview that can be compared with it.").to_owned()
}

fn not_matched_message() -> String {
    tr("The camera's look could not be matched closely enough for this photo.").to_owned()
}

fn prepare_failed_message(error: &dyn std::fmt::Display) -> String {
    tr_fmt(
        "Could not prepare the photo for Camera match: {error}",
        &[("error", &format!("{error:#}"))],
    )
}

impl CalibRawApp {
    /// Whether photos are matched to their camera's look: on unless the user
    /// turned it off, also before the setup question is answered.
    pub(crate) fn camera_match_enabled(&self) -> bool {
        self.preferences.camera_match_new_photos != Some(false)
    }

    /// Gives the open photo the Camera match a first open gives it, for a
    /// reset of its adjustments: with Camera match on, at the default
    /// amount, its own look or a fit when it has none; with it off, or for a
    /// photo it does not apply to, none. Part of the reset's edit.
    pub(in crate::app) fn reset_camera_match(&mut self) {
        let next = if self.camera_match_enabled() && self.camera_match_offered() {
            self.develop
                .camera_match
                .clone()
                .with_amount(CameraLook::DEFAULT_AMOUNT)
        } else {
            CameraMatch::Off
        };
        self.install_camera_match(next);
    }

    /// Whether the open photo can be matched to its camera, so the Camera
    /// match tile is offered.
    pub(crate) fn camera_match_offered(&self) -> bool {
        self.develop
            .original_raw
            .as_deref()
            .is_some_and(|raw| camera_match_applies(raw, &self.develop.exposure))
    }

    /// Makes `state` the open photo's Camera match and renders it; a pending
    /// one is fitted next.
    pub(in crate::app) fn install_camera_match(&mut self, state: CameraMatch) {
        let pending = state.is_pending();
        self.develop.camera_match = state;
        self.sync_preview_look();
        if pending && !self.foreground_operation_is(ForegroundOperationKind::CameraMatch) {
            self.request_camera_match_for(CameraMatchRequest::Pending);
        }
    }

    fn set_camera_match(&mut self, state: CameraMatch) {
        if self.develop.camera_match == state {
            return;
        }
        self.install_camera_match(state);
        self.note_edit_changed();
    }

    /// Sets the amount of Camera match, 0–1, while it is on.
    pub(crate) fn set_camera_match_amount(&mut self, amount: f32) {
        if !self.develop.camera_match.is_off() {
            let state = self.develop.camera_match.clone().with_amount(amount);
            self.set_camera_match(state);
        }
    }

    /// Turns Camera match off as a change of the edit.
    pub(crate) fn remove_camera_match(&mut self) {
        self.set_camera_match(CameraMatch::Off);
    }

    /// Fills in the pending match of the open photo and of its edit history
    /// with the outcome of its fit (`CameraMatch::resolved`) and saves it.
    /// Not a step of its own: the fit is a fact of the photo.
    fn resolve_pending_camera_match(&mut self, fitted: Option<&CameraLook>) {
        self.develop.camera_match = std::mem::take(&mut self.develop.camera_match).resolved(fitted);
        self.persistence
            .history
            .resolve_pending_camera_match(fitted);
        self.sync_preview_look();
        self.queue_explicit_sidecar_save();
    }

    /// Asks for a fit by hand, which the next frame starts.
    pub(crate) fn request_camera_match(&mut self) {
        self.request_camera_match_for(CameraMatchRequest::ByHand);
    }

    fn request_camera_match_for(&mut self, request: CameraMatchRequest) {
        self.develop.camera_match_requested = Some(request);
        self.egui_ctx.request_repaint();
    }

    /// Starts the fit [`Self::request_camera_match`] or a pending match asked
    /// for.
    pub(in crate::app) fn start_requested_camera_match(&mut self, frame: &eframe::Frame) {
        let Some(request) = self.develop.camera_match_requested.take() else {
            return;
        };
        if self.foreground_operation_active() || self.lens_correction_busy() {
            match request {
                CameraMatchRequest::ByHand => {
                    self.ui.notice = Some(
                        tr("Finish or cancel the current editing operation first.").to_owned(),
                    );
                }
                // Started once the other operation has ended.
                CameraMatchRequest::Pending => self.develop.camera_match_requested = Some(request),
            }
            return;
        }
        if request == CameraMatchRequest::Pending && !self.develop.camera_match.is_pending() {
            return;
        }
        let Some(full_raw) = self.develop.loaded_raw.as_ref().map(Arc::clone) else {
            return;
        };
        if !self.camera_match_offered() {
            // A rendered photo or a negative has no look to match.
            if request == CameraMatchRequest::Pending {
                self.resolve_pending_camera_match(None);
            }
            return;
        }
        let Some(render_state) = frame.wgpu_render_state() else {
            self.report_error(
                ErrorKind::CameraMatch,
                tr("eframe is not running with the wgpu backend.").to_owned(),
            );
            return;
        };
        let device = render_state.device.clone();
        let queue = render_state.queue.clone();
        let exposure = self.develop.exposure;
        let kept_preview = self.develop.camera_preview.clone();
        let path = self.develop.current_path.clone();
        let template = self
            .preview
            .pipeline()
            .map(RawGpuPipeline::program_template)
            .or_else(|| self.preview.program_template.clone());
        let cancellation = Arc::new(AtomicBool::new(false));
        let repaint = self.egui_ctx.clone();
        let (sender, receiver) = mpsc::channel();
        let spawn_result = std::thread::Builder::new()
            .name("calibraw-camera-match".to_owned())
            .spawn(move || {
                let preview =
                    kept_preview.or_else(|| path.as_deref().and_then(read_camera_preview));
                let result = match preview {
                    Some(preview) => run_open_photo_fit(
                        &device,
                        &queue,
                        &full_raw,
                        &exposure,
                        &preview,
                        template.as_ref(),
                    ),
                    None => OpenPhotoFit::NotMatched(no_preview_message()),
                };
                let _ = sender.send(result);
                repaint.request_repaint();
            });
        match spawn_result {
            Ok(_) => {
                self.begin_foreground_operation(ForegroundOperation {
                    kind: ForegroundOperationKind::CameraMatch,
                    document_id: self.persistence.document_generation,
                    cancellation,
                    progress: ForegroundProgress::indeterminate(tr(
                        "Comparing the photo with its embedded preview…",
                    )),
                    cancelling: false,
                    receiver: ForegroundOperationReceiver::CameraMatch(receiver),
                    context: ForegroundOperationContext::CameraMatch(request),
                });
            }
            Err(error) => self.report_error(
                ErrorKind::CameraMatch,
                tr_fmt(
                    "Could not start {action}: {error}",
                    &[("action", &tr(CAMERA_MATCH_CARD_TITLE)), ("error", &error)],
                ),
            ),
        }
    }

    pub(in crate::app) fn poll_camera_match_worker(&mut self) {
        if !self.foreground_operation_is(ForegroundOperationKind::CameraMatch) {
            return;
        }
        let Some(operation) = self.foreground_operation.take() else {
            return;
        };
        let ForegroundOperationReceiver::CameraMatch(receiver) = &operation.receiver else {
            self.foreground_operation = Some(operation);
            return;
        };
        let ForegroundOperationContext::CameraMatch(request) = operation.context else {
            self.foreground_operation = Some(operation);
            return;
        };
        let (events, disconnected) = drain_worker_events(Some(receiver), |_| true);
        let result = match events.into_iter().next() {
            Some(result) => result,
            None if disconnected => OpenPhotoFit::Failed(tr_fmt(
                "The {action} worker stopped unexpectedly.",
                &[("action", &tr(CAMERA_MATCH_CARD_TITLE))],
            )),
            None => {
                self.foreground_operation = Some(operation);
                return;
            }
        };
        if !operation.accepts_result(self.persistence.document_generation) {
            return;
        }
        match (result, request) {
            (OpenPhotoFit::Fitted(look), CameraMatchRequest::ByHand) => {
                self.set_camera_match(CameraMatch::Fitted(look));
            }
            (OpenPhotoFit::Fitted(look), CameraMatchRequest::Pending) => {
                self.resolve_pending_camera_match(Some(&look));
            }
            // As a first open leaves such a photo.
            (OpenPhotoFit::NotMatched(reason), CameraMatchRequest::Pending) => {
                calibraw_core::diagnostics::record(format!(
                    "Camera match turned on without a look to match: {reason}"
                ));
                self.resolve_pending_camera_match(None);
            }
            // A pending match stays pending, so the photo is fitted when it
            // next opens.
            (OpenPhotoFit::NotMatched(error) | OpenPhotoFit::Failed(error), _) => {
                self.report_error(ErrorKind::CameraMatch, error);
            }
        }
        self.egui_ctx.request_repaint();
    }
}
