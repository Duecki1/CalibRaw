use crate::execution_provider::{CpuFallbackProfile, FallbackSession, SessionOptions};
use crate::model_artifact::{DownloadOptions, ModelArtifact};
use crate::model_install::ModelInstallSpec;
use crate::model_runtime::{with_model_session, AiModel, ModelRetention};
use crate::pipeline::MaskImage;
use crate::ModelDownloadProgress;
use anyhow::{Context, Result};
use image::{imageops::FilterType, ImageBuffer, Luma, Rgba};
use ort::value::Tensor;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
#[cfg(not(target_os = "android"))]
use std::fs;
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex, OnceLock,
    },
    time::Duration,
};

fn ensure_ai_not_cancelled(cancellation: &AtomicBool) -> Result<()> {
    anyhow::ensure!(
        !cancellation.load(Ordering::Acquire),
        "background task cancelled"
    );
    Ok(())
}

pub use crate::model_artifact::sha256_file_hex;

mod depth;

/// `birefnet-lowQ.onnx` with its deformable convolutions rewritten by
/// `packaging/models/birefnet_gridsample.py`. The original export peaks near
/// 6.5 GB and overruns GPU memory under DirectML; this one peaks near 2 GB and
/// matches it to float32 rounding.
pub const BIREFNET_MODEL_BYTES: u64 = 223_665_266;
pub const BIREFNET_MODEL_URL: &str = "https://huggingface.co/Duecki/CalibRaw-Artifacts/resolve/main/models/briefnet/birefnet-lowQ-gridsample.onnx";
pub const BIREFNET_MODEL_SHA256_HEX: &str =
    "7c0194fad87cc56371054c14e3cff16ee40b095dcae1183321e6c4b5bb8e21d4";
pub const SKYSEG_MODEL_FILENAME: &str = "skyseg-u2net-fp32.onnx";
/// Metadata for the depth artifact.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DepthModelSpec {
    pub name: &'static str,
    pub download_bytes: u64,
    pub cache_filename: &'static str,
    pub input_edge: u32,
    pub artifact_url: &'static str,
}

pub const DEPTH_MODEL: DepthModelSpec = depth::MODEL_SPEC;
/// The depth model desktop builds downloaded before `DEPTH_MODEL`, in the
/// model cache.
///
/// migration: remove in v1.4.0. Listed in `calibraw_core`'s `migrations`.
pub const RETIRED_DEPTH_MODEL_FILENAME: &str = "da3mono_large_700x700.onnx";
pub const SKYSEG_MODEL_BYTES: u64 = 175_997_079;
const SKYSEG_MODEL_INSTALL: ModelInstallSpec = ModelInstallSpec {
    artifact: ModelArtifact {
        name: "SkySeg U2Net",
        url: Some("https://huggingface.co/Duecki/CalibRaw-Artifacts/resolve/main/models/skyseg/skyseg.onnx"),
        sha256: "ab9c34c64c3d821220a2886a4a06da4642ffa14d5b30e8d5339056a089aa1d39",
        bytes: SKYSEG_MODEL_BYTES,
    },
    download: MASK_MODEL_DOWNLOAD,
    progress_label: "SkySeg U2Net",
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BiRefNetModelSpec {
    pub checkpoint: &'static str,
    pub download_label: &'static str,
    pub url: &'static str,
    pub bytes: u64,
    pub sha256_hex: &'static str,
    pub cache_filename: &'static str,
    pub input_width: u32,
    pub input_height: u32,
    /// Peak GPU memory of one inference: measured with ONNX Runtime's CUDA
    /// provider growing its arena by exact requests, less the CUDA context,
    /// rounded up to 100 MiB. DirectML pools allocations similarly.
    pub gpu_memory_bytes: u64,
}

const MIB: u64 = 1 << 20;

impl BiRefNetModelSpec {
    fn install(self) -> ModelInstallSpec {
        ModelInstallSpec {
            artifact: ModelArtifact {
                name: self.checkpoint,
                url: Some(self.url),
                sha256: self.sha256_hex,
                bytes: self.bytes,
            },
            download: MASK_MODEL_DOWNLOAD,
            progress_label: self.download_label,
        }
    }
}

/// The BiRefNet General-Lite subject model: 1024 x 1024 input, 224 MB. The cache
/// filename predates the rewrite; the changed SHA-256 replaces older downloads.
pub const BIREFNET_MODEL: BiRefNetModelSpec = BiRefNetModelSpec {
    checkpoint: "BiRefNet General-Lite",
    download_label: "BiRefNet General-Lite",
    url: BIREFNET_MODEL_URL,
    bytes: BIREFNET_MODEL_BYTES,
    sha256_hex: BIREFNET_MODEL_SHA256_HEX,
    cache_filename: "birefnet-general-lite.onnx",
    input_width: 1024,
    input_height: 1024,
    gpu_memory_bytes: 2900 * MIB,
};

/// The same checkpoint exported at 768 x 768 by
/// `packaging/models/birefnet_export.py` and rewritten by
/// `birefnet_gridsample.py`, so its operators match `BIREFNET_MODEL`'s. About
/// half the inference cost; boundaries are softer and small details may drop.
pub const BIREFNET_LOW_MODEL: BiRefNetModelSpec = BiRefNetModelSpec {
    checkpoint: "BiRefNet General-Lite 768",
    download_label: "BiRefNet General-Lite 768",
    url: "https://huggingface.co/Duecki/CalibRaw-Artifacts/resolve/main/models/briefnet/birefnet-lite-768-gridsample.onnx",
    bytes: 204_958_552,
    sha256_hex: "f5c9e0044d82d9ce8023b2f3ce4b7879621361630da3ee6a78315f27f1238b85",
    cache_filename: "birefnet-general-lite-768.onnx",
    input_width: 768,
    input_height: 768,
    gpu_memory_bytes: 1800 * MIB,
};

/// `BIREFNET_MODEL` converted by `packaging/models/birefnet_fp16.py` for GPU
/// sessions. Needs about half the GPU memory; masks match float32 with an IoU
/// of 0.9989.
pub const BIREFNET_FP16_MODEL: BiRefNetModelSpec = BiRefNetModelSpec {
    checkpoint: "BiRefNet General-Lite",
    download_label: "BiRefNet General-Lite (GPU)",
    url: "https://huggingface.co/Duecki/CalibRaw-Artifacts/resolve/main/models/briefnet/birefnet-lowQ-gridsample-fp16.onnx",
    bytes: 114_115_335,
    sha256_hex: "4966ed930390ec1570f4d6be7d003f2e6a275ffa26d3a8f34c80982a56b16ff3",
    cache_filename: "birefnet-general-lite-fp16.onnx",
    input_width: 1024,
    input_height: 1024,
    gpu_memory_bytes: 1800 * MIB,
};

/// `BIREFNET_LOW_MODEL` converted by `packaging/models/birefnet_fp16.py` for
/// GPU sessions; masks match float32 with an IoU of 0.9982.
pub const BIREFNET_LOW_FP16_MODEL: BiRefNetModelSpec = BiRefNetModelSpec {
    checkpoint: "BiRefNet General-Lite 768",
    download_label: "BiRefNet General-Lite 768 (GPU)",
    url: "https://huggingface.co/Duecki/CalibRaw-Artifacts/resolve/main/models/briefnet/birefnet-lite-768-gridsample-fp16.onnx",
    bytes: 104_756_493,
    sha256_hex: "d6d1c6dc4cb451fa1e56f3cbb1035dd803e6a4fc04e5c71ca0f373d0a71f7b44",
    cache_filename: "birefnet-general-lite-768-fp16.onnx",
    input_width: 768,
    input_height: 768,
    gpu_memory_bytes: 1100 * MIB,
};

/// Numeric format of the subject model file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SubjectModelPrecision {
    #[default]
    Float32,
    /// For GPU sessions. CPU providers have few float16 kernels and run it
    /// about 30% slower than `Float32`.
    Float16,
}

