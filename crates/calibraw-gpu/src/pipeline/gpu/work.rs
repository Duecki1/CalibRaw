//! GPU work cut into submissions that each run for a short time.
//!
//! A driver ends work that keeps the GPU busy too long in one piece: Adreno
//! resets the device when one submission runs for about two seconds, Mali
//! stops a job after a few. The UI's frames also wait behind whatever was
//! submitted before them. How long a pass runs depends on the GPU, the photo
//! and the edit, so it is measured rather than assumed per device:
//!
//! - A stage is encoded through a [`WorkEncoder`], which closes a command
//!   buffer (a *chunk*) whenever the passes in it are expected to run for
//!   [`CHUNK_TARGET`], and submits each chunk as it closes.
//! - A pass expected to run longer on its own is dispatched in bands of
//!   workgroup rows, one chunk each. Its entry point adds the band's first
//!   row to its invocation (`dispatch_id` in common.wgsl), so the pass
//!   writes the same texels whatever its bands.
//! - A pass that was never measured runs in bands of [`PROBE_WORKGROUPS`].
//! - Every chunk's time is measured when the GPU reports it done and kept
//!   per pass for the process ([`Costs`]).
//!
//! On a fast GPU a stage stays one submission, as before; a slow one gets as
//! many as its speed requires.

use super::resources::GpuBudgetReservation;
use super::wait::{wait_for_submission, GpuWait};
use crate::stage_timing::{self, StageTiming};
use std::collections::{HashMap, VecDeque};
use std::fmt::Write as _;
use std::hash::{Hash, Hasher};
use std::ops::{Deref, DerefMut};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The GPU time one submission is sized for. A twentieth of the shortest
/// driver limit known (Adreno's two seconds), because an estimate is only
/// corrected after a chunk ran longer than expected.
const CHUNK_TARGET: Duration = Duration::from_millis(100);

/// [`CHUNK_TARGET`], or `CALIBRAW_GPU_CHUNK_MS` milliseconds (fractions
/// allowed), which makes a fast GPU cut its work like a slow one to check
/// that bands render the same picture.
fn chunk_target() -> Duration {
    static TARGET: OnceLock<Duration> = OnceLock::new();
    *TARGET.get_or_init(|| {
        std::env::var("CALIBRAW_GPU_CHUNK_MS")
            .ok()
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|milliseconds| *milliseconds > 0.0)
            .map_or(CHUNK_TARGET, |milliseconds| {
                Duration::from_secs_f64(milliseconds / 1_000.0)
            })
    })
}

/// The workgroups in one band of a pass that was never measured: 16,000
/// pixels at 8x8 invocations. The slowest pass seen (X-Trans homogeneity on
/// an Adreno 830) needs over 4 microseconds per pixel, which lost the device
/// with bands of half a megapixel.
const PROBE_WORKGROUPS: u64 = 256;

/// A chunk that took this many times its estimate raises the estimates of
/// its passes.
const RAISE_ABOVE: f64 = 1.5;

/// Chunks reported done within this time of each other finished before the
/// same poll, so only their sum is known.
const SAME_POLL: Duration = Duration::from_millis(1);

/// Chunks that finished before one poll within this time have their
/// estimates lowered to their sum. Above it the sum says too little about
/// each pass: a later, larger render regroups them.
const LOWER_BELOW: Duration = Duration::from_millis(25);

/// The invocation rows of a workgroup of every banded pass.
const WORKGROUP_ROWS: u32 = 8;

/// The bytes of `DispatchBand` in common.wgsl.
const BAND_BYTES: u64 = 16;

/// How a program's dispatches are measured and cut.
#[derive(Clone, Copy, Debug)]
pub(super) struct PassWork {
    /// Identifies the program's entry point in [`Costs`].
    key: u64,
    /// Whether the entry point adds the band's first row to its invocation.
    banded: bool,
    /// Driver-specific execution bound, independent of timing estimates.
    max_band_workgroups: u64,
}

