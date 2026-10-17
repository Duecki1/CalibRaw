//! The rule for where a session runs: on a GPU provider when the session fits
//! in the memory free on that provider's adapter, otherwise on the CPU. It is
//! a pure function of the session and the state it is given, so every model
//! on every platform follows it the same way.

use super::session::SessionOptions;
use std::fmt;

const MIB: u64 = 1 << 20;

/// What a session must find free beyond its measured peak, in percent of the
/// peak: the peaks come from one GPU and provider (see `registry`), and the
/// editor keeps allocating while AI runs.
const MEMORY_MARGIN_PERCENT: u64 = 20;

/// How much more memory than at its failure must be free before a model that
/// ran out of GPU memory goes back to the GPU, at least: free memory moves by
/// a few MiB between jobs without anything having been released.
const MIN_RETRY_GROWTH: u64 = 256 * MIB;

/// An accelerated execution provider: WebGPU on desktop. Android has none
/// (docs/GPU_REWORK.md P6.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuProvider {
    WebGpu,
}

impl GpuProvider {
    /// The name in diagnostics, statuses and the GPU job record.
    pub const fn name(self) -> &'static str {
        match self {
            Self::WebGpu => "WebGPU",
        }
    }
}

/// Where a session runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    Gpu { provider: GpuProvider },
    Cpu { reason: CpuReason },
}

impl Placement {
    /// The GPU provider's name, or `CPU`.
    pub const fn provider_name(self) -> &'static str {
        match self {
            Self::Gpu { provider } => provider.name(),
            Self::Cpu { .. } => "CPU",
        }
    }
}

/// Why a session runs on the CPU. `place` decides the first six; the last
/// two are where a session moves when its GPU provider fails.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CpuReason {
    /// The model's session options keep it on the CPU on this device (depth
    /// with `MemoryClass::Constrained`).
    CpuOnly,
    /// GPU AI is off in Settings.
    Disabled,
    /// GPU AI is off for this process after the GPU was lost during a job.
    Quarantined,
    /// The automatic runtime has no GPU build for this platform (macOS
    /// x86_64, Android).
    NoGpuBuild,
    /// The loaded ONNX Runtime has none of this platform's GPU providers, or
    /// its WebGPU provider found no adapter.
    NoGpuBackend,
    /// Every GPU provider's adapter has less memory free than the session
    /// needs, margin included; in bytes, for the first provider.
    InsufficientMemory { free: u64, needed: u64 },
    /// The GPU ran out of memory creating or running the model's session.
    /// The model goes back to the GPU once more memory is free than then, or,
    /// where free memory is not reported, after a backoff
    /// (`OutOfMemoryRecord`).
    OutOfMemory,
    /// Every GPU provider failed to create the session, or the one in use
    /// failed to run it, other than by running out of memory.
    GpuFailed,
}

impl CpuReason {
    /// Whether the GPU was short of memory, which may change by the next job.
    pub(crate) const fn memory_bound(self) -> bool {
        matches!(self, Self::InsufficientMemory { .. } | Self::OutOfMemory)
    }
}

impl fmt::Display for CpuReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::CpuOnly => formatter.write_str("CPU-only on this device"),
            Self::Disabled => formatter.write_str("GPU AI is off in Settings"),
            Self::Quarantined => formatter.write_str("GPU AI is off after the GPU was lost"),
            Self::NoGpuBuild => {
                formatter.write_str("no GPU build of ONNX Runtime for this platform")
            }
            Self::NoGpuBackend => formatter.write_str("the ONNX Runtime has no GPU provider"),
            Self::InsufficientMemory { free, needed } => write!(
                formatter,
                "{} MiB of GPU memory free, {} MiB needed",
                free / MIB,
                needed / MIB
            ),
            Self::OutOfMemory => formatter.write_str("the GPU ran out of memory"),
            Self::GpuFailed => formatter.write_str("the GPU provider failed"),
        }
    }
}

