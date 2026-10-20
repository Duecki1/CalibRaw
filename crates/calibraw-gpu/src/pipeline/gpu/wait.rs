//! Blocking waits for a GPU submission, polled in short slices.
//!
//! A wait without a timeout passes the largest timeout to the driver, and some
//! Android drivers (PowerVR BXM on a Redmi) return from it before the work is
//! done, which wgpu reports as a timeout. Slices ask again after an early
//! return, and they release wgpu's fence lock between polls: `Queue::submit`
//! needs that lock, so an unbroken wait on the editor device would hold off the
//! UI's frames until the work finished.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// The longest single poll.
const POLL_SLICE: Duration = Duration::from_millis(100);
/// A poll that reports a timeout sooner than this returned early.
const EARLY_RETURN: Duration = Duration::from_millis(50);
/// Pause after an early return, so a driver that never blocks is not polled
/// in a hot loop.
const EARLY_RETURN_PAUSE: Duration = Duration::from_millis(5);
/// How long a [`GpuWait::Limited`] wait lasts before it fails. Far above any
/// single submission CalibRaw makes, even on a software adapter such as
/// lavapipe, so it only ends work the driver has lost.
const LIMITED_WAIT: Duration = Duration::from_secs(600);
/// Waits at least this long are shown to the user as waiting for the GPU.
pub(crate) const SLOW_GPU_WAIT: Duration = Duration::from_secs(10);

/// How long a blocking readback waits for its GPU work.
pub enum GpuWait<'a> {
    /// Up to ten minutes, then the readback fails.
    Limited,
    /// As long as the watcher returns true. It is called after every poll
    /// with the time waited so far, so it can show a slow wait and stop the
    /// wait when its task is cancelled.
    Watched(&'a mut dyn FnMut(Duration) -> bool),
}

impl GpuWait<'_> {
    /// The same wait for one more submission of a multi-part readback.
    pub(super) fn reborrow(&mut self) -> GpuWait<'_> {
        match self {
            Self::Limited => GpuWait::Limited,
            Self::Watched(watch) => GpuWait::Watched(&mut **watch),
        }
    }
}

#[derive(Debug)]
pub(super) enum GpuWaitError {
    Poll(wgpu::PollError),
    /// The watcher of a [`GpuWait::Watched`] wait stopped it.
    Stopped {
        waited: Duration,
    },
    /// A [`GpuWait::Limited`] wait ran out.
    LimitReached {
        waited: Duration,
    },
}

/// Waits until `submission` has completed on `device`. `operation` names the
/// wait in diagnostics.
pub(super) fn wait_for_submission(
    device: &wgpu::Device,
    submission: &wgpu::SubmissionIndex,
    operation: &str,
    wait: GpuWait<'_>,
) -> Result<(), GpuWaitError> {
    let started = Instant::now();
    let poll = |slice| {
        device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission.clone()),
                timeout: Some(slice),
            })
            .map(|_| ())
    };
    let early_return = |after: Duration| {
        static RECORDED: AtomicBool = AtomicBool::new(false);
        if !RECORDED.swap(true, Ordering::Relaxed) {
            let message = format!(
                "GPU wait for {operation} readback returned after {:.1} ms of a {} ms poll before its submission completed; polling again",
                after.as_secs_f64() * 1000.0,
                POLL_SLICE.as_millis()
            );
            log::warn!("{message}");
            calibraw_core::diagnostics::record(message);
        }
        std::thread::sleep(EARLY_RETURN_PAUSE);
    };
    wait_in_slices(poll, wait, || started.elapsed(), early_return)
}

