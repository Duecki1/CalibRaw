//! At most one batch of preview GPU work in flight.
//!
//! Each frame dispatches the pending preview stages. On a GPU slower than
//! the display, frames would queue work faster than it runs: the preview
//! falls behind the slider, and the UI's own frames wait behind the queue.
//! The gate closes when a frame submits preview work and opens once the GPU
//! has finished everything submitted until then. Meanwhile edits only update
//! the target parameters, so the next batch renders the newest ones and no
//! obsolete render is ever queued.
//!
//! The gate also times the batches it waits for, which tells how fast the GPU
//! is at the size the preview is rendered (`median_latency_ms`).

use eframe::{egui, wgpu};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// The batches the median is taken over.
const LATENCY_WINDOW: usize = 5;
/// Fewer samples than this say nothing: the first batches of a session also
/// wait for shader programs to compile.
const MIN_LATENCY_SAMPLES: usize = 3;

/// The kinds of batch that are timed: all but `Batch::Other`.
const TIMED_BATCH_KINDS: usize = 3;

/// The last few batch times, in milliseconds.
#[derive(Debug, Default)]
struct LatencyWindow {
    samples: [f32; LATENCY_WINDOW],
    len: usize,
    next: usize,
    /// Every batch timed so far.
    total: u64,
}

impl LatencyWindow {
    fn record(&mut self, milliseconds: f32) {
        self.samples[self.next] = milliseconds;
        self.next = (self.next + 1) % LATENCY_WINDOW;
        self.len = (self.len + 1).min(LATENCY_WINDOW);
        self.total += 1;
    }

    /// The median, which one slow batch (a shader compiling, a window moving)
    /// does not move, once there are enough samples.
    fn median(&self) -> Option<f32> {
        if self.len < MIN_LATENCY_SAMPLES {
            return None;
        }
        let mut sorted = self.samples;
        let sorted = &mut sorted[..self.len];
        sorted.sort_by(f32::total_cmp);
        Some(sorted[sorted.len() / 2])
    }
}

/// Which batches a time belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Batch {
    /// The fitted preview at the size it is shown.
    Fitted,
    /// The reduced preview shown while a control is dragged.
    Interactive,
    /// A zoomed-in update: the crop at the size it is shown, with whatever
    /// else the edit made necessary.
    Detail,
    /// Anything else: the navigation graph, a build.
    Other,
}

#[derive(Clone, Debug)]
pub(crate) struct PreviewDispatchGate {
    open: Arc<AtomicBool>,
    /// The windows of the timed kinds (`Batch::window`).
    latency: Arc<Mutex<[LatencyWindow; TIMED_BATCH_KINDS]>>,
}

impl Default for PreviewDispatchGate {
    fn default() -> Self {
        Self {
            open: Arc::new(AtomicBool::new(true)),
            latency: Arc::default(),
        }
    }
}

impl PreviewDispatchGate {
    /// Whether the previous batch has finished, so the next may be submitted.
    /// Polls `device` without blocking first: wgpu reports finished work only
    /// when the device is maintained, and an idle app submits nothing else.
    pub(crate) fn poll_open(&self, device: &wgpu::Device) -> bool {
        if !self.open.load(Ordering::Acquire) {
            // A failed poll means a lost device, which cannot render the
            // preview anyway; `gpu_recovery` restarts the app where it can.
            let _ = device.poll(wgpu::PollType::Poll);
        }
        self.open.load(Ordering::Acquire)
    }

