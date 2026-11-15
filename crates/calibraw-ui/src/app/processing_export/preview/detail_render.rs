//! Rendering a new zoomed crop on a worker thread.
//!
//! Building a crop's graph, rasterizing its masks and rendering it took up to
//! about two seconds on a phone, and on the UI thread nothing moved meanwhile:
//! not the view, not a slider. A worker now does that work, with paced
//! submissions (`RawGpuPipeline::pace_submissions`) so the UI's frames reach
//! the GPU between its chunks, and the canvas paints the last completed crop
//! (`CropPipeline`) until the new one has finished.
//!
//! A crop takes its tone statistics and full-image maps from a full-frame
//! graph that the UI thread renders, so the work goes there and back:
//!
//! 1. Worker ([`render_raw_stage`]): build the graph, or refill the current
//!    crop's when it fits; upload its masks; render the RAW stage with Remove.
//! 2. UI thread: copy the full frame's statistics and maps into it
//!    (`RawGpuPipeline::inherit_full_frame_tone`); only the UI thread drives
//!    the full-frame graph.
//! 3. Worker ([`render_remaining_stages`]): the tone guide and the output
//!    stage; then it waits until the GPU has run them.
//!
//! The UI thread then copies the output into the shown texture and installs
//! the crop in the same frame, so the picture and its placement change
//! together. The order of the work on the GPU queue is the one the UI thread
//! used before.
//!
//! Concurrency: while a job runs, the worker alone holds the crop's graph;
//! the UI thread keeps only the copy it paints. The UI thread renders nothing
//! into a crop meanwhile (`advance_zoomed_processing` waits for the job), and
//! a crop being prepared or rendered blocks the next request
//! (`advance_preview_detail`), so at most one job exists.

use super::*;
use crate::i18n::tr_fmt;
use std::sync::atomic::{AtomicBool, Ordering};

/// What a crop is rendered from, owned so a worker can take it.
pub(in crate::app) struct DetailRenderJob {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    /// The current crop's graph when it fits the new crop, else `None` and
    /// a graph is built from `template`.
    pub reused: Option<RawGpuPipeline>,
    pub template: RawGpuProgramTemplate,
    /// Whether a built graph reserves room for a previewed camera profile.
    pub profile_preview: bool,
    pub full_raw: Arc<LoadedRaw>,
    pub masks: Arc<MaskStack>,
    pub params: GpuParams,
    /// The Remove edits and exposure the RAW stage applies.
    pub remove: Arc<RemoveEditState>,
    pub exposure: ExposureParams,
    /// The crop without its graph: placement, revision and source.
    pub crop: PreviewDetail<()>,
    pub started: Instant,
    /// Set once the UI thread forgot the crop (`DetailRebuild::dropped_flag`):
    /// its result goes nowhere, so the steps not yet started are skipped and
    /// the graph is released as soon as the worker ends.
    pub dropped: Arc<AtomicBool>,
}

impl DetailRenderJob {
    fn dropped(&self) -> bool {
        self.dropped.load(Ordering::Acquire)
    }
}

/// A crop whose graph a worker has rendered, on its way back to the UI
/// thread.
pub(crate) struct DetailRender {
    gpu: RawGpuPipeline,
    job: DetailRenderJob,
    /// The UI thread copied a full frame's statistics into the graph.
    inherited: bool,
}

impl DetailRender {
    pub(in crate::app) fn job(&self) -> &DetailRenderJob {
        &self.job
    }

    pub(in crate::app) fn gpu(&self) -> &RawGpuPipeline {
        &self.gpu
    }

    /// Notes that the full frame's statistics went into the graph, so the
    /// worker renders the tone guide with them.
    pub(in crate::app) fn note_inherited(&mut self) {
        self.inherited = true;
    }

    pub(in crate::app) fn into_parts(self) -> (RawGpuPipeline, DetailRenderJob) {
        (self.gpu, self.job)
    }
}

/// Starts step 1 on a worker; its result comes back as
/// `PreviewDetailRebuildEvent::RawRendered`.
pub(in crate::app) fn spawn_raw_stage(
    job: DetailRenderJob,
    repaint: egui::Context,
) -> Result<mpsc::Receiver<PreviewDetailRebuildEvent>, String> {
    spawn_render(repaint, move || {
        PreviewDetailRebuildEvent::RawRendered(render_raw_stage(job).map(Box::new))
    })
}

/// Starts step 3 on a worker; its result comes back as
/// `PreviewDetailRebuildEvent::Rendered`.
pub(in crate::app) fn spawn_remaining_stages(
    render: DetailRender,
    repaint: egui::Context,
) -> Result<mpsc::Receiver<PreviewDetailRebuildEvent>, String> {
    spawn_render(repaint, move || {
        PreviewDetailRebuildEvent::Rendered(Ok(Box::new(render_remaining_stages(render))))
    })
}