impl SubjectModelPrecision {
    /// The precision for subject masks with the current settings: float16
    /// where sessions are expected on DirectML or CUDA, i.e. Windows and
    /// Linux with GPU acceleration enabled and Automatic mode's runtime built
    /// with a GPU provider. A manually selected runtime is not known before it
    /// loads, so it keeps float32, as does CoreML, which has not been tested
    /// with float16.
    pub fn for_current_sessions(manual_runtime: bool) -> Self {
        #[cfg(not(target_os = "android"))]
        let gpu = cfg!(any(target_os = "windows", target_os = "linux"))
            && !manual_runtime
            && crate::onnx_runtime_artifact::ai_sessions_may_use_gpu(false);
        #[cfg(target_os = "android")]
        let gpu = {
            let _ = manual_runtime;
            false
        };
        if gpu {
            Self::Float16
        } else {
            Self::Float32
        }
    }
}

/// How much work a subject mask may take. Serialized in the performance
/// settings; keep the names stable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubjectMaskQuality {
    /// `BIREFNET_LOW_MODEL` or its float16 conversion, one pass.
    Low,
    /// `BIREFNET_MODEL` or its float16 conversion, one pass.
    #[default]
    Medium,
    /// `Medium`'s model with the cropped refinement pass. Desktop only.
    High,
}

impl SubjectMaskQuality {
    /// The qualities this platform offers.
    pub const AVAILABLE: &'static [Self] = if cfg!(target_os = "android") {
        &[Self::Low, Self::Medium]
    } else {
        &[Self::Low, Self::Medium, Self::High]
    };

    pub const fn label(self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
        }
    }

    pub const fn model(self, precision: SubjectModelPrecision) -> BiRefNetModelSpec {
        match (self, precision) {
            (Self::Low, SubjectModelPrecision::Float32) => BIREFNET_LOW_MODEL,
            (Self::Low, SubjectModelPrecision::Float16) => BIREFNET_LOW_FP16_MODEL,
            (Self::Medium | Self::High, SubjectModelPrecision::Float32) => BIREFNET_MODEL,
            (Self::Medium | Self::High, SubjectModelPrecision::Float16) => BIREFNET_FP16_MODEL,
        }
    }

    const fn session_model(self, precision: SubjectModelPrecision) -> AiModel {
        match (self, precision) {
            (Self::Low, SubjectModelPrecision::Float32) => AiModel::BiRefNetLow,
            (Self::Low, SubjectModelPrecision::Float16) => AiModel::BiRefNetLowFp16,
            (Self::Medium | Self::High, SubjectModelPrecision::Float32) => AiModel::BiRefNet,
            (Self::Medium | Self::High, SubjectModelPrecision::Float16) => AiModel::BiRefNetFp16,
        }
    }

    const fn crop_refinement(self) -> bool {
        matches!(self, Self::High)
    }
}

