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

use super::*;
use crate::pipeline::downscale_display_rgba8;
use std::sync::atomic::{AtomicBool, Ordering};

/// How long nothing may change before the render starts.
const IDLE_BEFORE_RENDER: Duration = Duration::from_millis(800);

/// A rendered image: tightly packed RGBA8, display-encoded, in source space.
pub(crate) struct FullResolutionRgba {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

struct FullResolutionResult {
    serial: u64,
    document_generation: u64,
    image: Result<FullResolutionRgba, String>,
}

struct FullResolutionImage {
    texture: egui::TextureHandle,
    serial: u64,
    document_generation: u64,
}

pub(crate) struct FullResolutionState {
    enabled: bool,
    receiver: Option<mpsc::Receiver<FullResolutionResult>>,
    cancel: Option<Arc<AtomicBool>>,
    image: Option<FullResolutionImage>,
    /// Counts the changes that outdate a rendered image.
    serial: u64,
    changed_at: Instant,
    /// The document a render failed for, which is not tried again.
    failed_for: Option<u64>,
}

impl FullResolutionState {
    pub(crate) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            receiver: None,
            cancel: None,
            image: None,
            serial: 0,
            changed_at: Instant::now(),
            failed_for: None,
        }
    }

    pub(crate) fn enabled(&self) -> bool {
        self.enabled
    }

    pub(crate) fn set_enabled(&mut self, enabled: bool) {
        if self.enabled != enabled {
            self.enabled = enabled;
            self.failed_for = None;
            self.invalidate();
        }
    }

    /// The shown image no longer matches the edit: stop rendering it and drop it.
    pub(crate) fn invalidate(&mut self) {
        self.serial = self.serial.wrapping_add(1);
        self.changed_at = Instant::now();
        if let Some(cancel) = self.cancel.take() {
            cancel.store(true, Ordering::Release);
        }
        self.receiver = None;
        self.image = None;
    }
}

/// Everything a render needs, owned so it can run on its own thread.
struct FullResolutionJob {
    device: wgpu::Device,
    queue: wgpu::Queue,
    raw: Arc<LoadedRaw>,
    exposure: ExposureParams,
    masks: Arc<MaskStack>,
    remove: RemoveEditState,
    geometry: GeometryTransform,
    programs: Option<RawGpuProgramTemplate>,
    /// The longest edge of the result.
    target_edge: u32,
    cancel: Arc<AtomicBool>,
}

impl CalibRawApp {
    /// The texture to show instead of the fitted proxy, while it is current.
    pub(crate) fn full_resolution_texture(&self) -> Option<egui::TextureId> {
        let state = &self.preview.full_resolution;
        let image = state.image.as_ref()?;
        (state.enabled
            && image.serial == state.serial
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
            || state.receiver.is_some()
            || state.failed_for == Some(self.persistence.document_generation)
            || self.ui.active_tab != AppTab::Develop
            || self.preview.zoomed_in()
            || self.preview.original_requested
            || self.preview.processing_pending()
            || self.preview_is_preparing()
            || self.lens_correction_busy()
            || self.foreground_operation_active()
            || self.export.task.is_some()
            || self.export.batch.is_some()
            || self.preview.detail_rebuild_receiver.is_some()
        {
            return false;
        }
        let (Some(full), Some(preview)) = (&self.develop.loaded_raw, &self.develop.preview_raw)
        else {
            return false;
        };
        full.width.max(full.height) > preview.width.max(preview.height).saturating_add(5)
    }

    pub(in crate::app) fn advance_full_resolution(&mut self, frame: &eframe::Frame) {
        if !self.full_resolution_due() {
            return;
        }
        let pointer_down = self
            .egui_ctx
            .input(|input| input.pointer.any_down() || input.any_touches());
        let idle = self.preview.full_resolution.changed_at.elapsed();
        if pointer_down || idle < IDLE_BEFORE_RENDER {
            self.egui_ctx
                .request_repaint_after(IDLE_BEFORE_RENDER.saturating_sub(idle));
            return;
        }
        let (Some(render_state), Some(raw)) = (frame.wgpu_render_state(), &self.develop.loaded_raw)
        else {
            return;
        };
        let raw = Arc::clone(raw);
        let masks = self.preview_mask_stack();
        let max_texture_side = self.egui_ctx.input(|input| input.max_texture_side) as u32;
        let target_edge = PreviewQuality::Max
            .proxy_edge_for_fitted_source(
                self.preview.viewport_pixels,
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
            remove: (*self.inpaint.edits).clone(),
            geometry: self.develop.geometry,
            programs: self.preview.program_template.clone(),
            target_edge,
            cancel: Arc::clone(&cancel),
        };
        let serial = self.preview.full_resolution.serial;
        let document_generation = self.persistence.document_generation;
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
                    serial,
                    document_generation,
                    image,
                });
                context.request_repaint();
            });
        match spawn {
            Ok(_) => {
                let state = &mut self.preview.full_resolution;
                state.receiver = Some(receiver);
                state.cancel = Some(cancel);
            }
            Err(error) => {
                self.preview.full_resolution.failed_for = Some(document_generation);
                self.report_error(
                    ErrorKind::Preview,
                    format!("Could not start the full-resolution preview: {error}"),
                );
            }
        }
    }

    pub(in crate::app) fn poll_full_resolution(&mut self) {
        let received = self
            .preview
            .full_resolution
            .receiver
            .as_ref()
            .map(mpsc::Receiver::try_recv);
        let result = match received {
            Some(Ok(result)) => result,
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                let state = &mut self.preview.full_resolution;
                state.receiver = None;
                state.cancel = None;
                state.failed_for = Some(self.persistence.document_generation);
                self.report_error(
                    ErrorKind::Preview,
                    "The full-resolution preview worker stopped unexpectedly.".to_owned(),
                );
                return;
            }
            Some(Err(mpsc::TryRecvError::Empty)) | None => return,
        };
        let state = &mut self.preview.full_resolution;
        state.receiver = None;
        state.cancel = None;
        if result.serial != state.serial
            || result.document_generation != self.persistence.document_generation
        {
            // Outdated while it rendered; the next idle moment renders it again.
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
                    serial: result.serial,
                    document_generation: result.document_generation,
                });
                self.egui_ctx.request_repaint();
            }
            Err(error) => {
                state.failed_for = Some(result.document_generation);
                self.report_error(
                    ErrorKind::Preview,
                    format!("Could not render the full-resolution preview: {error}"),
                );
            }
        }
    }
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
    let params =
        GpuParams::new(&job.exposure, &job.masks, raw).with_vignette_geometry(job.geometry);
    let options = fitted_pipeline_options(job.masks.masks.len());
    let pipeline = match job.programs.as_ref() {
        Some(programs) => RawGpuPipeline::new(
            &job.device,
            &job.queue,
            raw,
            &params,
            options.programs(programs),
        ),
        None => RawGpuPipeline::new(&job.device, &job.queue, raw, &params, options),
    }?;
    CalibRawApp::upload_preview_masks(&pipeline, &job.queue, &job.masks, raw)
        .map_err(anyhow::Error::msg)?;
    pipeline.recompute_with_remove(
        &job.queue,
        &job.device,
        &params,
        RemoveSceneContext::full_frame(&job.remove, raw, &job.exposure),
    )?;
    anyhow::ensure!(
        !job.cancel.load(Ordering::Acquire),
        "full-resolution render cancelled"
    );
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
            remove: RemoveEditState::default(),
            geometry: GeometryTransform::default(),
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
