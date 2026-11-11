//! Linux, Android: `/proc/meminfo`.

#[cfg(any(target_os = "linux", target_os = "android"))]
pub(super) fn total_memory_bytes() -> Option<u64> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    parse_mem_total_bytes(&meminfo)
}

/// Total RAM in bytes from `/proc/meminfo` (`MemTotal:  7812345 kB`).
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
