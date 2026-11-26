//! Edit replay: renders the photo at each edit stage and encodes a 30 FPS
//! MP4 that replays the edit category by category, or card by card and mask
//! by mask ([`ReplayDetail`]).
//!
//! Runs on a worker thread. Progress is reported through a callback and the
//! MP4 is published atomically at the destination. Encoders live in `desktop`
//! (FFmpeg) and `android` (the platform codec, MediaCodec on Android); the
//! platform's `VideoEncoder` capability picks one.

mod frames;
mod highlight;
mod plan;

mod android;
mod desktop;
use desktop::ensure_ffmpeg_available;

use crate::i18n::{tr, tr_fmt};
use crate::pipeline::{
    spawn_tiled_export, ExportBitDepth, ExportEvent, ExportFiles, ExportFormat, ExportMetadata,
    ExportResizeMode, ExportSettings, ExportTarget, ExposureParams, GeometryTransform,
    GpuProgramPrewarm, LoadedRaw, MaskStack, OutputColorSpace, RemoveEditState, TileSpec,
    TiledExportJob,
};
use crate::platform::{Platform, VideoEncoder};
use calibraw_gpu::wgpu;
use frames::{
    brand_outro_frame, crossfade, draw_stage_title, fit_to_canvas, fitted_rect, highlight_mask,
    replay_canvas_dimensions, smootherstep, split_frame, stage_title_alpha, RenderedStill,
    BRAND_TITLE,
};
use highlight::mask_highlight;
pub(crate) use plan::ReplayDetail;
use plan::{replay_stage_plan, ReplayRenderState, ReplayStage};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

const REPLAY_FPS: u32 = 30;
const REPLAY_LONG_EDGE: u32 = 1920;
const SPLIT_HOLD_FRAMES: u32 = 30;
const SPLIT_SWEEP_FRAMES: u32 = 27;
const FULL_FRAME_HOLD_FRAMES: u32 = 30;
const STAGE_TRANSITION_FRAMES: u32 = 24;
const STAGE_HOLD_FRAMES: u32 = 30;
/// A mask's highlight fades in over the photo before the mask applies.
const HIGHLIGHT_FADE_FRAMES: u32 = 12;
const HIGHLIGHT_HOLD_FRAMES: u32 = 24;
const FINAL_HOLD_FRAMES: u32 = 45;
const OUTRO_FADE_TO_BLACK_FRAMES: u32 = 18;
const OUTRO_BRAND_FADE_FRAMES: u32 = 12;
const OUTRO_HOLD_FRAMES: u32 = 60;
/// Share of the progress bar spent rendering stills; encoding fills the rest.
const RENDER_PROGRESS_WEIGHT: f32 = 0.28;

// Every hold has to outlive its own fades, otherwise a stage could leave a half-faded frame on
// screen. These are relationships between constants, so they are checked at compile time.
const _: () = assert!(SPLIT_HOLD_FRAMES >= REPLAY_FPS);
const _: () = assert!(FULL_FRAME_HOLD_FRAMES >= REPLAY_FPS);
const _: () = assert!(STAGE_HOLD_FRAMES >= REPLAY_FPS);
const _: () = assert!(FINAL_HOLD_FRAMES >= REPLAY_FPS);
const _: () = assert!(OUTRO_HOLD_FRAMES >= REPLAY_FPS * 2);

/// The edit to replay, captured on the UI thread.
pub(crate) struct ReplayRequest {
    pub(crate) device: wgpu::Device,
    pub(crate) queue: wgpu::Queue,
    pub(crate) raw: Arc<LoadedRaw>,
    pub(crate) original_exposure: ExposureParams,
    pub(crate) final_exposure: ExposureParams,
    pub(crate) final_geometry: GeometryTransform,
    pub(crate) final_masks: MaskStack,
    pub(crate) final_remove: RemoveEditState,
    pub(crate) final_look: Option<crate::luts::ResolvedLook>,
    pub(crate) detail: ReplayDetail,
    pub(crate) gpu_export_prewarm: Option<Arc<GpuProgramPrewarm>>,
    pub(crate) platform: Arc<dyn Platform>,
}

