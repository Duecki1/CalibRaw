//! Returning freed heap memory to the system.
//!
//! Android's allocator keeps the pages of freed allocations in the process
//! and, with the app default `M_DECAY_TIME` 1, releases them only during later
//! allocations. After a large job the system therefore still counts memory as
//! used that the app gave back, and its low-memory killer ends other apps, or
//! this one, to reclaim it. Measured on a Lenovo Tab M11 after a subject mask:
//! 1.68 GB of native heap mapped, 275 MB of it allocated.

use std::ffi::c_int;
use std::sync::mpsc;
use std::sync::OnceLock;
use std::time::Instant;

/// Purges free memory, including the threads' caches (API 34+).
const M_PURGE_ALL: c_int = -104;
/// Purges free memory outside the threads' caches (API 28+).
const M_PURGE: c_int = -101;

extern "C" {
    /// Bionic's `mallopt`, available since API 26: 1 on success, 0 for an
    /// option this Android does not have.
    fn mallopt(option: c_int, value: c_int) -> c_int;
}

/// Asks a background thread to return the heap's free pages to the system.
/// Requests made while a purge is queued or running are served by one more.
///
/// A purge blocks for up to a few hundred milliseconds (0.36 s for 1.3 GB on
/// a Helio G88), so it never runs on the caller's thread.
pub fn release_free_heap() {
    static PURGES: OnceLock<Option<mpsc::Sender<()>>> = OnceLock::new();
    let purges = PURGES.get_or_init(|| {
        let (sender, requests) = mpsc::channel::<()>();
        std::thread::Builder::new()
            .name("calibraw-heap-purge".to_owned())
            .spawn(move || {
                while requests.recv().is_ok() {
                    while requests.try_recv().is_ok() {}
                    purge();
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

fn purge() {
    let resident_before = resident_mib();
    let started = Instant::now();
    // SAFETY: `mallopt` only reads its two integers. Both options ignore the
    // value, are thread-safe, and an Android without one returns 0 and
    // changes nothing.
    let purged = unsafe { mallopt(M_PURGE_ALL, 0) == 1 || mallopt(M_PURGE, 0) == 1 };
    if !purged {
        return;
    }
    if let (Some(before), Some(after)) = (resident_before, resident_mib()) {
        calibraw_core::diagnostics::record(format!(
            "Returned freed memory: resident {before} -> {after} MiB in {} ms",
            started.elapsed().as_millis()
        ));
    }
}

/// The process's resident memory (`VmRSS`), in MiB.
fn resident_mib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with("VmRSS:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kib / 1024)
}
