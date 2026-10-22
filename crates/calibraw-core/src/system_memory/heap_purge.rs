//! Android (Bionic), Linux (glibc): returning the allocator's free pages on a
//! background thread (`super::release_free_heap`).

use std::{ffi::c_int, sync::mpsc, sync::OnceLock, time::Instant};

/// Queues a purge, starting the purge thread on first use.
pub(super) fn request() {
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
fn resident_mib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with("VmRSS:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kib / 1024)
}
