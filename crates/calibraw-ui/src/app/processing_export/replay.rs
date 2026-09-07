use super::*;

use crate::pipeline::{ExportBitDepth, ExportResizeMode};
use image::{imageops::FilterType, RgbImage};
use std::ffi::OsString;
use std::io::Write as _;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;

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
const RENDER_PROGRESS_WEIGHT: f32 = 0.28;
const BRAND_TITLE: &str = "CalibRaw";
const BRAND_SUBTITLE: &str = "A fast, GPU-accelerated open source RAW editor.";
const BRAND_ICON_PNG: &[u8] =
    include_bytes!("../../../../../packaging/icons/calibraw-256.png");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReplayStageKind {
    Edit,
    Crop,
    Rotate,
    Transform,
    Masks,
    Remove,
}

impl ReplayStageKind {
    const fn label(self) -> &'static str {
        match self {
            Self::Edit => "Edit",
            Self::Crop => "Crop",
            Self::Rotate => "Rotate",
            Self::Transform => "Transform",
            Self::Masks => "Masks",
            Self::Remove => "Remove",
        }
    }
}

#[derive(Clone)]
struct ReplayRenderState {
    exposure: ExposureParams,
    geometry: GeometryTransform,
    masks: MaskStack,
    remove: RemoveEditState,
}

impl ReplayRenderState {
    fn original(original_exposure: ExposureParams) -> Self {
        Self {
            exposure: original_exposure,
            geometry: GeometryTransform::default(),
            masks: MaskStack::default(),
            remove: RemoveEditState::default(),
        }
    }
}

struct ReplayStage {
    kind: ReplayStageKind,
    state: ReplayRenderState,
}

struct EditReplaySnapshot {
    device: wgpu::Device,
    queue: wgpu::Queue,
    raw: Arc<LoadedRaw>,
    original_exposure: ExposureParams,
    final_exposure: ExposureParams,
    final_geometry: GeometryTransform,
    final_masks: MaskStack,
    final_remove: RemoveEditState,
    gpu_export_prewarm: Option<Arc<GpuProgramPrewarm>>,
}

struct RenderedStill {
    width: u32,
    height: u32,
    rgb: Vec<u8>,
}

struct ReplayFrameWriter {
    child: std::process::Child,
    stdin: Option<std::process::ChildStdin>,
}

impl ReplayFrameWriter {
    fn start(path: &Path, width: u32, height: u32) -> Result<Self, String> {
        let ffmpeg = ffmpeg_program();
        let size = format!("{width}x{height}");
        let fps = REPLAY_FPS.to_string();
        let mut command = Command::new(&ffmpeg);
        command
            .arg("-hide_banner")
            .arg("-loglevel")
            .arg("error")
            .arg("-y")
            .arg("-f")
            .arg("rawvideo")
            .arg("-pix_fmt")
            .arg("rgb24")
            .arg("-s")
            .arg(size)
            .arg("-r")
            .arg(&fps)
            .arg("-i")
            .arg("pipe:0")
            .arg("-an")
            .arg("-c:v")
            .arg("libx264")
            .arg("-preset")
            .arg("medium")
            .arg("-crf")
            .arg("23")
            .arg("-pix_fmt")
            .arg("yuv420p")
            .arg("-movflags")
            .arg("+faststart")
            .arg("-f")
            .arg("mp4")
            .arg(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        command.creation_flags(0x08000000);

        let mut child = command.spawn().map_err(|error| {
            format!(
                "Could not start FFmpeg ({:?}). Install FFmpeg with H.264/libx264 support or set CALIBRAW_FFMPEG: {error}",
                ffmpeg
            )
        })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "FFmpeg did not provide a video input pipe".to_owned())?;
        Ok(Self {
            child,
            stdin: Some(stdin),
        })
    }

    fn write_frame(&mut self, rgb: &[u8]) -> Result<(), String> {
        self.stdin
            .as_mut()
            .ok_or_else(|| "FFmpeg video input pipe is closed".to_owned())?
            .write_all(rgb)
            .map_err(|error| format!("Could not send replay frame to FFmpeg: {error}"))
    }

    fn finish(mut self) -> Result<(), String> {
        drop(self.stdin.take());
        let output = self
            .child
            .wait_with_output()
            .map_err(|error| format!("Could not finish FFmpeg replay encoding: {error}"))?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        if detail.is_empty() {
            Err(format!("FFmpeg exited with {}", output.status))
        } else {
            Err(format!("FFmpeg failed: {detail}"))
        }
    }

