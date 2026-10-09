//! Optional process-wide serialization of photo file reads (HDD mode).
//!
//! On a rotational disk, several workers reading different files at once make
//! the head seek between them, which is slower than reading one file after
//! another. When enabled, the read-heavy sections of thumbnail, display-metadata
//! and RAW loading run one at a time; decoding and resampling stay outside the
//! gate and keep running in parallel.
//!
//! The gate is a leaf lock: code holding it only reads files and parses
//! headers, and never waits for a lock that may be held while waiting for the
//! gate. Nested calls on the thread that already holds it run inline, so a gated
//! caller may call a gated callee.

use std::cell::Cell;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

/// Files larger than this are only partly warmed by [`prefetch_file`]; the
/// decoder reads the rest itself.
const MAX_PREFETCH_BYTES: u64 = 512 * 1024 * 1024;
const PREFETCH_CHUNK_BYTES: usize = 1024 * 1024;

static ENABLED: AtomicBool = AtomicBool::new(false);
static GATE: Mutex<()> = Mutex::new(());

thread_local! {
    static HELD_BY_THIS_THREAD: Cell<bool> = const { Cell::new(false) };
}

pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Release);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Acquire)
}

/// Runs `read` inside the gate when serialization is enabled, otherwise
/// directly. `read` must not wait for decode gates, worker permits or other
/// coordination locks.
pub fn run<T>(read: impl FnOnce() -> T) -> T {
    if !enabled() || HELD_BY_THIS_THREAD.with(Cell::get) {
        return read();
    }

    struct HeldMarker;
    impl Drop for HeldMarker {
        fn drop(&mut self) {
            HELD_BY_THIS_THREAD.with(|held| held.set(false));
        }
    }

    // The data is `()`, so a poisoned gate is still valid.
    let _gate = GATE.lock().unwrap_or_else(PoisonError::into_inner);
    HELD_BY_THIS_THREAD.with(|held| held.set(true));
    // Declared after the guard so the marker clears before the gate unlocks.
    let _held = HeldMarker;
    read()
}

/// Reads `path` sequentially inside the gate so a decoder that reads it in
/// small pieces afterwards is served from the page cache. Does nothing when
/// serialization is disabled. Errors are ignored; the decoder reports them.
pub fn prefetch_file(path: &Path) {
    if !enabled() {
        return;
    }
    run(|| {
        let Ok(file) = File::open(path) else {
            return;
        };
        let mut file = file.take(MAX_PREFETCH_BYTES);
        let mut buffer = vec![0; PREFETCH_CHUNK_BYTES];
        while let Ok(read) = file.read(&mut buffer) {
            if read == 0 {
                break;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_runs_on_one_thread_do_not_deadlock() {
        // Tests share the process-wide flag, so only enable it; disabling could
        // race with another test observing the gate.
        set_enabled(true);
        assert_eq!(run(|| run(|| 7)), 7);
        assert!(!HELD_BY_THIS_THREAD.with(Cell::get));
    }

    #[test]
    fn prefetch_tolerates_missing_files() {
        set_enabled(true);
        prefetch_file(Path::new("/nonexistent/calibraw-prefetch-test"));
    }
}
