//! How the app sets up local AI: which ONNX Runtime to load, whether sessions
//! may use the GPU, where models are cached and which GPU the editor renders
//! with. [`configure`] applies everything once at startup; [`update`] applies
//! a change from Settings.

use crate::placement;
use crate::registry::artifact::sha256_file_hex;
use crate::runtime::AutomaticOnnxRuntimeKind;
use anyhow::Result;
use calibraw_gpu::memory::AdapterMemory;
use std::{
    fmt,
    path::{Path, PathBuf},
    sync::{PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard},
};

/// A SHA-256 digest: 64 lowercase hexadecimal digits, checked once when it is
/// made.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Sha256(String);

impl Sha256 {
    pub fn parse(hex: &str) -> Result<Self> {
        anyhow::ensure!(
            hex.len() == 64
                && hex
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
            "{hex:?} is not a SHA-256 of 64 lowercase hexadecimal digits"
        );
        Ok(Self(hex.to_owned()))
    }

    /// The digest of the file at `path`.
    pub fn of_file(path: &Path) -> Result<Self> {
        Ok(Self(sha256_file_hex(path)?))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Sha256 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Which ONNX Runtime library AI jobs load.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum RuntimeSource {
    /// On desktop the library bundled by the packager, or else the pinned
    /// download for this platform; on Android the one built into the app.
    #[default]
    Automatic,
    /// A library the user selected, loaded only while its SHA-256 still
    /// matches the one they approved.
    Manual { path: PathBuf, sha256: Sha256 },
}

/// The GPU the editor renders with. DirectML sessions run on it, so AI work
/// and its free-memory check concern the same adapter.
#[derive(Clone, Debug)]
pub struct GpuIdentity {
    /// PCI IDs as `wgpu` reports them; 0 on Metal, which has none.
    vendor_id: u32,
    device_id: u32,
    backend: wgpu::Backend,
    /// The editor's device, kept for the memory query, which Metal answers
    /// only through a device.
    device: wgpu::Device,
}

impl GpuIdentity {
    pub fn new(device: &wgpu::Device) -> Self {
        let info = device.adapter_info();
        Self {
            vendor_id: info.vendor,
            device_id: info.device,
            backend: info.backend,
            device: device.clone(),
        }
    }

    pub(crate) fn vendor_id(&self) -> u32 {
        self.vendor_id
    }

    pub(crate) fn device_id(&self) -> u32 {
        self.device_id
    }

    pub(crate) fn backend(&self) -> wgpu::Backend {
        self.backend
    }

    /// GPU memory this process may use on the adapter now, where its backend
    /// reports it.
    pub(crate) fn memory(&self) -> Option<AdapterMemory> {
        calibraw_gpu::memory::adapter_memory(&self.device)
    }
}

/// What this device offers local AI. Model code reads these instead of the
/// OS; platform bugs are `placement::quirks` instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AiCapabilities {
    pub(crate) memory: MemoryClass,
    /// Mask sessions (subject, sky, depth, object) may stay loaded between
    /// jobs while their feature is warm.
    pub(crate) keep_mask_sessions_warm: bool,
    /// The platform's GPU providers run float16 models: DirectML and CUDA.
    /// CoreML has not been tested with float16; Android has no float16 models.
    pub(crate) float16_sessions: bool,
}

/// How much memory AI may take beside the editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MemoryClass {
    /// Shared with the system and the RAW preview (Android): models unload
    /// after each job, depth runs on a lean CPU session and High subject
    /// quality is unavailable.
    Constrained,
    Standard,
}

pub(crate) const CAPABILITIES: AiCapabilities = if cfg!(target_os = "android") {
    AiCapabilities {
        memory: MemoryClass::Constrained,
        keep_mask_sessions_warm: false,
        float16_sessions: false,
    }
} else {
    AiCapabilities {
        memory: MemoryClass::Standard,
        keep_mask_sessions_warm: true,
        float16_sessions: cfg!(any(target_os = "windows", target_os = "linux")),
    }
};

