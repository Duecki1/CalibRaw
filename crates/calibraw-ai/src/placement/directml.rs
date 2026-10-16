//! The GPU DirectML sessions run on: the one the editor renders with, so AI
//! work and the free-memory check concern the same adapter. Without it,
//! DirectML picks its own adapter, which on hybrid laptops may be the other
//! GPU.

use std::sync::OnceLock;
use windows::{
    core::Interface,
    Win32::Graphics::Dxgi::{
        CreateDXGIFactory1, IDXGIAdapter3, IDXGIFactory1, DXGI_ERROR_NOT_FOUND,
        DXGI_MEMORY_SEGMENT_GROUP_LOCAL, DXGI_QUERY_VIDEO_MEMORY_INFO,
    },
};

/// PCI vendor and device ID of the editor's GPU.
static EDITOR_GPU: OnceLock<(u32, u32)> = OnceLock::new();

/// Runs DirectML sessions on the GPU with these PCI IDs, as `wgpu` reports
/// them for the editor's adapter.
pub(super) fn set_directml_adapter(vendor_id: u32, device_id: u32) {
    if EDITOR_GPU.set((vendor_id, device_id)).is_err() {
        log::warn!("the DirectML adapter was already chosen");
    }
}

pub(super) struct DirectMlAdapter {
    /// Position in `IDXGIFactory1::EnumAdapters1` order, which is what
    /// DirectML's `device_id` selects.
    pub(super) index: i32,
    /// Local video memory this process may still use without the OS paging
    /// it out: its budget minus its current usage. Integrated GPUs report the
    /// shared system memory granted to them here.
    pub(super) available_bytes: u64,
}

/// The editor's GPU as DirectML numbers it, or `None` when it is unknown or
/// DXGI does not list it.
pub(super) fn editor_adapter() -> Option<DirectMlAdapter> {
    let &(vendor_id, device_id) = EDITOR_GPU.get()?;
    match find_adapter(vendor_id, device_id) {
        Ok(Some(adapter)) => Some(adapter),
        Ok(None) => {
            log::warn!(
                "DXGI lists no adapter {vendor_id:04x}:{device_id:04x}; DirectML picks its own"
            );
            None
        }
        Err(error) => {
            log::warn!("could not query DXGI adapters for DirectML: {error}");
            None
        }
    }
}

fn find_adapter(vendor_id: u32, device_id: u32) -> windows::core::Result<Option<DirectMlAdapter>> {
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
        let Ok(index) = i32::try_from(index) else {
            return Ok(None);
        };
        let adapter: IDXGIAdapter3 = adapter.cast()?;
        let mut memory = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
        // SAFETY: node 0 exists on every adapter and `memory` outlives the call.
        unsafe { adapter.QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &mut memory)? };
        return Ok(Some(DirectMlAdapter {
            index,
            available_bytes: memory.Budget.saturating_sub(memory.CurrentUsage),
        }));
    }
    Ok(None)
}
