use crate::execution_provider::{FallbackSession, SessionOptions};
use crate::model_artifact::{
    ensure_artifact, install_artifact_from_reader, verify_artifact, DownloadOptions, ModelArtifact,
};
use crate::model_runtime::{acquire_model_session, AiModel, ModelRetention};
use anyhow::{Context, Result};
use calibraw_core::color_math::LINEAR_SRGB_TO_REC2020;
use calibraw_core::file_ops::write_atomically;
use calibraw_core::matrix::{self, Matrix3};
use calibraw_gpu::wgpu;
use ort::value::Tensor;
use ring::digest::{Context as Sha256Context, SHA256};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::Duration,
};
use zip::{write::FileOptions, CompressionMethod, ZipArchive, ZipWriter};

use crate::pipeline::{
    AiDenoisedImage, CfaKind, CompactPixelMap, ExposureParams, GpuParams, LoadedRaw, MaskStack,
    PipelineOptions, ProcessingQuality, RawGpuPipeline,
};

mod inference;
mod result_cache;
mod tiling;
use inference::*;
pub use result_cache::*;
use tiling::*;

pub const RAWNIND_PACKAGE_URL: &str =
    "https://huggingface.co/Duecki/CalibRaw-Artifacts/resolve/91085ce0ec322a4a7cbd20059688690218e52f9a/models/rawnind/rawdenoise-nind.dtmodel";
pub const RAWNIND_PACKAGE_BYTES: u64 = 57_700_134;
pub const RAWNIND_PACKAGE_SHA256: &str =
    "d71b5f1e727c85a359e6f74dca9e2016c9d8fc3e2f7ac3e9b347d80ceca969af";
const BAYER_MODEL_BYTES: u64 = 31_056_425;
const BAYER_MODEL_SHA256: &str = "da27509dab6a2915da67e988acd86cf71f9d5bbc8d1aa0ed32933578a887b901";
const LINEAR_MODEL_BYTES: u64 = 31_053_823;
const LINEAR_MODEL_SHA256: &str =
    "df957efadcc152c007d5d3b0917bdff9e41c0d4a0efe56584ef30b36393cd181";

const RAWNIND_PACKAGE_ARTIFACT: ModelArtifact = ModelArtifact {
    name: "RawNIND model package",
    url: Some(RAWNIND_PACKAGE_URL),
    sha256: RAWNIND_PACKAGE_SHA256,
    bytes: RAWNIND_PACKAGE_BYTES,
};
const BAYER_MODEL_ARTIFACT: ModelArtifact = ModelArtifact {
    name: "RawNIND Bayer model",
    url: None,
    sha256: BAYER_MODEL_SHA256,
    bytes: BAYER_MODEL_BYTES,
};
const LINEAR_MODEL_ARTIFACT: ModelArtifact = ModelArtifact {
    name: "RawNIND linear model",
    url: None,
    sha256: LINEAR_MODEL_SHA256,
    bytes: LINEAR_MODEL_BYTES,
};
const RAWNIND_DOWNLOAD: DownloadOptions = DownloadOptions {
    connect_timeout: Duration::from_secs(45),
    response_timeout: Duration::from_secs(60),
    body_timeout: Duration::from_secs(30 * 60),
    attempts: 5,
    resume: true,
};
const TILE_EDGE: usize = 512;
const OVERLAP: usize = 64;
const CORE_EDGE: usize = TILE_EDGE - 2 * OVERLAP;
const MAX_MODEL_ABS: f32 = 60_000.0;
const RESULT_FILE_MAGIC: [u8; 8] = *b"CALIBRAW";
const RESULT_FILE_VERSION: u32 = 2;
const RESULT_FILE_MANIFEST: &str = "manifest.bin";
const RESULT_FILE_PAYLOAD: &str = "denoised-pixels.bin";
const RESULT_FILE_HEADER_BYTES: usize = 96;
const RESULT_FILE_IO_CHUNK: usize = 1024 * 1024;

