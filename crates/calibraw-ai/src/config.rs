//! How the app sets up local AI: which ONNX Runtime to load, whether sessions
//! may use the GPU, where models are cached and which GPU the editor renders
//! with. [`configure`] applies everything once at startup; [`update`] applies
//! a change from Settings.

use crate::placement;
use crate::registry::artifact::sha256_file_hex;
use anyhow::Result;
use calibraw_gpu::memory::AdapterMemory;
use std::{
    fmt,
    path::{Path, PathBuf},
    sync::{OnceLock, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard},
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

/// The GPU the editor renders with. Sessions on a provider that runs on it
/// are placed by its free memory.
#[derive(Clone, Debug)]
pub struct GpuIdentity {
    /// The adapter's name, for statuses.
    name: String,
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
            name: info.name,
            vendor_id: info.vendor,
            device_id: info.device,
            backend: info.backend,
            device: device.clone(),
        }
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
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
    /// Devices with at least this much physical memory, in bytes, segment
    /// people's body parts; the others outline entire people only. The
    /// body-part model needs gigabytes and several seconds per person.
    pub(crate) people_body_parts_memory: u64,
}

/// A phone sold with 12 GB reports about 11.2 GiB (a Pixel 7 Pro 11.19 GiB),
/// one with 8 GB about 7.3 GiB.
const MOBILE_PEOPLE_BODY_PARTS_MEMORY: u64 = 10 << 30;

/// How much memory AI may take beside the editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MemoryClass {
    /// Shared with the system and the RAW preview (Android and iOS): models unload
    /// after each job, depth runs on a lean CPU session and High subject
    /// quality is unavailable.
    Constrained,
    Standard,
}

pub(crate) const CAPABILITIES: AiCapabilities =
    if cfg!(any(target_os = "android", target_os = "ios")) {
        AiCapabilities {
            memory: MemoryClass::Constrained,
            keep_mask_sessions_warm: false,
            people_body_parts_memory: MOBILE_PEOPLE_BODY_PARTS_MEMORY,
        }
    } else {
        AiCapabilities {
            memory: MemoryClass::Standard,
            keep_mask_sessions_warm: true,
            people_body_parts_memory: 0,
        }
    };

const GIB: u64 = 1 << 30;

/// How CPU sessions run on `MemoryClass::Constrained`, where every model runs
/// beside the RAW preview and the system ends apps that grow too large.
/// Chosen once per process from the device (`constrained_cpu_sessions`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ConstrainedCpuSessions {
    /// Intra-op threads of a session, the calling thread included.
    pub(crate) threads: usize,
    /// Whether the CPU provider keeps its arena allocator, which saves
    /// allocations on every run but holds a session's peak until it ends.
    pub(crate) arena: bool,
}

impl ConstrainedCpuSessions {
    /// Two threads, as many as the Tab M11's Helio G88 has fast cores (two
    /// Cortex-A75s beside six A55s), and no arena. For devices whose RAM or
    /// cores are not known, and iOS.
    const ENTRY: Self = Self {
        threads: 2,
        arena: false,
    };

    /// The sessions for a device with `memory` bytes of RAM and `fast_cores`
    /// fast cores: one thread per fast core, as slow cores hold the others
    /// back, up to a cap by RAM, as every thread holds its own scratch
    /// buffers and the preview needs cores too. The arena only from 10 GiB,
    /// what a phone sold with 12 GB reports (a Galaxy S25 Ultra 10.9 GiB).
    /// A phone sold with 6 GB reports about 5.5 GiB, one with 8 GB about 7.3.
    const fn for_device(memory: Option<u64>, fast_cores: Option<usize>) -> Self {
        let Some(memory) = memory else {
            return Self::ENTRY;
        };
        let (max_threads, arena) = if memory < 6 * GIB {
            (2, false)
        } else if memory < 10 * GIB {
            (4, false)
        } else {
            (6, true)
        };
        let threads = match fast_cores {
            Some(fast_cores) if fast_cores < max_threads => fast_cores,
            Some(_) => max_threads,
            None => Self::ENTRY.threads,
        };
        Self {
            threads: if threads == 0 { 1 } else { threads },
            arena,
        }
    }
}

/// How CPU sessions run on `MemoryClass::Constrained` on this device. Records
/// the choice in diagnostics the first time.
pub(crate) fn constrained_cpu_sessions() -> ConstrainedCpuSessions {
    static SESSIONS: OnceLock<ConstrainedCpuSessions> = OnceLock::new();
    *SESSIONS.get_or_init(|| {
        let (memory, fast_cores) = if cfg!(target_os = "android") {
            (
                calibraw_core::system_memory::total_memory_bytes(),
                fast_core_count(),
            )
        } else {
            (None, None)
        };
        let sessions = ConstrainedCpuSessions::for_device(memory, fast_cores);
        calibraw_core::diagnostics::record(format!(
            "AI CPU sessions: {} threads, arena {} ({} MiB RAM, {} fast cores)",
            sessions.threads,
            if sessions.arena { "on" } else { "off" },
            memory.map_or_else(|| "unknown".to_owned(), |bytes| (bytes >> 20).to_string()),
            fast_cores.map_or_else(|| "unknown".to_owned(), |cores| cores.to_string()),
        ));
        sessions
    })
}

