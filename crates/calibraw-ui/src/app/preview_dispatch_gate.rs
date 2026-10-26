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
