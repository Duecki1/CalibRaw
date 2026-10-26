//! GPU failures during a session: classifying an execution provider error,
//! counting the WebGPU errors the provider only logs, what a session does
//! after either, and switching GPU AI off after a lost device, which the CPU
//! fallback must not paper over.

use super::place::CpuReason;
use anyhow::Result;
#[cfg(not(target_os = "android"))]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(not(target_os = "android"))]
static AI_GPU_QUARANTINED: AtomicBool = AtomicBool::new(false);
/// A device loss that switched GPU AI off and that the app was not told of.
#[cfg(not(target_os = "android"))]
static AI_GPU_DEVICE_LOST: AtomicBool = AtomicBool::new(false);

/// The WebGPU errors ONNX Runtime logged so far, by `LoggedGpuError`.
static LOGGED_OUT_OF_MEMORY: AtomicU64 = AtomicU64::new(0);
static LOGGED_OTHER_GPU_ERRORS: AtomicU64 = AtomicU64::new(0);

/// A GPU failure an execution provider reported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AiGpuFailure {
    OutOfMemory,
    /// The GPU was reset or removed, typically by Windows' driver timeout,
    /// or WebGPU lost its device.
    DeviceLost,
}

impl AiGpuFailure {
    /// Classifies an execution provider error, or `None` for an ordinary one.
    fn of_error(error: &anyhow::Error) -> Option<Self> {
        let message = format!("{error:#}").to_ascii_lowercase();
        if is_gpu_memory_failure(&message) {
            Some(Self::OutOfMemory)
        } else if is_gpu_device_lost(&message) {
            Some(Self::DeviceLost)
        } else {
            None
        }
    }
}

/// A WebGPU error the provider logged instead of returning it.
///
/// Plugin 0.4.0 checks a run for validation errors only. Any other WebGPU
/// error, running out of memory among them, reaches ONNX Runtime's log alone
/// (`SetUncapturedErrorCallback` in `webgpu_context.cc`), and the creation or
/// run that caused it returns success with whatever its buffers hold, or
/// fails on a validation error that names no memory. The runtime's logger
/// counts these errors (`count_logged_gpu_error`), and a GPU session checks
/// after it was created and after each run whether more were counted than
/// before its creation (`LoggedGpuErrors`). Sessions are created and run one
/// at a time (`slot`), so what was counted meanwhile is theirs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LoggedGpuError {
    OutOfMemory,
    /// A validation, internal or unknown WebGPU error.
    Other,
}

impl LoggedGpuError {
    /// The WebGPU error `message`, a line ONNX Runtime logged, reports, if
    /// any: the plugin logs them as `WebGPU device error(<type>): <message>`.
    fn of_logged_message(message: &str) -> Option<Self> {
        let message = message.to_ascii_lowercase();
        if !message.contains("webgpu device error(") {
            return None;
        }
        Some(if is_gpu_memory_failure(&message) {
            Self::OutOfMemory
        } else {
            Self::Other
        })
    }

    /// The failure of `operation`, which returned success although the
    /// provider logged this error.
    fn error(self, operation: &str) -> anyhow::Error {
        match self {
            Self::OutOfMemory => {
                anyhow::anyhow!("{operation} returned, but the GPU ran out of memory meanwhile")
            }
            Self::Other => {
                anyhow::anyhow!("{operation} returned, but WebGPU reported an error meanwhile")
            }
        }
    }
}

/// Counts `message`, an error ONNX Runtime logged, if it reports a WebGPU
/// error, and returns whether it does.
pub(crate) fn count_logged_gpu_error(message: &str) -> bool {
    let count = match LoggedGpuError::of_logged_message(message) {
        Some(LoggedGpuError::OutOfMemory) => &LOGGED_OUT_OF_MEMORY,
        Some(LoggedGpuError::Other) => &LOGGED_OTHER_GPU_ERRORS,
        None => return false,
    };
    count.fetch_add(1, Ordering::AcqRel);
    true
}

