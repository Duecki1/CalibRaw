//! Where sessions run: the execution providers tried for each session, the
//! CPU fallback, what happens when the GPU fails, and the provider each model
//! ended up on.

#[cfg(target_os = "windows")]
mod directml;
mod failure;
mod job_record;
mod providers;
pub(crate) mod quirks;
mod session;
mod status;

pub use failure::{take_ai_gpu_failure, AiGpuFailure};
pub use job_record::{end_gpu_job_records, gpu_job_in_progress};
pub(crate) use job_record::{set_gpu_job_record_path, GpuJobRecord};
pub(crate) use session::{
    create_session_with_fallback, CpuFallbackProfile, FallbackSession, SessionOptions,
};
pub use status::{active_execution_providers, ExecutionProviderStatus};

use crate::config::GpuIdentity;
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
        PROVIDER_GENERATION.fetch_add(1, Ordering::AcqRel);
        crate::slot::unload_cold_session();
    }
}

pub fn ai_acceleration_enabled() -> bool {
    #[cfg(not(target_os = "android"))]
    {
        AI_ACCELERATION_ENABLED.load(Ordering::Acquire) && !failure::quarantined()
    }
    #[cfg(target_os = "android")]
    {
        true
    }
}

/// The GPU the editor renders with, once the app has set it.
static EDITOR_GPU: OnceLock<GpuIdentity> = OnceLock::new();

/// Runs DirectML sessions on `gpu`; other providers pick their own device.
/// Records the memory it has free in diagnostics.
pub(crate) fn set_editor_gpu(gpu: GpuIdentity) {
    let identity = format!(
        "{:04x}:{:04x} ({:?})",
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