/// The loop of [`wait_for_submission`], with the device, the clock and the
/// early-return handling passed in for tests.
fn wait_in_slices(
    mut poll: impl FnMut(Duration) -> Result<(), wgpu::PollError>,
    mut wait: GpuWait<'_>,
    mut elapsed: impl FnMut() -> Duration,
    mut early_return: impl FnMut(Duration),
) -> Result<(), GpuWaitError> {
    loop {
        let poll_started = elapsed();
        match poll(POLL_SLICE) {
            Ok(()) => return Ok(()),
            Err(wgpu::PollError::Timeout) => {}
            Err(error) => return Err(GpuWaitError::Poll(error)),
        }
        let waited = elapsed();
        let poll_duration = waited.saturating_sub(poll_started);
        if poll_duration < EARLY_RETURN {
            early_return(poll_duration);
        }
        match &mut wait {
            GpuWait::Limited => {
                if waited >= LIMITED_WAIT {
                    return Err(GpuWaitError::LimitReached { waited });
                }
            }
            GpuWait::Watched(watch) => {
                if !watch(waited) {
                    return Err(GpuWaitError::Stopped { waited });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// A clock that advances only when the fake device polls.
    struct FakeDevice {
        now: Cell<Duration>,
        polls: Cell<u32>,
    }

    impl FakeDevice {
        fn new() -> Self {
            Self {
                now: Cell::new(Duration::ZERO),
                polls: Cell::new(0),
            }
        }

        /// Polls for `took`, then answers `result`.
        fn poll(
            &self,
            took: Duration,
            result: Result<(), wgpu::PollError>,
        ) -> Result<(), wgpu::PollError> {
            self.now.set(self.now.get() + took);
            self.polls.set(self.polls.get() + 1);
            result
        }
    }

    #[test]
    fn a_timed_out_poll_is_polled_again_until_the_work_completes() {
        let device = FakeDevice::new();
        let result = wait_in_slices(
            |slice| {
                let done = device.polls.get() == 2;
                device.poll(
                    slice,
                    if done {
                        Ok(())
                    } else {
                        Err(wgpu::PollError::Timeout)
                    },
                )
            },
            GpuWait::Limited,
            || device.now.get(),
            |_| panic!("no poll returned early"),
        );
        assert!(result.is_ok());
        assert_eq!(device.polls.get(), 3);
    }

    #[test]
    fn other_poll_errors_end_the_wait_at_once() {
        let device = FakeDevice::new();
        let result = wait_in_slices(
            |slice| device.poll(slice, Err(wgpu::PollError::WrongSubmissionIndex(2, 1))),
            GpuWait::Limited,
            || device.now.get(),
            |_| {},
        );
        assert!(matches!(
            result,
            Err(GpuWaitError::Poll(wgpu::PollError::WrongSubmissionIndex(
                2, 1
            )))
        ));
        assert_eq!(device.polls.get(), 1);
    }

    #[test]
    fn a_limited_wait_fails_once_its_limit_has_passed() {
        let device = FakeDevice::new();
        let result = wait_in_slices(
            |slice| device.poll(slice, Err(wgpu::PollError::Timeout)),
            GpuWait::Limited,
            || device.now.get(),
            |_| {},
        );
        let Err(GpuWaitError::LimitReached { waited }) = result else {
            panic!("expected the limit to end the wait, got {result:?}");
        };
        assert!(waited >= LIMITED_WAIT && waited < LIMITED_WAIT + POLL_SLICE);
    }

    #[test]
    fn a_watched_wait_has_no_limit_and_stops_when_its_watcher_does() {
        let device = FakeDevice::new();
        let mut watch = |waited: Duration| waited < LIMITED_WAIT * 2;
        let result = wait_in_slices(
            |slice| device.poll(slice, Err(wgpu::PollError::Timeout)),
            GpuWait::Watched(&mut watch),
            || device.now.get(),
            |_| {},
        );
        let Err(GpuWaitError::Stopped { waited }) = result else {
            panic!("expected the watcher to stop the wait, got {result:?}");
        };
        assert!(waited >= LIMITED_WAIT * 2);
    }

    #[test]
    fn an_early_return_pauses_before_the_next_poll() {
        let device = FakeDevice::new();
        let mut early_returns = Vec::new();
        let result = wait_in_slices(
            |_| {
                let done = device.polls.get() == 3;
                device.poll(
                    Duration::from_millis(1),
                    if done {
                        Ok(())
                    } else {
                        Err(wgpu::PollError::Timeout)
                    },
                )
            },
            GpuWait::Limited,
            || device.now.get(),
            |after| early_returns.push(after),
        );
        assert!(result.is_ok());
        assert_eq!(early_returns, vec![Duration::from_millis(1); 3]);
    }
}