/// The WebGPU errors ONNX Runtime had logged at one point, to tell whether
/// the provider logged one since.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct LoggedGpuErrors {
    out_of_memory: u64,
    other: u64,
}

impl LoggedGpuErrors {
    pub(super) fn now() -> Self {
        Self {
            out_of_memory: LOGGED_OUT_OF_MEMORY.load(Ordering::Acquire),
            other: LOGGED_OTHER_GPU_ERRORS.load(Ordering::Acquire),
        }
    }

    /// The error logged since these counts were taken, running out of memory
    /// before any other.
    pub(super) fn since(self) -> Option<LoggedGpuError> {
        Self::now().logged_after(self)
    }

    fn logged_after(self, earlier: Self) -> Option<LoggedGpuError> {
        if self.out_of_memory != earlier.out_of_memory {
            Some(LoggedGpuError::OutOfMemory)
        } else if self.other != earlier.other {
            Some(LoggedGpuError::Other)
        } else {
            None
        }
    }
}

/// A GPU session creation or run that failed, or that returned success while
/// the provider logged a WebGPU error.
#[derive(Debug)]
pub(super) struct GpuFailure {
    error: anyhow::Error,
    logged: Option<LoggedGpuError>,
}

impl GpuFailure {
    /// What `operation` returned as `result` comes to, given the WebGPU error
    /// the provider logged meanwhile: its value only if it succeeded and
    /// nothing was logged. A value despite a logged error is dropped, since
    /// the buffers it was read from may never have been written.
    pub(super) fn check<T>(
        operation: &str,
        result: Result<T>,
        logged: Option<LoggedGpuError>,
    ) -> std::result::Result<T, Self> {
        match (result, logged) {
            (Ok(value), None) => Ok(value),
            (Ok(_), Some(logged_error)) => Err(Self {
                error: logged_error.error(operation),
                logged,
            }),
            (Err(error), logged) => Err(Self { error, logged }),
        }
    }

    pub(super) fn error(&self) -> &anyhow::Error {
        &self.error
    }

    pub(super) fn into_error(self) -> anyhow::Error {
        self.error
    }

    /// The returned error's class, or else running out of memory if that was
    /// logged: it explains an ordinary failure, typically a validation error
    /// on a buffer that could not be allocated, but not a lost device.
    fn class(&self) -> Option<AiGpuFailure> {
        AiGpuFailure::of_error(&self.error).or(match self.logged {
            Some(LoggedGpuError::OutOfMemory) => Some(AiGpuFailure::OutOfMemory),
            Some(LoggedGpuError::Other) | None => None,
        })
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
    pub(super) fn of(failure: &GpuFailure) -> Self {
        match failure.class() {
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
    pub(super) fn of(failure: &GpuFailure) -> Self {
        match failure.class() {
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

/// Whether `message`, in lowercase, reports running out of GPU memory.
///
/// ONNX Runtime's WebGPU provider writes a WebGPU error as `WebGPU device
/// error(<type>): <message>`; type 3 is `WGPUErrorType_OutOfMemory`. Plugin
/// 0.4.0 only logs it (`LoggedGpuError`); the 0.5 branch returns it as the
/// run's error. Dawn's messages name the backend's own code: Vulkan's
/// `VK_ERROR_OUT_OF_DEVICE_MEMORY`, Direct3D 12's `E_OUTOFMEMORY` text and
/// Metal's "Insufficient Memory". Host memory failures in the CPU nodes of a
/// GPU session name no GPU, so they are ordinary failures.
fn is_gpu_memory_failure(message: &str) -> bool {
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
            && ["gpu", "vulkan", "d3d12", "metal"]
                .iter()
                .any(|marker| message.contains(marker)))
}

/// Whether `message`, in lowercase, reports a lost WebGPU device (Dawn names
/// the object in brackets: `[Device] is lost.`), Vulkan's
/// `VK_ERROR_DEVICE_LOST`, or DXGI's device-removed, -hung and -reset codes
/// and their messages, which Direct3D 12 reports after a driver reset.
fn is_gpu_device_lost(message: &str) -> bool {
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
