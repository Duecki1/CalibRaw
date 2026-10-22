//! GPU time per processing stage, for profiling slow devices.
//!
//! Off unless [`request`] runs before the editor's device is created. That
//! device then adds [`device_features`] (timestamp queries) to its request,
//! and [`install`] starts timing it. While on, every compute pass a stage
//! encodes on that device writes a timestamp when it starts and when it ends,
//! and once the GPU has run the stage one line goes to `diagnostics`:
//!
//! ```text
//! GPU stage Output 652x978: 54.21 ms in 3 passes (prepare_scene_node 33.94, apply_scene_tone_node 20.07, view 0.20)
//! ```
//!
//! The total runs from the earliest pass start to the latest pass end. It is
//! reliable; the split between adjacent passes is not, because GPUs overlap
//! dispatches (Mali notably), so a pass may start before the previous one
//! ends.
//!
//! Off, a pass costs one atomic load. Passes encoded on other devices
//! (exports, developed thumbnails) or outside a stage entry point are never
//! timed.

use std::cell::RefCell;
use std::fmt::Write as _;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

/// Passes timed per stage; later ones run untimed and are counted.
const MAX_TIMED_PASSES: u32 = 256;

static REQUESTED: AtomicBool = AtomicBool::new(false);
static TIMER: OnceLock<Timer> = OnceLock::new();

struct Timer {
    device: wgpu::Device,
    /// Nanoseconds per timestamp tick.
    period_ns: f32,
}

thread_local! {
    /// The stage being encoded on this thread, between `begin` and `submit`.
    static ACTIVE: RefCell<Option<ActiveStage>> = const { RefCell::new(None) };
}

struct ActiveStage {
    query_set: wgpu::QuerySet,
    /// Labels of the timed passes; pass `i` wrote queries `2i` and `2i + 1`.
    passes: Vec<String>,
    untimed: u32,
}

/// Turns stage timing on for this process. Call before the editor's device is
/// created.
pub fn request() {
    REQUESTED.store(true, Ordering::Relaxed);
}

/// What the editor's device adds to its features: timestamp queries when
/// timing was requested and `adapter` offers them, otherwise nothing.
pub fn device_features(adapter: &wgpu::Adapter) -> wgpu::Features {
    if REQUESTED.load(Ordering::Relaxed) {
        adapter.features() & wgpu::Features::TIMESTAMP_QUERY
    } else {
        wgpu::Features::empty()
    }
}

/// Starts timing the stages encoded on `device` if timing was requested, and
/// records in `diagnostics` whether it could.
pub fn install(device: &wgpu::Device, queue: &wgpu::Queue) {
    if !REQUESTED.load(Ordering::Relaxed) {
        return;
    }
    if !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
        calibraw_core::diagnostics::record(
            "GPU stage timing is unavailable: the adapter has no timestamp queries",
        );
        return;
    }
    let timer = Timer {
        device: device.clone(),
        period_ns: queue.get_timestamp_period(),
    };
    if TIMER.set(timer).is_ok() {
        calibraw_core::diagnostics::record("GPU stage timing is on");
    }
}

/// Times the passes this thread encodes until [`submit`], when timing is on
/// for `device`. `name` (stage and size) is only built then.
pub(crate) fn begin(device: &wgpu::Device, name: impl FnOnce() -> String) -> Option<StageTiming> {
    let timer = TIMER.get()?;
    if timer.device != *device {
        return None;
    }
    let query_set = device.create_query_set(&wgpu::QuerySetDescriptor {
        label: Some("calibraw stage timing"),
        ty: wgpu::QueryType::Timestamp,
        count: 2 * MAX_TIMED_PASSES,
    });
    ACTIVE.with(|active| {
        *active.borrow_mut() = Some(ActiveStage {
            query_set,
            passes: Vec::new(),
            untimed: 0,
        });
    });
    Some(StageTiming {
        name: name(),
        _same_thread: PhantomData,
    })
}

/// The timestamps the next compute pass writes, while this thread encodes a
/// timed stage.
pub(crate) fn next_pass(label: &str) -> Option<PassTimestamps> {
    TIMER.get()?;
    ACTIVE.with(|active| {
        let mut active = active.borrow_mut();
        let stage = active.as_mut()?;
        let index = stage.passes.len() as u32;
        if index == MAX_TIMED_PASSES {
            stage.untimed += 1;
            return None;
        }
        stage.passes.push(label.to_owned());
        Some(PassTimestamps {
            query_set: stage.query_set.clone(),
            begin: 2 * index,
        })
    })
}

/// Submits `encoder`, whose passes `timing` timed, and records their times
/// once the GPU ran them.
pub(crate) fn submit(
    queue: &wgpu::Queue,
    mut encoder: wgpu::CommandEncoder,
    timing: Option<StageTiming>,
) {
    let pending = timing.and_then(|timing| timing.finish(&mut encoder));
    queue.submit(Some(encoder.finish()));
    if let Some(pending) = pending {
        pending.report();
    }
}

pub(crate) struct PassTimestamps {
    query_set: wgpu::QuerySet,
    begin: u32,
}