    /// Closes the gate until the GPU has finished the work submitted to
    /// `queue` so far; then it opens and requests a frame of `repaint`, which
    /// submits the next batch. The time the batch took joins the latency
    /// window of its kind. It is seen when the device is next polled, so it is
    /// accurate to about a frame.
    pub(crate) fn close_until_idle(
        &self,
        queue: &wgpu::Queue,
        repaint: &egui::Context,
        batch: Batch,
    ) {
        self.open.store(false, Ordering::Release);
        let open = Arc::clone(&self.open);
        let latency = Arc::clone(&self.latency);
        let repaint = repaint.clone();
        let submitted = Instant::now();
        queue.on_submitted_work_done(move || {
            if let Some(window) = Self::window(batch) {
                latency
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)[window]
                    .record(submitted.elapsed().as_secs_f32() * 1_000.0);
            }
            open.store(true, Ordering::Release);
            repaint.request_repaint();
        });
    }

    fn window(batch: Batch) -> Option<usize> {
        match batch {
            Batch::Fitted => Some(0),
            Batch::Interactive => Some(1),
            Batch::Detail => Some(2),
            Batch::Other => None,
        }
    }

    /// How many batches of `batch` have finished since the app started.
    pub(crate) fn completed(&self, batch: Batch) -> u64 {
        Self::window(batch).map_or(0, |window| {
            self.latency
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)[window]
                .total
        })
    }

    /// The median time of the last batches of `batch` in milliseconds, or
    /// `None` before there are enough (or for `Other`, which is not timed).
    pub(crate) fn median_latency_ms(&self, batch: Batch) -> Option<f32> {
        let window = Self::window(batch)?;
        self.latency
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)[window]
            .median()
    }
}

#[cfg(test)]
mod tests {
    use super::{Batch, LatencyWindow, PreviewDispatchGate};
    use eframe::{egui, wgpu};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[test]
    fn the_median_needs_a_few_samples_and_ignores_one_slow_batch() {
        let mut window = LatencyWindow::default();
        assert_eq!(window.median(), None);
        window.record(900.0);
        window.record(30.0);
        assert_eq!(window.median(), None);
        window.record(28.0);
        assert_eq!(window.median(), Some(30.0));
        // Once full, the oldest sample (the compile stall) drops out.
        for _ in 0..4 {
            window.record(32.0);
        }
        assert_eq!(window.median(), Some(32.0));
        window.record(2_000.0);
        assert_eq!(window.median(), Some(32.0));
    }

    #[test]
    fn the_gate_opens_and_repaints_once_the_gpu_finished_the_batch() {
        let gpu = calibraw_gpu::request_headless_device(&calibraw_gpu::HeadlessDeviceRequest {
            label: "preview dispatch gate test",
            adapter: calibraw_gpu::HeadlessAdapter::PreferredOrSoftware(
                wgpu::PowerPreference::default(),
            ),
            texture_dimension: wgpu::Limits::default().max_texture_dimension_2d,
        });
        let (device, queue) = match gpu {
            Ok(gpu) => (gpu.device, gpu.queue),
            Err(calibraw_gpu::HeadlessDeviceError::NoAdapter(_)) => {
                eprintln!("preview dispatch gate test skipped: no headless GPU adapter");
                return;
            }
            Err(error) => panic!("{error}"),
        };
        let context = egui::Context::default();
        let repaints = Arc::new(AtomicUsize::new(0));
        context.set_request_repaint_callback({
            let repaints = Arc::clone(&repaints);
            move |_| {
                repaints.fetch_add(1, Ordering::Relaxed);
            }
        });
        let gate = PreviewDispatchGate::default();
        assert!(gate.poll_open(&device));

        queue.submit(std::iter::empty());
        gate.close_until_idle(&queue, &context, Batch::Other);
        assert_eq!(repaints.load(Ordering::Relaxed), 0);

        // Checking the gate polls the device, so a finished batch is noticed
        // without another submission.
        let started = std::time::Instant::now();
        while !gate.poll_open(&device) {
            assert!(
                started.elapsed() < std::time::Duration::from_secs(10),
                "the batch never finished"
            );
            std::thread::yield_now();
        }
        assert_eq!(repaints.load(Ordering::Relaxed), 1);

        // Timed batches fill the latency window.
        assert_eq!(gate.median_latency_ms(Batch::Fitted), None);
        for _ in 0..3 {
            queue.submit(std::iter::empty());
            gate.close_until_idle(&queue, &context, Batch::Fitted);
            while !gate.poll_open(&device) {
                assert!(started.elapsed() < std::time::Duration::from_secs(10));
                std::thread::yield_now();
            }
        }
        assert!(gate.median_latency_ms(Batch::Fitted).is_some());
        assert_eq!(gate.median_latency_ms(Batch::Interactive), None);
    }
}
