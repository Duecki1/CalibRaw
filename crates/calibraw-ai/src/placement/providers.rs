//! The GPU providers a session may try, in order, before the CPU, and what is
//! known about the adapter each runs on.

use super::place::{GpuProvider, ProviderOffer};
use super::quirks;
use super::session::{CpuFallbackProfile, SessionOptions};
use crate::config::{GpuIdentity, MemoryClass, CAPABILITIES};
use anyhow::{Context, Result};
use ort::device::Device;
use ort::environment::Environment;
use ort::ep::ExecutionProviderDispatch;
use ort::session::builder::SessionBuilder;
use ort::AsPointer;

/// This platform's GPU providers, most preferred first. Android's built-in
/// runtime has none (docs/GPU_REWORK.md P6.1): its NNAPI ran on Android's
/// CPU reference code where no vendor driver exists, slower than ONNX
/// Runtime's own CPU kernels, and its prebuilt binary has no XNNPACK.
#[cfg(not(target_os = "android"))]
const PLATFORM_PROVIDERS: &[GpuProvider] = &[GpuProvider::WebGpu];
#[cfg(target_os = "android")]
const PLATFORM_PROVIDERS: &[GpuProvider] = &[];

/// The name ONNX Runtime gives the WebGPU plugin's execution provider.
const WEBGPU_EP: &str = "WebGpuExecutionProvider";

/// PCI vendor and device ID of the hardware `device` stands for.
fn hardware_ids(device: &Device<'_>) -> (u32, u32) {
    let api = ort::api();
    // SAFETY: `device` is an entry of its environment's `GetEpDevices` list,
    // which the borrow keeps alive. The environment owns the entry and the
    // hardware device it points to until the plugin is unregistered, which
    // CalibRaw never does. Both getters only read them.
    let vendor_id = unsafe { (api.HardwareDevice_VendorId)((api.EpDevice_Device)(device.ptr())) };
    (vendor_id, device.id())
}

/// The WebGPU device sessions are created for, and whether it stands for the
/// editor's adapter `gpu`: of several, the editor's, else the first. `None`
/// when the runtime has no WebGPU provider or it found no adapter.
///
/// The plugin picks its physical adapter itself, by `powerPreference`
/// (`webgpu_options`). The editor asks wgpu for the high-performance
/// adapter too, so both normally land on the same GPU.
fn webgpu_device<'e>(
    environment: &'e Environment,
    gpu: Option<&GpuIdentity>,
) -> Option<(Device<'e>, bool)> {
    let on_editor_adapter = |device: &Device<'_>| {
        gpu.is_some_and(|gpu| {
            // Apple GPUs have no PCI IDs, and Apple Silicon Macs one GPU.
            gpu.backend() == wgpu::Backend::Metal
                || hardware_ids(device) == (gpu.vendor_id(), gpu.device_id())
        })
    };
    let mut first = None;
    for device in environment
        .devices()
        .filter(|device| device.ep().is_ok_and(|name| name == WEBGPU_EP))
    {
        if on_editor_adapter(&device) {
            return Some((device, true));
        }
        first.get_or_insert(device);
    }
    first.map(|device| (device, false))
}

/// Whether the loaded runtime offers WebGPU, and if so whether it runs on
/// the editor's adapter `gpu`.
fn webgpu_offered(gpu: Option<&GpuIdentity>) -> Option<bool> {
    let environment = match ort::environment::current() {
        Ok(environment) => environment,
        Err(error) => {
            log::warn!("AI execution provider WebGPU skipped: {error}");
            return None;
        }
    };
    let offered = webgpu_device(&environment, gpu).map(|(_, on_editor_adapter)| on_editor_adapter);
    if offered.is_none() {
        log::info!("the selected ONNX Runtime offers no WebGPU device");
    }
    offered
}

/// The GPU providers the loaded runtime offers, most preferred first, with
/// the free memory of `gpu`, the editor's adapter, for those that run on it.
pub(super) fn offered_providers(gpu: Option<&GpuIdentity>) -> Vec<ProviderOffer> {
    PLATFORM_PROVIDERS
        .iter()
        .filter_map(|&provider| {
            let on_editor_adapter = match provider {
                GpuProvider::WebGpu => webgpu_offered(gpu)?,
            };
            let editor_gpu = gpu.filter(|_| on_editor_adapter);
            Some(ProviderOffer {
                provider,
                free_memory: editor_gpu
                    .and_then(GpuIdentity::memory)
                    .map(|memory| memory.available()),
                adapter: editor_gpu.map(|gpu| gpu.name().to_owned()),
            })
        })
        .collect()
}

/// Options for the WebGPU provider, prefixed with its name as `with_devices`
/// expects:
/// - `powerPreference`: the adapter the editor's eframe device asks for.
/// - `storageBufferCacheMode=bucket`: keep released storage buffers in size
///   buckets for the next run, which suits inputs of fixed shape.
/// - `forceCpuNodeNames`: the session's `quirks::WebGpuCpuNodes`.
fn webgpu_options(options: &SessionOptions) -> Vec<(String, String)> {
    let mut entries = vec![
        ("powerPreference", "high-performance".to_owned()),
        ("storageBufferCacheMode", "bucket".to_owned()),
    ];
    if let Some(names) = options.webgpu_cpu_nodes.node_names() {
        entries.push(("forceCpuNodeNames", names));
    }
    entries
        .into_iter()
        .map(|(key, value)| (format!("{WEBGPU_EP}.{key}"), value))
        .collect()
}

/// Adds `offer`'s provider to `builder`, then the CPU for the nodes it does
/// not run.
pub(super) fn with_gpu_provider(
    builder: SessionBuilder,
    offer: &ProviderOffer,
    options: &SessionOptions,
    gpu: Option<&GpuIdentity>,
) -> Result<SessionBuilder> {
    let name = offer.provider.name();
    let cpu = cpu_provider(options);
    let builder = match offer.provider {
        GpuProvider::WebGpu => {
            let environment = ort::environment::current()
                .map_err(|error| anyhow::anyhow!("get the ONNX Runtime environment: {error}"))?;
            let (device, _) =
                webgpu_device(&environment, gpu).context("the WebGPU provider has no device")?;
            builder
                .with_devices([device], Some(&webgpu_options(options)))
                .map_err(|error| anyhow::anyhow!("configure {name} execution provider: {error}"))?
                .with_execution_providers([cpu])
        }
    };
    builder.map_err(|error| anyhow::anyhow!("configure {name} execution provider: {error}"))
}

pub(super) fn cpu_provider(options: &SessionOptions) -> ExecutionProviderDispatch {
    let disable_arena = CAPABILITIES.memory == MemoryClass::Constrained
        || (options.cpu_fallback_profile == CpuFallbackProfile::SamEncoder
            && quirks::sam_encoder_cpu_needs_unfused_graph());
    ort::ep::CPU::default()
        .with_arena_allocator(!disable_arena)
        .build()
}
