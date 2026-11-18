use anyhow::{anyhow, Result};
use std::fmt;
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

struct OomSignal(AtomicBool);

impl OomSignal {
    const fn new() -> Self {
        Self(AtomicBool::new(false))
    }

    fn raise(&self) {
        self.0.store(true, Ordering::Release);
    }

    fn take(&self) -> bool {
        self.0.swap(false, Ordering::AcqRel)
    }
}

static GPU_OUT_OF_MEMORY: OomSignal = OomSignal::new();
/// The driver's message for an unreported loss of the editor's device.
static DEVICE_LOST: Mutex<Option<String>> = Mutex::new(None);
/// Whether the editor's device was ever lost. Unlike [`DEVICE_LOST`] it is
/// never taken, so a worker's poll still sees it after the UI reported the loss.
static DEVICE_WAS_LOST: AtomicBool = AtomicBool::new(false);

pub fn install_uncaptured_gpu_error_handler(device: &wgpu::Device) {
    device.on_uncaptured_error(Arc::new(|error| {
        record_gpu_error(&error, "uncaptured GPU operation");
    }));
}

pub fn take_gpu_out_of_memory() -> bool {
    GPU_OUT_OF_MEMORY.take()
}

/// Records the loss of `device`, typically a driver reset, for
/// `take_gpu_device_lost`. Every later use of the device fails, including
/// drawing the UI. Losses CalibRaw causes by destroying a device are ignored.
pub fn install_gpu_device_lost_handler(device: &wgpu::Device) {
    device.set_device_lost_callback(|reason, message| {
        if reason == wgpu::DeviceLostReason::Destroyed {
            return;
        }
        report_device_failure(format!("{message} (reason={reason:?})"));
    });
}

/// Stops further submissions and wakes the existing recovery path after a
/// driver loss or a required fence that the device could not satisfy.
pub(crate) fn report_device_failure(mut detail: String) {
    // Workers must stop polling before any logging or disk flush can wait.
    // Keep the first failure: later errors on this device are consequences.
    if DEVICE_WAS_LOST.swap(true, Ordering::AcqRel) {
        return;
    }
    if let Some(errors) = calibraw_core::diagnostics::recent_gpu_errors() {
        detail.push_str(&format!("\nRecent GPU errors:\n{errors}"));
    }
    log::error!("GPU processing stopped: {detail}");
    calibraw_core::diagnostics::record(format!("GPU device lost: {detail}"));
    if let Some(work) = crate::pipeline::in_flight_summary() {
        log::error!("GPU work in flight when the device was lost: {work}");
        calibraw_core::diagnostics::record(format!(
            "GPU work in flight when the device was lost: {work}"
        ));
        detail.push_str(&format!("\nGPU work in flight: {work}"));
    }
    // These are OS memory counters, never queries on the lost GPU device.
    calibraw_core::diagnostics::record(format!(
        "Memory at GPU loss: system_total_bytes={:?} system_available_bytes={:?}",
        calibraw_core::system_memory::total_memory_bytes(),
        calibraw_core::system_memory::available_memory_bytes(),
    ));
    *DEVICE_LOST
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(detail);
    // Everything above is queued before the UI can begin its recovery.
    calibraw_core::diagnostics::flush();
}

/// The driver's message if the device was lost since the last call.
pub fn take_gpu_device_lost() -> Option<String> {
    DEVICE_LOST
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
}

/// Whether the editor's device entered terminal recovery. Workers must not
/// submit more work after this, even after the UI has taken its message.
pub(crate) fn device_was_lost() -> bool {
    DEVICE_WAS_LOST.load(Ordering::Acquire)
}

/// Why [`poll_device`] failed.
#[derive(Debug)]
pub enum PollFailure {
    /// wgpu reported the failure, such as a timeout.
    Poll(wgpu::PollError),
    /// The device was lost, usually a driver reset.
    DeviceLost,
}

impl fmt::Display for PollFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Poll(error) => error.fmt(formatter),
            Self::DeviceLost => formatter.write_str("the GPU device was lost"),
        }
    }
}

impl std::error::Error for PollFailure {}

/// `device.poll(poll_type)`, with the loss of a device that has the
/// [`install_gpu_device_lost_handler`] as an error. wgpu 29 returns only
/// timeouts from `Device::poll` and panics on every other failure, a lost
/// device included. It runs the device-lost callback before that panic, so a
/// panic after the callback is the loss; any other panic is passed on.
pub fn poll_device(
    device: &wgpu::Device,
    poll_type: wgpu::PollType,
) -> Result<wgpu::PollStatus, PollFailure> {
    if device_was_lost() {
        return Err(PollFailure::DeviceLost);
    }
    // Unwind safety: the panic is raised by wgpu's front end after wgpu-core
    // returned the error, with no lock held, and the device is not used again.
    match catch_unwind(AssertUnwindSafe(|| device.poll(poll_type))) {
        Ok(result) => result.map_err(PollFailure::Poll),
        Err(_) if device_was_lost() => Err(PollFailure::DeviceLost),
        Err(panic) => resume_unwind(panic),
    }
}

fn record_gpu_error(error: &wgpu::Error, context: &str) {
    use std::error::Error as _;
    let message = match error {
        wgpu::Error::OutOfMemory { .. } => {
            GPU_OUT_OF_MEMORY.raise();
            format!("{context} ran out of GPU memory; the operation was cancelled")
        }
        wgpu::Error::Validation { description, .. } => {
            format!("{context} failed GPU validation: {description}")
        }
        wgpu::Error::Internal { description, .. } => {
            format!("{context} hit an internal GPU error: {description}")
        }
    };
    let mut detail = message;
    let mut source = error.source();
    while let Some(cause) = source {
        detail.push_str(&format!("; caused by: {cause}"));
        source = cause.source();
    }
    log::error!("{detail}");
    calibraw_core::diagnostics::record(detail);
}

pub(crate) struct GpuErrorScopes {
    validation: wgpu::ErrorScopeGuard,
    internal: wgpu::ErrorScopeGuard,
    out_of_memory: wgpu::ErrorScopeGuard,
}

impl GpuErrorScopes {
    pub(crate) fn push(device: &wgpu::Device) -> Self {
        let out_of_memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let internal = device.push_error_scope(wgpu::ErrorFilter::Internal);
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        Self {
            validation,
            internal,
            out_of_memory,
        }
    }

    pub(crate) fn finish(self, context: &'static str) -> Result<()> {
        let validation = pollster::block_on(self.validation.pop());
        let internal = pollster::block_on(self.internal.pop());
        let out_of_memory = pollster::block_on(self.out_of_memory.pop());
        // Preserve all failures even though the caller receives just the most
        // severe one. An allocation failure can otherwise hide validation.
        for error in [&out_of_memory, &internal, &validation]
            .into_iter()
            .flatten()
        {
            record_gpu_error(error, context);
        }
        let error = out_of_memory.or(internal).or(validation);
        if let Some(error) = error {
            return Err(anyhow!("{context} failed: {error}"));
        }
        Ok(())
    }
}
