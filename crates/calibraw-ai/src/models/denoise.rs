//! RawNIND denoising of Bayer and X-Trans RAWs, and the result saved next to
//! the RAW.

use crate::jobs::{spawn_job, AiJobEvent, JobContext, JobKind};
use crate::placement::FallbackSession;
use crate::registry::artifact::install_artifact_from_reader;
use crate::registry::{AiModel, InputShape, ModelSource, ModelSpec, RAWNIND_PACKAGE};
use crate::runtime::initialize_runtime;
use crate::slot::acquire_model_session;
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
};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipArchive, ZipWriter};

use crate::pipeline::{
    reflect_index, AiDenoisedImage, CfaKind, CompactPixelMap, ExposureParams, GpuParams, LoadedRaw,
    MaskStack, PipelineOptions, ProcessingQuality, RawGpuPipeline,
};

mod inference;
mod result_cache;
mod tiling;
use inference::*;
pub use result_cache::*;
use tiling::*;

/// Download size of the package holding both RawNIND models.
pub const PACKAGE_BYTES: u64 = RAWNIND_PACKAGE.artifact.bytes;
/// The folder in the model cache the RawNIND models are extracted to.
const MODEL_DIR: &str = "rawdenoise-nind-1.0";
const TILE_EDGE: usize = match AiModel::RawNindBayer.spec().input {
    InputShape::Tiled { tile, .. } => tile as usize,
    InputShape::Square { .. } | InputShape::Prompted { .. } | InputShape::Stretched { .. } => {
        panic!("RawNIND runs on tiles")
    }
};
const OVERLAP: usize = match AiModel::RawNindBayer.spec().input {
    InputShape::Tiled { overlap, .. } => overlap as usize,
    InputShape::Square { .. } | InputShape::Prompted { .. } | InputShape::Stretched { .. } => {
        panic!("RawNIND runs on tiles")
    }
};
const CORE_EDGE: usize = TILE_EDGE - 2 * OVERLAP;
const MAX_MODEL_ABS: f32 = 60_000.0;
const RESULT_FILE_MAGIC: [u8; 8] = *b"CALIBRAW";
const RESULT_FILE_VERSION: u32 = 2;
const RESULT_FILE_MANIFEST: &str = "manifest.bin";
const RESULT_FILE_PAYLOAD: &str = "denoised-pixels.bin";
const RESULT_FILE_HEADER_BYTES: usize = 96;
const RESULT_FILE_IO_CHUNK: usize = 1024 * 1024;

#[derive(Debug)]
pub enum AiDenoiseProgress {
    Phase {
        phase: &'static str,
        completed: usize,
        total: usize,
    },
    /// A [`RawNindJob::Restore`] found no usable result; the model was not run.
    SavedResultUnusable,
    /// The new result could not be written next to the RAW, so it only lasts
    /// for this session.
    ResultNotSaved(String),
}

pub type AiDenoiseEvent = AiJobEvent<AiDenoisedImage, AiDenoiseProgress>;

const DENOISE_JOB: JobKind = JobKind {
    thread: "calibraw-rawnind-denoise",
    worker: "RawNIND worker",
    panicked: "ONNX Runtime terminated RawNIND denoise",
    cancelled: "AI denoise cancelled",
};

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

/// The folder in the configured model cache that holds the RawNIND models.
pub fn model_cache_dir() -> PathBuf {
    crate::config::model_root().join(MODEL_DIR)
}