/// Conservative execution bound shared by banded processing and export geometry.
/// This identifies the measured driver, rather than imposing its workaround on all Mali GPUs.
pub(super) fn driver_workgroup_limit(adapter: &wgpu::AdapterInfo) -> Option<u64> {
    // SM-A336B, Mali-G68 r32p1: adaptive bands lose the device during
    // X100V export. Limiting recalculation alone moves the failure to
    // homogeneity; limiting Markesteijn still leaves large finish/NR
    // submissions. Keep every band conservative on this driver, without
    // changing shader arithmetic. The Xiaomi 11T reports the same driver
    // revision and device loss on export.
    let limited = adapter.backend == wgpu::Backend::Vulkan
        && adapter.vendor == 0x13b5
        && (adapter.name == "Mali-G68" || adapter.name == "Mali-G77 MC9")
        && adapter.driver_info.contains("r32p1");
    limited.then_some(PROBE_WORKGROUPS)
}

impl PassWork {
    /// The work of entry point `entry`, banded when its shader says so
    /// ([`super::shaders::entry_reads_dispatch_band`]).
    pub(super) fn for_entry(device: &wgpu::Device, entry: &str) -> Self {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        entry.hash(&mut hasher);
        let adapter = device.adapter_info();
        // On the Redmi BXM-8-256 two 48x5 map bands finished, then the
        // adaptive 48x26 dispatch stalled until the driver reset the GPU.
        let max_band_workgroups = if entry == "build_styled_glow_map_level"
            && adapter.backend == wgpu::Backend::Vulkan
            && adapter.vendor == 0x1010
            && adapter.device == 0x35010101
            && adapter.driver_info == "25.1@6715691"
        {
            PROBE_WORKGROUPS
        } else {
            driver_workgroup_limit(&adapter).unwrap_or(u64::MAX)
        };
        Self {
            key: hasher.finish(),
            banded: super::shaders::entry_reads_dispatch_band(entry),
            max_band_workgroups,
        }
    }

    /// The same entry point compiled as another program (an effect set, a
    /// demosaic variant), which has its own cost.
    fn variant(self, variant: u64) -> u64 {
        self.key ^ variant.wrapping_mul(0x9e37_79b9_7f4a_7c15)
    }
}

/// The first rows a band can start at, one uniform per workgroup row, bound
/// with a dynamic offset as `dispatch_band` (common.wgsl).
pub(super) struct DispatchBands {
    buffer: wgpu::Buffer,
    /// Bytes between two entries.
    stride: u32,
    entries: u32,
    _reservation: GpuBudgetReservation,
}

impl DispatchBands {
    /// Cover every supported 2D texture grid, including effect maps larger
    /// than the source image and padded pyramid levels. Bind groups retain
    /// this buffer, so its capacity cannot grow when an effect is enabled.
    pub(super) fn new(device: &wgpu::Device) -> anyhow::Result<Self> {
        let stride = device
            .limits()
            .min_uniform_buffer_offset_alignment
            .max(BAND_BYTES as u32);
        let entries = device
            .limits()
            .max_texture_dimension_2d
            .div_ceil(WORKGROUP_ROWS)
            .max(1);
        let reservation = GpuBudgetReservation::acquire_bytes(
            device,
            "dispatch band origins",
            u64::from(stride) * u64::from(entries),
        )?;
        let mut bytes = vec![0u8; stride as usize * entries as usize];
        for (entry, band) in bytes.chunks_exact_mut(stride as usize).enumerate() {
            let origin = [0u32, entry as u32 * WORKGROUP_ROWS, 0, 0];
            band[..BAND_BYTES as usize].copy_from_slice(bytemuck::cast_slice(&origin));
        }
        let buffer = super::resources::create_initialized_buffer(
            device,
            "calibraw dispatch bands",
            &bytes,
            wgpu::BufferUsages::UNIFORM,
        );
        Ok(Self {
            buffer,
            stride,
            entries,
            _reservation: reservation,
        })
    }

