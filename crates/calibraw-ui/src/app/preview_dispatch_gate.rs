//! At most one batch of preview GPU work in flight.
//!
//! Each frame dispatches the pending preview stages. On a GPU slower than
//! the display, frames would queue work faster than it runs: the preview
//! falls behind the slider, and the UI's own frames wait behind the queue.
//! The gate closes when a frame submits preview work and opens once the GPU
//! has finished everything submitted until then. Meanwhile edits only update
//! the target parameters, so the next batch renders the newest ones and no
//! obsolete render is ever queued.

use eframe::{egui, wgpu};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub(crate) struct PreviewDispatchGate {
    open: Arc<AtomicBool>,
}

impl Default for PreviewDispatchGate {
    fn default() -> Self {
        Self {
            open: Arc::new(AtomicBool::new(true)),
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
    /// submits the next batch.
    pub(crate) fn close_until_idle(&self, queue: &wgpu::Queue, repaint: &egui::Context) {
        self.open.store(false, Ordering::Release);
        let open = Arc::clone(&self.open);
        let repaint = repaint.clone();
        queue.on_submitted_work_done(move || {
            open.store(true, Ordering::Release);
            repaint.request_repaint();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::PreviewDispatchGate;
    use eframe::{egui, wgpu};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

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
        gate.close_until_idle(&queue, &context);
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
    }
}
