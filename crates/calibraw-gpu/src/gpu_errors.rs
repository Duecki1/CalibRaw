use anyhow::{anyhow, Result};
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
        log::error!("the GPU device was lost: {message}");
        calibraw_core::diagnostics::record(format!("GPU device lost: {message}"));
        *DEVICE_LOST
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(message);
    });
}

/// The driver's message if the device was lost since the last call.
pub fn take_gpu_device_lost() -> Option<String> {
    DEVICE_LOST
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
}

fn record_gpu_error(error: &wgpu::Error, context: &str) {
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
    log::error!("{message}");
    calibraw_core::diagnostics::record(message);
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
        let error = out_of_memory.or(internal).or(validation);
        if let Some(error) = error {
            record_gpu_error(&error, context);
            return Err(anyhow!("{context} failed: {error}"));
        }
        Ok(())
    }
}
