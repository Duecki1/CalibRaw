//! The full-resolution preview: once the editor has been idle, the whole photo
//! is rendered from the native RAW instead of the reduced proxy and scaled to
//! the screen, and shown in place of the fitted proxy until the next change.
//! Edits and zoom keep using the proxy, so the editor stays responsive.
//!
//! The render runs on a worker thread with its own GPU graph, built like the
//! fitted one but from the full RAW, so it shows exactly what the proxy shows
//! with every pixel the sensor has. Its output is read back and averaged down
//! in linear light; the result is an ordinary egui texture that needs no GPU
//! budget of its own once the graph is released.
//!
//! A render is valid while the fitted preview still shows what it showed when
//! the render started (`FittedOutput`), so no edit path has to announce that
//! it outdated the image. A photo whose graph does not fit the GPU is skipped
//! (`declined`), not reported: this is an extra, not something the user asked
//! to happen to this photo.

use super::*;
use crate::pipeline::downscale_display_rgba8;
use std::sync::atomic::{AtomicBool, Ordering};

/// How long nothing may change before the render starts.
const IDLE_BEFORE_RENDER: Duration = Duration::from_millis(800);

/// How often the idle check looks again while the pointer is down.
const POINTER_RECHECK: Duration = Duration::from_millis(100);

/// The largest graph built where the backend reports no GPU memory.
const UNREPORTED_MEMORY_LIMIT_BYTES: u64 = 1536 * 1024 * 1024;

/// The extra render may use this fraction of the adapter's reported free
/// memory: half.
const REPORTED_MEMORY_SHARE_DIVISOR: u64 = 2;

/// A rendered image: tightly packed RGBA8, display-encoded, in source space.
struct FullResolutionRgba {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

/// What the fitted preview shows: its texture, and how often its output has
/// been rendered (`RawGpuPipeline::output_revision`). Both change with every
/// edit, rebuild and document, so equal values mean the same picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FittedOutput {
    texture: egui::TextureId,
    revision: u64,
}

struct FullResolutionResult {
    document_generation: u64,
    image: Result<FullResolutionRgba, String>,
}

struct FullResolutionImage {
    texture: egui::TextureHandle,
    output: FittedOutput,
    document_generation: u64,
}

/// The render in flight. It is kept until its result arrives, even once
/// cancelled, so a second graph never builds beside the first.
struct RunningRender {
    receiver: mpsc::Receiver<FullResolutionResult>,
    cancel: Arc<AtomicBool>,
    output: FittedOutput,
}

pub(crate) struct FullResolutionState {
    enabled: bool,
    running: Option<RunningRender>,
    image: Option<FullResolutionImage>,
    /// The fitted output last seen and since when: the idle clock.
    seen: Option<(FittedOutput, Instant)>,
    /// The document a render failed or was declined for, which is not tried
    /// again.
    given_up_for: Option<u64>,
}

impl FullResolutionState {
    pub(crate) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            running: None,
            image: None,
            seen: None,
            given_up_for: None,
        }
    }

    pub(crate) fn enabled(&self) -> bool {
        self.enabled
    }

    pub(crate) fn set_enabled(&mut self, enabled: bool) {
        if self.enabled != enabled {
            self.enabled = enabled;
            self.given_up_for = None;
            self.seen = None;
            self.image = None;
            self.cancel_running();
        }
    }

    /// Tells the render in flight, if any, that its picture is no longer
    /// wanted. It stops before its next stage.
    pub(crate) fn cancel_running(&self) {
        if let Some(running) = &self.running {
            running.cancel.store(true, Ordering::Release);
        }
    }

    /// Notes what the fitted preview shows now and returns how long it has
    /// shown that. A change cancels a render of anything else and drops an
    /// image of anything else.
    fn observe(&mut self, output: Option<FittedOutput>, now: Instant) -> Duration {
        if self.seen.map(|(seen, _)| seen) != output {
            self.seen = output.map(|output| (output, now));
            if self
                .running
                .as_ref()
                .is_some_and(|running| Some(running.output) != output)
            {
                self.cancel_running();
            }
            if self
                .image
                .as_ref()
                .is_some_and(|image| Some(image.output) != output)
            {
                self.image = None;
            }
        }
        self.seen.map_or(Duration::ZERO, |(_, since)| {
            now.saturating_duration_since(since)
        })
    }
}