    pub(super) fn binding(&self, binding: u32) -> wgpu::BindGroupEntry<'_> {
        wgpu::BindGroupEntry {
            binding,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: &self.buffer,
                offset: 0,
                size: wgpu::BufferSize::new(BAND_BYTES),
            }),
        }
    }

    /// The dynamic offset of the band that starts at workgroup row `row`.
    fn offset(&self, row: u32) -> Option<u32> {
        (row < self.entries).then(|| row * self.stride)
    }
}

/// The layout entry of `dispatch_band` (common.wgsl).
pub(super) fn dispatch_band_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: true,
            min_binding_size: wgpu::BufferSize::new(BAND_BYTES),
        },
        count: None,
    }
}

/// Part of a pass in a chunk.
#[derive(Clone, Copy)]
struct Share {
    key: u64,
    workgroups: u64,
}

/// One compute pass of the processing pipeline: group 0 is the pass's own
/// bind group, 1 the scene-tone uniforms with the dispatch band, 2 the
/// effects uniforms.
pub(super) struct PassDispatch<'a> {
    pub(super) label: &'a str,
    pub(super) pipeline: &'a wgpu::ComputePipeline,
    pub(super) bind_groups: [&'a wgpu::BindGroup; 3],
    pub(super) workgroups: [u32; 3],
    /// The band origins bound in group 1.
    pub(super) bands: &'a DispatchBands,
    pub(super) work: PassWork,
    /// Which program of the entry point `pipeline` is; 0 for its only one.
    pub(super) variant: u64,
}

/// Maximum submitted processing chunks per pipeline, across stages and repaints.
/// wgpu keeps the command pool of every encoder that was ever alive at the
/// same time and never frees it, and a pool holds command buffers in batches
/// of 16 (about 2.5 MB of GPU memory on Adreno), so the number alive together
/// has to stay small. Encoding a whole stage first left 5,000 command buffers
/// (800 MB) behind after one export, and the driver then failed to allocate.
const MAX_IN_FLIGHT: usize = 12;

/// Where a [`WorkEncoder`]'s chunks go.
enum Target<'a> {
    /// Each chunk is submitted when it closes. `paced` also waits whenever
    /// [`CHUNK_TARGET`] of work is outstanding, which a worker thread does so
    /// that the UI's frames reach the GPU between its chunks.
    Queue {
        queue: &'a wgpu::Queue,
        submissions: &'a Mutex<VecDeque<wgpu::SubmissionIndex>>,
        paced: bool,
    },
    /// Nothing is submitted: one command buffer that is dropped, to compile
    /// the programs a stage uses.
    Discard,
}

/// Encodes a stage as chunks and submits each as soon as it is closed.
/// Commands other than passes go to the open chunk through `Deref`, in order
/// with the passes.
pub(super) struct WorkEncoder<'a> {
    device: &'a wgpu::Device,
    target: Target<'a>,
    label: &'static str,
    encoder: wgpu::CommandEncoder,
    open: Vec<Share>,
    open_description: String,
    /// The estimate of the open chunk; `None` with an unmeasured pass.
    open_estimate: Option<Duration>,
    /// The estimate of what was submitted since the last wait.
    outstanding: Duration,
}

impl Deref for WorkEncoder<'_> {
    type Target = wgpu::CommandEncoder;

    fn deref(&self) -> &wgpu::CommandEncoder {
        &self.encoder
    }
}

impl DerefMut for WorkEncoder<'_> {
    fn deref_mut(&mut self) -> &mut wgpu::CommandEncoder {
        &mut self.encoder
    }
}

impl<'a> WorkEncoder<'a> {
    /// A stage whose chunks go to `queue`. `paced` is for a worker thread
    /// ([`Target::Queue`]); the UI thread waits only at [`MAX_IN_FLIGHT`].
    pub(super) fn new(
        device: &'a wgpu::Device,
        queue: &'a wgpu::Queue,
        label: &'static str,
        submissions: &'a Mutex<VecDeque<wgpu::SubmissionIndex>>,
        paced: bool,
    ) -> Self {
        Self::with_target(
            device,
            Target::Queue {
                queue,
                submissions,
                paced,
            },
            label,
        )
    }

