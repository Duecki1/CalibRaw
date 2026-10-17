//! Models that ran out of GPU memory in this process, by model file, so that
//! `place` does not put them back on the GPU for every job only to fail
//! again: a model returns once more memory is free than at its failure, or,
//! where free memory is not reported, after a backoff that doubles with each
//! failure. A successful GPU run forgets the model, and so does changing the
//! GPU AI setting.

use super::place::OutOfMemoryRecord;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};

/// The backoff after a model's first failure; it doubles with each further
/// one up to `LONGEST_BACKOFF`.
const FIRST_BACKOFF: Duration = Duration::from_secs(60);
const LONGEST_BACKOFF: Duration = Duration::from_secs(30 * 60);

#[derive(Clone, Copy, Debug)]
struct Failure {
    /// Bytes free on the adapter when the model was placed on it, if known.
    free_before: Option<u64>,
    /// Failures since the model last ran on the GPU.
    count: u32,
    retry_at: Instant,
}

#[derive(Debug, Default)]
struct Failures(BTreeMap<PathBuf, Failure>);

impl Failures {
    fn record(&mut self, model: &Path, free_before: Option<u64>, now: Instant) -> Duration {
        let count = self
            .0
            .get(model)
            .map_or(0, |failure| failure.count)
            .saturating_add(1);
        let backoff = backoff(count);
        self.0.insert(
            model.to_path_buf(),
            Failure {
                free_before,
                count,
                retry_at: now + backoff,
            },
        );
        backoff
    }

    fn lookup(&self, model: &Path, now: Instant) -> Option<OutOfMemoryRecord> {
        self.0.get(model).map(|failure| OutOfMemoryRecord {
            free_before: failure.free_before,
            backoff_over: now >= failure.retry_at,
        })
    }
}

/// The wait after a model's `count`th failure in a row.
fn backoff(count: u32) -> Duration {
    let doublings = count.saturating_sub(1).min(5);
    FIRST_BACKOFF
        .saturating_mul(1 << doublings)
        .min(LONGEST_BACKOFF)
}

/// A leaf lock: never held while creating or running a session.
static FAILURES: Mutex<Failures> = Mutex::new(Failures(BTreeMap::new()));

fn failures() -> MutexGuard<'static, Failures> {
    FAILURES.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Records that the model at `model` ran out of GPU memory after being placed
/// on an adapter with `free_before` bytes free.
pub(super) fn record(model: &Path, free_before: Option<u64>) {
    let backoff = failures().record(model, free_before, Instant::now());
    log::info!(
        "AI model {} ran out of GPU memory; it stays on the CPU until {}",
        model.display(),
        match free_before {
            Some(free) => format!("more than {} MiB is free", free >> 20),
            None => format!("{} s have passed", backoff.as_secs()),
        }
    );
}

/// What `place` needs to know of the model at `model` running out of GPU
/// memory before, if it did.
pub(super) fn lookup(model: &Path) -> Option<OutOfMemoryRecord> {
    failures().lookup(model, Instant::now())
}

/// Forgets the failures of the model at `model`, which just ran on the GPU.
pub(super) fn forget(model: &Path) {
    failures().0.remove(model);
}

/// Forgets every failure, for a change of the GPU AI setting.
#[cfg_attr(
    target_os = "android",
    allow(dead_code, reason = "Android has no GPU AI setting")
)]
pub(super) fn forget_all() {
    failures().0.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: Duration = Duration::from_secs(60);

    #[test]
    fn the_backoff_doubles_from_one_minute_to_half_an_hour() {
        let minutes: Vec<u64> = (1..=8).map(|count| backoff(count).as_secs() / 60).collect();
        assert_eq!(minutes, [1, 2, 4, 8, 16, 30, 30, 30]);
        assert_eq!(backoff(u32::MAX), LONGEST_BACKOFF);
    }

    #[test]
    fn each_failure_waits_longer_until_the_model_is_forgotten() {
        let model = Path::new("/models/birefnet-general-lite.onnx");
        let other = Path::new("/models/skyseg.onnx");
        let start = Instant::now();
        let mut failures = Failures::default();
        assert_eq!(failures.lookup(model, start), None);

        failures.record(model, None, start);
        let record = |failures: &Failures, after: Duration| failures.lookup(model, start + after);
        assert_eq!(
            record(&failures, Duration::ZERO),
            Some(OutOfMemoryRecord {
                free_before: None,
                backoff_over: false,
            })
        );
        assert_eq!(
            record(&failures, MINUTE).map(|r| r.backoff_over),
            Some(true)
        );
        assert_eq!(failures.lookup(other, start), None);

        // Failing again at the retry doubles the wait.
        failures.record(model, Some(3000 << 20), start + MINUTE);
        let again = record(&failures, 2 * MINUTE).unwrap();
        assert_eq!(again.free_before, Some(3000 << 20));
        assert!(!again.backoff_over);
        assert!(record(&failures, 3 * MINUTE).unwrap().backoff_over);

        failures.0.remove(model);
        assert_eq!(record(&failures, Duration::ZERO), None);
        failures.record(model, None, start);
        assert!(record(&failures, MINUTE).unwrap().backoff_over);
    }
}