/// The encoder the platform's `VideoEncoder` capability names.
enum ReplayFrameWriter {
    Ffmpeg(desktop::ReplayFrameWriter),
    Platform(android::ReplayFrameWriter),
}

impl ReplayFrameWriter {
    fn start(
        platform: &dyn Platform,
        path: &Path,
        width: u32,
        height: u32,
    ) -> Result<Self, String> {
        Ok(match platform.caps().video_encoder {
            VideoEncoder::Ffmpeg => {
                Self::Ffmpeg(desktop::ReplayFrameWriter::start(path, width, height)?)
            }
            VideoEncoder::Platform => Self::Platform(android::ReplayFrameWriter::start(
                platform, path, width, height,
            )?),
        })
    }

    /// One RGB frame of the size the writer was started with.
    fn write_frame(&mut self, rgb: &[u8]) -> Result<(), String> {
        match self {
            Self::Ffmpeg(writer) => writer.write_frame(rgb),
            Self::Platform(writer) => writer.write_frame(rgb),
        }
    }

    fn finish(self) -> Result<(), String> {
        match self {
            Self::Ffmpeg(writer) => writer.finish(),
            Self::Platform(writer) => writer.finish(),
        }
    }

    fn cancel(&mut self) {
        match self {
            Self::Ffmpeg(writer) => writer.cancel(),
            Self::Platform(writer) => writer.cancel(),
        }
    }
}

/// Progress of a running replay.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ReplayProgress {
    /// Share of the whole job that is done, from 0 to 1.
    pub(crate) fraction: f32,
    pub(crate) phase: String,
    /// Video frames encoded so far, and in all.
    pub(crate) completed_frames: usize,
    pub(crate) total_frames: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ReplayError {
    /// The cancellation flag was set; no output was published.
    Cancelled,
    Failed(String),
}

impl fmt::Display for ReplayError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => formatter.write_str(tr("edit replay cancelled")),
            Self::Failed(message) => formatter.write_str(message),
        }
    }
}

impl From<String> for ReplayError {
    fn from(message: String) -> Self {
        Self::Failed(message)
    }
}

fn ensure_not_cancelled(cancellation: &AtomicBool) -> Result<(), ReplayError> {
    if cancellation.load(Ordering::Acquire) {
        Err(ReplayError::Cancelled)
    } else {
        Ok(())
    }
}

fn render_still(
    request: &ReplayRequest,
    state: &ReplayRenderState,
    path: &Path,
    cancellation: &Arc<AtomicBool>,
    mut tile_progress: impl FnMut(usize, usize),
) -> Result<RenderedStill, ReplayError> {
    ensure_not_cancelled(cancellation)?;
    let settings = ExportSettings {
        resize_mode: ExportResizeMode::LongEdge,
        edge_or_dimension: REPLAY_LONG_EDGE,
        percentage: 100.0,
        allow_upscale: true,
        keep_metadata: false,
        include_rating: false,
        jpeg_quality: 90,
        bit_depth: ExportBitDepth::Eight,
        // Video frames are encoded as sRGB.
        color_space: OutputColorSpace::Srgb,
        watermark: None,
    };
    let receiver = spawn_tiled_export(
        ExportFormat::Png,
        TiledExportJob {
            device: request.device.clone(),
            queue: request.queue.clone(),
            raw: Arc::clone(&request.raw),
            geometry: state.geometry,
            exposure: state.exposure,
            masks: state.masks.clone(),
            remove: state.remove.clone(),
            look: state.look.clone(),
            files: ExportFiles::Single(ExportTarget::File(path.to_path_buf())),
            tile_spec: TileSpec::default(),
            settings,
            metadata: ExportMetadata::default(),
            cancellation: Arc::clone(cancellation),
            program_prewarm: request.gpu_export_prewarm.as_ref().map(Arc::clone),
        },
    );
    loop {
        match receiver.recv() {
            Ok(ExportEvent::Progress {
                completed_tiles,
                total_tiles,
            }) => tile_progress(completed_tiles, total_tiles),
            Ok(ExportEvent::WaitingForGpu { .. }) => {}
            Ok(ExportEvent::Finished(Ok(_))) => break,
            // A cancelled export reports an error; the flag decides which it was.
            Ok(ExportEvent::Finished(Err(error))) => {
                ensure_not_cancelled(cancellation)?;
                return Err(ReplayError::Failed(error));
            }
            Err(_) => {
                return Err(ReplayError::Failed(
                    tr("replay render worker stopped unexpectedly").to_owned(),
                ))
            }
        }
    }
    ensure_not_cancelled(cancellation)?;
    let decoded = image::ImageReader::open(path)
        .map_err(|error| {
            tr_fmt(
                "Could not open rendered replay frame: {error}",
                &[("error", &error)],
            )
        })?
        .decode()
        .map_err(|error| {
            tr_fmt(
                "Could not decode rendered replay frame: {error}",
                &[("error", &error)],
            )
        })?
        .to_rgb8();
    Ok(RenderedStill {
        width: decoded.width(),
        height: decoded.height(),
        rgb: decoded.into_raw(),
    })
}

