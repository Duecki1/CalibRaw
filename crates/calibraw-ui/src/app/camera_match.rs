//! Camera match: the look fitted so the photo renders as its camera's own
//! JPEG preview does (`calibraw_core::pipeline::camera_look`).
//!
//! A photo that opens without a sidecar while the setting is on is fitted on
//! the document-load worker, before the new-photo preset applies and before
//! the photo's start settles, so the look is part of how the photo starts
//! and Reset keeps it. Changing the camera profile fits the look again. The
//! Camera match tile fits it by hand as a foreground job.
//!
//! An automatic fit the guard rejects leaves no look and a diagnostics line;
//! a fit started by hand reports its failure in the error dialog.

use super::*;
use crate::i18n::{tr, tr_fmt};
use crate::pipeline::camera_look::{
    fit_camera_look, CameraLook, CameraLookFit, CameraLookFitError, FitStage, FitVerdict,
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

/// A fit started by hand: its look, or why there is none.
type ManualFitResult = Result<CameraLook, String>;

/// Renders on a graph of its own and fits, for a fit started by hand.
fn run_manual_fit(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    full_raw: &LoadedRaw,
    exposure: &ExposureParams,
    preview: &RawThumbnail,
    template: Option<&RawGpuProgramTemplate>,
) -> ManualFitResult {
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
    let pipeline = RawGpuPipeline::new(device, queue, raw, &params, options)
        .map_err(|error| prepare_failed_message(&format!("{error:#}")))?;
    let fit = fit_on_pipeline(device, queue, &pipeline, raw, exposure, preview)
        .map_err(|error| fit_error_message(&error))?;
    calibraw_core::diagnostics::record(format!(
        "Camera match started by hand: {}",
        fit_summary(&fit)
    ));
    match (fit.verdict, fit.look) {
        (FitVerdict::Rejected, _) | (_, None) => Err(not_matched_message()),
        (_, Some(look)) => Ok(look),
    }
}

fn fit_error_message(error: &CameraLookFitError) -> String {
    match error {
        CameraLookFitError::PreviewTooSmall { .. } => no_preview_message(),
        CameraLookFitError::Alignment { .. } | CameraLookFitError::TooFewSamples { .. } => {
            tr("The photo's embedded preview does not show the same picture as the RAW.").to_owned()
        }
        CameraLookFitError::Render(error) => prepare_failed_message(error),
        CameraLookFitError::Solve => not_matched_message(),
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
    /// Whether the open photo can be matched to its camera, so the Camera
    /// match tile is offered.
    pub(crate) fn camera_match_offered(&self) -> bool {
        self.develop
            .original_raw
            .as_deref()
            .is_some_and(|raw| camera_match_applies(raw, &self.develop.exposure))
    }

    /// Makes `look` the open photo's camera look and renders it.
    pub(in crate::app) fn install_camera_look(&mut self, look: Option<CameraLook>) {
        self.develop.camera_look = look;
        self.sync_preview_look();
    }

    fn set_camera_look(&mut self, look: Option<CameraLook>) {
        if self.develop.camera_look == look {
            return;
        }
        self.install_camera_look(look);
        self.note_edit_changed();
    }

    /// Sets the amount of the camera look, 0–1.
    pub(crate) fn set_camera_look_amount(&mut self, amount: f32) {
        if let Some(look) = &self.develop.camera_look {
            let mut look = look.clone();
            look.set_amount(amount);
            self.set_camera_look(Some(look));
        }
    }

    /// Removes the camera look as a change of the edit.
    pub(crate) fn remove_camera_look(&mut self) {
        self.set_camera_look(None);
    }

    /// Asks for a fit by hand, which the next frame starts.
    pub(crate) fn request_camera_match(&mut self) {
        self.develop.camera_match_requested = true;
        self.egui_ctx.request_repaint();
    }

    /// Starts the fit [`Self::request_camera_match`] asked for.
    pub(in crate::app) fn start_requested_camera_match(&mut self, frame: &eframe::Frame) {
        if !std::mem::take(&mut self.develop.camera_match_requested) {
            return;
        }
        if self.foreground_operation_active() || self.lens_correction_busy() {
            self.ui.notice =
                Some(tr("Finish or cancel the current editing operation first.").to_owned());
            return;
        }
        let (Some(full_raw), true) = (
            self.develop.loaded_raw.as_ref().map(Arc::clone),
            self.camera_match_offered(),
        ) else {
            return;
        };
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
                    Some(preview) => run_manual_fit(
                        &device,
                        &queue,
                        &full_raw,
                        &exposure,
                        &preview,
                        template.as_ref(),
                    ),
                    None => Err(no_preview_message()),
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
                    context: ForegroundOperationContext::CameraMatch,
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
        let (events, disconnected) = drain_worker_events(Some(receiver), |_| true);
        let result: ManualFitResult = match events.into_iter().next() {
            Some(result) => result,
            None if disconnected => Err(tr_fmt(
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
        match result {
            Ok(look) => self.set_camera_look(Some(look)),
            Err(error) => self.report_error(ErrorKind::CameraMatch, error),
        }
        self.egui_ctx.request_repaint();
    }
}
