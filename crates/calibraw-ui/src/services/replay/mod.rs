//! Edit replay: renders the photo at each edit stage and encodes a 30 FPS
//! MP4 that replays the edit category by category.
//!
//! Runs on a worker thread. Progress is reported through a callback and the
//! MP4 is published atomically at the destination. Platform encoders live in
//! `desktop` (FFmpeg) and `android` (MediaCodec through `calibraw-ffi`).

mod frames;
mod plan;

#[cfg(not(target_os = "android"))]
mod desktop;
#[cfg(not(target_os = "android"))]
use desktop::{ensure_ffmpeg_available, ReplayFrameWriter};
#[cfg(any(target_os = "android", test))]
mod android;
#[cfg(target_os = "android")]
use android::ReplayFrameWriter;

use crate::pipeline::{
    spawn_tiled_export, ColorLutEdit, ExportBitDepth, ExportEvent, ExportFormat, ExportMetadata,
    ExportResizeMode, ExportSettings, ExportTarget, ExposureParams, GeometryTransform,
    GpuProgramPrewarm, LoadedRaw, MaskStack, RemoveEditState, TileSpec, TiledExportJob,
};
use calibraw_gpu::wgpu;
use frames::{
    brand_outro_frame, crossfade, draw_stage_title, fit_to_canvas, replay_canvas_dimensions,
    smootherstep, split_frame, stage_title_alpha, RenderedStill,
};
use plan::{add_color_lut, replay_stage_plan, ReplayRenderState};
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
    pub(crate) final_color_lut: Option<ColorLutEdit>,
    pub(crate) gpu_export_prewarm: Option<Arc<GpuProgramPrewarm>>,
    #[cfg(target_os = "android")]
    pub(crate) android_app: calibraw_ffi::AndroidApp,
}

/// Progress of a running replay.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ReplayProgress {
    /// Share of the whole job that is done, from 0 to 1.
    pub(crate) fraction: f32,
    pub(crate) phase: String,
    /// Encoded video frames; both are zero while stills are rendered.
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
            Self::Cancelled => formatter.write_str("edit replay cancelled"),
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
        jpeg_quality: 90,
        bit_depth: ExportBitDepth::Eight,
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
            color_lut: state.color_lut.clone(),
            target: ExportTarget::File(path.to_path_buf()),
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
            Ok(ExportEvent::Finished(Ok(_))) => break,
            // A cancelled export reports an error; the flag decides which it was.
            Ok(ExportEvent::Finished(Err(error))) => {
                ensure_not_cancelled(cancellation)?;
                return Err(ReplayError::Failed(error));
            }
            Err(_) => {
                return Err(ReplayError::Failed(
                    "replay render worker stopped unexpectedly".to_owned(),
                ))
            }
        }
    }
    ensure_not_cancelled(cancellation)?;
    let decoded = image::ImageReader::open(path)
        .map_err(|error| format!("Could not open rendered replay frame: {error}"))?
        .decode()
        .map_err(|error| format!("Could not decode rendered replay frame: {error}"))?
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
        color_lut: request.final_color_lut.clone(),
    }
}

