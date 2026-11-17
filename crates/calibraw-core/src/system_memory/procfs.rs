//! Linux, Android: `/proc/meminfo`.

#[cfg(any(target_os = "linux", target_os = "android"))]
pub(super) fn total_memory_bytes() -> Option<u64> {
    meminfo_bytes("MemTotal:")
}

#[cfg(any(target_os = "linux", target_os = "android"))]
pub(super) fn available_memory_bytes() -> Option<u64> {
    meminfo_bytes("MemAvailable:")
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn meminfo_bytes(field: &str) -> Option<u64> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    parse_meminfo_bytes(&meminfo, field)
}

/// The value of `field` in `/proc/meminfo`, in bytes (`MemTotal:  7812345 kB`).
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