impl PassTimestamps {
    pub(crate) fn writes(&self) -> wgpu::ComputePassTimestampWrites<'_> {
        wgpu::ComputePassTimestampWrites {
            query_set: &self.query_set,
            beginning_of_pass_write_index: Some(self.begin),
            end_of_pass_write_index: Some(self.begin + 1),
        }
    }
}

/// A stage being timed on this thread; dropping it stops timing.
pub(crate) struct StageTiming {
    name: String,
    /// The passes are collected per thread, so this must not move to another.
    _same_thread: PhantomData<*const ()>,
}

impl StageTiming {
    /// Copies the timestamps into a buffer the CPU can read, in `encoder`, the
    /// encoder the passes went to.
    fn finish(mut self, encoder: &mut wgpu::CommandEncoder) -> Option<PendingStageTiming> {
        let stage = ACTIVE.with(|active| active.borrow_mut().take())?;
        let timer = TIMER.get()?;
        if stage.passes.is_empty() {
            return None;
        }
        // Only written queries are resolved: Vulkan waits for unwritten ones.
        let queries = 2 * stage.passes.len() as u32;
        let size = u64::from(queries) * u64::from(wgpu::QUERY_SIZE);
        let resolve = timer.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("calibraw stage timing resolve"),
            size,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = timer.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("calibraw stage timing readback"),
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        encoder.resolve_query_set(&stage.query_set, 0..queries, &resolve, 0);
        encoder.copy_buffer_to_buffer(&resolve, 0, &readback, 0, size);
        Some(PendingStageTiming {
            name: std::mem::take(&mut self.name),
            passes: stage.passes,
            untimed: stage.untimed,
            readback,
            period_ns: timer.period_ns,
        })
    }
}

impl Drop for StageTiming {
    fn drop(&mut self) {
        ACTIVE.with(|active| active.borrow_mut().take());
    }
}

struct PendingStageTiming {
    name: String,
    passes: Vec<String>,
    untimed: u32,
    readback: wgpu::Buffer,
    period_ns: f32,
}

impl PendingStageTiming {
    /// Records the times once the submitted stage finished; the device's next
    /// poll or submission runs the callback.
    fn report(self) {
        let buffer = self.readback.clone();
        self.readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                if result.is_err() {
                    return;
                }
                let ticks: Vec<u64> = buffer
                    .slice(..)
                    .get_mapped_range()
                    .chunks_exact(8)
                    .filter_map(|bytes| bytes.try_into().ok().map(u64::from_le_bytes))
                    .collect();
                buffer.unmap();
                calibraw_core::diagnostics::record(summary(
                    &self.name,
                    &self.passes,
                    self.untimed,
                    &ticks,
                    self.period_ns,
                ));
            });
    }
}

/// The diagnostics line for a stage whose pass `i` started at `ticks[2i]` and
/// ended at `ticks[2i + 1]`.
fn summary(name: &str, passes: &[String], untimed: u32, ticks: &[u64], period_ns: f32) -> String {
    let milliseconds =
        |start: u64, end: u64| end.saturating_sub(start) as f64 * f64::from(period_ns) / 1e6;
    let total = match (ticks.iter().min(), ticks.iter().max()) {
        (Some(&start), Some(&end)) => milliseconds(start, end),
        _ => 0.0,
    };
    let noun = if passes.len() == 1 { "pass" } else { "passes" };
    let mut line = format!("GPU stage {name}: {total:.2} ms in {} {noun}", passes.len());
    if untimed > 0 {
        let _ = write!(line, " and {untimed} untimed");
    }
    line.push_str(" (");
    for (index, (label, pass)) in passes.iter().zip(ticks.chunks_exact(2)).enumerate() {
        if index > 0 {
            line.push_str(", ");
        }
        let _ = write!(line, "{label} {:.2}", milliseconds(pass[0], pass[1]));
    }
    line.push(')');
    line
}

#[cfg(test)]
mod tests {
    use super::{next_pass, summary};

    #[test]
    fn the_summary_totals_from_the_first_start_to_the_last_end() {
        let passes = ["prepare".to_owned(), "tone".to_owned()];
        // The second pass starts before the first ends, as on Mali.
        let ticks = [1_000_000, 4_000_000, 3_500_000, 6_000_000];
        assert_eq!(
            summary("Output 652x978", &passes, 0, &ticks, 1.0),
            "GPU stage Output 652x978: 5.00 ms in 2 passes (prepare 3.00, tone 2.50)"
        );
    }

    #[test]
    fn the_summary_counts_untimed_passes_and_converts_ticks() {
        let passes = ["raw".to_owned()];
        assert_eq!(
            summary("Raw 256x384", &passes, 3, &[10, 30], 50.0),
            "GPU stage Raw 256x384: 0.00 ms in 1 pass and 3 untimed (raw 0.00)"
        );
        assert_eq!(
            summary("Raw 256x384", &passes, 0, &[0, 2_000_000], 2.0),
            "GPU stage Raw 256x384: 4.00 ms in 1 pass (raw 4.00)"
        );
    }

    #[test]
    fn passes_are_untimed_while_timing_is_off() {
        assert!(next_pass("calibraw pass 1").is_none());
    }
}