/// A GPU provider the loaded runtime offers, and what is known about the
/// adapter it runs on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProviderOffer {
    pub(crate) provider: GpuProvider,
    /// Bytes this process may still allocate on the provider's adapter, where
    /// that is known; an unknown amount does not keep a session off it.
    pub(crate) free_memory: Option<u64>,
    /// The editor's adapter name, when the provider runs on it.
    pub(crate) adapter: Option<String>,
}

/// A model that ran out of GPU memory in this process
/// (`placement::out_of_memory`), as `place` sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OutOfMemoryRecord {
    /// Bytes free on the provider's adapter when the model was placed on it
    /// before failing, where that was known.
    pub(crate) free_before: Option<u64>,
    /// The backoff after the last failure is over. It decides only where free
    /// memory is not known, now or at the failure.
    pub(crate) backoff_over: bool,
}

impl OutOfMemoryRecord {
    /// Whether the model may try a provider with `free` bytes free again:
    /// once free memory has grown since the failure by the margin of its
    /// measured peak `peak` (at least `MIN_RETRY_GROWTH`), or where either
    /// amount is unknown, once the backoff is over. Retrying sooner would
    /// rebuild the session on the GPU, fail and rebuild it on the CPU for
    /// every job.
    const fn allows(self, free: Option<u64>, peak: Option<u64>) -> bool {
        match (free, self.free_before) {
            (Some(free), Some(before)) => free >= before.saturating_add(retry_growth(peak)),
            _ => self.backoff_over,
        }
    }
}

/// The growth in free memory `OutOfMemoryRecord::allows` asks for.
const fn retry_growth(peak: Option<u64>) -> u64 {
    let margin = match peak {
        Some(peak) => needed_memory(peak) - peak,
        None => 0,
    };
    if margin > MIN_RETRY_GROWTH {
        margin
    } else {
        MIN_RETRY_GROWTH
    }
}

/// Whether a GPU session of a model with measured peak `peak` may stay
/// loaded after its job, with `free` bytes now free on its adapter: while
/// the margin `place` asked for beside the peak still is, which is the room
/// placement left the editor. Placing the model again needs that margin on
/// top of the peak unloading returns, so a model unloaded for want of it
/// does not come straight back. Unknown free memory or peak does not unload
/// a session, as neither keeps one off the GPU.
pub(crate) const fn may_stay_loaded(free: Option<u64>, peak: Option<u64>) -> bool {
    match (free, peak) {
        (Some(free), Some(peak)) => free >= needed_memory(peak) - peak,
        _ => true,
    }
}

/// What `place` knows besides the session.
#[derive(Clone, Copy, Debug)]
pub(crate) struct AccelerationState<'a> {
    /// GPU AI is on in Settings; always on Android.
    pub(crate) enabled: bool,
    /// GPU AI is off for this process after the GPU was lost.
    pub(crate) quarantined: bool,
    /// The runtime has a GPU build for this platform; false only for an
    /// automatic runtime without one.
    pub(crate) gpu_build: bool,
    /// The GPU providers the loaded runtime offers, most preferred first.
    pub(crate) providers: &'a [ProviderOffer],
    /// Whether and how the model ran out of GPU memory before.
    pub(crate) out_of_memory: Option<OutOfMemoryRecord>,
}

/// The memory a session must find free for a measured peak of `peak` bytes.
pub(crate) const fn needed_memory(peak: u64) -> u64 {
    peak.saturating_add(peak / 100 * MEMORY_MARGIN_PERCENT)
}