#[derive(Debug)]
pub enum AiDenoiseEvent {
    DownloadProgress {
        downloaded: u64,
        total: u64,
    },
    Progress {
        phase: &'static str,
        completed: usize,
        total: usize,
    },
    /// A [`RawNindJob::Restore`] found no usable result; the model was not run.
    SavedResultUnusable,
    /// The new result could not be written next to the RAW, so it only lasts
    /// for this session.
    ResultNotSaved(String),
    Finished(Result<AiDenoisedImage, String>),
}

/// What a RawNIND worker does.
#[derive(Clone, Debug)]
pub enum RawNindJob {
    /// Loads the result saved at `path`. Never runs the model, so restoring
    /// needs no models, runtime or user consent.
    Restore { path: PathBuf },
    /// Runs the model and saves the result to `save_to` when given.
    Infer {
        save_to: Option<PathBuf>,
        allow_model_download: bool,
    },
}

#[cfg(not(target_os = "android"))]
pub fn model_cache_dir() -> PathBuf {
    crate::desktop_model_cache_root().join("rawdenoise-nind-1.0")
}

#[allow(clippy::too_many_arguments)]
pub fn spawn_rawnind_denoise(
    model_dir: PathBuf,
    runtime_path: Option<PathBuf>,
    runtime_sha256: Option<String>,
    raw: Arc<LoadedRaw>,
    device: Option<wgpu::Device>,
    queue: Option<wgpu::Queue>,
    job: RawNindJob,
    cancellation: Arc<AtomicBool>,
) -> mpsc::Receiver<AiDenoiseEvent> {
    let (sender, receiver) = mpsc::channel();
    let worker_sender = sender.clone();
    let spawn = std::thread::Builder::new()
        .name("calibraw-rawnind-denoise".to_owned())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                (|| {
                    ensure_not_cancelled(&cancellation)?;
                    let (save_to, allow_model_download) = match job {
                        RawNindJob::Restore { path } => {
                            return restore_saved_result(&path, &raw, &worker_sender);
                        }
                        RawNindJob::Infer {
                            save_to,
                            allow_model_download,
                        } => (save_to, allow_model_download),
                    };
                    let _ = worker_sender.send(AiDenoiseEvent::Progress {
                        phase: "Checking RawNIND models",
                        completed: 0,
                        total: 0,
                    });
                    anyhow::ensure!(
                        allow_model_download || models_are_verified(&model_dir),
                        "RawNIND models are not installed; enable AI denoise again to authorize the download"
                    );
                    ensure_models(&model_dir, &worker_sender, &cancellation)?;
                    ensure_not_cancelled(&cancellation)?;
                    let _ = worker_sender.send(AiDenoiseEvent::Progress {
                        phase: "Starting AI runtime",
                        completed: 0,
                        total: 0,
                    });
                    crate::ai_masks::initialize_runtime(
                        runtime_path.as_deref(),
                        runtime_sha256.as_deref(),
                    )?;
                    let image = match raw.cfa_kind {
                        CfaKind::Bayer => infer_bayer(
                            &model_dir.join("model_bayer.onnx"),
                            &raw,
                            &worker_sender,
                            &cancellation,
                        ),
                        CfaKind::XTrans => infer_linear(
                            &model_dir.join("model_linear.onnx"),
                            &raw,
                            device
                                .as_ref()
                                .context("X-Trans AI denoise requires CalibRaw's wgpu device")?,
                            queue
                                .as_ref()
                                .context("X-Trans AI denoise requires CalibRaw's wgpu queue")?,
                            &worker_sender,
                            &cancellation,
                        ),
                    }?;
                    if let Some(path) = save_to.as_deref() {
                        ensure_not_cancelled(&cancellation)?;
                        let _ = worker_sender.send(AiDenoiseEvent::Progress {
                            phase: "Saving AI denoise result",
                            completed: 0,
                            total: 0,
                        });
                        if let Err(error) = save_result(path, &raw, &image, &cancellation) {
                            ensure_not_cancelled(&cancellation)?;
                            log::warn!(
                                "could not persist AI-denoise result {}: {error:#}",
                                path.display()
                            );
                            let _ = worker_sender
                                .send(AiDenoiseEvent::ResultNotSaved(format!("{error:#}")));
                            calibraw_core::diagnostics::record(format!(
                                "AI-denoise result write failed for {}: {error:#}",
                                path.display()
                            ));
                        }
                    }
                    ensure_not_cancelled(&cancellation)?;
                    Ok(image)
                })()
            }))
            .unwrap_or_else(|panic| {
                let message = panic
                    .downcast_ref::<&str>()
                    .copied()
                    .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
                    .unwrap_or("unknown ONNX Runtime failure");
                Err(anyhow::anyhow!(
                    "ONNX Runtime terminated RawNIND denoise: {message}"
                ))
            });
            let _ = worker_sender.send(AiDenoiseEvent::Finished(
                result.map_err(|error| format!("{error:#}")),
            ));
        });
    if let Err(error) = spawn {
        let _ = sender.send(AiDenoiseEvent::Finished(Err(format!(
            "could not start RawNIND worker: {error}"
        ))));
    }
    receiver
}