fn final_render_state(request: &ReplayRequest) -> ReplayRenderState {
    ReplayRenderState {
        exposure: request.final_exposure,
        geometry: request.final_geometry.sanitized(),
        masks: request.final_masks.clone(),
        remove: request.final_remove.clone(),
        look: request.final_look.clone(),
    }
}

/// Frames a stage takes: the highlight of its mask, if it has one, then the
/// crossfade into its still and a hold on it.
fn stage_frames(stage: &ReplayStage) -> u32 {
    let highlight = if stage.highlight.is_some() {
        HIGHLIGHT_FADE_FRAMES + HIGHLIGHT_HOLD_FRAMES
    } else {
        0
    };
    highlight + STAGE_TRANSITION_FRAMES + STAGE_HOLD_FRAMES
}

fn total_replay_frames(stages: &[ReplayStage]) -> u32 {
    SPLIT_HOLD_FRAMES
        + SPLIT_SWEEP_FRAMES * 2
        + FULL_FRAME_HOLD_FRAMES * 2
        + stages.iter().map(stage_frames).sum::<u32>()
        + FINAL_HOLD_FRAMES
        + OUTRO_FADE_TO_BLACK_FRAMES
        + OUTRO_BRAND_FADE_FRAMES
        + OUTRO_HOLD_FRAMES
}

/// How far frame `frame` of a transition of `frames` frames is, from 0 on its
/// first frame to 1 on its last.
fn ramp(frame: u32, frames: u32) -> f32 {
    if frames <= 1 {
        1.0
    } else {
        frame as f32 / (frames - 1) as f32
    }
}

/// Progress over the whole replay. Rendering stills fills
/// `RENDER_PROGRESS_WEIGHT` of it and encoding frames the rest; the two
/// interleave, since each stage's still renders just before its frames.
struct ProgressTracker<'a> {
    report: &'a mut dyn FnMut(ReplayProgress),
    render_count: usize,
    rendered: usize,
    total_frames: usize,
    completed_frames: usize,
}

impl ProgressTracker<'_> {
    fn send(&mut self, still_fraction: f32, phase: String) {
        let rendered = (self.rendered as f32 + still_fraction) / self.render_count.max(1) as f32;
        let encoded = self.completed_frames as f32 / self.total_frames.max(1) as f32;
        (self.report)(ReplayProgress {
            fraction: RENDER_PROGRESS_WEIGHT * rendered + (1.0 - RENDER_PROGRESS_WEIGHT) * encoded,
            phase,
            completed_frames: self.completed_frames,
            total_frames: self.total_frames,
        });
    }

    fn still_rendering(&mut self, label: &str, tile_fraction: f32) {
        self.send(
            tile_fraction,
            tr_fmt("Rendering {label}…", &[("label", &label)]),
        );
    }

    fn still_rendered(&mut self, label: &str) {
        self.rendered += 1;
        self.send(0.0, tr_fmt("Rendered {label}", &[("label", &label)]));
    }

    fn frame_encoded(&mut self, phase: &str) {
        self.completed_frames += 1;
        if self.completed_frames == self.total_frames || self.completed_frames.is_multiple_of(3) {
            self.send(0.0, phase.to_owned());
        }
    }
}