    fn cancel(&mut self) {
        self.stdin.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn ffmpeg_program() -> OsString {
    std::env::var_os("CALIBRAW_FFMPEG")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| OsString::from("ffmpeg"))
}

fn ensure_ffmpeg_available() -> Result<(), String> {
    let ffmpeg = ffmpeg_program();
    let output = Command::new(&ffmpeg)
        .arg("-hide_banner")
        .arg("-encoders")
        .output()
        .map_err(|error| {
            format!(
                "Could not start FFmpeg ({:?}). Install FFmpeg with H.264/libx264 support or set CALIBRAW_FFMPEG: {error}",
                ffmpeg
            )
        })?;
    if !output.status.success() {
        return Err(format!("FFmpeg encoder probe exited with {}", output.status));
    }
    let encoders = String::from_utf8_lossy(&output.stdout);
    if !encoders.contains("libx264") {
        return Err("FFmpeg is available but does not provide the libx264 H.264 encoder".to_owned());
    }
    Ok(())
}

fn replay_stage_plan(
    original_exposure: ExposureParams,
    final_exposure: ExposureParams,
    final_geometry: GeometryTransform,
    final_masks: &MaskStack,
    final_remove: &RemoveEditState,
) -> Vec<ReplayStage> {
    let final_geometry = final_geometry.sanitized();
    let mut current = ReplayRenderState::original(original_exposure);
    let mut stages = Vec::new();

    if final_exposure != original_exposure {
        current.exposure = final_exposure;
        stages.push(ReplayStage {
            kind: ReplayStageKind::Edit,
            state: current.clone(),
        });
    }

    if crop_used(final_geometry) {
        current.geometry.crop = final_geometry.crop;
        current.geometry.aspect_ratio = final_geometry.aspect_ratio;
        stages.push(ReplayStage {
            kind: ReplayStageKind::Crop,
            state: current.clone(),
        });
    }

    if rotate_used(final_geometry) {
        current.geometry.quarter_turns = final_geometry.quarter_turns;
        current.geometry.rotation_degrees = final_geometry.rotation_degrees;
        current.geometry.flip_horizontal = final_geometry.flip_horizontal;
        current.geometry.flip_vertical = final_geometry.flip_vertical;
        stages.push(ReplayStage {
            kind: ReplayStageKind::Rotate,
            state: current.clone(),
        });
    }

    if transform_used(final_geometry) {
        current.geometry.horizontal_transform = final_geometry.horizontal_transform;
        current.geometry.vertical_transform = final_geometry.vertical_transform;
        stages.push(ReplayStage {
            kind: ReplayStageKind::Transform,
            state: current.clone(),
        });
    }

    if masks_used(final_masks) {
        current.masks = final_masks.clone();
        stages.push(ReplayStage {
            kind: ReplayStageKind::Masks,
            state: current.clone(),
        });
    }

    if remove_used(final_remove) {
        current.remove = final_remove.clone();
        stages.push(ReplayStage {
            kind: ReplayStageKind::Remove,
            state: current,
        });
    }

    stages
}

fn crop_used(geometry: GeometryTransform) -> bool {
    geometry.crop != GeometryTransform::default().crop
}

fn rotate_used(geometry: GeometryTransform) -> bool {
    geometry.quarter_turns != 0
        || geometry.rotation_degrees.abs() >= 1e-4
        || geometry.flip_horizontal
        || geometry.flip_vertical
}

fn transform_used(geometry: GeometryTransform) -> bool {
    geometry.horizontal_transform.abs() >= 1e-4 || geometry.vertical_transform.abs() >= 1e-4
}

fn masks_used(masks: &MaskStack) -> bool {
    masks.masks.iter().any(|mask| {
        if !mask.enabled || mask.opacity <= 1e-6 {
            return false;
        }
        if !mask
            .components
            .iter()
            .any(|component| component.enabled && component.geometry.is_initialized())
        {
            return false;
        }

        match mask.effect {
            crate::pipeline::MaskEffect::Adjustment => !mask.adjustments.is_neutral(),
            crate::pipeline::MaskEffect::Blur => mask.effect_settings.blur.is_active(),
            crate::pipeline::MaskEffect::LensBlur => mask.effect_settings.lens_blur.is_active(),
            crate::pipeline::MaskEffect::MotionBlur => mask.effect_settings.motion_blur.is_active(),
            crate::pipeline::MaskEffect::RadialBlur => mask.effect_settings.radial_blur.is_active(),
            crate::pipeline::MaskEffect::TiltShift => mask.effect_settings.tilt_shift.is_active(),
            crate::pipeline::MaskEffect::Glow => mask.effect_settings.glow.is_active(),
            crate::pipeline::MaskEffect::LightRays => mask.effect_settings.light_rays.is_active(),
            crate::pipeline::MaskEffect::Neon => mask.effect_settings.neon.is_active(),
            crate::pipeline::MaskEffect::EdgeGlow => mask.effect_settings.edge_glow.is_active(),
            crate::pipeline::MaskEffect::Pixelate => mask.effect_settings.pixelate.is_active(),
            crate::pipeline::MaskEffect::Fog => mask.effect_settings.fog.is_active(),
            crate::pipeline::MaskEffect::Smoke => mask.effect_settings.smoke.is_active(),
        }
    })
}

fn remove_used(remove: &RemoveEditState) -> bool {
    remove.strokes.iter().any(|stroke| {
        stroke.composite_opacity() > 1e-6
            && (stroke.retouch.is_some()
                || !stroke.patches.is_empty()
                || !stroke.brush.points.is_empty())
    })
}

fn render_state(
    snapshot: &EditReplaySnapshot,
    state: &ReplayRenderState,
    path: &Path,
    cancellation: &Arc<AtomicBool>,
    mut tile_progress: impl FnMut(usize, usize),
) -> Result<RenderedStill, String> {
    if cancellation.load(Ordering::Acquire) {
        return Err("edit replay cancelled".to_owned());
    }
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
            device: snapshot.device.clone(),
            queue: snapshot.queue.clone(),
            raw: Arc::clone(&snapshot.raw),
            geometry: state.geometry,
            exposure: state.exposure,
            masks: state.masks.clone(),
            remove: state.remove.clone(),
            path: path.to_path_buf(),
            tile_spec: TileSpec::default(),
            settings,
            metadata: ExportMetadata::default(),
            cancellation: Arc::clone(cancellation),
            program_prewarm: snapshot.gpu_export_prewarm.as_ref().map(Arc::clone),
        },
    );
    loop {
        match receiver.recv() {
            Ok(ExportEvent::Progress {
                completed_tiles,
                total_tiles,
            }) => tile_progress(completed_tiles, total_tiles),
            Ok(ExportEvent::Finished(Ok(_))) => break,
            Ok(ExportEvent::Finished(Err(error))) => return Err(error),
            Err(_) => return Err("replay render worker stopped unexpectedly".to_owned()),
        }
    }
    if cancellation.load(Ordering::Acquire) {
        return Err("edit replay cancelled".to_owned());
    }
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

