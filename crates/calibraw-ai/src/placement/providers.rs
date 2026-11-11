//! The GPU providers a session may try, in order, before the CPU, and what is
//! known about the adapter each runs on.

use super::place::{GpuBuffers, GpuProvider, ProviderOffer};
use super::quirks;
use super::session::{CpuFallbackProfile, SessionOptions};
use crate::config::{constrained_cpu_sessions, GpuIdentity, MemoryClass, CAPABILITIES};
use anyhow::Result;
use ort::ep::ExecutionProviderDispatch;
use ort::session::builder::SessionBuilder;
#[cfg(not(target_os = "android"))]
use {
    anyhow::Context,
    ort::{device::Device, environment::Environment, AsPointer},
};

/// This platform's GPU providers, most preferred first. Android's runtime
/// has WebGPU built in (`android/app/src/main/cpp/cmake/OnnxRuntime.cmake`)
/// and no NNAPI: NNAPI ran on Android's CPU reference code where no vendor
/// driver exists, slower than ONNX Runtime's own CPU kernels.
const PLATFORM_PROVIDERS: &[GpuProvider] = &[GpuProvider::WebGpu];

/// The name ONNX Runtime gives the WebGPU plugin's execution provider.
#[cfg(not(target_os = "android"))]
const WEBGPU_EP: &str = "WebGpuExecutionProvider";

/// Whether the GPU shares the system's memory, so that sessions always
/// release their buffers (`ProviderOffer::shared_memory`): on Android.
const SHARED_GPU_MEMORY: bool = cfg!(target_os = "android");

/// PCI vendor and device ID of the hardware `device` stands for.
#[cfg(not(target_os = "android"))]
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
#[cfg(not(target_os = "android"))]
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
/// the editor's adapter `gpu`. Android's runtime has it built in, where ONNX
/// Runtime finds no GPU device (its device discovery there reports the CPU
/// only), and the phone or tablet has one GPU, the editor's.
#[cfg(target_os = "android")]
fn webgpu_offered(_gpu: Option<&GpuIdentity>) -> Option<bool> {
    use ort::ep::ExecutionProvider;
    match ort::ep::WebGPU::default().is_available() {
        Ok(true) => Some(true),
        Ok(false) => {
            log::info!("the built-in ONNX Runtime has no WebGPU provider");
            None
        }
        Err(error) => {
            log::warn!("AI execution provider WebGPU skipped: {error}");
            None
        }
    }
}

/// Whether the loaded runtime offers WebGPU, and if so whether it runs on
/// the editor's adapter `gpu`.
#[cfg(not(target_os = "android"))]
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
                shared_memory: SHARED_GPU_MEMORY,
            })
        })
        .collect()
}

/// Options for the WebGPU provider, prefixed with its name as `with_devices`
/// expects:
/// - `powerPreference`: the adapter the editor's eframe device asks for.
/// - `storageBufferCacheMode`: `bucket` keeps released storage buffers in
///   size buckets for reuse, which suits inputs of fixed shape; `disabled`
///   frees them (`GpuBuffers`).
/// - `forceCpuNodeNames`: the session's `quirks::WebGpuCpuNodes`.
#[cfg(not(target_os = "android"))]
fn webgpu_options(options: &SessionOptions, buffers: GpuBuffers) -> Vec<(String, String)> {
    let cache_mode = match buffers {
        GpuBuffers::Cached => "bucket",
        GpuBuffers::Released => "disabled",
    };
    let mut entries = vec![
        ("powerPreference", "high-performance".to_owned()),
        ("storageBufferCacheMode", cache_mode.to_owned()),
    ];
    if let Some(names) = options.webgpu_cpu_nodes.node_names() {
        entries.push(("forceCpuNodeNames", names));
    }
    entries
        .into_iter()
        .map(|(key, value)| (format!("{WEBGPU_EP}.{key}"), value))
        .collect()
}

/// The built-in WebGPU provider with the options the desktop plugin gets
/// (`webgpu_options`) but `powerPreference`, which a device with one GPU does
/// not need. It fails the session when it cannot be added rather than leaving
/// it to the CPU, so the failure is classified and recorded like a plugin's.
#[cfg(target_os = "android")]
fn builtin_webgpu(options: &SessionOptions, buffers: GpuBuffers) -> ExecutionProviderDispatch {
    use ort::ep::webgpu::BufferCacheMode;
    let mut provider = ort::ep::WebGPU::default().with_storage_buffer_cache_mode(match buffers {
        GpuBuffers::Cached => BufferCacheMode::Bucket,
        GpuBuffers::Released => BufferCacheMode::Disabled,
    });
    if let Some(names) = options.webgpu_cpu_nodes.node_names() {
        provider = provider.with_force_cpu_node_names(names);
    }
    provider.build().error_on_failure()
}

/// Adds `offer`'s provider to `builder`, handling released buffers as
/// `buffers` says, then the CPU for the nodes it does not run.
#[cfg_attr(
    target_os = "android",
    expect(
        unused_variables,
        reason = "Android devices have one GPU, the editor's"
    )
)]
pub(super) fn with_gpu_provider(
    builder: SessionBuilder,
    offer: &ProviderOffer,
    buffers: GpuBuffers,
    options: &SessionOptions,
    gpu: Option<&GpuIdentity>,
) -> Result<SessionBuilder> {
    let name = offer.provider.name();
    let cpu = cpu_provider(options);
    let builder = match offer.provider {
        #[cfg(target_os = "android")]
        GpuProvider::WebGpu => {
            builder.with_execution_providers([builtin_webgpu(options, buffers), cpu])
        }
        #[cfg(not(target_os = "android"))]
        GpuProvider::WebGpu => {
            let environment = ort::environment::current()
                .map_err(|error| anyhow::anyhow!("get the ONNX Runtime environment: {error}"))?;
            let (device, _) =
                webgpu_device(&environment, gpu).context("the WebGPU provider has no device")?;
            builder
                .with_devices([device], Some(&webgpu_options(options, buffers)))
                .map_err(|error| anyhow::anyhow!("configure {name} execution provider: {error}"))?
                .with_execution_providers([cpu])
        }
    };
    builder.map_err(|error| anyhow::anyhow!("configure {name} execution provider: {error}"))
}

pub(super) fn cpu_provider(options: &SessionOptions) -> ExecutionProviderDispatch {
    let disable_arena = (CAPABILITIES.memory == MemoryClass::Constrained
        && !constrained_cpu_sessions().arena)
        || (options.cpu_fallback_profile == CpuFallbackProfile::SamEncoder
            && quirks::sam_encoder_cpu_needs_unfused_graph());
    ort::ep::CPU::default()
        .with_arena_allocator(!disable_arena)
        .build()
}
