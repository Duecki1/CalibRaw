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
//! An export on that device ([`begin_export`]) sums its stages per phase
//! instead, and records one line per phase when it finishes, with the passes
//! slowest first:
//!
//! ```text
//! GPU export final tiles: 1834.10 ms in 70 stages (apply_scene_tone_node 610.32, ...)
//! ```
//!
//! Off, a pass costs one atomic load. Passes encoded on other devices
//! (headless exports, developed thumbnails) or outside a stage entry point are
//! never timed.

use std::cell::RefCell;
use std::fmt::Write as _;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

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
    /// The export this thread's stages are summed into, between
    /// [`begin_export`] and [`ExportTiming::finish`].
    static EXPORT: RefCell<Option<ExportPhase>> = const { RefCell::new(None) };
}

#[derive(Clone)]
struct ExportPhase {
    totals: Arc<ExportTotals>,
    phase: &'static str,
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
        let export = EXPORT.with(|export| export.borrow().clone());
        if let Some(export) = &export {
            export.totals.add_pending();
        }
        Some(PendingStageTiming {
            name: std::mem::take(&mut self.name),
            passes: stage.passes,
            untimed: stage.untimed,
            readback,
            period_ns: timer.period_ns,
            export,
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
    /// The export phase the times are summed into, instead of a line.
    export: Option<ExportPhase>,
}

impl PendingStageTiming {
    /// Records the times once the submitted stage finished; the device's next
    /// poll or submission runs the callback.
    fn report(self) {
        let buffer = self.readback.clone();
        self.readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let ticks: Option<Vec<u64>> = result.is_ok().then(|| {
                    let ticks = buffer
                        .slice(..)
                        .get_mapped_range()
                        .chunks_exact(8)
                        .filter_map(|bytes| bytes.try_into().ok().map(u64::from_le_bytes))
                        .collect();
                    buffer.unmap();
                    ticks
                });
                match (&self.export, ticks) {
                    (Some(export), ticks) => export.totals.settle(
                        export.phase,
                        ticks.as_deref().map(|ticks| StageTicks {
                            passes: &self.passes,
                            untimed: self.untimed,
                            ticks,
                            period_ns: self.period_ns,
                        }),
                    ),
                    (None, Some(ticks)) => calibraw_core::diagnostics::record(summary(
                        &self.name,
                        &self.passes,
                        self.untimed,
                        &ticks,
                        self.period_ns,
                    )),
                    (None, None) => {}
                }
            });
    }
}

/// Milliseconds between two timestamps of `period_ns` nanoseconds per tick.
fn milliseconds(start: u64, end: u64, period_ns: f32) -> f64 {
    end.saturating_sub(start) as f64 * f64::from(period_ns) / 1e6
}

/// Milliseconds from the earliest pass start to the latest pass end.
fn stage_span(ticks: &[u64], period_ns: f32) -> f64 {
    match (ticks.iter().min(), ticks.iter().max()) {
        (Some(&start), Some(&end)) => milliseconds(start, end, period_ns),
        _ => 0.0,
    }
}

/// The diagnostics line for a stage whose pass `i` started at `ticks[2i]` and
/// ended at `ticks[2i + 1]`.
fn summary(name: &str, passes: &[String], untimed: u32, ticks: &[u64], period_ns: f32) -> String {
    let milliseconds = |start: u64, end: u64| milliseconds(start, end, period_ns);
    let total = stage_span(ticks, period_ns);
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

/// How long [`ExportTiming::finish`] waits for the last stages' times.
const EXPORT_SETTLE_TIMEOUT: Duration = Duration::from_secs(10);

/// Sums the times of the stages this thread encodes on `device` until
/// [`ExportTiming::finish`], when timing is on for `device`. Each stage counts
/// toward the phase last set with [`ExportTiming::phase`].
pub(crate) fn begin_export(device: &wgpu::Device) -> Option<ExportTiming> {
    let timer = TIMER.get()?;
    if timer.device != *device {
        return None;
    }
    let totals = Arc::new(ExportTotals::default());
    EXPORT.with(|export| {
        *export.borrow_mut() = Some(ExportPhase {
            totals: Arc::clone(&totals),
            phase: "",
        });
    });
    Some(ExportTiming {
        totals,
        device: device.clone(),
        _same_thread: PhantomData,
    })
}

/// An export whose stages this thread sums; dropping it stops summing.
pub(crate) struct ExportTiming {
    totals: Arc<ExportTotals>,
    device: wgpu::Device,
    /// The stages are collected per thread, so this must not move to another.
    _same_thread: PhantomData<*const ()>,
}

impl ExportTiming {
    /// Counts the stages encoded from now on toward `phase`.
    pub(crate) fn phase(&self, phase: &'static str) {
        EXPORT.with(|export| {
            if let Some(export) = export.borrow_mut().as_mut() {
                export.phase = phase;
            }
        });
    }

    /// Waits for the times of every submitted stage, then records one line per
    /// phase.
    pub(crate) fn finish(self) {
        let deadline = Instant::now() + EXPORT_SETTLE_TIMEOUT;
        let mut state = self.totals.lock();
        while state.pending > 0 && Instant::now() < deadline {
            drop(state);
            // Map callbacks run in polls; another thread's poll may run them too.
            let _ = self.device.poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(Duration::from_millis(100)),
            });
            state = self.totals.lock();
            if state.pending > 0 {
                state = match self
                    .totals
                    .settled
                    .wait_timeout(state, Duration::from_millis(10))
                {
                    Ok((state, _)) => state,
                    Err(poisoned) => poisoned.into_inner().0,
                };
            }
        }
        for phase in &state.phases {
            calibraw_core::diagnostics::record(phase.summary());
        }
        if state.pending > 0 {
            calibraw_core::diagnostics::record(format!(
                "GPU export timing incomplete: {} stages reported no times",
                state.pending
            ));
        }
    }
}