/// Places a session: on the first offered provider whose adapter has room
/// for it, or whose free memory or the session's need is unknown, otherwise
/// on the CPU. A model that ran out of GPU memory before skips providers its
/// `OutOfMemoryRecord` does not allow yet.
pub(crate) fn place(session: &SessionOptions, state: &AccelerationState<'_>) -> Placement {
    let reason = if !session.allow_acceleration {
        CpuReason::CpuOnly
    } else if !state.enabled {
        CpuReason::Disabled
    } else if state.quarantined {
        CpuReason::Quarantined
    } else if !state.gpu_build {
        CpuReason::NoGpuBuild
    } else {
        let needed = session.gpu_memory_bytes.map(needed_memory);
        let mut shortfall = None;
        for offer in state.providers {
            if let Some(record) = state.out_of_memory {
                if !record.allows(offer.free_memory, session.gpu_memory_bytes) {
                    shortfall.get_or_insert(CpuReason::OutOfMemory);
                    continue;
                }
            }
            match (offer.free_memory, needed) {
                (Some(free), Some(needed)) if free < needed => {
                    shortfall.get_or_insert(CpuReason::InsufficientMemory { free, needed });
                }
                _ => {
                    return Placement::Gpu {
                        provider: offer.provider,
                    }
                }
            }
        }
        shortfall.unwrap_or(CpuReason::NoGpuBackend)
    };
    Placement::Cpu { reason }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer(provider: GpuProvider, free_mib: Option<u64>) -> ProviderOffer {
        ProviderOffer {
            provider,
            free_memory: free_mib.map(|mib| mib * MIB),
            adapter: None,
        }
    }

    const fn gpu(provider: GpuProvider) -> Placement {
        Placement::Gpu { provider }
    }

    const fn cpu(reason: CpuReason) -> Placement {
        Placement::Cpu { reason }
    }

    #[test]
    fn the_margin_is_a_fifth_of_the_peak() {
        assert_eq!(needed_memory(2900 * MIB), 3480 * MIB);
        assert_eq!(needed_memory(200 * MIB), 240 * MIB);
        assert_eq!(needed_memory(u64::MAX), u64::MAX);
    }

    struct Case {
        name: &'static str,
        session: SessionOptions,
        enabled: bool,
        quarantined: bool,
        gpu_build: bool,
        providers: Vec<ProviderOffer>,
        out_of_memory: Option<OutOfMemoryRecord>,
        expected: Placement,
    }

    /// A case with GPU AI on.
    fn case(
        name: &'static str,
        session: SessionOptions,
        providers: Vec<ProviderOffer>,
        expected: Placement,
    ) -> Case {
        Case {
            name,
            session,
            enabled: true,
            quarantined: false,
            gpu_build: true,
            providers,
            out_of_memory: None,
            expected,
        }
    }

    /// A case for a model that ran out of GPU memory with `free_before_mib`
    /// free.
    fn after_out_of_memory(free_before_mib: Option<u64>, backoff_over: bool, case: Case) -> Case {
        Case {
            out_of_memory: Some(OutOfMemoryRecord {
                free_before: free_before_mib.map(|mib| mib * MIB),
                backoff_over,
            }),
            ..case
        }
    }

    #[test]
    fn sessions_run_on_the_gpu_when_they_fit_and_on_the_cpu_otherwise() {
        use GpuProvider::WebGpu;
        let peak_2900 = SessionOptions::new("BiRefNet").with_gpu_memory(2900 * MIB);
        let unknown_peak = SessionOptions::new("SkySeg U2Net");
        let short = cpu(CpuReason::InsufficientMemory {
            free: 3000 * MIB,
            needed: 3480 * MIB,
        });
        let cases = [
            case(
                "fits",
                peak_2900,
                vec![offer(WebGpu, Some(4000))],
                gpu(WebGpu),
            ),
            case(
                "fits exactly, margin included",
                peak_2900,
                vec![offer(WebGpu, Some(3480))],
                gpu(WebGpu),
            ),
            case(
                "fits without the margin only",
                peak_2900,
                vec![offer(WebGpu, Some(3000))],
                short,
            ),
            case(
                "free memory unknown",
                peak_2900,
                vec![offer(WebGpu, None)],
                gpu(WebGpu),
            ),
            case(
                "peak unknown",
                unknown_peak,
                vec![offer(WebGpu, Some(1))],
                gpu(WebGpu),
            ),
            case(
                "the next provider runs on another adapter",
                peak_2900,
                vec![offer(WebGpu, Some(3000)), offer(WebGpu, None)],
                gpu(WebGpu),
            ),
            case(
                "no provider has room; the first shortfall is reported",
                peak_2900,
                vec![offer(WebGpu, Some(3000)), offer(WebGpu, Some(100))],
                short,
            ),
            case(
                "no GPU provider",
                peak_2900,
                Vec::new(),
                cpu(CpuReason::NoGpuBackend),
            ),
            Case {
                gpu_build: false,
                ..case(
                    "no GPU build for this platform",
                    peak_2900,
                    Vec::new(),
                    cpu(CpuReason::NoGpuBuild),
                )
            },
            Case {
                enabled: false,
                ..case(
                    "off in Settings",
                    peak_2900,
                    vec![offer(WebGpu, None)],
                    cpu(CpuReason::Disabled),
                )
            },
            Case {
                quarantined: true,
                ..case(
                    "off after a lost GPU",
                    peak_2900,
                    vec![offer(WebGpu, None)],
                    cpu(CpuReason::Quarantined),
                )
            },
            Case {
                enabled: false,
                quarantined: true,
                ..case(
                    "Settings come before a lost GPU",
                    peak_2900,
                    vec![offer(WebGpu, None)],
                    cpu(CpuReason::Disabled),
                )
            },
            Case {
                enabled: false,
                gpu_build: false,
                ..case(
                    "Settings come before the missing build",
                    peak_2900,
                    Vec::new(),
                    cpu(CpuReason::Disabled),
                )
            },
            case(
                "a CPU-only session",
                peak_2900.cpu_only(),
                vec![offer(WebGpu, None)],
                cpu(CpuReason::CpuOnly),
            ),
            // The margin of a 2900 MiB peak is 580 MiB.
            after_out_of_memory(
                Some(4000),
                true,
                case(
                    "ran out of memory; no more is free than then",
                    peak_2900,
                    vec![offer(WebGpu, Some(4500))],
                    cpu(CpuReason::OutOfMemory),
                ),
            ),
            after_out_of_memory(
                Some(4000),
                false,
                case(
                    "ran out of memory; the margin more is free now",
                    peak_2900,
                    vec![offer(WebGpu, Some(4580))],
                    gpu(WebGpu),
                ),
            ),
            after_out_of_memory(
                Some(1000),
                false,
                case(
                    "more memory is free, but not what the model needs",
                    peak_2900,
                    vec![offer(WebGpu, Some(1600))],
                    cpu(CpuReason::InsufficientMemory {
                        free: 1600 * MIB,
                        needed: 3480 * MIB,
                    }),
                ),
            ),
            after_out_of_memory(
                None,
                false,
                case(
                    "ran out of memory, free memory unknown, backoff running",
                    peak_2900,
                    vec![offer(WebGpu, None)],
                    cpu(CpuReason::OutOfMemory),
                ),
            ),
            after_out_of_memory(
                None,
                true,
                case(
                    "ran out of memory, free memory unknown, backoff over",
                    peak_2900,
                    vec![offer(WebGpu, None)],
                    gpu(WebGpu),
                ),
            ),
            after_out_of_memory(
                Some(4000),
                true,
                case(
                    "free memory known then but not now: the backoff decides",
                    unknown_peak,
                    vec![offer(WebGpu, None)],
                    gpu(WebGpu),
                ),
            ),
            Case {
                enabled: false,
                ..after_out_of_memory(
                    None,
                    true,
                    case(
                        "Settings come before a memory failure",
                        peak_2900,
                        vec![offer(WebGpu, None)],
                        cpu(CpuReason::Disabled),
                    ),
                )
            },
        ];
        for case in cases {
            let state = AccelerationState {
                enabled: case.enabled,
                quarantined: case.quarantined,
                gpu_build: case.gpu_build,
                providers: &case.providers,
                out_of_memory: case.out_of_memory,
            };
            assert_eq!(place(&case.session, &state), case.expected, "{}", case.name);
        }
    }

    #[test]
    fn a_retry_needs_the_peak_margin_or_256_mib_more_free_memory() {
        assert_eq!(retry_growth(Some(2900 * MIB)), 580 * MIB);
        assert_eq!(retry_growth(Some(400 * MIB)), MIN_RETRY_GROWTH);
        assert_eq!(retry_growth(None), MIN_RETRY_GROWTH);
        // The sum saturates instead of overflowing.
        let record = OutOfMemoryRecord {
            free_before: Some(u64::MAX - MIB),
            backoff_over: false,
        };
        assert!(record.allows(Some(u64::MAX), None));
        assert!(!record.allows(Some(u64::MAX - MIB), None));
    }

    /// The SAM 2.1 encoder holds about 2100 MiB once placed; its margin is
    /// 420 MiB.
    #[test]
    fn a_session_stays_loaded_while_its_margin_is_free() {
        let encoder = Some(2100 * MIB);
        assert!(may_stay_loaded(Some(500 * MIB), encoder));
        assert!(may_stay_loaded(Some(420 * MIB), encoder));
        assert!(!may_stay_loaded(Some(419 * MIB), encoder));
        assert!(!may_stay_loaded(Some(0), encoder));
        assert!(may_stay_loaded(Some(140 * MIB), Some(700 * MIB)));
        assert!(!may_stay_loaded(Some(139 * MIB), Some(700 * MIB)));
        assert!(may_stay_loaded(None, encoder));
        assert!(may_stay_loaded(Some(0), None));
    }

    /// Placed with just enough free, a model that uses its peak keeps its
    /// margin and stays. Once the editor took some of it, the model unloads
    /// and is not placed again until that much is free once more, so it does
    /// not reload for every job.
    #[test]
    fn a_model_unloaded_for_memory_does_not_come_straight_back() {
        let encoder = SessionOptions::new("SAM 2.1 encoder").with_gpu_memory(2100 * MIB);
        let peak = encoder.gpu_memory_bytes;
        let place_with = |free_mib: u64| {
            let providers = [offer(GpuProvider::WebGpu, Some(free_mib))];
            place(
                &encoder,
                &AccelerationState {
                    enabled: true,
                    quarantined: false,
                    gpu_build: true,
                    providers: &providers,
                    out_of_memory: None,
                },
            )
        };
        assert_eq!(place_with(2520), gpu(GpuProvider::WebGpu));
        assert!(may_stay_loaded(Some((2520 - 2100) * MIB), peak));

        // The editor took 100 MiB while the encoder was loaded.
        assert!(!may_stay_loaded(Some(320 * MIB), peak));
        assert_eq!(
            place_with(320 + 2100),
            cpu(CpuReason::InsufficientMemory {
                free: 2420 * MIB,
                needed: 2520 * MIB,
            })
        );
    }

    #[test]
    fn only_memory_shortfalls_are_worth_another_placement() {
        let memory_bound = [
            CpuReason::InsufficientMemory { free: 0, needed: 1 },
            CpuReason::OutOfMemory,
        ];
        for reason in memory_bound {
            assert!(reason.memory_bound(), "{reason:?}");
        }
        for reason in [
            CpuReason::CpuOnly,
            CpuReason::Disabled,
            CpuReason::Quarantined,
            CpuReason::NoGpuBuild,
            CpuReason::NoGpuBackend,
            CpuReason::GpuFailed,
        ] {
            assert!(!reason.memory_bound(), "{reason:?}");
        }
    }

    #[test]
    fn reasons_read_as_status_text() {
        let short = CpuReason::InsufficientMemory {
            free: 1200 * MIB,
            needed: 3480 * MIB,
        };
        assert_eq!(
            short.to_string(),
            "1200 MiB of GPU memory free, 3480 MiB needed"
        );
        assert_eq!(
            CpuReason::OutOfMemory.to_string(),
            "the GPU ran out of memory"
        );
    }
}
