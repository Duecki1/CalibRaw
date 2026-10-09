//! Installed physical memory, for sizing background work pools.

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
