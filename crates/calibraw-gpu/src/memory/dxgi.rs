//! Windows: DXGI reports each adapter's video memory budget for this process,
//! whichever wgpu backend renders on it. Adapters are matched by PCI vendor
//! and device ID, so of two identical GPUs the first DXGI lists is used.

use super::AdapterMemory;
use windows::{
    core::Interface,
    Win32::Graphics::Dxgi::{
        CreateDXGIFactory1, IDXGIAdapter3, IDXGIFactory1, DXGI_ERROR_NOT_FOUND,
        DXGI_MEMORY_SEGMENT_GROUP_LOCAL, DXGI_QUERY_VIDEO_MEMORY_INFO,
    },
};

/// An adapter as DXGI lists it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DxgiAdapter {
    /// Position in `IDXGIFactory1::EnumAdapters1` order, which is how
    /// DirectML selects a device.
    pub index: u32,
    /// The local segment group: dedicated video memory on discrete GPUs, the
    /// shared system memory granted to the GPU on integrated ones.
    pub memory: AdapterMemory,
}

pub(super) fn adapter_memory(device: &wgpu::Device) -> Option<AdapterMemory> {
    let info = device.adapter_info();
    match find_dxgi_adapter(info.vendor, info.device) {
        Ok(adapter) => adapter.map(|adapter| adapter.memory),
        Err(error) => {
            log::warn!("could not query DXGI adapter memory: {error}");
            None
        }
    }
}

/// The first adapter DXGI lists with these PCI IDs, as `wgpu` reports them in
/// `AdapterInfo::vendor` and `AdapterInfo::device`, or `None` if there is none.
pub fn find_dxgi_adapter(
    vendor_id: u32,
    device_id: u32,
) -> windows::core::Result<Option<DxgiAdapter>> {
    // SAFETY: creates a DXGI factory; the returned interface owns its
    // reference and releases it on drop.
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1()? };
    for index in 0u32.. {
        // SAFETY: `index` is any value; past the last adapter DXGI returns
        // DXGI_ERROR_NOT_FOUND, which ends the search.
        let adapter = match unsafe { factory.EnumAdapters1(index) } {
            Ok(adapter) => adapter,
            Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => return Ok(None),
            Err(error) => return Err(error),
        };
        // SAFETY: `adapter` is a valid adapter returned by the factory.
        let description = unsafe { adapter.GetDesc1()? };
        if description.VendorId != vendor_id || description.DeviceId != device_id {
            continue;
        }
        let adapter: IDXGIAdapter3 = adapter.cast()?;
        let mut memory = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
        // SAFETY: node 0 exists on every adapter and `memory` outlives the call.
        unsafe { adapter.QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &mut memory)? };
        return Ok(Some(DxgiAdapter {
            index,
            memory: AdapterMemory {
                budget: memory.Budget,
                usage: memory.CurrentUsage,
            },
        }));
    }
    Ok(None)
}
