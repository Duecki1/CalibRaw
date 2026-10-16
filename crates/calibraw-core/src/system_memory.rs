//! Installed and currently available physical memory, for sizing background
//! work pools and refusing work that would not fit.

/// Total physical memory in bytes, or `None` when the platform does not report it.
pub fn total_memory_bytes() -> Option<u64> {
    platform_total_memory_bytes()
}

/// Memory the kernel estimates new allocations can get without swapping, in
/// bytes (`MemAvailable`), or `None` when the platform does not report it.
/// Android can free more by killing cached background apps, so this is a
/// lower bound there.
pub fn available_memory_bytes() -> Option<u64> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
        parse_meminfo_bytes(&meminfo, "MemAvailable:")
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        None
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn platform_total_memory_bytes() -> Option<u64> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    parse_meminfo_bytes(&meminfo, "MemTotal:")
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

/// The `field` line of `/proc/meminfo` in bytes, e.g. `MemTotal:` from
/// `MemTotal:  7812345 kB`.
#[cfg(any(target_os = "linux", target_os = "android", test))]
fn parse_meminfo_bytes(meminfo: &str, field: &str) -> Option<u64> {
    let mut fields = meminfo
        .lines()
        .find_map(|line| line.strip_prefix(field))?
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
    fn meminfo_fields_are_parsed_in_bytes() {
        let meminfo = "MemTotal:        7812344 kB\nMemFree:          123456 kB\nMemAvailable:    2345678 kB\n";
        assert_eq!(
            parse_meminfo_bytes(meminfo, "MemTotal:"),
            Some(7_812_344 * 1024)
        );
        assert_eq!(
            parse_meminfo_bytes(meminfo, "MemAvailable:"),
            Some(2_345_678 * 1024)
        );
        assert_eq!(parse_meminfo_bytes("MemFree: 1 kB\n", "MemTotal:"), None);
        assert_eq!(parse_meminfo_bytes("MemTotal: x kB\n", "MemTotal:"), None);
        assert_eq!(parse_meminfo_bytes("MemTotal: 12 MB\n", "MemTotal:"), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_reports_available_memory() {
        assert!(available_memory_bytes().is_some_and(|bytes| bytes > 0));
    }

    #[cfg(any(target_os = "linux", windows, target_os = "macos"))]
    #[test]
    fn desktop_platforms_report_installed_memory() {
        assert!(total_memory_bytes().is_some_and(|bytes| bytes > 0));
    }
}
