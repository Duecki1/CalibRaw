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
}