/// How much longer to wait before the render may start, or `None` when it may:
/// the preview has been still for the idle time and nothing is pressed.
fn wait_before_render(idle: Duration, pointer_down: bool) -> Option<Duration> {
    if idle < IDLE_BEFORE_RENDER {
        Some(IDLE_BEFORE_RENDER - idle)
    } else if pointer_down {
        // Waiting out the idle time again would be no wait at all.
        Some(POINTER_RECHECK)
    } else {
        None
    }
}

/// Why the graph of a `edge` px photo is not built, given the GPU's texture
/// limit, the bytes the graph reserves and the bytes it may take.
fn declined(edge: u32, texture_limit: u32, needed: u64, allowed: u64) -> Option<String> {
    if edge > texture_limit {
        return Some(format!(
            "the photo is {edge} px wide and the GPU's texture limit is {texture_limit} px"
        ));
    }
    (needed > allowed).then(|| {
        format!(
            "it needs {:.0} MiB of GPU memory and {:.0} MiB are free to use",
            needed as f64 / (1024.0 * 1024.0),
            allowed as f64 / (1024.0 * 1024.0)
        )
    })
}

/// Everything a render needs, owned so it can run on its own thread.
struct FullResolutionJob {
    device: wgpu::Device,
    queue: wgpu::Queue,
    raw: Arc<LoadedRaw>,
    exposure: ExposureParams,
    masks: Arc<MaskStack>,
    remove: Arc<RemoveEditState>,
    geometry: GeometryTransform,
    look: Option<crate::luts::ResolvedLook>,
    programs: Option<RawGpuProgramTemplate>,
    /// The longest edge of the result.
    target_edge: u32,
    cancel: Arc<AtomicBool>,
}

impl CalibRawApp {
    /// What the fitted preview shows now.
    fn fitted_output(&self) -> Option<FittedOutput> {
        self.preview
            .gpu_pipeline
            .as_ref()
            .map(|presented| FittedOutput {
                texture: presented.texture(),
                revision: presented.gpu().output_revision(),
            })
    }

    /// The texture to show instead of the fitted proxy, while it is current.
    pub(crate) fn full_resolution_texture(&self) -> Option<egui::TextureId> {
        let state = &self.preview.full_resolution;
        let image = state.image.as_ref()?;
        (state.enabled
            && Some(image.output) == self.fitted_output()
            && image.document_generation == self.persistence.document_generation
            && !self.preview.zoomed_in()
            && !self.preview.original_requested
            && !self.preview.processing_pending())
        .then(|| image.texture.id())
    }

    /// Whether the full-resolution render may start: idle, nothing else
    /// holding the GPU, and a proxy smaller than the photo.
    fn full_resolution_due(&self) -> bool {
        let state = &self.preview.full_resolution;
        if !state.enabled
            || !self.caps().full_resolution_preview()
            || state.image.is_some()
            || state.running.is_some()
            || state.given_up_for == Some(self.persistence.document_generation)
            || self.ui.active_tab != AppTab::Develop
            || self.preview.zoomed_in()
            || self.preview.original_requested
            || self.develop.target_profile_color.is_some()
            || self.preview.processing_pending()
            || self.preview_is_preparing()
            || self.lens_correction_busy()
            || self.foreground_operation_active()
            || self.export.task.is_some()
            || self.export.batch.is_some()
            || self.preview.detail_rebuild.is_some()
        {
            return false;
        }
        let (Some(full), Some(preview)) = (&self.develop.loaded_raw, &self.develop.preview_raw)
        else {
            return false;
        };
        full.width.max(full.height)
            > preview
                .width
                .max(preview.height)
                .saturating_add(PROXY_EDGE_TOLERANCE)
    }

