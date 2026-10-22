//! macOS: the `hw.memsize` sysctl.

pub(super) fn total_memory_bytes() -> Option<u64> {
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
