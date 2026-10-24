//! wgpu devices for processing without a window, and the device limits every
//! CalibRaw device starts from.

use std::fmt;

/// Limits CalibRaw's compute pipelines need on `adapter`, before callers raise
/// `max_texture_dimension_2d` to what their work requires.
///
/// GL adapters request the limits the driver reports. `downlevel_webgl2_defaults()`
/// sets every compute and storage-texture limit to 0, which rejects compute
/// pipelines with storage textures even where the driver supports them (Mesa Iris
/// on Haswell reports OpenGL 4.6). wgpu accepts a request equal to the adapter's own
/// limits, and that is a superset of the WebGL2 request.
pub fn base_device_limits(adapter: &wgpu::Adapter) -> wgpu::Limits {
    if adapter.get_info().backend == wgpu::Backend::Gl {
        adapter.limits()
    } else {
        let mut limits = wgpu::Limits::default();
        // wgpu's Metal backend reports 15 on iOS (60 varying components / 4),
        // below the default of 16, so the default request would fail there.
        // Other adapters report at least 16 and keep the default.
        limits.max_inter_stage_shader_variables = limits
            .max_inter_stage_shader_variables
            .min(adapter.limits().max_inter_stage_shader_variables);
        limits
    }
}

/// Backends CalibRaw processes photos on: `WGPU_BACKEND`, or else Vulkan, Metal
/// and DX12, never GL.
///
/// wgpu's GL backend runs every call under one context lock that panics when a
/// caller waits longer than 6 s (1 s with WGL), and it compiles shaders while
/// holding that lock. Compiling CalibRaw's pipelines on a slow GPU takes longer, so
/// a background compile on a GL device brings down any thread that touches the
/// device meanwhile. Machines whose only hardware driver is GL (Intel Haswell on
/// Linux, where wgpu hides the non-conformant hasvk Vulkan driver) process on a
/// software adapter such as lavapipe or WARP instead.
pub fn processing_backends() -> wgpu::Backends {
    wgpu::Backends::from_env().unwrap_or(wgpu::Backends::PRIMARY) - wgpu::Backends::GL
}

/// Which adapter a headless device runs on.
#[derive(Clone, Copy, Debug)]
pub enum HeadlessAdapter {
    /// The adapter wgpu picks for this power preference.
    Preferred(wgpu::PowerPreference),
    /// The preferred adapter, or else wgpu's software fallback adapter.
    PreferredOrSoftware(wgpu::PowerPreference),
    /// Only wgpu's software fallback adapter, so results do not depend on the
    /// machine's GPU.
    Software,
}

/// What a headless device is for.
#[derive(Clone, Copy, Debug)]
pub struct HeadlessDeviceRequest<'a> {
    pub label: &'a str,
    pub adapter: HeadlessAdapter,
    /// The largest 2D texture edge the work allocates.
    pub texture_dimension: u32,
}

pub struct HeadlessDevice {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub adapter_info: wgpu::AdapterInfo,
}

#[derive(Debug)]
pub enum HeadlessDeviceError {
    NoAdapter(wgpu::RequestAdapterError),
    /// The adapter's `max_texture_dimension_2d` is below the request.
    TextureTooLarge {
        required: u32,
        supported: u32,
        adapter: String,
    },
    Device(wgpu::RequestDeviceError),
}

impl fmt::Display for HeadlessDeviceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoAdapter(error) => write!(formatter, "no usable GPU adapter: {error}"),
            Self::TextureTooLarge {
                required,
                supported,
                adapter,
            } => write!(
                formatter,
                "a {required}-pixel texture is required, but adapter {adapter:?} supports {supported}"
            ),
            Self::Device(error) => write!(formatter, "could not create the GPU device: {error}"),
        }
    }
}

impl std::error::Error for HeadlessDeviceError {}

/// Creates a device and queue for compute work without a surface, with the
/// 2D texture limit raised to `request.texture_dimension`.
pub fn request_headless_device(
    request: &HeadlessDeviceRequest<'_>,
) -> Result<HeadlessDevice, HeadlessDeviceError> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: processing_backends(),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter =
        request_adapter(&instance, request.adapter).map_err(HeadlessDeviceError::NoAdapter)?;
    let adapter_info = adapter.get_info();
    let supported = adapter.limits().max_texture_dimension_2d;
    if request.texture_dimension > supported {
        return Err(HeadlessDeviceError::TextureTooLarge {
            required: request.texture_dimension,
            supported,
            adapter: adapter_info.name,
        });
    }
    let mut required_limits = base_device_limits(&adapter);
    required_limits.max_texture_dimension_2d = request.texture_dimension;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some(request.label),
        required_limits,
        ..Default::default()
    }))
    .map_err(HeadlessDeviceError::Device)?;
    Ok(HeadlessDevice {
        device,
        queue,
        adapter_info,
    })
}

fn request_adapter(
    instance: &wgpu::Instance,
    choice: HeadlessAdapter,
) -> Result<wgpu::Adapter, wgpu::RequestAdapterError> {
    let request = |power_preference, force_fallback_adapter| {
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference,
            compatible_surface: None,
            force_fallback_adapter,
        }))
    };
    let software = || request(wgpu::PowerPreference::LowPower, true);
    match choice {
        HeadlessAdapter::Preferred(power_preference) => request(power_preference, false),
        HeadlessAdapter::PreferredOrSoftware(power_preference) => {
            request(power_preference, false).or_else(|_| software())
        }
        HeadlessAdapter::Software => software(),
    }
}