fn spawn_render(
    repaint: egui::Context,
    render: impl FnOnce() -> PreviewDetailRebuildEvent + Send + 'static,
) -> Result<mpsc::Receiver<PreviewDetailRebuildEvent>, String> {
    let (sender, receiver) = mpsc::channel();
    std::thread::Builder::new()
        .name("calibraw-crop-render".to_owned())
        .spawn(move || {
            let event = std::panic::catch_unwind(std::panic::AssertUnwindSafe(render))
                .unwrap_or_else(|panic| {
                    let message = panic
                        .downcast_ref::<&str>()
                        .copied()
                        .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
                        .unwrap_or("unknown panic");
                    PreviewDetailRebuildEvent::Rendered(Err(tr_fmt(
                        "zoom preview rendering panicked: {message}",
                        &[("message", &message)],
                    )))
                });
            // The UI may have dropped the crop meanwhile; the graph then
            // goes with the event.
            let _ = sender.send(event);
            repaint.request_repaint();
        })
        .map(|_| receiver)
        .map_err(|error| {
            tr_fmt(
                "Could not start rendering the zoomed preview: {error}",
                &[("error", &error.to_string())],
            )
        })
}

/// Step 1: the graph, its masks and its RAW stage.
fn render_raw_stage(mut job: DetailRenderJob) -> Result<DetailRender, String> {
    // Nothing receives the result of a dropped crop: the message is never
    // shown, and no graph is built for it.
    if job.dropped() {
        return Err("zoomed crop dropped".to_owned());
    }
    let gpu = match job.reused.take() {
        Some(gpu) => {
            gpu.upload_raw_tile(&job.queue, &job.crop.raw)
                .map_err(|error| {
                    tr_fmt(
                        "Could not update the zoomed preview crop: {error}",
                        &[("error", &format!("{error:#}"))],
                    )
                })?;
            gpu
        }
        None => RawGpuPipeline::new(
            &job.device,
            &job.queue,
            &job.crop.raw,
            &job.params,
            detail_pipeline_options(job.masks.masks.len())
                .profile_preview(job.profile_preview)
                .programs(&job.template),
        )
        .map_err(|error| {
            tr_fmt(
                "Could not render the zoomed preview: {error}",
                &[("error", &format!("{error:#}"))],
            )
        })?,
    };
    gpu.pace_submissions(true);
    let rendered = CalibRawApp::upload_detail_masks(
        &gpu,
        &job.queue,
        &job.masks,
        &job.full_raw,
        job.crop.mask_source_region,
        job.crop.mask_texture_extent,
        None,
    )
    .and_then(|()| {
        gpu.dispatch_stage_with_remove(
            &job.queue,
            &job.device,
            &job.params,
            ProcessingStage::Raw,
            job.crop
                .remove_scene(&job.remove, &job.full_raw, &job.exposure),
        )
        .map_err(|error| {
            tr_fmt(
                "Could not apply Remove to zoomed preview: {error}",
                &[("error", &format!("{error:#}"))],
            )
        })
    });
    // Back on the UI thread the graph must not wait for the GPU.
    gpu.pace_submissions(false);
    rendered?;
    Ok(DetailRender {
        gpu,
        job,
        inherited: false,
    })
}

/// Step 3: the tone guide and the output stage, run to completion on the GPU.
fn render_remaining_stages(render: DetailRender) -> DetailRender {
    let DetailRender {
        gpu,
        job,
        inherited,
    } = &render;
    // A dropped crop's render is never received; the graph goes with it.
    if job.dropped() {
        return render;
    }
    gpu.pace_submissions(true);
    if *inherited {
        gpu.dispatch_inherited_tone_guide(&job.queue, &job.device);
    } else {
        gpu.dispatch_stage(&job.queue, &job.device, &job.params, ProcessingStage::Tone);
    }
    gpu.dispatch_stage(
        &job.queue,
        &job.device,
        &job.params,
        ProcessingStage::Output,
    );
    wait_for_queue(&job.device, &job.queue);
    gpu.pace_submissions(false);
    render
}

/// Waits until the GPU has run everything submitted to `queue`, polling
/// without blocking: a blocking poll holds the lock `Queue::submit` needs, so
/// it would hold off the UI's frames for as long as it waited.
fn wait_for_queue(device: &wgpu::Device, queue: &wgpu::Queue) {
    let done = Arc::new(AtomicBool::new(false));
    let signal = Arc::clone(&done);
    queue.on_submitted_work_done(move || signal.store(true, Ordering::Release));
    while !done.load(Ordering::Acquire) {
        // A failed poll means a lost device, which the UI's device-loss
        // handling reports; the crop is then dropped with the graphs.
        if device.poll(wgpu::PollType::Poll).is_err() {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}
