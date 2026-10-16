//! GPU failures during a session: classifying an execution provider error,
//! what a session does after one, and switching GPU AI off after a lost
//! device, which the CPU fallback must not paper over.

use super::place::CpuReason;
#[cfg(not(target_os = "android"))]
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(not(target_os = "android"))]
static AI_GPU_QUARANTINED: AtomicBool = AtomicBool::new(false);
/// A device loss that switched GPU AI off and that the app was not told of.
#[cfg(not(target_os = "android"))]
static AI_GPU_DEVICE_LOST: AtomicBool = AtomicBool::new(false);

/// A GPU failure an execution provider reported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AiGpuFailure {
    OutOfMemory,
    /// The GPU was reset or removed, typically by Windows' driver timeout,
    /// or WebGPU lost its device.
    DeviceLost,
}

impl AiGpuFailure {
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

/// What a session does after its GPU provider failed to create it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SetupFailure {
    /// Try the next provider, or the CPU after the last.
    NextProvider,
    /// Use the CPU for this job: another provider would compete for the same
    /// memory.
    Cpu(CpuReason),
}

impl SetupFailure {
    pub(super) fn of_error(error: &anyhow::Error) -> Self {
        match AiGpuFailure::of_error(error) {
            Some(AiGpuFailure::OutOfMemory) => Self::Cpu(CpuReason::OutOfMemory),
            Some(AiGpuFailure::DeviceLost) | None => Self::NextProvider,
        }
    }
}

/// What a session does after its GPU provider failed to run it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RunFailure {
    /// Switch GPU AI off and stop the job: the device is gone, and the app
    /// restarts or must hear of it.
    StopJob,
    /// Rebuild the session on the CPU and run the job again there.
    RerunOnCpu(CpuReason),
}

impl RunFailure {
    pub(super) fn of_error(error: &anyhow::Error) -> Self {
        match AiGpuFailure::of_error(error) {
            Some(AiGpuFailure::DeviceLost) => Self::StopJob,
            Some(AiGpuFailure::OutOfMemory) => Self::RerunOnCpu(CpuReason::OutOfMemory),
            None => Self::RerunOnCpu(CpuReason::GpuFailed),
        }
    }
}

#[cfg(not(target_os = "android"))]
pub(super) fn quarantined() -> bool {
    AI_GPU_QUARANTINED.load(Ordering::Acquire)
}

/// Android has no switch-off.
#[cfg(target_os = "android")]
pub(super) fn quarantined() -> bool {
    false
}

#[cfg(not(target_os = "android"))]
pub(super) fn clear_quarantine() {
    AI_GPU_QUARANTINED.store(false, Ordering::Release);
    AI_GPU_DEVICE_LOST.store(false, Ordering::Release);
}

/// Whether the GPU was lost during an AI job, switching GPU AI off, since
/// the last call.
#[cfg(not(target_os = "android"))]
pub fn take_ai_gpu_device_loss() -> bool {
    AI_GPU_DEVICE_LOST.swap(false, Ordering::AcqRel)
}

#[cfg(target_os = "android")]
pub fn take_ai_gpu_device_loss() -> bool {
    false
}

#[cfg(not(target_os = "android"))]
pub(super) fn quarantine_after_device_loss(operation: &str, provider: &str, error: &anyhow::Error) {
    AI_GPU_QUARANTINED.store(true, Ordering::Release);
    AI_GPU_DEVICE_LOST.store(true, Ordering::Release);
    let message = format!(
        "AI {operation} lost the GPU device on {provider}; cancelled the AI job and disabled GPU AI until it is explicitly re-enabled"
    );
    log::error!("{message}: {error:#}");
    calibraw_core::diagnostics::record(message);
}

#[cfg(target_os = "android")]
pub(super) fn quarantine_after_device_loss(
    _operation: &str,
    _provider: &str,
    _error: &anyhow::Error,
) {
}