    /// A stage that is encoded and dropped ([`Target::Discard`]).
    pub(super) fn discarded(device: &'a wgpu::Device, label: &'static str) -> Self {
        Self::with_target(device, Target::Discard, label)
    }

    fn with_target(device: &'a wgpu::Device, target: Target<'a>, label: &'static str) -> Self {
        Self {
            device,
            target,
            label,
            encoder: Self::begin(device, label),
            open: Vec::new(),
            open_description: String::new(),
            open_estimate: Some(Duration::ZERO),
            outstanding: Duration::ZERO,
        }
    }

    fn begin(device: &wgpu::Device, label: &'static str) -> wgpu::CommandEncoder {
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(label) })
    }

    /// Closes the open chunk and submits it, so what is encoded next is its
    /// own submission.
    pub(super) fn cut(&mut self) {
        if matches!(self.target, Target::Discard) || crate::gpu_errors::device_was_lost() {
            return;
        }
        let encoder = std::mem::replace(&mut self.encoder, Self::begin(self.device, self.label));
        self.submit_chunk(encoder);
    }

    /// Submits `encoder`, which holds the open chunk.
    fn submit_chunk(&mut self, encoder: wgpu::CommandEncoder) {
        // A failed pacing wait may have published terminal device loss.
        // Do not submit the rest of this stage to an invalid device; its
        // readback and the UI's recovery report the original failure.
        if crate::gpu_errors::device_was_lost() {
            return;
        }
        let shares = std::mem::take(&mut self.open);
        let description = std::mem::take(&mut self.open_description);
        let estimate = self.open_estimate.replace(Duration::ZERO);
        let Target::Queue {
            queue,
            submissions,
            paced,
        } = self.target
        else {
            return;
        };
        // Serialize admission and submission for this graph. Completion and
        // device-loss callbacks do not take this lock.
        let mut pending = submissions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if pending.len() == MAX_IN_FLIGHT {
            if !self.wait_for_chunk(&pending[0], "GPU work in flight") {
                return;
            }
            pending.pop_front();
        }
        if crate::gpu_errors::device_was_lost() {
            return;
        }
        let submission = queue.submit(Some(encoder.finish()));
        if !shares.is_empty() {
            self.outstanding += estimate.unwrap_or_else(chunk_target);
            let submitted = Instant::now();
            let in_flight = InFlight::begin(description, submitted);
            queue.on_submitted_work_done(move || {
                let ran = costs().chunk_done(&shares, estimate, submitted, Instant::now());
                in_flight.finish(ran);
            });
        }
        if paced && self.outstanding >= chunk_target() {
            if self.wait_for_chunk(&submission, "paced GPU work") {
                pending.clear();
                self.outstanding = Duration::ZERO;
            }
        } else {
            pending.push_back(submission);
        }
    }

    /// A required wait that fails ends processing on this device. Continuing
    /// would either exceed the limit or present a partially rendered stage.
    fn wait_for_chunk(&self, submission: &wgpu::SubmissionIndex, operation: &str) -> bool {
        match wait_for_submission(self.device, submission, operation, GpuWait::Limited) {
            Ok(()) => true,
            Err(error) => {
                crate::gpu_errors::report_device_failure(format!("{operation}: {error:?}"));
                false
            }
        }
    }

    /// Encodes passes whose cost is not tracked (the scene bake, with
    /// `dispatch_compute`). Where `encode` dispatched any, the chunk ends
    /// with them and its estimate is unknown.
    pub(super) fn untracked<R>(
        &mut self,
        encode: impl FnOnce(&mut wgpu::CommandEncoder) -> R,
    ) -> R {
        let before = super::resources::untracked_dispatches();
        let result = encode(&mut self.encoder);
        if super::resources::untracked_dispatches() != before {
            self.open_estimate = None;
            self.cut();
        }
        result
    }

    /// Encodes `pass`, in bands where it is expected to run longer than
    /// [`CHUNK_TARGET`] as a whole. Each band is sized from what is known
    /// when it is encoded, so a pass measured while its first bands ran
    /// takes the rest in bands of the right size.
    pub(super) fn dispatch(&mut self, pass: PassDispatch<'_>) {
        let [columns, rows, layers] = pass.workgroups;
        if columns == 0 || rows == 0 || layers == 0 {
            return;
        }
        let target = chunk_target();
        let key = pass.work.variant(pass.variant);
        let row_workgroups = u64::from(columns) * u64::from(layers);
        let banded = pass.work.banded && !matches!(self.target, Target::Discard);

        let mut row = 0;
        while row < rows {
            if crate::gpu_errors::device_was_lost() {
                return;
            }
            // Every banded grid is a 2D texture bounded by the device limit
            // used to allocate the table. Never enlarge a band to hide a
            // missing origin: that would bypass both execution bounds.
            let offset = pass
                .bands
                .offset(row)
                .expect("dispatch band table covers every supported 2D texture row");
            let left = rows - row;
            let cost = costs().per_workgroup(key);
            let estimate = |band_rows: u32| {
                cost.map(|nanoseconds| {
                    Duration::from_nanos(
                        (nanoseconds * (row_workgroups * u64::from(band_rows)) as f64) as u64,
                    )
                })
            };
            let mut band = if banded {
                match estimate(left) {
                    Some(rest) if rest <= target => left,
                    Some(rest) => {
                        let share = target.as_secs_f64() / rest.as_secs_f64();
                        ((f64::from(left) * share) as u32).clamp(1, left)
                    }
                    None => u32::try_from(PROBE_WORKGROUPS / row_workgroups)
                        .unwrap_or(u32::MAX)
                        .clamp(1, left),
                }
            } else {
                left
            };
            if banded {
                let limit = (pass.work.max_band_workgroups / row_workgroups).max(1);
                band = band.min(u32::try_from(limit).unwrap_or(u32::MAX));
            }
            let band_estimate = estimate(band);
            let whole_pass = row == 0 && band == rows;
            // A band is a chunk of its own; whole passes share a chunk until
            // it reaches the target.
            let fits = match (self.open_estimate, band_estimate) {
                (Some(open), Some(band)) => whole_pass && open + band <= target,
                _ => false,
            };
            if !fits && !self.open.is_empty() {
                self.cut();
            }
            self.encode_band(&pass, offset, [columns, band, layers]);
            self.open.push(Share {
                key,
                workgroups: row_workgroups * u64::from(band),
            });
            if !self.open_description.is_empty() {
                self.open_description.push_str(", ");
            }
            let _ = write!(
                self.open_description,
                "{} {columns}x{band}x{layers}",
                pass.label
            );
            self.open_estimate = match (self.open_estimate, band_estimate) {
                (Some(open), Some(band)) => Some(open + band),
                _ => None,
            };
            if !whole_pass || band_estimate.is_none() {
                self.cut();
            }
            row += band;
        }
    }

    fn encode_band(&mut self, pass: &PassDispatch<'_>, band_offset: u32, workgroups: [u32; 3]) {
        let timestamps = stage_timing::next_pass(pass.label);
        let mut compute = self
            .encoder
            .begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some(pass.label),
                timestamp_writes: timestamps
                    .as_ref()
                    .map(stage_timing::PassTimestamps::writes),
            });
        compute.set_pipeline(pass.pipeline);
        compute.set_bind_group(0, pass.bind_groups[0], &[]);
        compute.set_bind_group(1, pass.bind_groups[1], &[band_offset]);
        compute.set_bind_group(2, pass.bind_groups[2], &[]);
        compute.dispatch_workgroups(workgroups[0], workgroups[1], workgroups[2]);
    }

    /// Submits the last chunk. `timing` timed the stage's passes.
    pub(super) fn submit(mut self, timing: Option<StageTiming>) {
        if crate::gpu_errors::device_was_lost() {
            return;
        }
        let pending = stage_timing::resolve(timing, &mut self.encoder);
        let encoder = std::mem::replace(&mut self.encoder, Self::begin(self.device, self.label));
        self.submit_chunk(encoder);
        if let Some(pending) = pending {
            pending.report();
        }
    }
}