pub struct AiConfig {
    /// Whether sessions may use GPU execution providers. Android always may.
    pub acceleration: bool,
    pub runtime: RuntimeSource,
    /// The runtime `RuntimeSource::Automatic` downloads where there is a
    /// choice (desktop Linux x86_64).
    pub automatic_runtime: AutomaticOnnxRuntimeKind,
    /// The folder models and the automatic runtime are cached in.
    pub model_root: PathBuf,
    /// Where the GPU job record is kept (`gpu_job_in_progress`); without it,
    /// jobs are only tracked in memory.
    pub job_record_path: Option<PathBuf>,
    pub gpu: Option<GpuIdentity>,
}

/// A setting that may change while CalibRaw runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AiConfigChange {
    /// Sessions created from now on may use the GPU, or must not. Enabling
    /// it also lifts a switch-off after a GPU failure.
    Acceleration(bool),
    /// Takes effect only before a runtime is loaded; the loaded one stays
    /// until CalibRaw restarts.
    Runtime(RuntimeSource),
    AutomaticRuntime(AutomaticOnnxRuntimeKind),
}

struct Settings {
    runtime: RuntimeSource,
    automatic_runtime: AutomaticOnnxRuntimeKind,
    /// `None` until configured.
    model_root: Option<PathBuf>,
}

static SETTINGS: RwLock<Settings> = RwLock::new(Settings {
    runtime: RuntimeSource::Automatic,
    automatic_runtime: AutomaticOnnxRuntimeKind::Cpu,
    model_root: None,
});

/// Applies `config`. Call once at startup, before the first AI job. Returns
/// the GPU job a previous run left in the job record, i.e. the one that run
/// did not survive.
pub fn configure(config: AiConfig) -> Option<String> {
    let AiConfig {
        acceleration,
        runtime,
        automatic_runtime,
        model_root,
        job_record_path,
        gpu,
    } = config;
    {
        let mut settings = write();
        settings.runtime = runtime;
        settings.automatic_runtime = automatic_runtime;
        settings.model_root = Some(model_root);
    }
    placement::set_ai_acceleration_enabled(acceleration);
    if let Some(gpu) = gpu {
        placement::set_editor_gpu(gpu);
    }
    job_record_path.and_then(placement::set_gpu_job_record_path)
}

pub fn update(change: AiConfigChange) {
    match change {
        AiConfigChange::Acceleration(enabled) => placement::set_ai_acceleration_enabled(enabled),
        AiConfigChange::Runtime(runtime) => write().runtime = runtime,
        AiConfigChange::AutomaticRuntime(kind) => write().automatic_runtime = kind,
    }
}

pub(crate) fn runtime_source() -> RuntimeSource {
    read().runtime.clone()
}

/// Android has no automatic runtime download.
#[cfg(not(target_os = "android"))]
pub(crate) fn automatic_runtime_kind() -> AutomaticOnnxRuntimeKind {
    read().automatic_runtime
}

/// The configured model folder, or before `configure` the platform default.
pub(crate) fn model_root() -> PathBuf {
    read().model_root.clone().unwrap_or_else(default_model_root)
}

#[cfg(not(target_os = "android"))]
fn default_model_root() -> PathBuf {
    desktop_model_cache_root()
}

/// Android apps keep models in their internal data folder, which only the
/// app knows; tests use the temporary folder.
#[cfg(target_os = "android")]
fn default_model_root() -> PathBuf {
    std::env::temp_dir().join("models")
}

#[cfg(not(target_os = "android"))]
pub fn desktop_model_cache_root() -> PathBuf {
    let root = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    root.join("calibraw/models")
}

fn read() -> RwLockReadGuard<'static, Settings> {
    SETTINGS.read().unwrap_or_else(PoisonError::into_inner)
}

fn write() -> RwLockWriteGuard<'static, Settings> {
    SETTINGS.write().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::Sha256;

    #[test]
    fn sha256_accepts_only_lowercase_hex_digests() {
        let digest = "7702832f291b1ad6d8269d712184a9ddc87c9bac3833fa10b3f2140830fb4c47";
        assert_eq!(Sha256::parse(digest).unwrap().as_str(), digest);
        assert!(Sha256::parse(&digest.to_ascii_uppercase()).is_err());
        assert!(Sha256::parse(&digest[1..]).is_err());
        assert!(Sha256::parse(&format!("{}g", &digest[1..])).is_err());
    }
}