    pub(in crate::app) fn advance_full_resolution(&mut self, frame: &eframe::Frame) {
        let output = self.fitted_output();
        let idle = self.preview.full_resolution.observe(output, Instant::now());
        // Whatever the pointer is pressing is about to change the picture, and
        // the preview that follows it needs the GPU more than this render does.
        let pointer_down = super::interaction::pointer_engaged(&self.egui_ctx);
        if pointer_down {
            self.preview.full_resolution.cancel_running();
        }
        if !self.full_resolution_due() {
            return;
        }
        if let Some(wait) = wait_before_render(idle, pointer_down) {
            self.egui_ctx.request_repaint_after(wait);
            return;
        }
        let (Some(render_state), Some(raw), Some(output)) =
            (frame.wgpu_render_state(), &self.develop.loaded_raw, output)
        else {
            return;
        };
        let raw = Arc::clone(raw);
        let masks = self.preview_mask_stack();
        let document_generation = self.persistence.document_generation;

        let needed = RawGpuPipeline::preview_admitted_bytes(
            raw.width,
            raw.height,
            raw.cfa_kind,
            preview_mask_layer_capacity(masks.masks.len()),
        );
        let allowed = calibraw_gpu::memory::adapter_memory(&render_state.device)
            .map_or(UNREPORTED_MEMORY_LIMIT_BYTES, |memory| {
                memory.available() / REPORTED_MEMORY_SHARE_DIVISOR
            });
        if let Some(reason) = declined(
            raw.width.max(raw.height),
            render_state.device.limits().max_texture_dimension_2d,
            needed,
            allowed,
        ) {
            calibraw_core::diagnostics::record(format!(
                "Full-resolution preview skipped for this photo: {reason}"
            ));
            self.preview.full_resolution.given_up_for = Some(document_generation);
            return;
        }

        let max_texture_side = self.egui_ctx.input(|input| input.max_texture_side) as u32;
        let target_edge = PreviewQuality::Max
            .proxy_edge_for_fitted_source(
                self.preview.fitted_viewport_pixels,
                raw.width,
                raw.height,
                self.develop.geometry,
                raw.cfa_kind,
                masks.masks.len(),
            )
            .min(max_texture_side)
            .min(raw.width.max(raw.height));
        let cancel = Arc::new(AtomicBool::new(false));
        let job = FullResolutionJob {
            device: render_state.device.clone(),
            queue: render_state.queue.clone(),
            raw,
            exposure: self.develop.target_exposure,
            masks,
            remove: Arc::clone(&self.inpaint.edits),
            geometry: self.develop.geometry,
            look: self.develop.target_look.clone(),
            programs: self.preview.program_template.clone(),
            target_edge,
            cancel: Arc::clone(&cancel),
        };
        let (sender, receiver) = mpsc::channel();
        let context = self.egui_ctx.clone();
        let spawn = std::thread::Builder::new()
            .name("calibraw-full-resolution".to_owned())
            .spawn(move || {
                let image = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    render_full_resolution(&job)
                }))
                .unwrap_or_else(|panic| {
                    let message = panic
                        .downcast_ref::<&str>()
                        .copied()
                        .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
                        .unwrap_or("unknown panic");
                    Err(anyhow::anyhow!(
                        "full-resolution render panicked: {message}"
                    ))
                })
                .map_err(|error| format!("{error:#}"));
                let _ = sender.send(FullResolutionResult {
                    document_generation,
                    image,
                });
                context.request_repaint();
            });
        match spawn {
            Ok(_) => {
                self.preview.full_resolution.running = Some(RunningRender {
                    receiver,
                    cancel,
                    output,
                });
            }
            Err(error) => {
                self.preview.full_resolution.given_up_for = Some(document_generation);
                self.report_error(
                    ErrorKind::Preview,
                    format!("Could not start the full-resolution preview: {error}"),
                );
            }
        }
    }

    pub(in crate::app) fn poll_full_resolution(&mut self) {
        let Some(running) = self.preview.full_resolution.running.as_ref() else {
            return;
        };
        let received = running.receiver.try_recv();
        if matches!(received, Err(mpsc::TryRecvError::Empty)) {
            return;
        }
        let output = running.output;
        let cancelled = running.cancel.load(Ordering::Acquire);
        let document_generation = self.persistence.document_generation;
        self.preview.full_resolution.running = None;
        let result = match received {
            Ok(result) => result,
            Err(_) => {
                if !cancelled {
                    self.preview.full_resolution.given_up_for = Some(document_generation);
                    self.report_error(
                        ErrorKind::Preview,
                        "The full-resolution preview worker stopped unexpectedly.".to_owned(),
                    );
                }
                return;
            }
        };
        // Outdated while it rendered: the next idle moment renders it again.
        if cancelled
            || result.document_generation != document_generation
            || Some(output) != self.fitted_output()
        {
            return;
        }
        match result.image {
            Ok(rendered) => {
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [rendered.width as usize, rendered.height as usize],
                    &rendered.rgba,
                );
                let texture = self.egui_ctx.load_texture(
                    "calibraw-full-resolution",
                    image,
                    egui::TextureOptions::LINEAR,
                );
                self.preview.full_resolution.image = Some(FullResolutionImage {
                    texture,
                    output,
                    document_generation: result.document_generation,
                });
                self.egui_ctx.request_repaint();
            }
            Err(error) => {
                self.preview.full_resolution.given_up_for = Some(result.document_generation);
                self.report_error(
                    ErrorKind::Preview,
                    format!("Could not render the full-resolution preview: {error}"),
                );
            }
        }
    }
}

