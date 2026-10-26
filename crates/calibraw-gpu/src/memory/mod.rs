//! How much GPU memory this process may still use on a device's adapter,
//! as the OS or driver reports it. Each backend has its own source; GL and
//! unknown backends report nothing.

#[cfg(target_os = "windows")]
mod dxgi;
#[cfg(target_os = "macos")]
mod metal;
#[cfg(any(target_os = "linux", target_os = "android"))]
mod vulkan;

/// GPU memory of one adapter, in bytes, for this process: dedicated video
/// memory on discrete GPUs, the share of system memory granted to the GPU on
/// integrated ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdapterMemory {
    /// What this process may use before the OS starts paging its GPU memory
    /// or allocations fail. Includes `usage`.
    pub budget: u64,
    /// What this process uses now.
    pub usage: u64,
}

impl AdapterMemory {
    /// What this process may still allocate: the budget minus current usage,
    /// or 0 once usage exceeds the budget.
    pub fn available(&self) -> u64 {
        self.budget.saturating_sub(self.usage)
    }
}

/// The memory of `device`'s adapter, or `None` where its backend does not
/// report it or the query failed.
///
/// - Windows: DXGI `QueryVideoMemoryInfo` (local segment) of the adapter with
///   the same PCI IDs, whatever backend `device` uses.
/// - Linux, Android: `VK_EXT_memory_budget` on Vulkan devices whose driver
///   has it, summed over the device-local heaps.
/// - macOS: Metal's `recommendedMaxWorkingSetSize` and
///   `currentAllocatedSize`.
///
/// It takes the device, not the adapter, because Metal reports memory only
/// through a device.
pub fn adapter_memory(device: &wgpu::Device) -> Option<AdapterMemory> {
    #[cfg(target_os = "windows")]
    let memory = dxgi::adapter_memory(device);
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let memory = vulkan::adapter_memory(device);
    #[cfg(target_os = "macos")]
    let memory = metal::adapter_memory(device);
    #[cfg(not(any(
        target_os = "windows",
        target_os = "linux",
        target_os = "android",
        target_os = "macos"
    )))]
    let memory = {
        let _ = device;
        None
    };
    memory
}
