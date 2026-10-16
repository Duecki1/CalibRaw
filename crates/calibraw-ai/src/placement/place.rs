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

/// An accelerated execution provider: WebGPU on desktop, NNAPI and XNNPACK
/// on Android. XNNPACK is a CPU library, but Android treats it like a GPU
/// provider until docs/GPU_REWORK.md P6.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuProvider {
    WebGpu,
    Nnapi,
    Xnnpack,
}

impl GpuProvider {
    /// The name in diagnostics, statuses and the GPU job record.
    pub const fn name(self) -> &'static str {
        match self {
            Self::WebGpu => "WebGPU",
            Self::Nnapi => "NNAPI",
            Self::Xnnpack => "XNNPACK",
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
    /// x86_64).
    NoGpuBuild,
    /// The loaded ONNX Runtime has none of this platform's GPU providers, or
    /// its WebGPU provider found no adapter.
    NoGpuBackend,
    /// Every GPU provider's adapter has less memory free than the session
    /// needs, margin included; in bytes, for the first provider.
    InsufficientMemory { free: u64, needed: u64 },
    /// The GPU ran out of memory creating or running the session. The next
    /// job places the model again.
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
}

/// The memory a session must find free for a measured peak of `peak` bytes.
pub(crate) const fn needed_memory(peak: u64) -> u64 {
    peak.saturating_add(peak / 100 * MEMORY_MARGIN_PERCENT)
}

/// Places a session: on the first offered provider whose adapter has room
/// for it, or whose free memory or the session's need is unknown, otherwise
/// on the CPU.
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
            expected,
        }
    }

    #[test]
    fn sessions_run_on_the_gpu_when_they_fit_and_on_the_cpu_otherwise() {
        use GpuProvider::{Nnapi, WebGpu, Xnnpack};
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
                vec![offer(Nnapi, Some(3000)), offer(Xnnpack, None)],
                gpu(Xnnpack),
            ),
            case(
                "no provider has room; the first shortfall is reported",
                peak_2900,
                vec![offer(Nnapi, Some(3000)), offer(Xnnpack, Some(100))],
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
        ];
        for case in cases {
            let state = AccelerationState {
                enabled: case.enabled,
                quarantined: case.quarantined,
                gpu_build: case.gpu_build,
                providers: &case.providers,
            };
            assert_eq!(place(&case.session, &state), case.expected, "{}", case.name);
        }
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