/// A chunk that ran at most this long ([`CostState::chunk_done`]'s bound)
/// or longer is recorded in diagnostics with its passes.
const SLOW_CHUNK: Duration = Duration::from_millis(500);

/// A chunk the GPU has not reported done.
pub(super) struct InFlight {
    id: u64,
}

struct InFlightChunk {
    id: u64,
    description: String,
    submitted: Instant,
}

fn in_flight() -> std::sync::MutexGuard<'static, Vec<InFlightChunk>> {
    static IN_FLIGHT: Mutex<Vec<InFlightChunk>> = Mutex::new(Vec::new());
    // Entries are added and removed whole.
    IN_FLIGHT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl InFlight {
    pub(super) fn begin(description: String, submitted: Instant) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        calibraw_core::diagnostics::trace(format!(
            "GPU chunk submit id={id} thread={:?}: {description}",
            std::thread::current().id(),
        ));
        in_flight().push(InFlightChunk {
            id,
            description,
            submitted,
        });
        Self { id }
    }

    /// Forgets the chunk, which ran for at most `ran`.
    pub(super) fn finish(self, ran: Duration) {
        let chunk = {
            let mut chunks = in_flight();
            chunks
                .iter()
                .position(|chunk| chunk.id == self.id)
                .map(|index| chunks.remove(index))
        };
        if let Some(chunk) = chunk {
            calibraw_core::diagnostics::trace(format!(
                "GPU chunk complete id={} elapsed_upper_ms={:.1}: {}",
                chunk.id,
                ran.as_secs_f64() * 1_000.0,
                chunk.description,
            ));
            if ran >= SLOW_CHUNK {
                calibraw_core::diagnostics::record(format!(
                    "GPU chunk ran up to {:.0} ms: {}",
                    ran.as_secs_f64() * 1_000.0,
                    chunk.description
                ));
            }
        }
    }
}

