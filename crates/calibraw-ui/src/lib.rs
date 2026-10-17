pub(crate) mod appearance;

pub mod diagnostics {
    pub use calibraw_core::diagnostics::*;
}

pub mod file_ops {
    pub use calibraw_core::file_ops::*;
}

pub(crate) mod export_naming;
pub(crate) mod performance_settings;
pub(crate) mod platform;
pub(crate) use calibraw_core::presets;

pub mod pipeline {
    pub use calibraw_gpu::pipeline::*;
}

pub mod sidecar {
    pub use calibraw_core::sidecar::*;
}

mod app;
mod services;
mod ui;

pub use app::CalibRawApp;
pub use calibraw_core::SOURCE_REVISION;

/// Memory-budget policy for wgpu's `VK_EXT_memory_budget` resource-creation gate.
///
/// The gate compares the *driver-reported* heap usage/budget against a percentage
/// before every texture/buffer allocation. On desktop drivers (notably
/// NVIDIA/Vulkan on Linux) the reported numbers are unreliable: usage includes all
/// processes plus driver reservations, budgets understate physical VRAM, and wgpu
/// must check every device-local heap because the allocator cannot pin a target
/// heap. That makes the gate fail resource creation while gigabytes of VRAM are
/// physically free, so it is disabled on desktop; genuine physical OOM is still
/// surfaced by the driver itself.
///
/// Devices with constrained, usually unified, memory benefit from the guard
/// (memory-pressure kills there are expensive), so the default thresholds stay
/// enabled.
///
/// `CALIBRAW_GPU_MEMORY_BUDGET_THRESHOLD=<1-100>` overrides the desktop default for
/// testing; an invalid or empty value falls back to the platform default.
fn memory_budget_thresholds(memory: platform::MemoryClass) -> eframe::wgpu::MemoryBudgetThresholds {
    if let Ok(raw) = std::env::var("CALIBRAW_GPU_MEMORY_BUDGET_THRESHOLD") {
        if let Ok(percent) = raw.trim().parse::<u8>() {
            let percent = percent.clamp(1, 100);
            return eframe::wgpu::MemoryBudgetThresholds {
                for_resource_creation: Some(percent),
                for_device_loss: Some(percent.clamp(1, 97)),
            };
        }
    }
    if memory == platform::MemoryClass::Constrained {
        eframe::wgpu::MemoryBudgetThresholds {
            for_resource_creation: Some(70),
            for_device_loss: Some(97),
        }
    } else {
        eframe::wgpu::MemoryBudgetThresholds {
            for_resource_creation: None,
            for_device_loss: None,
        }
    }
}

/// Options for the editor window; each platform's entry point adds its own.
fn native_options(caps: platform::PlatformCaps) -> eframe::NativeOptions {
    let mut options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };

    if let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut options.wgpu_options.wgpu_setup {
        setup.instance_descriptor.memory_budget_thresholds = memory_budget_thresholds(caps.memory);
        setup.device_descriptor = std::sync::Arc::new(|adapter| {
            let info = adapter.get_info();
            let adapter_limits = adapter.limits();
            calibraw_core::diagnostics::set_gpu_info(format!(
                "name={}\nbackend={:?}\ndevice_type={:?}\nvendor=0x{:04x}\ndevice=0x{:04x}\ndriver={}\ndriver_info={}\nmax_texture_dimension_2d={}\nfeatures={:?}",
                info.name,
                info.backend,
                info.device_type,
                info.vendor,
                info.device,
                info.driver,
                info.driver_info,
                adapter_limits.max_texture_dimension_2d,
                adapter.features(),
            ));
            let mut required_limits = calibraw_gpu::base_device_limits(adapter);
            required_limits.max_texture_dimension_2d = adapter_limits.max_texture_dimension_2d;
            let required_features = adapter.features() & eframe::wgpu::Features::PIPELINE_CACHE;
            eframe::wgpu::DeviceDescriptor {
                label: Some("CalibRaw wgpu device"),
                required_features,
                required_limits,
                ..Default::default()
            }
        });
    }

    options
}

/// The desktop program's `main`: answers an ONNX Runtime probe when started
/// as one, otherwise runs the editor. Does nothing on Android, which starts
/// through `android_main`.
pub fn main() -> eframe::Result {
    platform::main()
}
