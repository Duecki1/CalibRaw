//! macOS: Metal reports the working set a device can hold with good
//! performance and what this process has allocated on it. Apple GPUs share
//! system memory, so the working set is a share of RAM.

use super::AdapterMemory;
use objc2_metal::MTLDevice;
use wgpu::hal::api::Metal;

pub(super) fn adapter_memory(device: &wgpu::Device) -> Option<AdapterMemory> {
    // SAFETY: only two read-only properties of the `MTLDevice` are read, and
    // nothing is created, destroyed or submitted through the HAL device. The
    // guard keeps it alive until the end of this function.
    let hal_device = unsafe { device.as_hal::<Metal>() }?;
    let raw = hal_device.raw_device();
    Some(AdapterMemory {
        budget: raw.recommendedMaxWorkingSetSize(),
        // `NSUInteger` is `usize`, at most 64 bits on every supported target.
        usage: raw.currentAllocatedSize() as u64,
    })
}