fn final_render_state(snapshot: &EditReplaySnapshot) -> ReplayRenderState {
    ReplayRenderState {
        exposure: snapshot.final_exposure,
        geometry: snapshot.final_geometry.sanitized(),
        masks: snapshot.final_masks.clone(),
        remove: snapshot.final_remove.clone(),
    }
}

fn even_dimension(value: u32) -> u32 {
    let value = value.max(2);
    if value.is_multiple_of(2) {
        value
    } else {
        value - 1
    }
}

fn replay_canvas_dimensions(width: u32, height: u32) -> (u32, u32) {
    let width = width.max(1);
    let height = height.max(1);
    let (maximum_width, maximum_height) = if width >= height {
        (1920.0_f64, 1080.0_f64)
    } else {
        (1080.0_f64, 1920.0_f64)
    };
    let scale = (maximum_width / f64::from(width)).min(maximum_height / f64::from(height));
    let output_width = (f64::from(width) * scale).round().max(2.0) as u32;
    let output_height = (f64::from(height) * scale).round().max(2.0) as u32;
    (even_dimension(output_width), even_dimension(output_height))
}

fn fit_to_canvas(still: &RenderedStill, canvas_width: u32, canvas_height: u32) -> Vec<u8> {
    let source = RgbImage::from_raw(still.width, still.height, still.rgb.clone())
        .expect("rendered replay still has a valid RGB byte count");
    let scale = (canvas_width as f64 / still.width.max(1) as f64)
        .min(canvas_height as f64 / still.height.max(1) as f64);
    let width = (still.width as f64 * scale)
        .round()
        .clamp(1.0, canvas_width as f64) as u32;
    let height = (still.height as f64 * scale)
        .round()
        .clamp(1.0, canvas_height as f64) as u32;
    let resized = if width == still.width && height == still.height {
        source
    } else {
        image::imageops::resize(&source, width, height, FilterType::Lanczos3)
    };
    let mut canvas = vec![18u8; canvas_width as usize * canvas_height as usize * 3];
    let x0 = (canvas_width - width) / 2;
    let y0 = (canvas_height - height) / 2;
    for y in 0..height {
        let source_start = y as usize * width as usize * 3;
        let destination_start = ((y + y0) as usize * canvas_width as usize + x0 as usize) * 3;
        let bytes = width as usize * 3;
        canvas[destination_start..destination_start + bytes]
            .copy_from_slice(&resized.as_raw()[source_start..source_start + bytes]);
    }
    canvas
}

fn smootherstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

fn crossfade(a: &[u8], b: &[u8], amount: f32, output: &mut [u8]) {
    let amount = amount.clamp(0.0, 1.0);
    let inverse = 1.0 - amount;
    for ((out, left), right) in output.iter_mut().zip(a.iter()).zip(b.iter()) {
        *out = (f32::from(*left) * inverse + f32::from(*right) * amount)
            .round()
            .clamp(0.0, 255.0) as u8;
    }
}

fn split_frame(
    original: &[u8],
    final_edit: &[u8],
    width: u32,
    height: u32,
    divider: f32,
    output: &mut [u8],
) {
    let split_x = (divider.clamp(0.0, 1.0) * width as f32).round() as u32;
    for y in 0..height {
        for x in 0..width {
            let index = (y as usize * width as usize + x as usize) * 3;
            let source = if x < split_x { original } else { final_edit };
            output[index..index + 3].copy_from_slice(&source[index..index + 3]);
        }
    }
    let divider_width = (width / 480).clamp(2, 5);
    let start = split_x.saturating_sub(divider_width / 2);
    let end = split_x.saturating_add(divider_width.div_ceil(2)).min(width);
    for y in 0..height {
        for x in start..end {
            let index = (y as usize * width as usize + x as usize) * 3;
            output[index..index + 3].copy_from_slice(&[238, 238, 238]);
        }
    }
}