fn restore_saved_result(
    path: &Path,
    raw: &LoadedRaw,
    events: &mpsc::Sender<AiDenoiseEvent>,
) -> Result<AiDenoisedImage> {
    let _ = events.send(AiDenoiseEvent::Progress {
        phase: "Restoring saved AI denoise",
        completed: 0,
        total: 0,
    });
    let error = match load_saved_result(path, raw) {
        Ok(Some(image)) => {
            calibraw_core::diagnostics::record(format!(
                "AI-denoise worker restored {} without model inference",
                path.display()
            ));
            return Ok(image);
        }
        Ok(None) => anyhow::anyhow!("no saved AI-denoise result at {}", path.display()),
        Err(error) => {
            log::warn!(
                "discarding invalid AI-denoise result {}: {error:#}",
                path.display()
            );
            if let Err(remove_error) = fs::remove_file(path) {
                if remove_error.kind() != std::io::ErrorKind::NotFound {
                    log::warn!(
                        "could not remove invalid AI-denoise result {}: {remove_error}",
                        path.display()
                    );
                }
            }
            error
        }
    };
    calibraw_core::diagnostics::record(format!(
        "AI-denoise worker could not restore a saved result: {error:#}"
    ));
    let _ = events.send(AiDenoiseEvent::SavedResultUnusable);
    Err(error)
}

fn ensure_not_cancelled(cancellation: &AtomicBool) -> Result<()> {
    anyhow::ensure!(
        !cancellation.load(Ordering::Acquire),
        "AI denoise cancelled"
    );
    Ok(())
}

fn ensure_models(
    model_dir: &Path,
    events: &mpsc::Sender<AiDenoiseEvent>,
    cancellation: &AtomicBool,
) -> Result<()> {
    let bayer = model_dir.join("model_bayer.onnx");
    let linear = model_dir.join("model_linear.onnx");
    if verify_artifact(&bayer, BAYER_MODEL_ARTIFACT).is_ok()
        && verify_artifact(&linear, LINEAR_MODEL_ARTIFACT).is_ok()
    {
        return Ok(());
    }
    fs::create_dir_all(model_dir)
        .with_context(|| format!("create RawNIND model cache {}", model_dir.display()))?;

    let package = model_dir.join("rawdenoise-nind.dtmodel");
    ensure_artifact(
        &package,
        RAWNIND_PACKAGE_ARTIFACT,
        RAWNIND_DOWNLOAD,
        |downloaded, total| {
            let _ = events.send(AiDenoiseEvent::DownloadProgress { downloaded, total });
        },
        || ensure_not_cancelled(cancellation),
    )?;
    ensure_not_cancelled(cancellation)?;
    extract_model(
        &package,
        "rawdenoise-nind/model_bayer.onnx",
        &bayer,
        BAYER_MODEL_ARTIFACT,
    )?;
    extract_model(
        &package,
        "rawdenoise-nind/model_linear.onnx",
        &linear,
        LINEAR_MODEL_ARTIFACT,
    )?;
    if let Err(error) = fs::remove_file(&package) {
        log::warn!(
            "could not remove verified RawNIND package {} after extraction: {error}",
            package.display()
        );
    }
    Ok(())
}