/// Renders one still of the replay into `directory` and decodes it.
fn render_replay_still(
    request: &ReplayRequest,
    directory: &Path,
    cancellation: &Arc<AtomicBool>,
    progress: &mut ProgressTracker<'_>,
    label: &str,
    state: &ReplayRenderState,
) -> Result<RenderedStill, ReplayError> {
    let path = directory.join(format!("still-{:02}.png", progress.rendered));
    let still = render_still(request, state, &path, cancellation, |done, total| {
        let tile_fraction = if total == 0 {
            0.0
        } else {
            done as f32 / total as f32
        };
        progress.still_rendering(label, tile_fraction);
    })?;
    progress.still_rendered(label);
    Ok(still)
}

fn write_video_frame(
    writer: &mut ReplayFrameWriter,
    cancellation: &Arc<AtomicBool>,
    frame: &[u8],
) -> Result<(), ReplayError> {
    if cancellation.load(Ordering::Acquire) {
        writer.cancel();
        return Err(ReplayError::Cancelled);
    }
    Ok(writer.write_frame(frame)?)
}

/// Composes the replay's frames on the canvas and feeds them to the encoder.
struct ReplayEncoder<'a> {
    writer: ReplayFrameWriter,
    cancellation: &'a Arc<AtomicBool>,
    width: u32,
    height: u32,
    /// The frame being composed.
    scratch: Vec<u8>,
}