impl Drop for ExportTiming {
    fn drop(&mut self) {
        EXPORT.with(|export| export.borrow_mut().take());
    }
}

/// The times of one stage: pass `i` started at `ticks[2i]` and ended at
/// `ticks[2i + 1]`.
struct StageTicks<'a> {
    passes: &'a [String],
    untimed: u32,
    ticks: &'a [u64],
    period_ns: f32,
}

#[derive(Default)]
struct ExportTotals {
    state: Mutex<ExportTotalsState>,
    settled: Condvar,
}

#[derive(Default)]
struct ExportTotalsState {
    /// Submitted stages whose times have not arrived.
    pending: usize,
    /// In the order the phases first submitted a stage.
    phases: Vec<PhaseTotals>,
}

struct PhaseTotals {
    phase: &'static str,
    stages: u32,
    /// Sum of the stages' spans (`stage_span`).
    milliseconds: f64,
    untimed: u32,
    /// Summed milliseconds per pass label, in the order first seen.
    passes: Vec<(String, f64)>,
}

impl ExportTotals {
    fn lock(&self) -> std::sync::MutexGuard<'_, ExportTotalsState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn add_pending(&self) {
        self.lock().pending += 1;
    }

    /// Adds a stage's times to `phase`, or only settles it when they could not
    /// be read.
    fn settle(&self, phase: &'static str, stage: Option<StageTicks<'_>>) {
        let mut state = self.lock();
        state.pending = state.pending.saturating_sub(1);
        if let Some(stage) = stage {
            let index = match state.phases.iter().position(|totals| totals.phase == phase) {
                Some(index) => index,
                None => {
                    state.phases.push(PhaseTotals {
                        phase,
                        stages: 0,
                        milliseconds: 0.0,
                        untimed: 0,
                        passes: Vec::new(),
                    });
                    state.phases.len() - 1
                }
            };
            state.phases[index].add(&stage);
        }
        drop(state);
        self.settled.notify_all();
    }
}

impl PhaseTotals {
    fn add(&mut self, stage: &StageTicks<'_>) {
        self.stages += 1;
        self.milliseconds += stage_span(stage.ticks, stage.period_ns);
        self.untimed += stage.untimed;
        for (label, pass) in stage.passes.iter().zip(stage.ticks.chunks_exact(2)) {
            let time = milliseconds(pass[0], pass[1], stage.period_ns);
            match self.passes.iter_mut().find(|(known, _)| known == label) {
                Some((_, total)) => *total += time,
                None => self.passes.push((label.clone(), time)),
            }
        }
    }

    /// The phase's line, passes slowest first.
    fn summary(&self) -> String {
        let mut passes: Vec<&(String, f64)> = self.passes.iter().collect();
        passes.sort_by(|a, b| b.1.total_cmp(&a.1));
        let noun = if self.stages == 1 { "stage" } else { "stages" };
        let mut line = format!(
            "GPU export {}: {:.2} ms in {} {noun}",
            self.phase, self.milliseconds, self.stages
        );
        if self.untimed > 0 {
            let _ = write!(line, " and {} untimed passes", self.untimed);
        }
        line.push_str(" (");
        for (index, (label, time)) in passes.into_iter().enumerate() {
            if index > 0 {
                line.push_str(", ");
            }
            let _ = write!(line, "{label} {time:.2}");
        }
        line.push(')');
        line
    }
}