/// ONNX Runtime's WebGPU provider reports a WebGPU error as `WebGPU device
/// error(<type>): <message>`; type 3 is `WGPUErrorType_OutOfMemory`. Dawn's
/// messages name the backend's own code: Vulkan's
/// `VK_ERROR_OUT_OF_DEVICE_MEMORY`, Direct3D 12's `E_OUTOFMEMORY` text and
/// Metal's "Insufficient Memory". Host memory failures in the CPU nodes of a
/// GPU session name no GPU, so they are ordinary failures.
fn is_gpu_memory_failure(error: &anyhow::Error) -> bool {
    let message = format!("{error:#}").to_ascii_lowercase();
    message.contains("webgpu device error(3)")
        || ([
            "out of memory",
            "out_of_device_memory",
            "failed to allocate memory",
            "not enough memory resources",
            "insufficient memory",
        ]
        .iter()
        .any(|marker| message.contains(marker))
            && ["gpu", "vulkan", "d3d12", "metal", "nnapi"]
                .iter()
                .any(|marker| message.contains(marker)))
}

/// A lost WebGPU device (Dawn names the object in brackets: `[Device] is
/// lost.`), Vulkan's `VK_ERROR_DEVICE_LOST`, and DXGI's device-removed,
/// -hung and -reset codes and their messages, which Direct3D 12 reports
/// after a driver reset.
fn is_gpu_device_lost(error: &anyhow::Error) -> bool {
    let message = format!("{error:#}").to_ascii_lowercase();
    [
        "device lost",
        "device is lost",
        "] is lost",
        "device was lost",
        "devicelost",
        "error_device_lost",
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
    use super::*;

    #[test]
    fn gpu_allocation_failures_are_recognised() {
        for message in [
            "Non-zero status code returned while running Conv node. WebGPU device error(3): Out of memory creating buffer",
            "WebGPU device error(3): Allocation failed",
            "Dawn: VK_ERROR_OUT_OF_DEVICE_MEMORY while allocating Vulkan memory",
            "D3D12 CreateCommittedResource failed: Not enough memory resources are available to complete this operation. (0x8007000E)",
            "Metal command buffer failed: Insufficient Memory (00000008:kIOGPUCommandBufferCallbackErrorOutOfMemory)",
        ] {
            assert!(is_gpu_memory_failure(&anyhow::anyhow!(message)), "{message}");
        }
        for message in [
            "invalid tensor shape",
            "WebGPU device error(2): Validation failed",
            "BFCArena::AllocateRawInternal failed to allocate memory for requested buffer",
        ] {
            assert!(
                !is_gpu_memory_failure(&anyhow::anyhow!(message)),
                "{message}"
            );
        }
    }

    #[test]
    fn lost_devices_are_recognised() {
        for message in [
            "Failed to download data from buffer. wgpu::MapAsyncStatus value: 3: Device is lost",
            "WebGPU device error(4): [Device] is lost.",
            "Dawn Vulkan: VK_ERROR_DEVICE_LOST",
            "D3D12 device removed: DXGI_ERROR_DEVICE_REMOVED (0x887A0005)",
            "DXGI_ERROR_DEVICE_HUNG 887A0006",
        ] {
            assert_eq!(
                AiGpuFailure::of_error(&anyhow::anyhow!(message)),
                Some(AiGpuFailure::DeviceLost),
                "{message}"
            );
        }
        assert_eq!(
            AiGpuFailure::of_error(&anyhow::anyhow!("WebGPU device error(3): Out of memory")),
            Some(AiGpuFailure::OutOfMemory)
        );
        assert_eq!(
            AiGpuFailure::of_error(&anyhow::anyhow!("invalid tensor shape")),
            None
        );
    }

    #[test]
    fn running_out_of_memory_moves_the_job_to_the_cpu() {
        let out_of_memory = anyhow::anyhow!("WebGPU device error(3): Out of memory");
        let removed = anyhow::anyhow!("DXGI_ERROR_DEVICE_REMOVED 887A0005");
        let other = anyhow::anyhow!("invalid tensor shape");

        assert_eq!(
            SetupFailure::of_error(&out_of_memory),
            SetupFailure::Cpu(CpuReason::OutOfMemory)
        );
        assert_eq!(SetupFailure::of_error(&removed), SetupFailure::NextProvider);
        assert_eq!(SetupFailure::of_error(&other), SetupFailure::NextProvider);

        assert_eq!(
            RunFailure::of_error(&out_of_memory),
            RunFailure::RerunOnCpu(CpuReason::OutOfMemory)
        );
        assert_eq!(RunFailure::of_error(&removed), RunFailure::StopJob);
        assert_eq!(
            RunFailure::of_error(&other),
            RunFailure::RerunOnCpu(CpuReason::GpuFailed)
        );
    }
}