fn blend_pixel(pixel: &mut [u8], color: [u8; 3], alpha: u8) {
    let alpha = f32::from(alpha) / 255.0;
    let inverse = 1.0 - alpha;
    for channel in 0..3 {
        pixel[channel] = (f32::from(pixel[channel]) * inverse + f32::from(color[channel]) * alpha)
            .round()
            .clamp(0.0, 255.0) as u8;
    }
}

fn glyph_rows(character: char) -> [u8; 7] {
    match character {
        'A' => [0x0e, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11],
        'B' => [0x1e, 0x11, 0x11, 0x1e, 0x11, 0x11, 0x1e],
        'C' => [0x0e, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0e],
        'D' => [0x1e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1e],
        'E' => [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x1f],
        'F' => [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x10],
        'G' => [0x0e, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0f],
        'I' => [0x1f, 0x04, 0x04, 0x04, 0x04, 0x04, 0x1f],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1f],
        'M' => [0x11, 0x1b, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11],
        'O' => [0x0e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e],
        'P' => [0x1e, 0x11, 0x11, 0x1e, 0x10, 0x10, 0x10],
        'R' => [0x1e, 0x11, 0x11, 0x1e, 0x14, 0x12, 0x11],
        'S' => [0x0f, 0x10, 0x10, 0x0e, 0x01, 0x01, 0x1e],
        'T' => [0x1f, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0a, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x15, 0x0a],
        'a' => [0x00, 0x00, 0x0e, 0x01, 0x0f, 0x11, 0x0f],
        'b' => [0x10, 0x10, 0x16, 0x19, 0x11, 0x11, 0x1e],
        'c' => [0x00, 0x00, 0x0e, 0x10, 0x10, 0x11, 0x0e],
        'd' => [0x01, 0x01, 0x0d, 0x13, 0x11, 0x11, 0x0f],
        'e' => [0x00, 0x00, 0x0e, 0x11, 0x1f, 0x10, 0x0e],
        'f' => [0x06, 0x08, 0x1e, 0x08, 0x08, 0x08, 0x08],
        'i' => [0x04, 0x00, 0x0c, 0x04, 0x04, 0x04, 0x0e],
        'l' => [0x0c, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0e],
        'n' => [0x00, 0x00, 0x16, 0x19, 0x11, 0x11, 0x11],
        'o' => [0x00, 0x00, 0x0e, 0x11, 0x11, 0x11, 0x0e],
        'p' => [0x00, 0x00, 0x1e, 0x11, 0x1e, 0x10, 0x10],
        'r' => [0x00, 0x00, 0x16, 0x19, 0x10, 0x10, 0x10],
        's' => [0x00, 0x00, 0x0f, 0x10, 0x0e, 0x01, 0x1e],
        't' => [0x08, 0x08, 0x1e, 0x08, 0x08, 0x09, 0x06],
        'u' => [0x00, 0x00, 0x11, 0x11, 0x11, 0x13, 0x0d],
        'w' => [0x00, 0x00, 0x11, 0x11, 0x15, 0x15, 0x0a],
        '-' => [0x00, 0x00, 0x00, 0x0e, 0x00, 0x00, 0x00],
        ',' => [0x00, 0x00, 0x00, 0x00, 0x06, 0x06, 0x04],
        '.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x06, 0x06],
        ' ' => [0; 7],
        _ => [0; 7],
    }
}

fn bitmap_text_width(text: &str, scale: i32) -> i32 {
    let count = text.chars().count() as i32;
    if count == 0 {
        return 0;
    }
    count * 5 * scale + (count - 1) * scale
}

fn draw_bitmap_text(
    frame: &mut [u8],
    width: u32,
    height: u32,
    text: &str,
    start_x: i32,
    start_y: i32,
    scale: i32,
    color: [u8; 3],
    alpha: u8,
) {
    let mut cursor_x = start_x;
    for character in text.chars() {
        let rows = glyph_rows(character);
        for (row, bits) in rows.into_iter().enumerate() {
            for column in 0..5 {
                if bits & (1 << (4 - column)) == 0 {
                    continue;
                }
                for sy in 0..scale {
                    for sx in 0..scale {
                        let x = cursor_x + column * scale + sx;
                        let y = start_y + row as i32 * scale + sy;
                        if x < 0 || y < 0 || x >= width as i32 || y >= height as i32 {
                            continue;
                        }
                        let index = (y as usize * width as usize + x as usize) * 3;
                        blend_pixel(&mut frame[index..index + 3], color, alpha);
                    }
                }
            }
        }
        cursor_x += 6 * scale;
    }
}

fn fitted_bitmap_scale(text: &str, max_width: u32, preferred: u32) -> i32 {
    let units = (text.chars().count().max(1) * 6 - 1) as u32;
    (max_width / units).min(preferred).max(1) as i32
}