pub fn spawn_rawnind_denoise(
    model_dir: PathBuf,
    raw: Arc<LoadedRaw>,
    device: Option<wgpu::Device>,
    queue: Option<wgpu::Queue>,
    job_kind: RawNindJob,
    cancellation: Arc<AtomicBool>,
) -> mpsc::Receiver<AiDenoiseEvent> {
    spawn_job(&DENOISE_JOB, cancellation, move |job| {
        job.ensure_not_cancelled()?;
        let (save_to, allow_model_download) = match job_kind {
            RawNindJob::Restore { path } => {
                return restore_saved_result(&path, &raw, job);
            }
            RawNindJob::Infer {
                save_to,
                allow_model_download,
            } => (save_to, allow_model_download),
        };
        job.progress(AiDenoiseProgress::Phase {
            phase: "Checking RawNIND models",
            completed: 0,
            total: 0,
        });
        anyhow::ensure!(
            allow_model_download || models_are_verified(&model_dir),
            "RawNIND models are not installed; enable AI denoise again to authorize the download"
        );
        ensure_models(&model_dir, job)?;
        job.ensure_not_cancelled()?;
        job.progress(AiDenoiseProgress::Phase {
            phase: "Starting AI runtime",
            completed: 0,
            total: 0,
        });
        initialize_runtime()?;
        let progress = |progress| job.progress(progress);
        let image = match raw.cfa_kind {
            CfaKind::Bayer => infer_bayer(
                &model_path(&model_dir, AiModel::RawNindBayer),
                &raw,
                &progress,
                job.cancellation(),
            ),
            CfaKind::XTrans => infer_linear(
                &model_path(&model_dir, AiModel::RawNindLinear),
                &raw,
                device
                    .as_ref()
                    .context("X-Trans AI denoise requires CalibRaw's wgpu device")?,
                queue
                    .as_ref()
                    .context("X-Trans AI denoise requires CalibRaw's wgpu queue")?,
                &progress,
                job.cancellation(),
            ),
        }?;
        if let Some(path) = save_to.as_deref() {
            job.ensure_not_cancelled()?;
            job.progress(AiDenoiseProgress::Phase {
                phase: "Saving AI denoise result",
                completed: 0,
                total: 0,
            });
            if let Err(error) = save_result(path, &raw, &image, job.cancellation()) {
                job.ensure_not_cancelled()?;
                log::warn!(
                    "could not persist AI-denoise result {}: {error:#}",
                    path.display()
                );
                job.progress(AiDenoiseProgress::ResultNotSaved(format!("{error:#}")));
                calibraw_core::diagnostics::record(format!(
                    "AI-denoise result write failed for {}: {error:#}",
                    path.display()
                ));
            }
        }
        job.ensure_not_cancelled()?;
        Ok(image)
    })
}

/// Where `model`'s file is extracted in `model_dir`.
fn model_path(model_dir: &Path, model: AiModel) -> PathBuf {
    model_dir.join(model.spec().artifact.cache_filename)
}

fn restore_saved_result(
    path: &Path,
    raw: &LoadedRaw,
    job: &JobContext<AiDenoisedImage, AiDenoiseProgress>,
) -> Result<AiDenoisedImage> {
    job.progress(AiDenoiseProgress::Phase {
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
    job.progress(AiDenoiseProgress::SavedResultUnusable);
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
    job: &JobContext<AiDenoisedImage, AiDenoiseProgress>,
) -> Result<()> {
    let models = [AiModel::RawNindBayer.spec(), AiModel::RawNindLinear.spec()];
    if models
        .iter()
        .all(|spec| spec.is_installed(&model_path(model_dir, spec.id)))
    {
        return Ok(());
    }
    fs::create_dir_all(model_dir)
        .with_context(|| format!("create RawNIND model cache {}", model_dir.display()))?;

    let package = model_dir.join(RAWNIND_PACKAGE.artifact.cache_filename);
    job.install_package(&RAWNIND_PACKAGE, &package)?;
    job.ensure_not_cancelled()?;
    job.progress(AiDenoiseProgress::Phase {
        phase: "Extracting RawNIND models",
        completed: 0,
        total: 0,
    });
    for spec in models {
        extract_model(&package, spec, &model_path(model_dir, spec.id))?;
    }
    if let Err(error) = fs::remove_file(&package) {
        log::warn!(
            "could not remove verified RawNIND package {} after extraction: {error}",
            package.display()
        );
    }
    Ok(())
}

fn extract_model(package: &Path, spec: &ModelSpec, destination: &Path) -> Result<()> {
    if spec.is_installed(destination) {
        return Ok(());
    }
    let ModelSource::Package { member } = spec.source else {
        anyhow::bail!("{} is not part of the RawNIND package", spec.artifact.name);
    };
    let file = File::open(package).with_context(|| format!("open {}", package.display()))?;
    let mut archive = zip::ZipArchive::new(file).context("read RawNIND dtmodel ZIP")?;
    let mut source = archive
        .by_name(member)
        .with_context(|| format!("find {member} in RawNIND package"))?;
    let expected_bytes = spec.artifact.bytes;
    anyhow::ensure!(
        source.size() == expected_bytes,
        "{member} declares {} bytes, expected {expected_bytes}",
        source.size()
    );
    install_artifact_from_reader(destination, spec.artifact, &mut source, || Ok(()))
        .with_context(|| format!("extract {member} from RawNIND package"))
}
