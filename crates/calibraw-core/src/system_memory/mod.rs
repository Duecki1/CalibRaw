//! Installed physical memory, for sizing background work pools, and returning
//! freed heap memory to the system. Each platform has its own source.

#[cfg(any(target_os = "android", all(target_os = "linux", target_env = "gnu")))]
mod heap_purge;
#[cfg(any(target_os = "linux", target_os = "android", test))]
mod procfs;
#[cfg(target_os = "macos")]
mod sysctl;
#[cfg(windows)]
mod win32;

/// Total physical memory in bytes, or `None` when the platform does not report it.
pub fn total_memory_bytes() -> Option<u64> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let bytes = procfs::total_memory_bytes();
    #[cfg(windows)]
    let bytes = win32::total_memory_bytes();
    #[cfg(target_os = "macos")]
    let bytes = sysctl::total_memory_bytes();
    #[cfg(not(any(
        target_os = "linux",
        target_os = "android",
        windows,
        target_os = "macos"
    )))]
    let bytes = None;
    bytes
}

/// Asks a background thread to return the heap's free pages to the system.
/// Requests made while a purge is queued or running are served by one more.
///
/// Bionic and glibc keep the pages of freed allocations in the process, so
/// after a large job the system still counts memory as used that the app gave
/// back. On Android, whose app default `M_DECAY_TIME` 1 releases them only
/// during later allocations, the low-memory killer then ends other apps, or
/// this one; measured on a Lenovo Tab M11 after a subject mask: 1.68 GB of
/// native heap mapped, 275 MB of it allocated. Elsewhere the allocator
/// returns freed memory by itself and this does nothing.
///
/// A purge blocks for up to a few hundred milliseconds (0.36 s for 1.3 GB on
/// a Helio G88), so it never runs on the caller's thread.
pub fn release_free_heap() {
    #[cfg(any(target_os = "android", all(target_os = "linux", target_env = "gnu")))]
    heap_purge::request();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(any(target_os = "linux", windows, target_os = "macos"))]
    #[test]
    fn desktop_platforms_report_installed_memory() {
        assert!(total_memory_bytes().is_some_and(|bytes| bytes > 0));
    }
}