fn brand_outro_frame(width: u32, height: u32) -> Result<Vec<u8>, String> {
    let mut frame = vec![0u8; width as usize * height as usize * 3];
    let icon = image::load_from_memory(BRAND_ICON_PNG)
        .map_err(|error| format!("Could not decode embedded CalibRaw icon: {error}"))?
        .into_rgb8();
    let short_edge = width.min(height);
    let icon_side = ((short_edge as f32 * 0.18).round() as u32).clamp(96, 256);
    let icon = image::imageops::resize(&icon, icon_side, icon_side, FilterType::Lanczos3);

    let brand_scale = fitted_bitmap_scale(BRAND_TITLE, width * 3 / 4, (short_edge / 90).clamp(7, 14));
    let subtitle_scale =
        fitted_bitmap_scale(BRAND_SUBTITLE, width * 9 / 10, (short_edge / 230).clamp(3, 6));
    let brand_height = 7 * brand_scale;
    let subtitle_height = 7 * subtitle_scale;
    let gap_after_icon = (short_edge as f32 * 0.055).round() as i32;
    let gap_after_title = (short_edge as f32 * 0.035).round() as i32;
    let group_height = icon_side as i32
        + gap_after_icon
        + brand_height
        + gap_after_title
        + subtitle_height;
    let group_top = ((height as i32 - group_height) / 2).max(0);

    let icon_x = (width.saturating_sub(icon_side) / 2) as usize;
    let icon_y = group_top as usize;
    for y in 0..icon_side as usize {
        let source_start = y * icon_side as usize * 3;
        let destination_start = ((icon_y + y) * width as usize + icon_x) * 3;
        let bytes = icon_side as usize * 3;
        frame[destination_start..destination_start + bytes]
            .copy_from_slice(&icon.as_raw()[source_start..source_start + bytes]);
    }

    let brand_y = group_top + icon_side as i32 + gap_after_icon;
    let brand_x = (width as i32 - bitmap_text_width(BRAND_TITLE, brand_scale)) / 2;
    draw_bitmap_text(
        &mut frame,
        width,
        height,
        BRAND_TITLE,
        brand_x,
        brand_y,
        brand_scale,
        [255, 255, 255],
        255,
    );

    let subtitle_y = brand_y + brand_height + gap_after_title;
    let subtitle_x =
        (width as i32 - bitmap_text_width(BRAND_SUBTITLE, subtitle_scale)) / 2;
    draw_bitmap_text(
        &mut frame,
        width,
        height,
        BRAND_SUBTITLE,
        subtitle_x,
        subtitle_y,
        subtitle_scale,
        [205, 205, 205],
        255,
    );
    Ok(frame)
}

fn rounded_rect_contains(x: i32, y: i32, width: i32, height: i32, radius: i32) -> bool {
    if x < 0 || y < 0 || x >= width || y >= height {
        return false;
    }
    if x >= radius && x < width - radius || y >= radius && y < height - radius {
        return true;
    }
    let cx = if x < radius { radius } else { width - radius - 1 };
    let cy = if y < radius { radius } else { height - radius - 1 };
    let dx = x - cx;
    let dy = y - cy;
    dx * dx + dy * dy <= radius * radius
}

fn draw_stage_title(frame: &mut [u8], width: u32, height: u32, title: &str, alpha: f32) {
    let title = title.to_ascii_uppercase();
    let scale = (width.min(height) / 280).clamp(3, 6) as i32;
    let glyph_width = 5 * scale;
    let spacing = scale;
    let text_width = title.chars().count() as i32 * (glyph_width + spacing) - spacing;
    let padding_x = 4 * scale;
    let padding_y = 3 * scale;
    let box_width = text_width + padding_x * 2;
    let box_height = 7 * scale + padding_y * 2;
    let box_x = (width as i32 - box_width) / 2;
    let box_y = (height as f32 * 0.045).round() as i32;
    let radius = 3 * scale;
    let alpha = alpha.clamp(0.0, 1.0);
    let background_alpha = (176.0 * alpha).round() as u8;
    let text_alpha = (245.0 * alpha).round() as u8;

    for local_y in 0..box_height {
        for local_x in 0..box_width {
            if !rounded_rect_contains(local_x, local_y, box_width, box_height, radius) {
                continue;
            }
            let x = box_x + local_x;
            let y = box_y + local_y;
            if x < 0 || y < 0 || x >= width as i32 || y >= height as i32 {
                continue;
            }
            let index = (y as usize * width as usize + x as usize) * 3;
            blend_pixel(&mut frame[index..index + 3], [8, 8, 8], background_alpha);
        }
    }

    draw_bitmap_text(
        frame,
        width,
        height,
        &title,
        box_x + padding_x,
        box_y + padding_y,
        scale,
        [255, 255, 255],
        text_alpha,
    );
}

