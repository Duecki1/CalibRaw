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
        wgpu::Limits::default()
    }
}

/// What a headless device is for.
#[derive(Clone, Copy, Debug)]
pub struct HeadlessDeviceRequest<'a> {
    pub label: &'a str,
    /// Preferred adapter; a low-power software fallback adapter is tried next.
    pub power_preference: wgpu::PowerPreference,
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
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: request.power_preference,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .or_else(|_| {
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: true,
        }))
    })
    .map_err(HeadlessDeviceError::NoAdapter)?;
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
