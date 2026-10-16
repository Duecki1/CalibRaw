//! Where sessions run: the execution providers tried for each session, the
//! CPU fallback, what happens when the GPU fails, and the provider each model
//! ended up on.

#[cfg(target_os = "windows")]
mod directml;
mod failure;
mod job_record;
mod providers;
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
use std::sync::atomic::{AtomicBool, Ordering};

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
        crate::slot::invalidate_for_provider_change();
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

/// Runs DirectML sessions on `gpu`; other providers pick their own device.
pub(crate) fn set_editor_gpu(gpu: GpuIdentity) {
    #[cfg(target_os = "windows")]
    directml::set_directml_adapter(gpu.vendor_id, gpu.device_id);
    #[cfg(not(target_os = "windows"))]
    let _ = gpu;
}