fn stage_title_alpha(frame: u32, transition_frames: u32, hold_frames: u32) -> f32 {
    let total = transition_frames + hold_frames;
    let fade = 6u32.min(total / 2).max(1);
    if frame < fade {
        smootherstep((frame + 1) as f32 / fade as f32)
    } else if frame + fade >= total {
        smootherstep((total - frame) as f32 / fade as f32)
    } else {
        1.0
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

fn send_progress(
    sender: &mpsc::Sender<ReplayExportEvent>,
    repaint: &egui::Context,
    progress: f32,
    phase: impl Into<String>,
    completed_frames: usize,
    total_frames: usize,
) {
    let _ = sender.send(ReplayExportEvent::Progress {
        progress: progress.clamp(0.0, EXPORT_MAX_INCOMPLETE_FRACTION),
        phase: phase.into(),
        completed_frames,
        total_frames,
    });
    repaint.request_repaint();
}

fn write_video_frame(
    writer: &mut ReplayFrameWriter,
    cancellation: &Arc<AtomicBool>,
    frame: &[u8],
) -> Result<(), String> {
    if cancellation.load(Ordering::Acquire) {
        writer.cancel();
        return Err("edit replay cancelled".to_owned());
    }
    writer.write_frame(frame)
}

fn run_edit_replay_worker(
    snapshot: EditReplaySnapshot,
    destination: PathBuf,
    cancellation: Arc<AtomicBool>,
    sender: mpsc::Sender<ReplayExportEvent>,
    repaint: egui::Context,
) -> Result<PathBuf, String> {
    ensure_ffmpeg_available()?;
    let stages = replay_stage_plan(
        snapshot.original_exposure,
        snapshot.final_exposure,
        snapshot.final_geometry,
        &snapshot.final_masks,
        &snapshot.final_remove,
    );
    let render_count = stages.len() + 2;
    let render_dir = tempfile::Builder::new()
        .prefix("calibraw-edit-replay-")
        .tempdir()
        .map_err(|error| format!("Could not create replay render cache: {error}"))?;

    let original_state = ReplayRenderState::original(snapshot.original_exposure);
    let mut rendered = Vec::with_capacity(render_count);
    let mut render_endpoint = |index: usize,
                               label: &str,
                               state: &ReplayRenderState|
     -> Result<RenderedStill, String> {
        let path = render_dir.path().join(format!("stage-{index:02}.png"));
        let base = index as f32 / render_count as f32;
        let step = 1.0 / render_count as f32;
        let still = render_state(&snapshot, state, &path, &cancellation, |done, total| {
            let tile_fraction = if total == 0 {
                0.0
            } else {
                done as f32 / total as f32
            };
            send_progress(
                &sender,
                &repaint,
                RENDER_PROGRESS_WEIGHT * (base + step * tile_fraction),
                format!("Rendering {label}…"),
                0,
                0,
            );
        })?;
        send_progress(
            &sender,
            &repaint,
            RENDER_PROGRESS_WEIGHT * ((index + 1) as f32 / render_count as f32),
            format!("Rendered {label}"),
            0,
            0,
        );
        Ok(still)
    };

    rendered.push(render_endpoint(0, "original", &original_state)?);
    for (stage_index, stage) in stages.iter().enumerate() {
        rendered.push(render_endpoint(stage_index + 1, stage.kind.label(), &stage.state)?);
    }
    let final_state = final_render_state(&snapshot);
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
    let mut writer = ReplayFrameWriter::start(temporary.as_ref(), canvas_width, canvas_height)?;
    let total_frames = total_replay_frames(stages.len()) as usize;
    let mut completed_frames = 0usize;
    let mut scratch = vec![0u8; canvas_width as usize * canvas_height as usize * 3];

    let mut emit = |writer: &mut ReplayFrameWriter,
                    frame: &[u8],
                    phase: &str|
     -> Result<(), String> {
        write_video_frame(writer, &cancellation, frame)?;
        completed_frames += 1;
        if completed_frames == total_frames || completed_frames.is_multiple_of(3) {
            let encode_fraction = completed_frames as f32 / total_frames.max(1) as f32;
            send_progress(
                &sender,
                &repaint,
                RENDER_PROGRESS_WEIGHT + (1.0 - RENDER_PROGRESS_WEIGHT) * encode_fraction,
                phase,
                completed_frames,
                total_frames,
            );
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

    // The final hold always uses a separately rendered full snapshot. This makes
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
        return Err("edit replay cancelled".to_owned());
    }
    send_progress(
        &sender,
        &repaint,
        EXPORT_MAX_INCOMPLETE_FRACTION,
        "Finalizing MP4…",
        total_frames,
        total_frames,
    );
    writer.finish()?;
    if cancellation.load(Ordering::Acquire) {
        return Err("edit replay cancelled".to_owned());
    }
    crate::file_ops::replace_file(temporary.as_ref(), &destination)
        .map_err(|error| format!("Could not publish replay {}: {error}", destination.display()))?;
    if let Some(parent) = destination.parent().filter(|path| !path.as_os_str().is_empty()) {
        let _ = crate::file_ops::sync_parent_directory(parent);
    }
    let _ = temporary.keep();
    Ok(destination)
}

impl CalibRawApp {
    pub(crate) fn edit_replay_progress_state(&self) -> Option<(f32, String)> {
        self.export.task.as_ref().and_then(|task| {
            (task.kind == ExportTaskKind::Replay)
                .then(|| (task.progress.clamp(0.0, 1.0), task.phase.clone()))
        })
    }

    pub(crate) fn create_edit_replay(&mut self, frame: &eframe::Frame) {
        if !self.can_export() || self.inpaint.processing() {
            return;
        }
        let Some(stem) = self.templated_export_stem() else {
            return;
        };
        let default_name = format!("{stem}-edit-replay.mp4");
        let initial_directory = self
            .develop
            .current_path
            .as_deref()
            .and_then(|path| path.parent());
        let Some(destination) =
            crate::ui::choose_edit_replay_file_path(&default_name, initial_directory)
        else {
            return;
        };
        let Some(render_state) = frame.wgpu_render_state() else {
            self.ui.notice = Some("eframe is not running with the wgpu backend.".to_owned());
            return;
        };
        let Some(raw) = self.develop.loaded_raw.as_ref().map(Arc::clone) else {
            return;
        };
        let snapshot = EditReplaySnapshot {
            device: render_state.device.clone(),
            queue: render_state.queue.clone(),
            raw,
            original_exposure: self.preview.original_exposure,
            final_exposure: self.develop.exposure,
            final_geometry: self.develop.geometry,
            final_masks: self.masks.stack.clone(),
            final_remove: self.inpaint.edits.as_ref().clone(),
            gpu_export_prewarm: self.export.gpu_prewarm.as_ref().map(Arc::clone),
        };
        let cancellation = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::channel();
        let repaint = self.egui_ctx.clone();
        let worker_cancellation = Arc::clone(&cancellation);
        match std::thread::Builder::new()
            .name("calibraw-edit-replay".to_owned())
            .spawn(move || {
                let result = run_edit_replay_worker(
                    snapshot,
                    destination,
                    worker_cancellation,
                    sender.clone(),
                    repaint.clone(),
                );
                let _ = sender.send(ReplayExportEvent::Finished(result));
                repaint.request_repaint();
            })
        {
            Ok(_) => {
                self.export.task = Some(ExportTask::new(
                    ExportTaskKind::Replay,
                    cancellation,
                    Some(ExportTaskReceiver::Replay(receiver)),
                    None,
                    1,
                ));
                if let Some(task) = self.export.task.as_mut() {
                    task.phase = "Preparing edit replay…".to_owned();
                }
                self.ui.notice = None;
                self.egui_ctx.request_repaint();
            }
            Err(error) => {
                self.ui.notice = Some(format!("Could not start edit replay export: {error}"));
            }
        }
    }

    pub(super) fn poll_edit_replay_worker(&mut self) {
        let (events, disconnected) = match self
            .export
            .task
            .as_ref()
            .and_then(|task| task.receiver.as_ref())
        {
            Some(ExportTaskReceiver::Replay(receiver)) => {
                drain_worker_events(Some(receiver), |event| {
                    matches!(event, ReplayExportEvent::Finished(_))
                })
            }
            _ => return,
        };

        let mut finished = false;
        for event in events {
            match event {
                ReplayExportEvent::Progress {
                    progress,
                    phase,
                    completed_frames,
                    total_frames,
                } => {
                    if let Some(task) = self.export.task.as_mut() {
                        task.progress = progress;
                        task.phase = phase;
                        task.completed_tiles = completed_frames;
                        task.total_tiles = total_frames;
                    }
                }
                ReplayExportEvent::Finished(result) => {
                    finished = true;
                    let was_cancelled = self
                        .export
                        .task
                        .as_ref()
                        .is_some_and(|task| task.cancelling);
                    match result {
                        Ok(path) => {
                            self.ui.notice = Some(format!("Created edit replay {}", path.display()));
                        }
                        Err(error) if was_cancelled || error.contains("cancelled") => {
                            self.ui.notice = Some("Edit replay cancelled.".to_owned());
                        }
                        Err(error) => {
                            self.ui.notice = Some(format!("Edit replay failed: {error}"));
                            log::error!("edit replay failed: {error}");
                        }
                    }
                    super::export::clear_export_task(&mut self.export.task);
                }
            }
        }
        if disconnected && !finished {
            self.ui.notice = Some("Edit replay worker stopped unexpectedly.".to_owned());
            super::export::clear_export_task(&mut self.export.task);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_usage_helpers_split_geometry_categories() {
        let mut geometry = GeometryTransform::default();
        assert!(!crop_used(geometry));
        assert!(!rotate_used(geometry));
        assert!(!transform_used(geometry));

        geometry.crop = [0.1, 0.2, 0.9, 0.8];
        assert!(crop_used(geometry));
        assert!(!rotate_used(geometry));
        assert!(!transform_used(geometry));

        geometry.quarter_turns = 1;
        assert!(rotate_used(geometry));
        geometry.horizontal_transform = 4.0;
        assert!(transform_used(geometry));
    }

    #[test]
    fn stage_plan_is_ordered_and_skips_unused_categories() {
        let original_exposure = ExposureParams::scene_referred_default();
        let mut final_exposure = original_exposure;
        final_exposure.exposure = 0.75;
        let mut geometry = GeometryTransform::default();
        geometry.crop = [0.1, 0.1, 0.9, 0.9];
        geometry.quarter_turns = 1;
        geometry.vertical_transform = 3.0;
        let mut masks = MaskStack::default();
        let mut mask = crate::pipeline::LocalMask::new(MaskKind::Fullscreen, 1);
        mask.adjustments.exposure = 0.5;
        masks.masks.push(mask);
        let mut remove = RemoveEditState::default();
        let mut remove_stroke = crate::pipeline::RemoveStroke::default();
        remove_stroke
            .brush
            .points
            .push(crate::pipeline::RemoveBrushPoint {
                x: 0.5,
                y: 0.5,
                radius: 12.0,
            });
        remove.strokes.push(remove_stroke);

        let stages = replay_stage_plan(
            original_exposure,
            final_exposure,
            geometry,
            &masks,
            &remove,
        );
        let kinds = stages.iter().map(|stage| stage.kind).collect::<Vec<_>>();
        assert_eq!(
            kinds,
            vec![
                ReplayStageKind::Edit,
                ReplayStageKind::Crop,
                ReplayStageKind::Rotate,
                ReplayStageKind::Transform,
                ReplayStageKind::Masks,
                ReplayStageKind::Remove,
            ]
        );

        let stages = replay_stage_plan(
            original_exposure,
            original_exposure,
            GeometryTransform::default(),
            &MaskStack::default(),
            &RemoveEditState::default(),
        );
        assert!(stages.is_empty());
    }

    #[test]
    fn unused_masks_do_not_create_a_replay_stage() {
        let original_exposure = ExposureParams::scene_referred_default();
        let mut masks = MaskStack::default();
        masks
            .masks
            .push(crate::pipeline::LocalMask::new(MaskKind::Brush, 1));
        assert!(!masks_used(&masks));

        let mut active = crate::pipeline::LocalMask::new(MaskKind::Fullscreen, 1);
        active.adjustments.exposure = 0.25;
        masks.masks.push(active);
        assert!(masks_used(&masks));

        let stages = replay_stage_plan(
            original_exposure,
            original_exposure,
            GeometryTransform::default(),
            &masks,
            &RemoveEditState::default(),
        );
        assert_eq!(stages.len(), 1);
        assert_eq!(stages[0].kind, ReplayStageKind::Masks);
    }

    #[test]
    fn unused_remove_entries_do_not_create_a_replay_stage() {
        let mut remove = RemoveEditState::default();
        remove.strokes.push(crate::pipeline::RemoveStroke::default());
        assert!(!remove_used(&remove));

        remove.strokes[0]
            .brush
            .points
            .push(crate::pipeline::RemoveBrushPoint {
                x: 0.5,
                y: 0.5,
                radius: 8.0,
            });
        assert!(remove_used(&remove));
    }

    #[test]
    fn replay_dimensions_are_even_for_h264_420() {
        assert_eq!(even_dimension(1920), 1920);
        assert_eq!(even_dimension(1279), 1278);
        assert_eq!(even_dimension(1), 2);
    }

    #[test]
    fn replay_canvas_targets_1080p_in_both_orientations() {
        assert_eq!(replay_canvas_dimensions(6000, 4000), (1620, 1080));
        assert_eq!(replay_canvas_dimensions(4000, 6000), (1080, 1620));
        assert_eq!(replay_canvas_dimensions(4000, 3000), (1440, 1080));
        assert_eq!(replay_canvas_dimensions(3000, 4000), (1080, 1440));
    }

    #[test]
    fn smootherstep_has_stable_endpoints() {
        assert_eq!(smootherstep(0.0), 0.0);
        assert_eq!(smootherstep(1.0), 1.0);
        assert_eq!(smootherstep(-1.0), 0.0);
        assert_eq!(smootherstep(2.0), 1.0);
    }

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

    #[test]
    fn bitmap_font_covers_every_stage_title() {
        for title in ["EDIT", "CROP", "ROTATE", "TRANSFORM", "MASKS", "REMOVE"] {
            for character in title.chars() {
                assert_ne!(glyph_rows(character), [0; 7], "missing glyph {character}");
            }
        }
    }

    #[test]
    fn bitmap_font_covers_brand_outro_copy() {
        for text in [BRAND_TITLE, BRAND_SUBTITLE] {
            for character in text.chars().filter(|character| *character != ' ') {
                assert_ne!(glyph_rows(character), [0; 7], "missing glyph {character}");
            }
        }
    }

    #[test]
    fn replay_includes_longer_holds_and_brand_outro() {
        assert!(SPLIT_HOLD_FRAMES >= REPLAY_FPS);
        assert!(FULL_FRAME_HOLD_FRAMES >= REPLAY_FPS);
        assert!(STAGE_HOLD_FRAMES >= REPLAY_FPS);
        assert!(FINAL_HOLD_FRAMES >= REPLAY_FPS);
        assert!(OUTRO_HOLD_FRAMES >= REPLAY_FPS * 2);
        assert_eq!(
            total_replay_frames(0),
            SPLIT_HOLD_FRAMES
                + SPLIT_SWEEP_FRAMES * 2
                + FULL_FRAME_HOLD_FRAMES * 2
                + FINAL_HOLD_FRAMES
                + OUTRO_FADE_TO_BLACK_FRAMES
                + OUTRO_BRAND_FADE_FRAMES
                + OUTRO_HOLD_FRAMES
        );
    }
}