const MASK_MODEL_DOWNLOAD: DownloadOptions = DownloadOptions {
    connect_timeout: Duration::from_secs(45),
    response_timeout: Duration::from_secs(60),
    body_timeout: Duration::from_secs(60 * 60),
    attempts: 5,
    resume: true,
};
const IMAGENET_MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const IMAGENET_STD: [f32; 3] = [0.229, 0.224, 0.225];

#[cfg(not(target_os = "android"))]
static DESKTOP_RUNTIME_IDENTITY: OnceLock<(PathBuf, String)> = OnceLock::new();
static RUNTIME_INITIALIZED: OnceLock<()> = OnceLock::new();
static RUNTIME_INIT_LOCK: Mutex<()> = Mutex::new(());
#[cfg(not(target_os = "android"))]
type RuntimeProbeResult = (PathBuf, String);
#[cfg(not(target_os = "android"))]
static RUNTIME_PROBE_CACHE: OnceLock<Mutex<Option<RuntimeProbeResult>>> = OnceLock::new();

fn mask_model_retention(cache_supported: bool) -> ModelRetention {
    #[cfg(target_os = "android")]
    {
        let _ = cache_supported;
        ModelRetention::OneShot
    }
    #[cfg(not(target_os = "android"))]
    {
        if cache_supported {
            ModelRetention::WhileWarm
        } else {
            ModelRetention::OneShot
        }
    }
}

#[derive(Debug)]
pub enum AiMaskEvent {
    DownloadProgress(ModelDownloadProgress),
    Inferencing,
    Finished(Result<AiMaskResult, String>),
}

#[derive(Debug)]
pub struct AiMaskResult {
    pub width: u32,
    pub height: u32,
    pub mask: Vec<u8>,
}

impl AiMaskResult {
    pub fn into_probability_mask(self) -> Option<MaskImage> {
        MaskImage::new(self.width, self.height, self.mask)
    }
}

pub fn birefnet_model_is_verified(
    quality: SubjectMaskQuality,
    precision: SubjectModelPrecision,
    path: &Path,
) -> bool {
    quality.model(precision).install().is_installed(path)
}

pub fn skyseg_model_is_verified(path: &Path) -> bool {
    SKYSEG_MODEL_INSTALL.is_installed(path)
}

pub fn depth_model_is_verified(path: &Path) -> bool {
    depth::MODEL_INSTALL.is_installed(path)
}

pub fn object_models_are_verified(encoder: &Path, decoder: &Path) -> bool {
    object::SAM21_ENCODER_INSTALL.is_installed(encoder)
        && object::SAM21_DECODER_INSTALL.is_installed(decoder)
}

pub struct AiMaskWorkerRequest {
    pub model: AiMaskModel,
    /// Used by `AiMaskModel::Subject` only.
    pub subject_quality: SubjectMaskQuality,
    /// Used by `AiMaskModel::Subject` only.
    pub subject_precision: SubjectModelPrecision,
    pub model_path: PathBuf,
    pub allow_download: bool,
    pub runtime_path: Option<PathBuf>,
    pub runtime_sha256: Option<String>,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiMaskModel {
    Subject,
    Sky,
    Depth,
}

impl AiMaskModel {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Subject => "Subject",
            Self::Sky => "Sky",
            Self::Depth => "Depth",
        }
    }

    pub const fn noun(self) -> &'static str {
        match self {
            Self::Subject => "subject",
            Self::Sky => "sky",
            Self::Depth => "depth",
        }
    }
}

pub fn spawn_ai_mask(
    request: AiMaskWorkerRequest,
    cancellation: Arc<AtomicBool>,
) -> mpsc::Receiver<AiMaskEvent> {
    let (sender, receiver) = mpsc::channel();
    let worker_sender = sender.clone();
    let spawn = std::thread::Builder::new()
        .name("calibraw-onnx-mask".to_owned())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                (|| {
                    ensure_ai_not_cancelled(&cancellation)?;
                    let install = match request.model {
                        AiMaskModel::Subject => request
                            .subject_quality
                            .model(request.subject_precision)
                            .install(),
                        AiMaskModel::Sky => SKYSEG_MODEL_INSTALL,
                        AiMaskModel::Depth => depth::MODEL_INSTALL,
                    };
                    install.ensure_installed(
                        &request.model_path,
                        request.allow_download,
                        |progress| {
                            let _ = worker_sender.send(AiMaskEvent::DownloadProgress(progress));
                        },
                        || ensure_ai_not_cancelled(&cancellation),
                    )?;
                    ensure_ai_not_cancelled(&cancellation)?;
                    let _ = worker_sender.send(AiMaskEvent::Inferencing);
                    infer_ai_mask(request)
                })()
            }))
            .unwrap_or_else(|panic| {
                let message = panic
                    .downcast_ref::<&str>()
                    .copied()
                    .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
                    .unwrap_or("unknown ONNX Runtime failure");
                Err(anyhow::anyhow!(
                    "ONNX Runtime terminated inference: {message}"
                ))
            });
            let _ = worker_sender.send(AiMaskEvent::Finished(
                result.map_err(|error| format!("{error:#}")),
            ));
        });
    if let Err(error) = spawn {
        let _ = sender.send(AiMaskEvent::Finished(Err(format!(
            "could not start AI mask worker: {error}"
        ))));
    }
    receiver
}

