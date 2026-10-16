//! The GPU DirectML sessions run on: the one the editor renders with, so AI
//! work and the free-memory check concern the same adapter. Without it,
//! DirectML picks its own adapter, which on hybrid laptops may be the other
//! GPU.

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
    let gpu = super::EDITOR_GPU.get()?;
    let (vendor_id, device_id) = (gpu.vendor_id(), gpu.device_id());
    let found = calibraw_gpu::memory::find_dxgi_adapter(vendor_id, device_id).map(|adapter| {
        adapter.and_then(|adapter| {
            Some(DirectMlAdapter {
                index: i32::try_from(adapter.index).ok()?,
                available_bytes: adapter.memory.available(),
            })
        })
    });
    match found {
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