fn extract_model(
    package: &Path,
    member: &str,
    destination: &Path,
    artifact: ModelArtifact,
) -> Result<()> {
    if verify_artifact(destination, artifact).is_ok() {
        return Ok(());
    }
    let file = File::open(package).with_context(|| format!("open {}", package.display()))?;
    let mut archive = zip::ZipArchive::new(file).context("read RawNIND dtmodel ZIP")?;
    let mut source = archive
        .by_name(member)
        .with_context(|| format!("find {member} in RawNIND package"))?;
    let expected_bytes = artifact.bytes;
    anyhow::ensure!(
        source.size() == expected_bytes,
        "{member} declares {} bytes, expected {expected_bytes}",
        source.size()
    );
    install_artifact_from_reader(destination, artifact, &mut source, || Ok(()))
        .with_context(|| format!("extract {member} from RawNIND package"))
}

#[cfg(test)]
mod tests {
    use super::{
        bayer_rggb_origin, load_saved_result, match_gain_tile, reflect_index, run_model_tile,
        save_result, seam_weight, spawn_rawnind_denoise, AiDenoiseEvent, RawNindJob, CORE_EDGE,
        TILE_EDGE,
    };

    use crate::execution_provider::SessionOptions;
    use crate::model_runtime::{acquire_model_session, AiModel, ModelRetention};
    use crate::pipeline::{
        build_proxy, AiDenoisedImage, CameraProfile, CfaKind, CompactPixelMap, ExposureParams,
        GpuParams, LoadedRaw, MaskStack, NoiseProfile, PipelineOptions, ProcessingQuality,
        ProxySpec, RawGpuPipeline,
    };