#[cfg(not(target_os = "android"))]
pub fn probe_runtime_subprocess(runtime_path: &Path, expected_sha256: &str) -> Result<()> {
    let runtime_path = fs::canonicalize(runtime_path)
        .with_context(|| format!("resolve selected ONNX Runtime {}", runtime_path.display()))?;
    let cache = RUNTIME_PROBE_CACHE.get_or_init(|| Mutex::new(None));
    {
        let cached = cache
            .lock()
            .map_err(|_| anyhow::anyhow!("ONNX Runtime probe cache lock was poisoned"))?;
        if let Some((cached_path, cached_sha256)) = cached.as_ref() {
            if cached_path == &runtime_path && cached_sha256 == expected_sha256 {
                return Ok(());
            }
        }
    }

    let executable =
        std::env::current_exe().context("locate CalibRaw executable for ONNX Runtime probe")?;
    let status = std::process::Command::new(&executable)
        .arg("--calibraw-onnx-runtime-probe")
        .arg(&runtime_path)
        .arg(expected_sha256)
        .status()
        .with_context(|| {
            format!(
                "start isolated ONNX Runtime probe with {}",
                executable.display()
            )
        })?;
    if !status.success() {
        return Err(anyhow::anyhow!(
            "the selected ONNX Runtime failed CalibRaw's isolated compatibility probe ({status}). Use a matching native {} ONNX Runtime library",
            std::env::consts::ARCH
        ));
    }
    let mut cached = cache
        .lock()
        .map_err(|_| anyhow::anyhow!("ONNX Runtime probe cache lock was poisoned"))?;
    *cached = Some((runtime_path, expected_sha256.to_owned()));
    Ok(())
}

#[cfg(not(target_os = "android"))]
pub fn run_runtime_probe_process(runtime_path: &Path, expected_sha256: &str) -> Result<()> {
    initialize_runtime(Some(runtime_path), Some(expected_sha256))
}

#[cfg(not(target_os = "android"))]
pub fn initialize_runtime(
    runtime_path: Option<&Path>,
    expected_sha256: Option<&str>,
) -> Result<()> {
    let (selected, expected_sha256) = match (runtime_path, expected_sha256) {
        (Some(path), Some(sha256)) => (path.to_path_buf(), sha256.to_owned()),
        (None, None) => {
            crate::ensure_automatic_onnx_runtime().context("prepare the automatic ONNX Runtime")?
        }
        _ => anyhow::bail!(
            "manual ONNX Runtime selection is incomplete; select the library again in Settings"
        ),
    };
    anyhow::ensure!(
        expected_sha256.len() == 64
            && expected_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "the selected ONNX Runtime SHA-256 pin is invalid"
    );
    let runtime_path = fs::canonicalize(&selected)
        .with_context(|| format!("resolve selected ONNX Runtime {}", selected.display()))?;
    let metadata = fs::metadata(&runtime_path)
        .with_context(|| format!("inspect selected ONNX Runtime {}", runtime_path.display()))?;
    anyhow::ensure!(
        metadata.is_file(),
        "selected ONNX Runtime is not a regular file"
    );
    anyhow::ensure!(
        (1_000_000..=1_000_000_000).contains(&metadata.len()),
        "selected ONNX Runtime has an implausible size of {} bytes",
        metadata.len()
    );
    let actual_sha256 =
        sha256_file_hex(&runtime_path).context("verify selected ONNX Runtime before loading")?;
    anyhow::ensure!(
        actual_sha256 == expected_sha256,
        "selected ONNX Runtime changed after approval: expected SHA-256 {expected_sha256}, found {actual_sha256}; select it again only if you trust the replacement"
    );
    if let Some((loaded_path, loaded_sha256)) = DESKTOP_RUNTIME_IDENTITY.get() {
        anyhow::ensure!(
            loaded_path == &runtime_path && loaded_sha256 == &actual_sha256,
            "a different ONNX Runtime is already active in this process; restart CalibRaw before changing the pinned runtime"
        );
    }
    let _init_guard = RUNTIME_INIT_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("ONNX Runtime initialization lock was poisoned"))?;
    if RUNTIME_INITIALIZED.get().is_none() {
        calibraw_core::diagnostics::record(format!(
            "ONNX Runtime library loading from {}",
            runtime_path.display()
        ));
        let started = std::time::Instant::now();
        let builder = ort::init_from(&runtime_path).map_err(|error| {
            anyhow::anyhow!(
                "could not load ONNX Runtime from {}: {error}",
                runtime_path.display()
            )
        })?;
        anyhow::ensure!(
            builder.with_name("CalibRaw").commit(),
            "ONNX Runtime was already initialized before the selected pinned library could be committed"
        );
        calibraw_core::diagnostics::record(format!(
            "ONNX Runtime library loaded in {:.3}s",
            started.elapsed().as_secs_f64()
        ));
        DESKTOP_RUNTIME_IDENTITY
            .set((runtime_path.clone(), actual_sha256.clone()))
            .map_err(|_| anyhow::anyhow!("ONNX Runtime identity was initialized concurrently"))?;
        RUNTIME_INITIALIZED
            .set(())
            .map_err(|_| anyhow::anyhow!("ONNX Runtime state was initialized concurrently"))?;
    }
    let (loaded_path, loaded_sha256) = DESKTOP_RUNTIME_IDENTITY
        .get()
        .context("ONNX Runtime initialized without a pinned desktop identity")?;
    anyhow::ensure!(
        loaded_path == &runtime_path && loaded_sha256 == &actual_sha256,
        "a different ONNX Runtime is already active in this process; restart CalibRaw before changing the pinned runtime"
    );
    Ok(())
}

