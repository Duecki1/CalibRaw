//! Windows: `GlobalMemoryStatusEx`.

use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

pub(super) fn total_memory_bytes() -> Option<u64> {
    let status = memory_status()?;
    (status.ullTotalPhys > 0).then_some(status.ullTotalPhys)
}

pub(super) fn available_memory_bytes() -> Option<u64> {
    memory_status().map(|status| status.ullAvailPhys)
}

fn memory_status() -> Option<MEMORYSTATUSEX> {
    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    // SAFETY: `status` is a valid, writable MEMORYSTATUSEX whose `dwLength`
    // is set as the API requires; it lives for the duration of the call.
    let succeeded = unsafe { GlobalMemoryStatusEx(&mut status) } != 0;
    succeeded.then_some(status)
}