/// How many cores have at least half the capacity of the fastest, from the
/// kernel's `cpu_capacity` (1024 for the fastest core). Clock speeds do not
/// tell: the Helio G88's slow Cortex-A55s run at 90% of its A75s' clock.
/// `None` where the kernel reports no capacities.
fn fast_core_count() -> Option<usize> {
    let capacities: Vec<u32> = (0..)
        .map(|core| format!("/sys/devices/system/cpu/cpu{core}"))
        .take_while(|core| Path::new(core).is_dir())
        .filter_map(|core| {
            std::fs::read_to_string(format!("{core}/cpu_capacity"))
                .ok()?
                .trim()
                .parse()
                .ok()
        })
        .collect();
    let fastest = *capacities.iter().max()?;
    Some(
        capacities
            .iter()
            .filter(|&&capacity| capacity * 2 >= fastest)
            .count(),
    )
}

pub struct AiConfig {
    /// Whether sessions may use GPU execution providers. Android always may.
    pub acceleration: bool,
    pub runtime: RuntimeSource,
    /// The folder models and the automatic runtime are cached in.
    pub model_root: PathBuf,
    /// Where the GPU job record is kept (`gpu_ai_activity`); without it, GPU
    /// work is only tracked in memory.
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
}

struct Settings {
    runtime: RuntimeSource,
    /// `None` until configured.
    model_root: Option<PathBuf>,
}

static SETTINGS: RwLock<Settings> = RwLock::new(Settings {
    runtime: RuntimeSource::Automatic,
    model_root: None,
});

/// Applies `config`. Call once at startup, before the first AI job. Returns
/// the GPU work a previous run left in the job record, i.e. the work that run
/// did not survive.
pub fn configure(config: AiConfig) -> Option<placement::GpuAiActivity> {
    let AiConfig {
        acceleration,
        runtime,
        model_root,
        job_record_path,
        gpu,
    } = config;
    {
        let mut settings = write();
        settings.runtime = runtime;
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
    }
}

pub(crate) fn runtime_source() -> RuntimeSource {
    read().runtime.clone()
}

/// The configured model folder, or before `configure` the platform default.
pub(crate) fn model_root() -> PathBuf {
    read().model_root.clone().unwrap_or_else(default_model_root)
}

#[cfg(not(target_os = "android"))]
fn default_model_root() -> PathBuf {
    desktop_model_root()
}

/// Android apps keep models in their internal data folder, which only the
/// app knows; tests use the temporary folder.
#[cfg(target_os = "android")]
fn default_model_root() -> PathBuf {
    std::env::temp_dir().join("models")
}

/// Where desktop builds keep downloaded models and automatic runtimes: the
/// platform's folder for application data, which cleaners leave alone,
/// unlike a cache or the temporary folder.
/// - Windows: `%LOCALAPPDATA%\calibraw\models`, local rather than roaming
///   because the models are large.
/// - macOS: `~/Library/Application Support/calibraw/models`.
/// - Linux: `$XDG_DATA_HOME/calibraw/models`, by default
///   `~/.local/share/calibraw/models`.
///
/// Versions up to 1.3 used a cache folder, which on Windows was the
/// temporary folder; `calibraw_core::migrations::desktop_model_folder`
/// moves what they downloaded.
#[cfg(not(target_os = "android"))]
pub fn desktop_model_root() -> PathBuf {
    let env_path = |name| std::env::var_os(name).map(PathBuf::from);
    let data_root = if cfg!(windows) {
        env_path("LOCALAPPDATA").or_else(|| {
            env_path("USERPROFILE").map(|profile| profile.join("AppData").join("Local"))
        })
    } else if cfg!(target_os = "macos") {
        env_path("HOME").map(|home| home.join("Library").join("Application Support"))
    } else {
        // The XDG specification ignores a relative `XDG_DATA_HOME`.
        env_path("XDG_DATA_HOME")
            .filter(|path| path.is_absolute())
            .or_else(|| env_path("HOME").map(|home| home.join(".local").join("share")))
    };
    data_root
        .unwrap_or_else(std::env::temp_dir)
        .join("calibraw")
        .join("models")
}

fn read() -> RwLockReadGuard<'static, Settings> {
    SETTINGS.read().unwrap_or_else(PoisonError::into_inner)
}

fn write() -> RwLockWriteGuard<'static, Settings> {
    SETTINGS.write().unwrap_or_else(PoisonError::into_inner)
}