/// Whether this process has already loaded an ONNX Runtime library. The
/// library stays loaded until the process exits, so choosing a different one
/// only takes effect after a restart.
#[cfg(not(target_os = "android"))]
pub fn runtime_loaded_in_process() -> bool {
    DESKTOP_RUNTIME_IDENTITY.get().is_some()
}

#[cfg(target_os = "android")]
pub fn initialize_runtime(
    _runtime_path: Option<&Path>,
    _expected_sha256: Option<&str>,
) -> Result<()> {
    if RUNTIME_INITIALIZED.get().is_some() {
        return Ok(());
    }
    let _init_guard = RUNTIME_INIT_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("ONNX Runtime initialization lock was poisoned"))?;
    if RUNTIME_INITIALIZED.get().is_none() {
        anyhow::ensure!(
            ort::init().with_name("CalibRaw").commit(),
            "ONNX Runtime was already initialized before CalibRaw could configure it"
        );
        RUNTIME_INITIALIZED
            .set(())
            .map_err(|_| anyhow::anyhow!("ONNX Runtime state was initialized concurrently"))?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn running_from_appimage() -> bool {
    std::env::var_os("APPIMAGE").is_some() || std::env::var_os("APPDIR").is_some()
}

fn cache_object_ai_sessions() -> bool {
    #[cfg(target_os = "android")]
    {
        false
    }
    #[cfg(all(not(target_os = "android"), target_os = "linux"))]
    {
        !running_from_appimage()
    }
    #[cfg(all(not(target_os = "android"), not(target_os = "linux")))]
    {
        true
    }
}

fn infer_ai_mask(request: AiMaskWorkerRequest) -> Result<AiMaskResult> {
    let AiMaskWorkerRequest {
        model,
        subject_quality,
        subject_precision,
        model_path,
        allow_download: _,
        runtime_path,
        runtime_sha256,
        width,
        height,
        rgba,
    } = request;
    const MAX_AI_MASK_PIXELS: u64 = 17_000_000;
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .context("AI-mask input dimensions overflow")?;
    anyhow::ensure!(
        pixels > 0 && pixels <= MAX_AI_MASK_PIXELS,
        "AI-mask input {width}x{height} exceeds the {MAX_AI_MASK_PIXELS}-pixel limit"
    );
    let expected_bytes = pixels
        .checked_mul(4)
        .and_then(|value| usize::try_from(value).ok())
        .context("AI-mask input byte count overflow")?;
    anyhow::ensure!(
        rgba.len() == expected_bytes,
        "AI-mask RGBA buffer has {} bytes, expected {expected_bytes}",
        rgba.len()
    );
    calibraw_core::diagnostics::record(format!("AI {} mask: preparing ONNX Runtime", model.noun()));
    initialize_runtime(runtime_path.as_deref(), runtime_sha256.as_deref())?;
    let image = ImageBuffer::<Rgba<u8>, _>::from_raw(width, height, rgba)
        .context("invalid preview image for AI mask")?;
    let started = std::time::Instant::now();
    let mask = match model {
        AiMaskModel::Depth => depth::depth_mask(&model_path, &image)?,
        AiMaskModel::Sky => sky::sky_mask(&model_path, &image)?,
        AiMaskModel::Subject => {
            subject_mask(&model_path, &image, subject_quality, subject_precision)?
        }
    };
    calibraw_core::diagnostics::record(format!(
        "AI {} mask finished in {:.3}s",
        model.noun(),
        started.elapsed().as_secs_f64()
    ));
    Ok(AiMaskResult {
        width,
        height,
        mask,
    })
}

/// Runs BiRefNet over the full frame and, for `High` on desktop, follows it
/// with one native-resolution crop pass for finer boundary detail.
fn subject_mask(
    model_path: &Path,
    image: &ImageBuffer<Rgba<u8>, Vec<u8>>,
    quality: SubjectMaskQuality,
    precision: SubjectModelPrecision,
) -> Result<Vec<u8>> {
    let (width, height) = image.dimensions();
    let mut mask = infer_birefnet(model_path, image, quality, precision)?;
    let crop_refinement =
        subject_crop_refinement_enabled(quality.crop_refinement(), cfg!(target_os = "android"));
    if crop_refinement {
        if let Some(crop) = mask_refine::mask_crop_above(&mask, width, height, 5, 0.15) {
            let crop_image =
                image::imageops::crop_imm(image, crop.x, crop.y, crop.width, crop.height)
                    .to_image();
            let crop_alpha = infer_birefnet(model_path, &crop_image, quality, precision)?;
            mask_refine::merge_crop_pass(&mut mask, width, crop, &crop_alpha, 8);
        }
    }
    mask_refine::guided_filter_color(image.as_raw(), &mut mask, width, height, 8, 1e-4)?;
    mask_refine::harden_model_alpha(&mut mask);
    Ok(mask)
}

const fn subject_crop_refinement_enabled(requested: bool, android: bool) -> bool {
    requested && !android
}

pub(super) fn infer_birefnet(
    model_path: &Path,
    image: &ImageBuffer<Rgba<u8>, Vec<u8>>,
    quality: SubjectMaskQuality,
    precision: SubjectModelPrecision,
) -> Result<Vec<u8>> {
    let model = quality.model(precision);
    let resized = image::imageops::resize(
        image,
        model.input_width,
        model.input_height,
        FilterType::Lanczos3,
    );
    let input = normalized_rgb_input(&resized)?;
    let input = Tensor::from_array((
        [
            1usize,
            3,
            model.input_height as usize,
            model.input_width as usize,
        ],
        input,
    ))
    .context("create BiRefNet input tensor")?;

    let (output_width, output_height, logits) = with_model_session(
        quality.session_model(precision),
        model_path,
        SessionOptions::new("BiRefNet").with_gpu_memory(model.gpu_memory_bytes),
        mask_model_retention(true),
        |session| run_subject_session(session, input, model.input_width, model.input_height),
    )?;

    restore_birefnet_output(
        &logits,
        output_width,
        output_height,
        image.width(),
        image.height(),
    )
}

fn run_subject_session(
    session: &mut FallbackSession,
    input: Tensor<f32>,
    input_width: u32,
    input_height: u32,
) -> Result<(u32, u32, Vec<f32>)> {
    session.run_with_fallback("BiRefNet ONNX inference", |ort_session, _accelerated| {
        let outputs = ort_session
            .run(ort::inputs![&input])
            .context("run BiRefNet ONNX inference")?;
        let output = outputs
            .values()
            .next()
            .context("BiRefNet returned no output tensors")?;
        let (shape, logits) = output
            .try_extract_tensor::<f32>()
            .context("read BiRefNet output tensor")?;
        let (output_width, output_height, output_elements) =
            validate_birefnet_output_shape(shape, logits.len(), input_width, input_height)?;
        anyhow::ensure!(
            logits.iter().all(|value| value.is_finite()),
            "BiRefNet output contains non-finite logits"
        );
        let mut owned_logits = Vec::new();
        owned_logits
            .try_reserve_exact(output_elements)
            .context("reserve BiRefNet output logits")?;
        owned_logits.extend_from_slice(logits);
        Ok((output_width, output_height, owned_logits))
    })
}
fn validate_birefnet_output_shape(
    shape: &[i64],
    logits_len: usize,
    input_width: u32,
    input_height: u32,
) -> Result<(u32, u32, usize)> {
    anyhow::ensure!(
        shape.len() == 4 && shape[0] == 1 && shape[1] == 1,
        "unexpected BiRefNet output shape {shape:?}; expected [1, 1, H, W]"
    );
    let output_height =
        usize::try_from(shape[2]).context("BiRefNet output height is negative or too large")?;
    let output_width =
        usize::try_from(shape[3]).context("BiRefNet output width is negative or too large")?;
    anyhow::ensure!(
        output_width > 0 && output_height > 0,
        "BiRefNet output dimensions must be positive: {shape:?}"
    );
    let output_elements = output_width
        .checked_mul(output_height)
        .context("BiRefNet output dimensions overflow")?;
    anyhow::ensure!(
        output_elements <= input_width as usize * input_height as usize * 4,
        "BiRefNet output is implausibly large: {shape:?}"
    );
    anyhow::ensure!(
        logits_len == output_elements,
        "BiRefNet output shape {shape:?} describes {output_elements} values, but the tensor contains {logits_len}"
    );
    Ok((
        u32::try_from(output_width).context("BiRefNet output width exceeds u32")?,
        u32::try_from(output_height).context("BiRefNet output height exceeds u32")?,
        output_elements,
    ))
}

fn normalized_rgb_input(resized: &ImageBuffer<Rgba<u8>, Vec<u8>>) -> Result<Vec<f32>> {
    let (input_width, input_height) = resized.dimensions();
    let plane = usize::try_from(input_width)
        .ok()
        .and_then(|width| {
            usize::try_from(input_height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .context("AI input dimensions overflow")?;
    let values = plane.checked_mul(3).context("AI input size overflow")?;
    let mut input = Vec::new();
    input
        .try_reserve_exact(values)
        .context("reserve AI input tensor")?;
    input.resize(values, 0.0);
    for y in 0..input_height {
        for x in 0..input_width {
            let pixel = resized.get_pixel(x, y);
            let destination = (y * input_width + x) as usize;
            for channel in 0..3 {
                let value = pixel[channel] as f32 / 255.0;
                input[channel * plane + destination] =
                    (value - IMAGENET_MEAN[channel]) / IMAGENET_STD[channel];
            }
        }
    }
    Ok(input)
}

fn restore_birefnet_output(
    logits: &[f32],
    output_width: u32,
    output_height: u32,
    target_width: u32,
    target_height: u32,
) -> Result<Vec<u8>> {
    let output_elements = usize::try_from(output_width)
        .ok()
        .and_then(|width| {
            usize::try_from(output_height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .context("BiRefNet output dimensions overflow")?;
    anyhow::ensure!(
        logits.len() == output_elements,
        "BiRefNet logits do not match the declared output dimensions"
    );
    let mut probabilities = Vec::new();
    probabilities
        .try_reserve_exact(output_elements)
        .context("reserve BiRefNet probability map")?;
    for &logit in logits {
        anyhow::ensure!(
            logit.is_finite(),
            "BiRefNet output contains a non-finite logit"
        );
        probabilities.push(sigmoid_probability(logit));
    }
    let output =
        ImageBuffer::<Luma<f32>, Vec<f32>>::from_raw(output_width, output_height, probabilities)
            .context("invalid BiRefNet output buffer")?;
    let resized =
        image::imageops::resize(&output, target_width, target_height, FilterType::Lanczos3);
    Ok(resized
        .into_raw()
        .into_iter()
        .map(|probability| (probability.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
        .collect())
}

fn sigmoid_probability(logit: f32) -> f32 {
    if logit >= 0.0 {
        1.0 / (1.0 + (-logit).exp())
    } else {
        let exponential = logit.exp();
        exponential / (1.0 + exponential)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        birefnet_model_is_verified, infer_birefnet, normalized_rgb_input, restore_birefnet_output,
        sigmoid_probability, subject_crop_refinement_enabled, validate_birefnet_output_shape,
        SubjectMaskQuality, SubjectModelPrecision, BIREFNET_FP16_MODEL, BIREFNET_LOW_FP16_MODEL,
        BIREFNET_LOW_MODEL, BIREFNET_MODEL, IMAGENET_MEAN, IMAGENET_STD,
    };
    use image::{ImageBuffer, Rgba};
    use std::path::Path;

    /// `CALIBRAW_BIREFNET_MODEL` may be either published subject model.
    #[test]
    #[ignore = "requires CALIBRAW_BIREFNET_MODEL and ONNX Runtime via ORT_DYLIB_PATH"]
    fn published_birefnet_runs_on_the_expected_provider() {
        let path = std::env::var_os("CALIBRAW_BIREFNET_MODEL").expect("CALIBRAW_BIREFNET_MODEL");
        let runtime = std::env::var_os("ORT_DYLIB_PATH").expect("ORT_DYLIB_PATH");
        ort::init_from(Path::new(&runtime)).unwrap().commit();
        let (quality, precision) = [SubjectMaskQuality::Low, SubjectMaskQuality::Medium]
            .into_iter()
            .flat_map(|quality| {
                [
                    SubjectModelPrecision::Float32,
                    SubjectModelPrecision::Float16,
                ]
                .map(|precision| (quality, precision))
            })
            .find(|&(quality, precision)| {
                birefnet_model_is_verified(quality, precision, Path::new(&path))
            })
            .expect("CALIBRAW_BIREFNET_MODEL is not a published subject model");
        let image = ImageBuffer::from_fn(96, 64, |x, y| {
            Rgba([(x * 255 / 95) as u8, (y * 255 / 63) as u8, 128, 255])
        });
        let mask = infer_birefnet(Path::new(&path), &image, quality, precision).unwrap();
        assert_eq!(mask.len(), 96 * 64);
        let providers = crate::active_execution_providers();
        eprintln!("BiRefNet inference providers: {providers:?}");
        if let Ok(expected) = std::env::var("CALIBRAW_TEST_BIREFNET_PROVIDER") {
            assert!(providers.iter().any(|status| {
                status.model_name == "BiRefNet"
                    && status.active_provider == expected
                    && !status.degraded
            }));
        }
    }

    #[test]
    fn android_always_disables_subject_crop_refinement() {
        assert!(subject_crop_refinement_enabled(true, false));
        assert!(!subject_crop_refinement_enabled(false, false));
        assert!(!subject_crop_refinement_enabled(true, true));
    }

    #[test]
    fn subject_qualities_choose_model_and_crop_pass() {
        use SubjectModelPrecision::{Float16, Float32};
        assert_eq!(SubjectMaskQuality::Low.model(Float32), BIREFNET_LOW_MODEL);
        assert_eq!(SubjectMaskQuality::Medium.model(Float32), BIREFNET_MODEL);
        assert_eq!(SubjectMaskQuality::High.model(Float32), BIREFNET_MODEL);
        assert_eq!(
            SubjectMaskQuality::Low.model(Float16),
            BIREFNET_LOW_FP16_MODEL
        );
        assert_eq!(
            SubjectMaskQuality::Medium.model(Float16),
            BIREFNET_FP16_MODEL
        );
        assert_eq!(SubjectMaskQuality::High.model(Float16), BIREFNET_FP16_MODEL);
        assert!(!SubjectMaskQuality::Low.crop_refinement());
        assert!(!SubjectMaskQuality::Medium.crop_refinement());
        assert!(SubjectMaskQuality::High.crop_refinement());
    }

    /// Each model file has its own cache file and session, so switching
    /// quality or precision never reuses another file's download or session.
    #[test]
    fn every_subject_model_file_is_distinct() {
        use SubjectModelPrecision::{Float16, Float32};
        let choices = [
            (SubjectMaskQuality::Low, Float32),
            (SubjectMaskQuality::Low, Float16),
            (SubjectMaskQuality::Medium, Float32),
            (SubjectMaskQuality::Medium, Float16),
        ];
        for (index, &(quality, precision)) in choices.iter().enumerate() {
            let model = quality.model(precision);
            for &(other_quality, other_precision) in &choices[index + 1..] {
                let other = other_quality.model(other_precision);
                assert_ne!(model.cache_filename, other.cache_filename);
                assert_ne!(model.sha256_hex, other.sha256_hex);
                assert_ne!(model.url, other.url);
                assert_ne!(
                    quality.session_model(precision),
                    other_quality.session_model(other_precision)
                );
            }
        }
        for &quality in SubjectMaskQuality::AVAILABLE {
            let (float32, float16) = (quality.model(Float32), quality.model(Float16));
            assert_eq!(
                (float32.input_width, float32.input_height),
                (float16.input_width, float16.input_height)
            );
            assert!(float16.gpu_memory_bytes < float32.gpu_memory_bytes);
        }
    }

    #[test]
    fn manual_and_cpu_runtimes_use_float32_subject_models() {
        assert_eq!(
            SubjectModelPrecision::for_current_sessions(true),
            SubjectModelPrecision::Float32
        );
        if !cfg!(any(target_os = "windows", target_os = "linux")) {
            assert_eq!(
                SubjectModelPrecision::for_current_sessions(false),
                SubjectModelPrecision::Float32
            );
        }
    }

    #[test]
    fn every_platform_defaults_to_an_available_subject_quality() {
        assert!(SubjectMaskQuality::AVAILABLE.contains(&SubjectMaskQuality::default()));
        assert_eq!(
            SubjectMaskQuality::AVAILABLE.contains(&SubjectMaskQuality::High),
            !cfg!(target_os = "android")
        );
    }

    #[test]
    fn birefnet_preprocessing_normalizes_the_complete_native_tensor() {
        let image = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(
            2,
            1,
            vec![255, 128, 0, 255, 0, 64, 255, 255],
        )
        .unwrap();
        let input = normalized_rgb_input(&image).unwrap();

        assert_eq!(input.len(), 6);
        let expected = [
            (1.0 - IMAGENET_MEAN[0]) / IMAGENET_STD[0],
            (0.0 - IMAGENET_MEAN[0]) / IMAGENET_STD[0],
            (128.0 / 255.0 - IMAGENET_MEAN[1]) / IMAGENET_STD[1],
            (64.0 / 255.0 - IMAGENET_MEAN[1]) / IMAGENET_STD[1],
            (0.0 - IMAGENET_MEAN[2]) / IMAGENET_STD[2],
            (1.0 - IMAGENET_MEAN[2]) / IMAGENET_STD[2],
        ];
        for (actual, expected) in input.into_iter().zip(expected) {
            assert!((actual - expected).abs() < 1e-6);
        }
    }

    #[test]
    fn birefnet_output_keeps_soft_probabilities_when_restoring_size() {
        let mask = restore_birefnet_output(&[-2.0, 0.0, 2.0], 3, 1, 3, 1).unwrap();
        assert_eq!(mask, vec![30, 128, 225]);
    }

    #[test]
    fn birefnet_output_requires_single_batch_and_channel() {
        assert!(
            validate_birefnet_output_shape(&[1, 1, 1024, 1024], 1024 * 1024, 1024, 1024).is_ok()
        );
        assert!(
            validate_birefnet_output_shape(&[1, 2, 1024, 1024], 2 * 1024 * 1024, 1024, 1024)
                .is_err()
        );
        assert!(validate_birefnet_output_shape(&[1, 1, -1, 1024], 0, 1024, 1024).is_err());
        assert!(validate_birefnet_output_shape(&[1, 1, 2, 2], 5, 1024, 1024).is_err());
    }

    #[test]
    fn sigmoid_probabilities_keep_model_calibration() {
        assert!((sigmoid_probability(0.0) - 0.5).abs() < f32::EPSILON);
        assert!((sigmoid_probability(2.0) - 0.880797).abs() < 1e-6);
        assert!((sigmoid_probability(-2.0) - 0.119203).abs() < 1e-6);
    }
}

mod depth_refine;
mod mask_refine;
mod object;
mod sky;

pub use object::{
    spawn_object_mask, ObjectCropRect, ObjectInferenceCache, ObjectMaskEvent, ObjectMaskRequest,
    ObjectMaskResult, ObjectMaskWorkerRequest, SamTensorData, SAM21_DECODER_MODEL_URL,
    SAM21_DECODER_SHA256_HEX, SAM21_ENCODER_MODEL_URL, SAM21_ENCODER_SHA256_HEX,
    SAM21_MODEL_BYTES_ESTIMATE,
};