impl ReplayEncoder<'_> {
    fn emit(
        &mut self,
        progress: &mut ProgressTracker<'_>,
        frame: &[u8],
        phase: &str,
    ) -> Result<(), ReplayError> {
        write_video_frame(&mut self.writer, self.cancellation, frame)?;
        progress.frame_encoded(phase);
        Ok(())
    }

    fn emit_scratch(
        &mut self,
        progress: &mut ProgressTracker<'_>,
        phase: &str,
    ) -> Result<(), ReplayError> {
        write_video_frame(&mut self.writer, self.cancellation, &self.scratch)?;
        progress.frame_encoded(phase);
        Ok(())
    }

    /// Before / after: a split view that sweeps to the final edit and back.
    fn intro(
        &mut self,
        progress: &mut ProgressTracker<'_>,
        original: &[u8],
        final_edit: &[u8],
    ) -> Result<(), ReplayError> {
        let (width, height) = (self.width, self.height);
        split_frame(original, final_edit, width, height, 0.5, &mut self.scratch);
        for _ in 0..SPLIT_HOLD_FRAMES {
            self.emit_scratch(progress, tr("Before / after"))?;
        }
        for frame in 0..SPLIT_SWEEP_FRAMES {
            let t = smootherstep(ramp(frame, SPLIT_SWEEP_FRAMES));
            split_frame(
                original,
                final_edit,
                width,
                height,
                0.5 * (1.0 - t),
                &mut self.scratch,
            );
            self.emit_scratch(progress, tr("Revealing final edit"))?;
        }
        for _ in 0..FULL_FRAME_HOLD_FRAMES {
            self.emit(progress, final_edit, tr("Final edit"))?;
        }
        for frame in 0..SPLIT_SWEEP_FRAMES {
            let t = smootherstep(ramp(frame, SPLIT_SWEEP_FRAMES));
            split_frame(original, final_edit, width, height, t, &mut self.scratch);
            self.emit_scratch(progress, tr("Returning to original"))?;
        }
        for _ in 0..FULL_FRAME_HOLD_FRAMES {
            self.emit(progress, original, tr("Original"))?;
        }
        Ok(())
    }

    /// The [`stage_frames`] of `stage`: the highlight of its mask, drawn from
    /// `highlight`, the mask's coverage ([`highlight::mask_highlight`]), then
    /// a crossfade from `previous` to `next` while the highlight fades, and a
    /// hold on `next`. The stage's title shows throughout.
    fn stage(
        &mut self,
        progress: &mut ProgressTracker<'_>,
        stage: &ReplayStage,
        highlight: Option<&[u8]>,
        previous: &[u8],
        next: &[u8],
    ) -> Result<(), ReplayError> {
        let title = stage.title.as_str();
        let phase = tr_fmt("Applying {stage}", &[("stage", &title)]);
        let total = stage_frames(stage);
        let mut frame = 0;
        let mut emit_titled = |encoder: &mut Self, progress: &mut ProgressTracker<'_>| {
            draw_stage_title(
                &mut encoder.scratch,
                encoder.width,
                encoder.height,
                title,
                stage_title_alpha(frame, total),
            );
            frame += 1;
            encoder.emit_scratch(progress, &phase)
        };

        if let Some(coverage) = highlight {
            for step in 0..HIGHLIGHT_FADE_FRAMES {
                self.scratch.copy_from_slice(previous);
                let amount = smootherstep(ramp(step, HIGHLIGHT_FADE_FRAMES));
                highlight_mask(&mut self.scratch, coverage, amount);
                emit_titled(self, progress)?;
            }
            for _ in 0..HIGHLIGHT_HOLD_FRAMES {
                self.scratch.copy_from_slice(previous);
                highlight_mask(&mut self.scratch, coverage, 1.0);
                emit_titled(self, progress)?;
            }
        }
        for step in 0..STAGE_TRANSITION_FRAMES {
            let t = smootherstep(ramp(step, STAGE_TRANSITION_FRAMES));
            crossfade(previous, next, t, &mut self.scratch);
            if let Some(coverage) = highlight {
                highlight_mask(&mut self.scratch, coverage, 1.0 - t);
            }
            emit_titled(self, progress)?;
        }
        for _ in 0..STAGE_HOLD_FRAMES {
            self.scratch.copy_from_slice(next);
            emit_titled(self, progress)?;
        }
        Ok(())
    }

    /// A hold on the final edit, a fade to black and the brand card.
    fn outro(
        &mut self,
        progress: &mut ProgressTracker<'_>,
        final_edit: &[u8],
    ) -> Result<(), ReplayError> {
        let brand = brand_outro_frame(self.width, self.height)?;
        let black = vec![0u8; final_edit.len()];
        for _ in 0..FINAL_HOLD_FRAMES {
            self.emit(progress, final_edit, tr("Holding final edit"))?;
        }
        for frame in 0..OUTRO_FADE_TO_BLACK_FRAMES {
            let t = smootherstep(ramp(frame, OUTRO_FADE_TO_BLACK_FRAMES));
            crossfade(final_edit, &black, t, &mut self.scratch);
            self.emit_scratch(progress, tr("Ending replay"))?;
        }
        for frame in 0..OUTRO_BRAND_FADE_FRAMES {
            let t = smootherstep(ramp(frame, OUTRO_BRAND_FADE_FRAMES));
            crossfade(&black, &brand, t, &mut self.scratch);
            self.emit_scratch(progress, BRAND_TITLE)?;
        }
        for _ in 0..OUTRO_HOLD_FRAMES {
            self.emit(progress, &brand, BRAND_TITLE)?;
        }
        Ok(())
    }
}

/// The folder that holds the replay's full-size render stills.
///
/// This is the destination's folder, on the same volume as the output. A
/// Flatpak save dialog instead returns a document-portal path, where the app
/// may create and rename files but not directories, so the stills go to the
/// app's own cache folder.
fn render_cache_parent(destination: &Path) -> PathBuf {
    if crate::platform::is_flatpak() {
        if let Some(cache) = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from) {
            let cache = cache.join("calibraw");
            if std::fs::create_dir_all(&cache).is_ok() {
                return cache;
            }
        }
    }
    destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf()
}

