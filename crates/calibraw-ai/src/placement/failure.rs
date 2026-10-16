//! GPU failures during a session: classifying an execution provider error,
//! and switching GPU AI off after one that the CPU fallback must not paper
//! over.

#[cfg(not(target_os = "android"))]
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

#[cfg(not(target_os = "android"))]
static AI_GPU_QUARANTINED: AtomicBool = AtomicBool::new(false);
/// The unreported `AiGpuFailure`, encoded by `AiGpuFailure::code`; 0 for none.
#[cfg(not(target_os = "android"))]
static AI_GPU_FAILURE: AtomicU8 = AtomicU8::new(0);

/// Why GPU AI was switched off during a job.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiGpuFailure {
    OutOfMemory,
    /// The GPU was reset or removed, typically by Windows' driver timeout.
    DeviceLost,
}

impl AiGpuFailure {
    #[cfg(not(target_os = "android"))]
    const fn code(self) -> u8 {
        match self {
            Self::OutOfMemory => 1,
            Self::DeviceLost => 2,
        }
    }

    #[cfg(not(target_os = "android"))]
    const fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::OutOfMemory),
            2 => Some(Self::DeviceLost),
            _ => None,
        }
    }

    /// Classifies an execution provider error, or `None` for an ordinary one.
    pub(super) fn of_error(error: &anyhow::Error) -> Option<Self> {
        if is_gpu_memory_failure(error) {
            Some(Self::OutOfMemory)
        } else if is_gpu_device_lost(error) {
            Some(Self::DeviceLost)
        } else {
            None
        }
    }
}

#[cfg(not(target_os = "android"))]
pub(super) fn quarantined() -> bool {
    AI_GPU_QUARANTINED.load(Ordering::Acquire)
}

#[cfg(not(target_os = "android"))]
pub(super) fn clear_quarantine() {
    AI_GPU_QUARANTINED.store(false, Ordering::Release);
    AI_GPU_FAILURE.store(0, Ordering::Release);
}

/// The GPU failure that switched GPU AI off since the last call, if any.
#[cfg(not(target_os = "android"))]
pub fn take_ai_gpu_failure() -> Option<AiGpuFailure> {
    AiGpuFailure::from_code(AI_GPU_FAILURE.swap(0, Ordering::AcqRel))
}

#[cfg(target_os = "android")]
pub fn take_ai_gpu_failure() -> Option<AiGpuFailure> {
    None
}

#[cfg(not(target_os = "android"))]
pub(super) fn quarantine_ai_gpu(
    failure: AiGpuFailure,
    operation: &str,
    provider: &str,
    error: &anyhow::Error,
) {
    AI_GPU_QUARANTINED.store(true, Ordering::Release);
    AI_GPU_FAILURE.store(failure.code(), Ordering::Release);
    let cause = match failure {
        AiGpuFailure::OutOfMemory => "exhausted GPU memory",
        AiGpuFailure::DeviceLost => "lost the GPU device",
    };
    let message = format!(
        "AI {operation} {cause} on {provider}; cancelled the AI job and disabled GPU AI until it is explicitly re-enabled"
    );
    log::error!("{message}: {error:#}");
    calibraw_core::diagnostics::record(message);
}

#[cfg(target_os = "android")]
pub(super) fn quarantine_ai_gpu(
    _failure: AiGpuFailure,
    _operation: &str,
    _provider: &str,
    _error: &anyhow::Error,
) {
}

fn is_gpu_memory_failure(error: &anyhow::Error) -> bool {
    let message = format!("{error:#}").to_ascii_lowercase();
    (message.contains("out of memory")
        || message.contains("failed to allocate memory")
        || message.contains("not enough memory resources"))
        && (message.contains("cuda")
            || message.contains("directml")
            || message.contains("dml")
            || message.contains("tensorrt")
            || message.contains("rocm")
            || message.contains("gpu")
            || message.contains("bfc_arena"))
}

/// DXGI's device-removed, -hung and -reset codes, which DirectML reports after
/// a driver reset, and their messages.
fn is_gpu_device_lost(error: &anyhow::Error) -> bool {
    let message = format!("{error:#}").to_ascii_lowercase();
    [
        "887a0005",
        "887a0006",
        "887a0007",
        "device removed",
        "device hung",
    ]
    .iter()
    .any(|marker| message.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::{is_gpu_memory_failure, AiGpuFailure};

    #[test]
    fn gpu_allocation_failures_are_quarantined_instead_of_retried() {
        let error =
            anyhow::anyhow!("BFCArena::AllocateRawInternal failed to allocate memory on CUDA GPU");
        assert!(is_gpu_memory_failure(&error));
        assert!(is_gpu_memory_failure(&anyhow::anyhow!(
            r"providers\dml\DmlExecutionProvider\src\DmlCommandRecorder.cpp(372) 8007000E Not enough memory resources are available to complete this operation."
        )));
        assert!(!is_gpu_memory_failure(&anyhow::anyhow!(
            "invalid tensor shape"
        )));
    }

    #[test]
    fn directml_device_removal_is_a_device_loss() {
        let removed = anyhow::anyhow!(
            r"providers\dml\DmlExecutionProvider\src\DmlCommandRecorder.cpp(372) 887A0005 The GPU device instance has been suspended."
        );
        assert_eq!(
            AiGpuFailure::of_error(&removed),
            Some(AiGpuFailure::DeviceLost)
        );
        assert_eq!(
            AiGpuFailure::of_error(&anyhow::anyhow!("CUDA out of memory")),
            Some(AiGpuFailure::OutOfMemory)
        );
        assert_eq!(
            AiGpuFailure::of_error(&anyhow::anyhow!("invalid tensor shape")),
            None
        );
    }
}