    fn result_test_raw(width: u32, height: u32) -> LoadedRaw {
        let pixels = (width * height) as usize;
        LoadedRaw {
            width,
            height,
            camera_make: "Cache Test".to_owned(),
            camera_model: "Synthetic".to_owned(),
            lens_make: String::new(),
            lens_model: String::new(),
            focal_length: 0.0,
            aperture: 0.0,
            focus_distance: 0.0,
            capture_metadata: Default::default(),
            cfa_kind: CfaKind::Bayer,
            raw_pixels: (0..pixels).map(|index| index as u16 * 17).collect(),
            scene_linear_raster: None,
            color_indices: CompactPixelMap::repeating(width, height, 2, 2, vec![0, 1, 3, 2]),
            wb_coeffs: [2.0, 1.0, 1.5, 1.0],
            cam_to_srgb: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
            ],
            black_levels: [64.0; 4],
            black_levels_per_pixel: CompactPixelMap::repeating(width, height, 1, 1, vec![64.0]),
            white_levels: [16_383.0; 4],
            noise_profile: NoiseProfile::default(),
            camera_profile: CameraProfile::default(),
            camera_profile_source: None,
            available_camera_profiles: Vec::new(),
            white_balance_model: None,
            lens_geometry: None,
            ai_denoised: std::sync::Arc::new(std::sync::RwLock::new(None)),
            opposed_chroma_cache: Default::default(),
            opposed_chroma_source_identity: Default::default(),
            opposed_chroma_reference_source: true,
        }
    }

    fn adjacent_cfa_difference(image: &AiDenoisedImage, a: (u32, u32), b: (u32, u32)) -> f64 {
        let cfa = image.bayer_cfa().expect("Bayer integration result");
        let a = (a.1 * image.width + a.0) as usize;
        let b = (b.1 * image.width + b.0) as usize;
        f64::from(cfa[a].abs_diff(cfa[b]))
    }

    fn bayer_seam_gradient_ratio(raw: &LoadedRaw, image: &AiDenoisedImage) -> f64 {
        let (origin_x, origin_y) = bayer_rggb_origin(raw).unwrap();
        let packed_width = (raw.width - origin_x) / 2;
        let packed_height = (raw.height - origin_y) / 2;
        let mut seam = 0.0;
        let mut nearby = 0.0;
        let mut samples = 0u64;
        for packed_x in (CORE_EDGE as u32..packed_width).step_by(CORE_EDGE) {
            let x = origin_x + packed_x * 2;
            if x < 34 || x + 33 >= raw.width {
                continue;
            }
            for y in (origin_y..origin_y + packed_height * 2).step_by(8) {
                seam += adjacent_cfa_difference(image, (x - 2, y), (x, y));
                nearby += adjacent_cfa_difference(image, (x - 34, y), (x - 32, y));
                nearby += adjacent_cfa_difference(image, (x + 30, y), (x + 32, y));
                samples += 2;
            }
        }
        for packed_y in (CORE_EDGE as u32..packed_height).step_by(CORE_EDGE) {
            let y = origin_y + packed_y * 2;
            if y < 34 || y + 33 >= raw.height {
                continue;
            }
            for x in (origin_x..origin_x + packed_width * 2).step_by(8) {
                seam += adjacent_cfa_difference(image, (x, y - 2), (x, y));
                nearby += adjacent_cfa_difference(image, (x, y - 34), (x, y - 32));
                nearby += adjacent_cfa_difference(image, (x, y + 30), (x, y + 32));
                samples += 2;
            }
        }
        let seam_mean = seam / (samples.max(1) as f64 * 0.5);
        let nearby_mean = nearby / samples.max(1) as f64;
        seam_mean / nearby_mean.max(1e-12)
    }

    fn test_wgpu_device() -> (calibraw_gpu::wgpu::Device, calibraw_gpu::wgpu::Queue) {
        let instance = calibraw_gpu::wgpu::Instance::default();
        let adapter = pollster::block_on(
            instance.request_adapter(&calibraw_gpu::wgpu::RequestAdapterOptions::default()),
        )
        .expect("a wgpu adapter is required for RawNIND integration tests");
        pollster::block_on(
            adapter.request_device(&calibraw_gpu::wgpu::DeviceDescriptor {
                label: Some("RawNIND integration test"),
                ..Default::default()
            }),
        )
        .unwrap()
    }

    #[test]
    fn mirror_padding_matches_numpy_reflect_without_repeating_edges() {
        let values = (-5..=8)
            .map(|index| reflect_index(index, 4))
            .collect::<Vec<_>>();
        assert_eq!(values, [1, 2, 3, 2, 1, 0, 1, 2, 3, 2, 1, 0, 1, 2]);
    }

    #[test]
    fn neighboring_overlap_weights_sum_to_one() {
        const OVERLAP: usize = 64;
        const BOUNDARY: usize = 1_000;
        for coordinate in BOUNDARY - OVERLAP..BOUNDARY + OVERLAP {
            let left = seam_weight(coordinate, BOUNDARY - 384, BOUNDARY, OVERLAP, false, true);
            let right = seam_weight(coordinate, BOUNDARY, BOUNDARY + 384, OVERLAP, true, false);
            assert!((left + right - 1.0).abs() < f32::EPSILON);
            assert!(left > 0.0 && right > 0.0);
        }
    }

    #[test]
    fn saved_result_round_trips_and_rejects_changed_source() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path();
        let path = directory.join("synthetic.NEF.calibraw-denoise");
        let mut raw = result_test_raw(4, 4);
        let values = (0..4 * 4).map(|index| index as u16 * 97).collect();
        let expected = AiDenoisedImage::new_bayer_cfa(4, 4, values).unwrap();
        let cancellation = std::sync::atomic::AtomicBool::new(false);

        save_result(&path, &raw, &expected, &cancellation).unwrap();
        let restored = load_saved_result(&path, &raw).unwrap().unwrap();
        assert_eq!(restored.raw_cfa16.as_ref(), expected.raw_cfa16.as_ref());

        raw.raw_pixels[3] ^= 1;
        assert!(load_saved_result(&path, &raw).is_err());
    }

    #[test]
    fn saved_result_worker_finishes_without_models_runtime_or_gpu() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path();
        let path = directory.join("restore-only.NEF.calibraw-denoise");
        let raw = std::sync::Arc::new(result_test_raw(4, 4));
        let values = (0..4 * 4).map(|index| index as u16 * 97).collect();
        let expected = AiDenoisedImage::new_bayer_cfa(4, 4, values).unwrap();
        let cancellation = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        save_result(&path, &raw, &expected, &cancellation).unwrap();

        let receiver = spawn_rawnind_denoise(
            directory.join("models-do-not-exist"),
            None,
            None,
            std::sync::Arc::clone(&raw),
            None,
            None,
            RawNindJob::Restore { path },
            cancellation,
        );
        let restored = receiver
            .into_iter()
            .find_map(|event| match event {
                AiDenoiseEvent::Finished(result) => Some(result.unwrap()),
                _ => None,
            })
            .expect("restore worker must send a terminal event");
        assert_eq!(restored.raw_cfa16.as_ref(), expected.raw_cfa16.as_ref());
    }

    #[test]
    fn unusable_saved_result_never_runs_the_model() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path();
        let path = directory.join("changed.NEF.calibraw-denoise");
        let mut raw = result_test_raw(4, 4);
        let values = (0..4 * 4).map(|index| index as u16 * 97).collect();
        let saved = AiDenoisedImage::new_bayer_cfa(4, 4, values).unwrap();
        let cancellation = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        save_result(&path, &raw, &saved, &cancellation).unwrap();
        raw.raw_pixels[3] ^= 1;

        let receiver = spawn_rawnind_denoise(
            directory.join("models-do-not-exist"),
            None,
            None,
            std::sync::Arc::new(raw),
            None,
            None,
            RawNindJob::Restore { path: path.clone() },
            cancellation,
        );
        let events = receiver.into_iter().collect::<Vec<_>>();
        assert!(events
            .iter()
            .any(|event| matches!(event, AiDenoiseEvent::SavedResultUnusable)));
        assert!(matches!(
            events.last(),
            Some(AiDenoiseEvent::Finished(Err(_)))
        ));
        assert!(!path.exists());
        assert!(!directory.join("models-do-not-exist").exists());
    }

    #[test]
    fn inference_cannot_download_models_without_consent() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path();
        let raw = std::sync::Arc::new(result_test_raw(4, 4));
        let cancellation = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let receiver = spawn_rawnind_denoise(
            directory.join("models-do-not-exist"),
            None,
            None,
            raw,
            None,
            None,
            RawNindJob::Infer {
                save_to: Some(directory.join("missing.NEF.calibraw-denoise")),
                allow_model_download: false,
            },
            cancellation,
        );
        let error = receiver
            .into_iter()
            .find_map(|event| match event {
                AiDenoiseEvent::Finished(Err(error)) => Some(error),
                _ => None,
            })
            .expect("inference must stop before model acquisition");
        assert!(error.contains("authorize the download"));
        assert!(!directory.join("models-do-not-exist").exists());
    }

    #[test]
    fn daylight_white_balance_falls_back_to_as_shot_coefficients() {
        assert_eq!(
            result_test_raw(2, 2).rawnind_daylight_white_balance(),
            [2.0, 1.0, 1.5]
        );
    }

    #[test]
    #[ignore = "requires the published RawNIND Bayer model, ONNX Runtime shared library, and runtime graph contract"]
    fn raw_nind_published_bayer_graph_contract() {
        let model_dir = std::path::PathBuf::from(
            std::env::var_os("CALIBRAW_RAWNIND_MODEL_DIR")
                .expect("set CALIBRAW_RAWNIND_MODEL_DIR to the extracted dtmodel directory"),
        );
        let runtime = std::path::PathBuf::from(
            std::env::var_os("CALIBRAW_ONNX_RUNTIME")
                .expect("set CALIBRAW_ONNX_RUNTIME to libonnxruntime"),
        );
        let sha = crate::ai_masks::sha256_file_hex(&runtime).unwrap();
        crate::ai_masks::initialize_runtime(Some(&runtime), Some(&sha)).unwrap();
        let mut session = acquire_model_session(
            AiModel::RawNindBayer,
            model_dir.join("model_bayer.onnx"),
            SessionOptions::new("RawNIND Bayer"),
            ModelRetention::OneShot,
        )
        .unwrap();
        let mut output =
            run_model_tile(&mut session, 4, vec![0.1; 4 * TILE_EDGE * TILE_EDGE], 1024).unwrap();
        match_gain_tile(&vec![0.1; 4 * TILE_EDGE * TILE_EDGE], &mut output).unwrap();
        assert_eq!(output.len(), 3 * 1024 * 1024);
    }

    #[test]
    #[ignore = "requires the published RawNIND linear model, ONNX Runtime shared library, and runtime graph contract"]
    fn raw_nind_published_linear_graph_contract() {
        let model_dir = std::path::PathBuf::from(
            std::env::var_os("CALIBRAW_RAWNIND_MODEL_DIR")
                .expect("set CALIBRAW_RAWNIND_MODEL_DIR to the extracted dtmodel directory"),
        );
        let runtime = std::path::PathBuf::from(
            std::env::var_os("CALIBRAW_ONNX_RUNTIME")
                .expect("set CALIBRAW_ONNX_RUNTIME to libonnxruntime"),
        );
        let sha = crate::ai_masks::sha256_file_hex(&runtime).unwrap();
        crate::ai_masks::initialize_runtime(Some(&runtime), Some(&sha)).unwrap();
        let mut session = acquire_model_session(
            AiModel::RawNindLinear,
            model_dir.join("model_linear.onnx"),
            SessionOptions::new("RawNIND linear"),
            ModelRetention::OneShot,
        )
        .unwrap();
        let input = vec![0.1; 3 * TILE_EDGE * TILE_EDGE];
        let mut output = run_model_tile(&mut session, 3, input.clone(), TILE_EDGE).unwrap();
        match_gain_tile(&input, &mut output).unwrap();
        assert_eq!(output.len(), 3 * TILE_EDGE * TILE_EDGE);
    }

    #[test]
    #[ignore = "requires RawNIND model/runtime files, a Bayer RAW fixture, and a working wgpu device"]
    fn raw_nind_bayer_end_to_end_fixture() {
        let model_dir = std::path::PathBuf::from(
            std::env::var_os("CALIBRAW_RAWNIND_MODEL_DIR")
                .expect("set CALIBRAW_RAWNIND_MODEL_DIR to the extracted dtmodel directory"),
        );
        let runtime = std::path::PathBuf::from(
            std::env::var_os("CALIBRAW_ONNX_RUNTIME")
                .expect("set CALIBRAW_ONNX_RUNTIME to libonnxruntime"),
        );
        let raw_path = std::path::PathBuf::from(
            std::env::var_os("CALIBRAW_RAWNIND_TEST_RAW")
                .expect("set CALIBRAW_RAWNIND_TEST_RAW to a Bayer RAW fixture"),
        );
        let sha = crate::ai_masks::sha256_file_hex(&runtime).unwrap();
        crate::ai_masks::initialize_runtime(Some(&runtime), Some(&sha)).unwrap();
        let raw = crate::pipeline::load_raw_file(&raw_path).unwrap();
        assert_eq!(raw.cfa_kind, crate::pipeline::CfaKind::Bayer);
        let (events, _receiver) = std::sync::mpsc::channel();
        let cancellation = std::sync::atomic::AtomicBool::new(false);
        let image = super::infer_bayer(
            &model_dir.join("model_bayer.onnx"),
            &raw,
            &events,
            &cancellation,
        )
        .unwrap();
        assert!(image.is_valid_for(raw.width, raw.height));
        assert_eq!(image.raw_cfa16.len(), raw.raw_pixels.len());
        let seam_ratio = bayer_seam_gradient_ratio(&raw, &image);
        eprintln!("RawNIND seam/nearby-gradient ratio: {seam_ratio:.4}");
        assert!(
            seam_ratio < 1.5,
            "RawNIND tile boundaries are stronger than nearby image gradients"
        );

        raw.set_ai_denoised_image(image).unwrap();
        let proxy = build_proxy(&raw, ProxySpec { max_edge: 1600 });
        let (device, queue) = test_wgpu_device();
        let render = |exposure_stops: f32, ai_enabled: bool| {
            let exposure = ExposureParams {
                exposure: exposure_stops,
                ai_denoise_enabled: ai_enabled,
                ..ExposureParams::default()
            };
            raw.inpaint_opposed_chroma_for_exposure(&exposure);
            let params = GpuParams::new(&exposure, &MaskStack::default(), &proxy);
            let pipeline = RawGpuPipeline::new(
                &device,
                &queue,
                &proxy,
                &params,
                PipelineOptions::new(ProcessingQuality::Preview),
            )
            .unwrap();
            pipeline.recompute(&queue, &device, &params);
            pipeline
                .read_output_region_blocking(&device, &queue, 0, 0, proxy.width, proxy.height)
                .unwrap()
        };
        let is_pink = |pixel: &[u8]| {
            let red_blue = pixel[0].min(pixel[2]);
            red_blue > 24 && u16::from(pixel[1]) * 3 < u16::from(red_blue) * 2
        };
        for exposure_stops in [-1.0, -5.0] {
            let normal = render(exposure_stops, false);
            let denoised = render(exposure_stops, true);
            let false_pink_mask = normal
                .chunks_exact(4)
                .zip(denoised.chunks_exact(4))
                .map(|(normal, denoised)| !is_pink(normal) && is_pink(denoised))
                .collect::<Vec<_>>();
            let false_pink = false_pink_mask.iter().filter(|value| **value).count();
            let width = proxy.width as usize;
            let height = proxy.height as usize;
            let mut visited = vec![false; false_pink_mask.len()];
            let mut largest_region = 0usize;
            for seed in 0..false_pink_mask.len() {
                if !false_pink_mask[seed] || visited[seed] {
                    continue;
                }
                visited[seed] = true;
                let mut stack = vec![seed];
                let mut region = 0usize;
                while let Some(index) = stack.pop() {
                    region += 1;
                    let x = index % width;
                    let y = index / width;
                    for neighbor in [
                        (x > 0).then_some(index - 1),
                        (x + 1 < width).then_some(index + 1),
                        (y > 0).then_some(index - width),
                        (y + 1 < height).then_some(index + width),
                    ]
                    .into_iter()
                    .flatten()
                    {
                        if false_pink_mask[neighbor] && !visited[neighbor] {
                            visited[neighbor] = true;
                            stack.push(neighbor);
                        }
                    }
                }
                largest_region = largest_region.max(region);
            }
            eprintln!(
                "RawNIND {exposure_stops} EV newly pink pixels: {false_pink}/{}, largest region {largest_region}",
                normal.len() / 4
            );
            assert!(
                false_pink * 10_000 <= normal.len() / 4,
                "AI denoise creates visible false-pink highlight regions at {exposure_stops} EV"
            );
            assert!(
                largest_region <= 16,
                "AI denoise creates a contiguous false-pink region at {exposure_stops} EV"
            );
        }
    }

    #[test]
    #[ignore = "requires RawNIND model/runtime files, an X-Trans RAW fixture, and a working wgpu device"]
    fn raw_nind_xtrans_end_to_end_fixture() {
        let model_dir = std::path::PathBuf::from(
            std::env::var_os("CALIBRAW_RAWNIND_MODEL_DIR")
                .expect("set CALIBRAW_RAWNIND_MODEL_DIR to the extracted dtmodel directory"),
        );
        let runtime = std::path::PathBuf::from(
            std::env::var_os("CALIBRAW_ONNX_RUNTIME")
                .expect("set CALIBRAW_ONNX_RUNTIME to libonnxruntime"),
        );
        let raw_path = std::path::PathBuf::from(
            std::env::var_os("CALIBRAW_RAWNIND_TEST_RAW")
                .expect("set CALIBRAW_RAWNIND_TEST_RAW to an X-Trans RAW fixture"),
        );
        let sha = crate::ai_masks::sha256_file_hex(&runtime).unwrap();
        crate::ai_masks::initialize_runtime(Some(&runtime), Some(&sha)).unwrap();
        let raw = crate::pipeline::load_raw_file(&raw_path).unwrap();
        assert_eq!(raw.cfa_kind, crate::pipeline::CfaKind::XTrans);
        let (device, queue) = test_wgpu_device();
        let (events, _receiver) = std::sync::mpsc::channel();
        let cancellation = std::sync::atomic::AtomicBool::new(false);
        let image = super::infer_linear(
            &model_dir.join("model_linear.onnx"),
            &raw,
            &device,
            &queue,
            &events,
            &cancellation,
        )
        .unwrap();
        assert!(image.is_valid_for(raw.width, raw.height));
        assert!(image
            .rgb16f
            .iter()
            .all(|value| { half::f16::from_bits(*value).to_f32().is_finite() }));
    }
}
