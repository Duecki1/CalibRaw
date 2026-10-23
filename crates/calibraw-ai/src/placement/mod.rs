//! Where sessions run: the rule that puts each on a GPU provider when it fits
//! and on the CPU otherwise (`place`), the providers on offer, rebuilding on
//! the CPU when a GPU fails, and where each model ended up.

mod failure;
mod job_record;
mod memory;
mod out_of_memory;
mod place;
mod providers;
pub(crate) mod quirks;
mod session;
mod status;

pub(crate) use failure::count_logged_gpu_error;
pub use failure::take_ai_gpu_device_loss;
pub use job_record::{end_gpu_job_records, gpu_ai_activity, gpu_job_in_progress, GpuAiActivity};
pub(crate) use job_record::{set_gpu_job_record_path, GpuJobRecord};
use place::{
    may_stay_loaded, needed_memory, place, AccelerationState, OutOfMemoryRecord, ProviderOffer,
};
pub use place::{CpuReason, GpuProvider, Placement};
pub(crate) use session::{
    create_session_with_fallback, CpuFallbackProfile, FallbackSession, SessionOptions,
};
pub use status::{ai_session_statuses, AiSessionStatus};

use crate::config::{GpuIdentity, RuntimeSource};
#[cfg(not(target_os = "android"))]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

/// Rises whenever the providers new sessions may use change; a loaded session
/// of an older generation is not reused.
static PROVIDER_GENERATION: AtomicU64 = AtomicU64::new(0);

pub(crate) fn provider_generation() -> u64 {
    PROVIDER_GENERATION.load(Ordering::Acquire)
}

#[cfg(not(target_os = "android"))]
static AI_ACCELERATION_ENABLED: AtomicBool = AtomicBool::new(true);

/// Android always allows GPU providers.
#[cfg(target_os = "android")]
pub(crate) fn set_ai_acceleration_enabled(_enabled: bool) {}

#[cfg(not(target_os = "android"))]
pub(crate) fn set_ai_acceleration_enabled(enabled: bool) {
    if enabled {
        failure::clear_quarantine();
    }
    if AI_ACCELERATION_ENABLED.swap(enabled, Ordering::AcqRel) != enabled {
        status::clear();
        calibraw_core::diagnostics::record(format!(
            "AI GPU acceleration {} in Settings",
            if enabled { "enabled" } else { "disabled" }
        ));
        // Switching GPU AI back on is the way to retry models that ran out
        // of memory now.
        out_of_memory::forget_all();
        PROVIDER_GENERATION.fetch_add(1, Ordering::AcqRel);
        crate::slot::unload_cold_session();
    }
}

/// Whether GPU AI is on in Settings; always on Android.
fn acceleration_setting() -> bool {
    #[cfg(not(target_os = "android"))]
    {
        AI_ACCELERATION_ENABLED.load(Ordering::Acquire)
    }
    #[cfg(target_os = "android")]
    {
        true
    }
}

/// Whether new sessions may use a GPU provider: GPU AI is on and was not
/// switched off after a lost GPU.
pub fn ai_acceleration_enabled() -> bool {
    acceleration_settings().usable()
}

/// What `place` reads besides the session and the providers.
#[derive(Clone, Copy, Debug)]
struct AccelerationSettings {
    enabled: bool,
    quarantined: bool,
    gpu_build: bool,
}

impl AccelerationSettings {
    fn usable(self) -> bool {
        self.enabled && !self.quarantined
    }

    fn with_providers(
        self,
        providers: &[ProviderOffer],
        out_of_memory: Option<OutOfMemoryRecord>,
    ) -> AccelerationState<'_> {
        AccelerationState {
            enabled: self.enabled,
            quarantined: self.quarantined,
            gpu_build: self.gpu_build,
            providers,
            out_of_memory,
        }
    }
}

fn acceleration_settings() -> AccelerationSettings {
    AccelerationSettings {
        enabled: acceleration_setting(),
        quarantined: failure::quarantined(),
        gpu_build: !(crate::config::runtime_source() == RuntimeSource::Automatic
            && crate::runtime::automatic_runtime_lacks_gpu_build()),
    }
}

/// The GPU the editor renders with, once the app has set it.
static EDITOR_GPU: OnceLock<GpuIdentity> = OnceLock::new();

fn editor_gpu() -> Option<&'static GpuIdentity> {
    EDITOR_GPU.get()
}

/// Places sessions on a provider that runs on `gpu` by its free memory.
/// Records the memory it has free in diagnostics.
pub(crate) fn set_editor_gpu(gpu: GpuIdentity) {
    let identity = format!(
        "{} {:04x}:{:04x} ({:?})",
        gpu.name(),
        gpu.vendor_id(),
        gpu.device_id(),
        gpu.backend()
    );
    calibraw_core::diagnostics::record(match gpu.memory() {
        Some(memory) => format!(
            "editor GPU {identity}: {} MiB of {} MiB budget free",
            memory.available() >> 20,
            memory.budget >> 20
        ),
        None => format!("editor GPU {identity}: free memory not reported"),
    });
    if EDITOR_GPU.set(gpu).is_err() {
        log::warn!("the editor GPU was already set");
    }
}