/// The chunks the GPU has not reported done, oldest first, for the record of
/// a lost device: "submitted 2140 ms ago: xtrans_markesteijn_homogeneity
/// 229x36x1".
pub(crate) fn in_flight_summary() -> Option<String> {
    let chunks = in_flight();
    if chunks.is_empty() {
        return None;
    }
    let now = Instant::now();
    let mut summary = String::new();
    for chunk in chunks.iter().take(8) {
        if !summary.is_empty() {
            summary.push_str("; ");
        }
        let _ = write!(
            summary,
            "submitted {:.0} ms ago: {}",
            now.saturating_duration_since(chunk.submitted).as_secs_f64() * 1_000.0,
            chunk.description
        );
    }
    if chunks.len() > 8 {
        let _ = write!(summary, "; and {} more", chunks.len() - 8);
    }
    Some(summary)
}

/// What the GPU needs per workgroup of each program, measured from the
/// chunks it finished.
///
/// The GPU reports a chunk done when the device is next polled, so a chunk's
/// time is only bounded: it ran between the poll before the previous report
/// and this one. Estimates are those upper bounds. They rise when a chunk
/// takes [`RAISE_ABOVE`] times its estimate and fall to a smaller bound, so
/// an error makes submissions shorter than needed, not longer.
struct Costs {
    state: Mutex<CostState>,
}