/// Waits until the GPU has finished what was submitted to `queue` so far, and
/// returns whether it did: `false` once `cancel` is set, which stops the wait.
/// It polls the device instead of blocking on it, as a blocking wait on the
/// editor's device would hold off the UI's frames (`gpu::wait`).
fn wait_for_gpu(device: &wgpu::Device, queue: &wgpu::Queue, cancel: &AtomicBool) -> bool {
    let done = Arc::new(AtomicBool::new(false));
    let signal = Arc::clone(&done);
    queue.on_submitted_work_done(move || signal.store(true, Ordering::Release));
    while !done.load(Ordering::Acquire) {
        if cancel.load(Ordering::Acquire) {
            return false;
        }
        // A failed poll is a lost device: the readback that follows reports it.
        if device.poll(wgpu::PollType::Poll).is_err() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    true
}

/// Renders `job.raw` at native resolution through the preview graph and
/// averages it down to `job.target_edge`.
fn render_full_resolution(job: &FullResolutionJob) -> anyhow::Result<FullResolutionRgba> {
    let raw = &job.raw;
    anyhow::ensure!(
        !job.cancel.load(Ordering::Acquire),
        "full-resolution render cancelled"
    );
    if raw.uses_opposed_chroma(&job.exposure) {
        raw.inpaint_opposed_chroma_for_exposure(&job.exposure);
    }
    let params = GpuParams::new(&job.exposure, &job.masks, raw)
        .with_vignette_geometry(job.geometry)
        .with_look_lut(job.look.as_ref());
    let options = PipelineOptions {
        programs: job.programs.as_ref(),
        ..fitted_pipeline_options(job.masks.masks.len())
    };
    let pipeline = RawGpuPipeline::new(&job.device, &job.queue, raw, &params, options)?;
    CalibRawApp::upload_preview_masks(&pipeline, &job.queue, &job.masks, raw)
        .map_err(anyhow::Error::msg)?;
    // One stage at a time, so a cancel is noticed between them: stages already
    // submitted run to the end, and a native graph's take long.
    let remove = RemoveSceneContext::full_frame(&job.remove, raw, &job.exposure);
    for stage in [
        ProcessingStage::Raw,
        ProcessingStage::Tone,
        ProcessingStage::Output,
    ] {
        pipeline.dispatch_stage_with_remove(&job.queue, &job.device, &params, stage, remove)?;
        anyhow::ensure!(
            wait_for_gpu(&job.device, &job.queue, &job.cancel),
            "full-resolution render cancelled"
        );
    }
    let pixels = pipeline.read_output_region_blocking(
        &job.device,
        &job.queue,
        0,
        0,
        raw.width,
        raw.height,
    )?;
    drop(pipeline);
    anyhow::ensure!(
        !job.cancel.load(Ordering::Acquire),
        "full-resolution render cancelled"
    );
    let scale = (f64::from(job.target_edge.max(1)) / f64::from(raw.width.max(raw.height))).min(1.0);
    let width = ((f64::from(raw.width) * scale).round() as u32).max(1);
    let height = ((f64::from(raw.height) * scale).round() as u32).max(1);
    Ok(FullResolutionRgba {
        width,
        height,
        rgba: downscale_display_rgba8(&pixels, [raw.width, raw.height], [width, height]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(texture: u64, revision: u64) -> FittedOutput {
        FittedOutput {
            texture: egui::TextureId::Managed(texture),
            revision,
        }
    }

    fn running(output: FittedOutput) -> (RunningRender, Arc<AtomicBool>) {
        let (_sender, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        (
            RunningRender {
                receiver,
                cancel: Arc::clone(&cancel),
                output,
            },
            cancel,
        )
    }

    fn image(output: FittedOutput) -> FullResolutionImage {
        FullResolutionImage {
            texture: egui::Context::default().load_texture(
                "test",
                egui::ColorImage::filled([1, 1], egui::Color32::WHITE),
                egui::TextureOptions::LINEAR,
            ),
            output,
            document_generation: 0,
        }
    }

    #[test]
    fn an_edit_or_a_moved_view_cancels_the_render_in_flight() {
        let shown = output(1, 7);
        let mut app = CalibRawApp::empty(&egui::Context::default());
        for interrupt in [
            (|app: &mut CalibRawApp| app.queue_preview_processing(ProcessingStage::Output))
                as fn(&mut CalibRawApp),
            CalibRawApp::note_preview_motion,
        ] {
            let (render, cancel) = running(shown);
            app.preview.full_resolution.running = Some(render);
            assert!(!cancel.load(Ordering::Acquire));
            interrupt(&mut app);
            assert!(cancel.load(Ordering::Acquire));
        }
    }

    #[test]
    fn the_idle_clock_runs_while_the_preview_shows_the_same_output() {
        let mut state = FullResolutionState::new(true);
        let start = Instant::now();
        let shown = output(1, 7);
        assert_eq!(state.observe(Some(shown), start), Duration::ZERO);
        assert_eq!(
            state.observe(Some(shown), start + Duration::from_millis(300)),
            Duration::from_millis(300)
        );
    }

    #[test]
    fn another_output_restarts_the_clock_cancels_the_render_and_drops_the_image() {
        let start = Instant::now();
        let shown = output(1, 7);
        // An edit renders the output again; a rebuilt graph has another texture.
        for changed in [output(1, 8), output(2, 7)] {
            let mut state = FullResolutionState::new(true);
            state.observe(Some(shown), start);
            let (render, cancel) = running(shown);
            state.running = Some(render);
            state.image = Some(image(shown));

            // The same output keeps both.
            state.observe(Some(shown), start + Duration::from_secs(1));
            assert!(!cancel.load(Ordering::Acquire));
            assert!(state.image.is_some());

            let later = start + Duration::from_secs(2);
            assert_eq!(state.observe(Some(changed), later), Duration::ZERO);
            assert!(cancel.load(Ordering::Acquire), "{changed:?}");
            assert!(state.image.is_none(), "{changed:?}");
        }
    }

    #[test]
    fn the_render_waits_for_the_idle_time_and_for_the_pointer() {
        let idle = IDLE_BEFORE_RENDER;
        assert_eq!(
            wait_before_render(Duration::from_millis(300), false),
            Some(idle - Duration::from_millis(300))
        );
        assert_eq!(wait_before_render(idle, false), None);
        // Held down on a still image: look again later, never in a spin.
        assert_eq!(wait_before_render(idle * 3, true), Some(POINTER_RECHECK));
        assert!(POINTER_RECHECK > Duration::ZERO);
    }

    #[test]
    fn photos_that_do_not_fit_the_gpu_are_declined() {
        const MIB: u64 = 1024 * 1024;
        assert_eq!(declined(6_000, 16_384, 1_500 * MIB, 4_000 * MIB), None);
        assert!(declined(20_000, 16_384, MIB, 4_000 * MIB)
            .unwrap()
            .contains("texture limit"));
        let reason = declined(9_504, 16_384, 3_900 * MIB, 2_000 * MIB).unwrap();
        assert!(
            reason.contains("3900") && reason.contains("2000"),
            "{reason}"
        );
    }

    #[test]
    fn the_render_has_the_target_size_and_the_photo_gradient() {
        let gpu = calibraw_gpu::request_headless_device(&calibraw_gpu::HeadlessDeviceRequest {
            label: "full-resolution render test",
            adapter: calibraw_gpu::HeadlessAdapter::Software,
            texture_dimension: wgpu::Limits::default().max_texture_dimension_2d,
        });
        let (device, queue) = match gpu {
            Ok(gpu) => (gpu.device, gpu.queue),
            Err(calibraw_gpu::HeadlessDeviceError::NoAdapter(_)) => {
                eprintln!("full-resolution render test skipped: no headless GPU adapter");
                return;
            }
            Err(error) => panic!("{error}"),
        };
        let (width, height) = (96u32, 64u32);
        let mut pixels = Vec::new();
        for _y in 0..height {
            for x in 0..width {
                pixels.extend([0.02 + 0.5 * x as f32 / width as f32; 3]);
            }
        }
        let raw = Arc::new(LoadedRaw::from_scene_linear_rec2020(width, height, pixels).unwrap());
        let job = FullResolutionJob {
            device,
            queue,
            raw,
            exposure: ExposureParams::default(),
            masks: Arc::new(MaskStack::default()),
            remove: Arc::new(RemoveEditState::default()),
            geometry: GeometryTransform::default(),
            look: None,
            programs: None,
            target_edge: 48,
            cancel: Arc::new(AtomicBool::new(false)),
        };
        let rendered = render_full_resolution(&job).unwrap();
        assert_eq!((rendered.width, rendered.height), (48, 32));
        assert_eq!(rendered.rgba.len(), 48 * 32 * 4);
        let red = |x: usize| rendered.rgba[(16 * 48 + x) * 4];
        assert!(red(40) > red(4), "{} vs {}", red(40), red(4));

        job.cancel.store(true, Ordering::Release);
        assert!(render_full_resolution(&job).is_err());
    }
}
