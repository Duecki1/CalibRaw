//! Installed physical memory, for sizing background work pools, and returning
//! freed heap memory to the system.

#[cfg(any(target_os = "android", all(target_os = "linux", target_env = "gnu")))]
use std::{ffi::c_int, sync::mpsc, sync::OnceLock, time::Instant};

/// Total physical memory in bytes, or `None` when the platform does not report it.
pub fn total_memory_bytes() -> Option<u64> {
    platform_total_memory_bytes()
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn platform_total_memory_bytes() -> Option<u64> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    parse_mem_total_bytes(&meminfo)
}

#[cfg(windows)]
fn platform_total_memory_bytes() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    // SAFETY: `status` is a valid, writable MEMORYSTATUSEX whose `dwLength`
    // is set as the API requires; it lives for the duration of the call.
    let succeeded = unsafe { GlobalMemoryStatusEx(&mut status) } != 0;
    (succeeded && status.ullTotalPhys > 0).then_some(status.ullTotalPhys)
}

#[cfg(target_os = "macos")]
fn platform_total_memory_bytes() -> Option<u64> {
    let mut bytes: u64 = 0;
    let mut length = std::mem::size_of::<u64>();
    // SAFETY: the name is a NUL-terminated C string; `bytes` and `length`
    // describe a writable u64, which is the documented type of hw.memsize.
    // No new value is written (null pointer, zero length).
    let result = unsafe {
        libc::sysctlbyname(
            c"hw.memsize".as_ptr(),
            (&raw mut bytes).cast(),
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    };
    (result == 0 && length == std::mem::size_of::<u64>() && bytes > 0).then_some(bytes)
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    windows,
    target_os = "macos"
)))]
fn platform_total_memory_bytes() -> Option<u64> {
    None
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
    {
        static PURGES: OnceLock<Option<mpsc::Sender<()>>> = OnceLock::new();
        let purges = PURGES.get_or_init(|| {
            let (sender, requests) = mpsc::channel::<()>();
            std::thread::Builder::new()
                .name("calibraw-heap-purge".to_owned())
                .spawn(move || {
                    while requests.recv().is_ok() {
                        while requests.try_recv().is_ok() {}
                        purge_free_heap();
                    }
                })
                .map_err(|error| log::warn!("could not start the heap purge thread: {error}"))
                .ok()
                .map(|_| sender)
        });
        if let Some(purges) = purges {
            let _ = purges.send(());
        }
    }
}

#[cfg(any(target_os = "android", all(target_os = "linux", target_env = "gnu")))]
fn purge_free_heap() {
    let resident_before = resident_mib();
    let started = Instant::now();
    if !purge_allocator() {
        return;
    }
    if let (Some(before), Some(after)) = (resident_before, resident_mib()) {
        crate::diagnostics::record(format!(
            "Returned freed memory: resident {before} -> {after} MiB in {} ms",
            started.elapsed().as_millis()
        ));
    }
}

/// Returns the allocator's free pages; `false` when it could not.
#[cfg(target_os = "android")]
fn purge_allocator() -> bool {
    /// Purges free memory, including the threads' caches (API 34+).
    const M_PURGE_ALL: c_int = -104;
    /// Purges free memory outside the threads' caches (API 28+).
    const M_PURGE: c_int = -101;
    extern "C" {
        /// Bionic's `mallopt`, available since API 26: 1 on success, 0 for
        /// an option this Android does not have.
        fn mallopt(option: c_int, value: c_int) -> c_int;
    }
    // SAFETY: `mallopt` only reads its two integers. Both options ignore the
    // value, are thread-safe, and an Android without one returns 0 and
    // changes nothing.
    unsafe { mallopt(M_PURGE_ALL, 0) == 1 || mallopt(M_PURGE, 0) == 1 }
}

/// Returns the allocator's free pages; `false` when it could not.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn purge_allocator() -> bool {
    extern "C" {
        /// glibc's `malloc_trim`: returns the free pages of every arena,
        /// keeping `pad` bytes at the top of the main one; 1 when it released
        /// any memory.
        fn malloc_trim(pad: usize) -> c_int;
    }
    // SAFETY: `malloc_trim` takes a plain integer and locks each arena while
    // it trims it, so it is safe beside allocations on other threads.
    unsafe { malloc_trim(0) == 1 }
}

/// The process's resident memory (`VmRSS`), in MiB.
#[cfg(any(target_os = "android", all(target_os = "linux", target_env = "gnu")))]
fn resident_mib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with("VmRSS:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kib / 1024)
}

/// Total RAM in bytes from `/proc/meminfo` (`MemTotal:  7812345 kB`).
#[cfg(any(target_os = "linux", target_os = "android", test))]
fn parse_mem_total_bytes(meminfo: &str) -> Option<u64> {
    let mut fields = meminfo
        .lines()
        .find_map(|line| line.strip_prefix("MemTotal:"))?
        .split_whitespace();
    let value = fields.next()?.parse::<u64>().ok()?;
    match fields.next() {
        Some("kB") => value.checked_mul(1024),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mem_total_is_parsed_from_meminfo() {
        let meminfo = "MemTotal:        7812344 kB\nMemFree:          123456 kB\n";
        assert_eq!(parse_mem_total_bytes(meminfo), Some(7_812_344 * 1024));
        assert_eq!(parse_mem_total_bytes("MemFree: 1 kB\n"), None);
        assert_eq!(parse_mem_total_bytes("MemTotal: x kB\n"), None);
        assert_eq!(parse_mem_total_bytes("MemTotal: 12 MB\n"), None);
    }

    #[cfg(any(target_os = "linux", windows, target_os = "macos"))]
    #[test]
    fn desktop_platforms_report_installed_memory() {
        assert!(total_memory_bytes().is_some_and(|bytes| bytes > 0));
    }
}