/// Renders and encodes the replay of `request` and publishes it at
/// `destination`, replacing an existing file only once encoding succeeded.
/// Blocks; call it from a worker thread.
///
/// Each stage's still is rendered just before its frames are encoded, so
/// only a few canvases are held at once, however many stages there are.
pub(crate) fn render_edit_replay(
    request: ReplayRequest,
    destination: PathBuf,
    cancellation: &Arc<AtomicBool>,
    progress: &mut dyn FnMut(ReplayProgress),
) -> Result<PathBuf, ReplayError> {
    if request.platform.caps().video_encoder == VideoEncoder::Ffmpeg {
        ensure_ffmpeg_available()?;
    }
    let stages = replay_stage_plan(&request);
    let render_dir = tempfile::Builder::new()
        .prefix("calibraw-edit-replay-")
        .tempdir_in(render_cache_parent(&destination))
        .map_err(|error| {
            tr_fmt(
                "Could not create replay render cache: {error}",
                &[("error", &error)],
            )
        })?;
    let mut progress = ProgressTracker {
        report: progress,
        render_count: stages.len() + 2,
        rendered: 0,
        total_frames: total_replay_frames(&stages) as usize,
        completed_frames: 0,
    };
    let render = |progress: &mut ProgressTracker<'_>, label: &str, state: &ReplayRenderState| {
        render_replay_still(
            &request,
            render_dir.path(),
            cancellation,
            progress,
            label,
            state,
        )
    };

    // The opening compares the original with the final edit, whose still
    // also sets the canvas size, so both render first.
    let original_state = ReplayRenderState::original(request.original_exposure);
    let original_still = render(&mut progress, tr("original"), &original_state)?;
    let final_still = render(
        &mut progress,
        tr("final edit"),
        &final_render_state(&request),
    )?;
    let (canvas_width, canvas_height) =
        replay_canvas_dimensions(final_still.width, final_still.height);
    let (canvas_width, canvas_height) =
        request
            .platform
            .replay_encoder_size(canvas_width, canvas_height, REPLAY_FPS);
    let original_canvas = fit_to_canvas(&original_still, canvas_width, canvas_height);
    let final_canvas = fit_to_canvas(&final_still, canvas_width, canvas_height);
    drop(original_still);
    drop(final_still);

    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let temporary = tempfile::Builder::new()
        .prefix(".calibraw-edit-replay-")
        .suffix(".mp4")
        .tempfile_in(calibraw_core::file_ops::staging_directory(parent))
        .map_err(|error| {
            tr_fmt(
                "Could not create temporary replay output: {error}",
                &[("error", &error)],
            )
        })?
        .into_temp_path();
    // Declared after `temporary`, so an error drops (and stops) the encoder
    // before the unfinished file is removed.
    let mut encoder = ReplayEncoder {
        writer: ReplayFrameWriter::start(
            &*request.platform,
            temporary.as_ref(),
            canvas_width,
            canvas_height,
        )?,
        cancellation,
        width: canvas_width,
        height: canvas_height,
        scratch: vec![0u8; canvas_width as usize * canvas_height as usize * 3],
    };

    encoder.intro(&mut progress, &original_canvas, &final_canvas)?;
    let mut previous = original_canvas;
    for stage in &stages {
        let still = render(&mut progress, &stage.title, &stage.state)?;
        let photo_rect = fitted_rect(still.width, still.height, canvas_width, canvas_height);
        let next = fit_to_canvas(&still, canvas_width, canvas_height);
        drop(still);
        // Mask stages follow every geometry stage, so the previous still
        // frames the photo as this one does and the highlight fits both.
        let highlight = stage.highlight.map(|index| {
            mask_highlight(
                &stage.state.masks,
                index,
                &request.raw,
                stage.state.geometry,
                photo_rect,
                canvas_width,
                canvas_height,
            )
        });
        encoder.stage(&mut progress, stage, highlight.as_deref(), &previous, &next)?;
        previous = next;
    }
    drop(previous);
    // The final hold always uses a separately rendered full request. This makes
    // the last frame exact even if a future edit-state field is not yet assigned
    // to one of the replay stages.
    encoder.outro(&mut progress, &final_canvas)?;

    if cancellation.load(Ordering::Acquire) {
        encoder.writer.cancel();
        return Err(ReplayError::Cancelled);
    }
    let total_frames = progress.total_frames;
    (progress.report)(ReplayProgress {
        fraction: 1.0,
        phase: tr("Finalizing MP4…").to_owned(),
        completed_frames: total_frames,
        total_frames,
    });
    encoder.writer.finish()?;
    ensure_not_cancelled(cancellation)?;
    calibraw_core::file_ops::replace_file(temporary.as_ref(), &destination).map_err(|error| {
        tr_fmt(
            "Could not publish replay {destination}: {error}",
            &[("destination", &destination.display()), ("error", &error)],
        )
    })?;
    if let Some(parent) = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
    {
        let _ = calibraw_core::file_ops::sync_parent_directory(parent);
    }
    let _ = temporary.keep();
    Ok(destination)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_request() -> Option<ReplayRequest> {
        let calibraw_gpu::HeadlessDevice { device, queue, .. } =
            calibraw_gpu::request_headless_device(&calibraw_gpu::HeadlessDeviceRequest {
                label: "replay test",
                adapter: calibraw_gpu::HeadlessAdapter::Preferred(wgpu::PowerPreference::None),
                texture_dimension: wgpu::Limits::default().max_texture_dimension_2d,
            })
            .ok()?;
        let (width, height) = (96, 64);
        let rgb = (0..width * height)
            .flat_map(|index| {
                let (x, y) = (index % width, index / width);
                [x as f32 / width as f32, y as f32 / height as f32, 0.3]
            })
            .collect();
        let raw = LoadedRaw::from_scene_linear_rec2020(width as u32, height as u32, rgb).ok()?;
        let original_exposure = ExposureParams::scene_referred_default();
        let mut final_exposure = original_exposure;
        final_exposure.exposure = 0.8;
        Some(ReplayRequest {
            device,
            queue,
            raw: Arc::new(raw),
            original_exposure,
            final_exposure,
            final_geometry: GeometryTransform {
                crop: [0.1, 0.1, 0.9, 0.9],
                ..GeometryTransform::default()
            },
            final_masks: MaskStack::default(),
            final_remove: RemoveEditState::default(),
            final_look: None,
            detail: ReplayDetail::Normal,
            gpu_export_prewarm: None,
            platform: crate::platform::fake::FakePlatform::new(
                crate::platform::PlatformCaps::DESKTOP,
            ),
        })
    }

    #[test]
    #[ignore = "renders on a wgpu adapter and encodes with FFmpeg (libx264)"]
    fn replay_renders_encodes_and_publishes_an_mp4() {
        let request = test_request().expect("a wgpu adapter");
        let expected_frames = total_replay_frames(&replay_stage_plan(&request)) as usize;
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("replay.mp4");
        let cancellation = Arc::new(AtomicBool::new(false));
        let mut events = Vec::new();

        let published = render_edit_replay(
            request,
            destination.clone(),
            &cancellation,
            &mut |progress| events.push(progress),
        )
        .unwrap();

        assert_eq!(published, destination);
        let bytes = std::fs::read(&destination).unwrap();
        assert_eq!(&bytes[4..8], b"ftyp", "the output is an MP4 file");
        assert!(events
            .windows(2)
            .all(|pair| pair[1].fraction >= pair[0].fraction));
        // The last report covers every encoded frame.
        let last = events.last().unwrap();
        assert_eq!(last.total_frames, expected_frames);
        assert_eq!(last.completed_frames, last.total_frames);
        assert_eq!(
            std::fs::read_dir(directory.path()).unwrap().count(),
            1,
            "only the published MP4 remains"
        );
    }

    #[test]
    #[ignore = "renders on a wgpu adapter and encodes with FFmpeg (libx264)"]
    fn cancelled_replay_publishes_nothing_and_cleans_up() {
        let request = test_request().expect("a wgpu adapter");
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("replay.mp4");
        let cancellation = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancellation);

        let result = render_edit_replay(request, destination.clone(), &cancellation, &mut |_| {
            flag.store(true, Ordering::Release)
        });

        assert_eq!(result, Err(ReplayError::Cancelled));
        assert!(!destination.exists());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }
}