fn total_replay_frames(stage_count: usize) -> u32 {
    SPLIT_HOLD_FRAMES
        + SPLIT_SWEEP_FRAMES * 2
        + FULL_FRAME_HOLD_FRAMES * 2
        + stage_count as u32 * (STAGE_TRANSITION_FRAMES + STAGE_HOLD_FRAMES)
        + FINAL_HOLD_FRAMES
        + OUTRO_FADE_TO_BLACK_FRAMES
        + OUTRO_BRAND_FADE_FRAMES
        + OUTRO_HOLD_FRAMES
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

/// Renders and encodes the replay of `request` and publishes it at
/// `destination`, replacing an existing file only once encoding succeeded.
/// Blocks; call it from a worker thread.
pub(crate) fn render_edit_replay(
    request: ReplayRequest,
    destination: PathBuf,
    cancellation: &Arc<AtomicBool>,
    progress: &mut dyn FnMut(ReplayProgress),
) -> Result<PathBuf, ReplayError> {
    #[cfg(not(target_os = "android"))]
    ensure_ffmpeg_available()?;
    let mut stages = replay_stage_plan(
        request.original_exposure,
        request.final_exposure,
        request.final_geometry,
        &request.final_masks,
        &request.final_remove,
    );
    add_color_lut(
        &mut stages,
        request.original_exposure,
        request.final_color_lut.as_ref(),
    );
    let render_count = stages.len() + 2;
    let render_dir = tempfile::Builder::new()
        .prefix("calibraw-edit-replay-")
        .tempdir_in(destination.parent().unwrap_or_else(|| Path::new(".")))
        .map_err(|error| format!("Could not create replay render cache: {error}"))?;

    let original_state = ReplayRenderState::original(request.original_exposure);
    let mut rendered = Vec::with_capacity(render_count);
    let mut render_endpoint = |index: usize,
                               label: &str,
                               state: &ReplayRenderState|
     -> Result<RenderedStill, ReplayError> {
        let path = render_dir.path().join(format!("stage-{index:02}.png"));
        let base = index as f32 / render_count as f32;
        let step = 1.0 / render_count as f32;
        let still = render_still(&request, state, &path, cancellation, |done, total| {
            let tile_fraction = if total == 0 {
                0.0
            } else {
                done as f32 / total as f32
            };
            progress(ReplayProgress {
                fraction: RENDER_PROGRESS_WEIGHT * (base + step * tile_fraction),
                phase: format!("Rendering {label}…"),
                completed_frames: 0,
                total_frames: 0,
            });
        })?;
        progress(ReplayProgress {
            fraction: RENDER_PROGRESS_WEIGHT * ((index + 1) as f32 / render_count as f32),
            phase: format!("Rendered {label}"),
            completed_frames: 0,
            total_frames: 0,
        });
        Ok(still)
    };

    rendered.push(render_endpoint(0, "original", &original_state)?);
    for (stage_index, stage) in stages.iter().enumerate() {
        rendered.push(render_endpoint(
            stage_index + 1,
            stage.kind.label(),
            &stage.state,
        )?);
    }
    let final_state = final_render_state(&request);
    let final_still = render_endpoint(render_count - 1, "final edit", &final_state)?;

    let (canvas_width, canvas_height) =
        replay_canvas_dimensions(final_still.width, final_still.height);
    let original_canvas = fit_to_canvas(&rendered[0], canvas_width, canvas_height);
    let mut stage_canvases = rendered
        .iter()
        .skip(1)
        .map(|still| fit_to_canvas(still, canvas_width, canvas_height))
        .collect::<Vec<_>>();
    let final_canvas = fit_to_canvas(&final_still, canvas_width, canvas_height);
    // Release rendered endpoints before allocating and encoding video frames on mobile.
    drop(rendered);
    drop(final_still);
    let brand_canvas = brand_outro_frame(canvas_width, canvas_height)?;
    let black_canvas = vec![0u8; final_canvas.len()];

    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let temporary = tempfile::Builder::new()
        .prefix(".calibraw-edit-replay-")
        .suffix(".mp4")
        .tempfile_in(parent)
        .map_err(|error| format!("Could not create temporary replay output: {error}"))?
        .into_temp_path();
    #[cfg(not(target_os = "android"))]
    let mut writer = ReplayFrameWriter::start(temporary.as_ref(), canvas_width, canvas_height)?;
    #[cfg(target_os = "android")]
    let mut writer = ReplayFrameWriter::start(
        &request.android_app,
        temporary.as_ref(),
        canvas_width,
        canvas_height,
    )?;
    let total_frames = total_replay_frames(stages.len()) as usize;
    let mut completed_frames = 0usize;
    let mut scratch = vec![0u8; canvas_width as usize * canvas_height as usize * 3];

    let mut emit =
        |writer: &mut ReplayFrameWriter, frame: &[u8], phase: &str| -> Result<(), ReplayError> {
            write_video_frame(writer, cancellation, frame)?;
            completed_frames += 1;
            if completed_frames == total_frames || completed_frames.is_multiple_of(3) {
                let encode_fraction = completed_frames as f32 / total_frames.max(1) as f32;
                progress(ReplayProgress {
                    fraction: RENDER_PROGRESS_WEIGHT
                        + (1.0 - RENDER_PROGRESS_WEIGHT) * encode_fraction,
                    phase: phase.to_owned(),
                    completed_frames,
                    total_frames,
                });
            }
            Ok(())
        };

    split_frame(
        &original_canvas,
        &final_canvas,
        canvas_width,
        canvas_height,
        0.5,
        &mut scratch,
    );
    for _ in 0..SPLIT_HOLD_FRAMES {
        emit(&mut writer, &scratch, "Before / after")?;
    }
    for frame in 0..SPLIT_SWEEP_FRAMES {
        let t = if SPLIT_SWEEP_FRAMES <= 1 {
            1.0
        } else {
            frame as f32 / (SPLIT_SWEEP_FRAMES - 1) as f32
        };
        split_frame(
            &original_canvas,
            &final_canvas,
            canvas_width,
            canvas_height,
            0.5 * (1.0 - smootherstep(t)),
            &mut scratch,
        );
        emit(&mut writer, &scratch, "Revealing final edit")?;
    }
    for _ in 0..FULL_FRAME_HOLD_FRAMES {
        emit(&mut writer, &final_canvas, "Final edit")?;
    }
    for frame in 0..SPLIT_SWEEP_FRAMES {
        let t = if SPLIT_SWEEP_FRAMES <= 1 {
            1.0
        } else {
            frame as f32 / (SPLIT_SWEEP_FRAMES - 1) as f32
        };
        split_frame(
            &original_canvas,
            &final_canvas,
            canvas_width,
            canvas_height,
            smootherstep(t),
            &mut scratch,
        );
        emit(&mut writer, &scratch, "Returning to original")?;
    }
    for _ in 0..FULL_FRAME_HOLD_FRAMES {
        emit(&mut writer, &original_canvas, "Original")?;
    }

    let mut previous = original_canvas;
    for (stage_index, stage) in stages.iter().enumerate() {
        let next = stage_canvases
            .get_mut(stage_index)
            .expect("each replay stage has a rendered endpoint");
        let title_phase = format!("Applying {}", stage.kind.label());
        for frame in 0..STAGE_TRANSITION_FRAMES {
            let t = if STAGE_TRANSITION_FRAMES <= 1 {
                1.0
            } else {
                frame as f32 / (STAGE_TRANSITION_FRAMES - 1) as f32
            };
            crossfade(&previous, next, smootherstep(t), &mut scratch);
            draw_stage_title(
                &mut scratch,
                canvas_width,
                canvas_height,
                stage.kind.label(),
                stage_title_alpha(frame, STAGE_TRANSITION_FRAMES, STAGE_HOLD_FRAMES),
            );
            emit(&mut writer, &scratch, &title_phase)?;
        }
        for hold in 0..STAGE_HOLD_FRAMES {
            scratch.copy_from_slice(next);
            draw_stage_title(
                &mut scratch,
                canvas_width,
                canvas_height,
                stage.kind.label(),
                stage_title_alpha(
                    STAGE_TRANSITION_FRAMES + hold,
                    STAGE_TRANSITION_FRAMES,
                    STAGE_HOLD_FRAMES,
                ),
            );
            emit(&mut writer, &scratch, &title_phase)?;
        }
        previous = std::mem::take(next);
    }

    // The final hold always uses a separately rendered full request. This makes
    // the last frame exact even if a future edit-state field is not yet assigned
    // to one of the replay categories above.
    for _ in 0..FINAL_HOLD_FRAMES {
        emit(&mut writer, &final_canvas, "Holding final edit")?;
    }

    for frame in 0..OUTRO_FADE_TO_BLACK_FRAMES {
        let t = if OUTRO_FADE_TO_BLACK_FRAMES <= 1 {
            1.0
        } else {
            frame as f32 / (OUTRO_FADE_TO_BLACK_FRAMES - 1) as f32
        };
        crossfade(&final_canvas, &black_canvas, smootherstep(t), &mut scratch);
        emit(&mut writer, &scratch, "Ending replay")?;
    }
    for frame in 0..OUTRO_BRAND_FADE_FRAMES {
        let t = if OUTRO_BRAND_FADE_FRAMES <= 1 {
            1.0
        } else {
            frame as f32 / (OUTRO_BRAND_FADE_FRAMES - 1) as f32
        };
        crossfade(&black_canvas, &brand_canvas, smootherstep(t), &mut scratch);
        emit(&mut writer, &scratch, "CalibRaw")?;
    }
    for _ in 0..OUTRO_HOLD_FRAMES {
        emit(&mut writer, &brand_canvas, "CalibRaw")?;
    }

    if cancellation.load(Ordering::Acquire) {
        writer.cancel();
        return Err(ReplayError::Cancelled);
    }
    progress(ReplayProgress {
        fraction: 1.0,
        phase: "Finalizing MP4…".to_owned(),
        completed_frames: total_frames,
        total_frames,
    });
    writer.finish()?;
    ensure_not_cancelled(cancellation)?;
    calibraw_core::file_ops::replace_file(temporary.as_ref(), &destination).map_err(|error| {
        format!(
            "Could not publish replay {}: {error}",
            destination.display()
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

    #[test]
    fn replay_frame_count_grows_only_with_active_stages() {
        let base = total_replay_frames(0);
        assert_eq!(
            total_replay_frames(1) - base,
            STAGE_TRANSITION_FRAMES + STAGE_HOLD_FRAMES
        );
        assert_eq!(
            total_replay_frames(6) - base,
            6 * (STAGE_TRANSITION_FRAMES + STAGE_HOLD_FRAMES)
        );
    }

    fn test_request() -> Option<ReplayRequest> {
        let instance = wgpu::Instance::default();
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
                .ok()?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
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
            final_color_lut: None,
            gpu_export_prewarm: None,
        })
    }

    #[test]
    #[ignore = "renders on a wgpu adapter and encodes with FFmpeg (libx264)"]
    fn replay_renders_encodes_and_publishes_an_mp4() {
        let request = test_request().expect("a wgpu adapter");
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
        // Edit and Crop stages; the last report covers every encoded frame.
        let last = events.last().unwrap();
        assert_eq!(last.total_frames, total_replay_frames(2) as usize);
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