#[derive(Default)]
struct CostState {
    /// Nanoseconds per workgroup, by [`PassWork::variant`].
    per_workgroup: HashMap<u64, f64>,
    /// When the last chunk was reported done.
    last_done: Option<Instant>,
    /// When the last chunk of the group before the current one was reported.
    previous_group_done: Option<Instant>,
    /// The earliest the current group's work can have started.
    group_floor: Option<Instant>,
    group_started: Option<Instant>,
    /// The chunks reported done by the same poll as the last one.
    group: Vec<Share>,
}

fn costs() -> &'static Costs {
    static COSTS: OnceLock<Costs> = OnceLock::new();
    COSTS.get_or_init(|| Costs {
        state: Mutex::default(),
    })
}

impl Costs {
    fn lock(&self) -> std::sync::MutexGuard<'_, CostState> {
        // Every update leaves the table usable, so a panic elsewhere cannot
        // leave it half written.
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn per_workgroup(&self, key: u64) -> Option<f64> {
        self.lock().per_workgroup.get(&key).copied()
    }

    fn chunk_done(
        &self,
        shares: &[Share],
        estimate: Option<Duration>,
        submitted: Instant,
        now: Instant,
    ) -> Duration {
        self.lock().chunk_done(shares, estimate, submitted, now)
    }
}

impl CostState {
    fn chunk_done(
        &mut self,
        shares: &[Share],
        estimate: Option<Duration>,
        submitted: Instant,
        now: Instant,
    ) -> Duration {
        let same_poll = self
            .last_done
            .is_some_and(|last| now.saturating_duration_since(last) <= SAME_POLL);
        if !same_poll {
            self.close_group();
            // The work of this group started after the chunk before it
            // finished, which was after the poll that reported the group
            // before that.
            self.group_floor = self.previous_group_done;
            self.previous_group_done = self.last_done;
            self.group_started = None;
        }
        self.last_done = Some(now);
        let started = self
            .group_floor
            .map_or(submitted, |floor| floor.max(submitted));
        self.group_started.get_or_insert(started);
        let ran = now.saturating_duration_since(started);
        let bound = ran.as_nanos() as f64;

        // A chunk that ran longer than expected: its passes cost more than
        // was known, in unknown shares.
        if let Some(estimate) = estimate {
            let estimate = estimate.as_nanos() as f64;
            if estimate > 0.0 && bound > estimate * RAISE_ABOVE {
                let scale = bound / estimate;
                for key in distinct_keys(shares) {
                    if let Some(cost) = self.per_workgroup.get_mut(&key) {
                        *cost *= scale;
                    }
                }
            }
        }
        // No pass of the chunk took longer than the chunk.
        for share in shares {
            let limit = bound / share.workgroups.max(1) as f64;
            self.per_workgroup
                .entry(share.key)
                .and_modify(|cost| *cost = cost.min(limit))
                .or_insert(limit);
        }
        self.group.extend_from_slice(shares);
        ran
    }

    /// Lowers the estimates of a group that finished quickly to its total
    /// time: together its chunks took no longer.
    fn close_group(&mut self) {
        let group = std::mem::take(&mut self.group);
        let (Some(started), Some(done)) = (self.group_started, self.last_done) else {
            return;
        };
        let total = done.saturating_duration_since(started);
        if group.is_empty() || total > LOWER_BELOW {
            return;
        }
        let estimate: f64 = group
            .iter()
            .filter_map(|share| {
                self.per_workgroup
                    .get(&share.key)
                    .map(|cost| cost * share.workgroups as f64)
            })
            .sum();
        // A total below the clock's resolution says nothing.
        let total = total.as_nanos().max(100_000) as f64;
        if estimate > total {
            let scale = total / estimate;
            for key in distinct_keys(&group) {
                if let Some(cost) = self.per_workgroup.get_mut(&key) {
                    *cost *= scale;
                }
            }
        }
    }
}

fn distinct_keys(shares: &[Share]) -> Vec<u64> {
    let mut keys: Vec<u64> = shares.iter().map(|share| share.key).collect();
    keys.sort_unstable();
    keys.dedup();
    keys
}
