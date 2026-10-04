pub use wgpu;

mod device;
pub use device::{
    base_device_limits, request_headless_device, HeadlessDevice, HeadlessDeviceError,
    HeadlessDeviceRequest,
};

mod gpu_errors;
pub use gpu_errors::{install_uncaptured_gpu_error_handler, take_gpu_out_of_memory};

pub mod pipeline;
