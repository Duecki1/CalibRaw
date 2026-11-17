//! How much GPU memory the process may commit to resident resources, and how
//! much of it is free now.
//!
//! Windows reports a per-process video memory budget through DXGI, but the
//! driver only refuses allocations once that budget is spent, and on an
//! integrated GPU a spike can take system memory the rest of the machine
//! needs. The admission limit keeps pipelines below that point: a share of the
//! budget the adapter reports free at startup, capped by a share of installed
//! RAM on adapters that render from it. Every reservation is also checked
//! against what the adapter and the system have free now.
//!
//! Android sizes its own budgets from device RAM (`gpu_working_set_limit_bytes`
//! and `gpu_admission_limit_bytes`); other desktops leave allocation to the
//! driver, whose OOM the GPU error scopes report.

#[cfg(target_os = "windows")]
use super::dxgi::{AdapterKey, AdapterMemoryReport};
use wgpu::DeviceType;

#[cfg(target_os = "windows")]
use std::sync::OnceLock;

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;

/// Upper bound of the admission limit on a discrete GPU: enough for the
/// largest previews and export tiles, while leaving the rest of a large card
/// to other processes.
#[cfg(target_os = "windows")]
const DISCRETE_CEILING_BYTES: u64 = 12 * GIB;
/// Share of the memory DXGI reports free, as a fraction, an integrated or
/// software adapter may commit: three quarters.
#[cfg(target_os = "windows")]
const SHARED_ADAPTER_FREE_NUMERATOR: u64 = 3;
#[cfg(target_os = "windows")]
const SHARED_ADAPTER_FREE_DENOMINATOR: u64 = 4;
/// Share of installed RAM an integrated adapter may commit: three eighths.
#[cfg(target_os = "windows")]
const SHARED_ADAPTER_RAM_NUMERATOR: u64 = 3;
#[cfg(target_os = "windows")]
const SHARED_ADAPTER_RAM_DENOMINATOR: u64 = 8;
/// Share of the memory DXGI reports free, as a fraction, a discrete adapter
/// may commit: seven eighths.
#[cfg(target_os = "windows")]
const DEDICATED_ADAPTER_FREE_NUMERATOR: u64 = 7;
#[cfg(target_os = "windows")]
const DEDICATED_ADAPTER_FREE_DENOMINATOR: u64 = 8;

/// Without a DXGI budget, the admission limit is a sixth of installed RAM,
/// between these bounds; 1.5 GiB when RAM is unknown. The result keeps a
/// machine usable even where DXGI cannot answer for the adapter wgpu chose.
#[cfg(target_os = "windows")]
const FALLBACK_RAM_DIVISOR: u64 = 6;
#[cfg(target_os = "windows")]
const FALLBACK_MIN_BYTES: u64 = 512 * MIB;
#[cfg(target_os = "windows")]
const FALLBACK_MAX_BYTES: u64 = 3 * GIB;
#[cfg(target_os = "windows")]
const UNKNOWN_RAM_CEILING_BYTES: u64 = 1536 * MIB;

/// Headroom kept free beyond the reservations, at least: the driver's own
/// allocations, the swap chain, and memory other processes take while the
/// editor runs, as a sixteenth of the limit.
#[cfg(target_os = "windows")]
const RESERVE_DIVISOR: u64 = 16;
#[cfg(target_os = "windows")]
const MIN_RESERVE_BYTES: u64 = 128 * MIB;
/// Share of the system memory reported free, as a divisor, an integrated
/// adapter's live headroom may use: half.
#[cfg(target_os = "windows")]
const SYSTEM_FREE_SHARE_DIVISOR: u64 = 2;

/// A developer override of the desktop admission limit, in MiB. `0` disables
/// the limit. An invalid value is ignored.
#[cfg(target_os = "windows")]
const LIMIT_OVERRIDE_ENV: &str = "CALIBRAW_GPU_ADMISSION_LIMIT_MIB";

#[cfg(target_os = "windows")]
static POLICY: OnceLock<AdmissionPolicy> = OnceLock::new();

/// Reads the process's admission policy from `device` once. Call it before
/// building pipelines; every later allocation query uses the stored policy.
/// A platform without a policy ignores the call.
pub fn initialize_admission_policy(device: &wgpu::Device) {
    #[cfg(target_os = "windows")]
    {
        POLICY.get_or_init(|| AdmissionPolicy::for_device(device));
    }
    #[cfg(not(target_os = "windows"))]
    let _ = device;
}

/// Bytes resident GPU resources may commit together, or `None` where the
/// process has no admission limit. The policy must have been initialized
/// ([`initialize_admission_policy`]).
pub fn admission_limit_bytes() -> Option<u64> {
    #[cfg(target_os = "windows")]
    {
        POLICY.get().and_then(|policy| policy.ceiling)
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

/// Bytes resident resources may still reserve now, after `reserved` bytes of
/// them: the limit left, the adapter's live free budget less a reserve, and on
/// an integrated adapter half of the system memory reported free. `None` where
/// the process has no admission limit.
pub fn admission_headroom_bytes(reserved: u64) -> Option<u64> {
    #[cfg(target_os = "windows")]
    {
        POLICY
            .get()
            .and_then(|policy| policy.headroom(reserved))
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = reserved;
        None
    }
}

#[cfg(target_os = "windows")]
struct AdmissionPolicy {
    /// `None` when the developer override lifted the limit.
    ceiling: Option<u64>,
    adapter: AdapterKey,
    /// Whether the adapter renders from system memory, which bounds its
    /// headroom by the system's free memory too.
    shared_memory: bool,
}

#[cfg(target_os = "windows")]
impl AdmissionPolicy {
    fn for_device(device: &wgpu::Device) -> Self {
        let info = device.adapter_info();
        let adapter = AdapterKey {
            vendor_id: info.vendor,
            device_id: info.device,
            device_type: info.device_type,
        };
        let report = match super::dxgi::memory_for_adapter(adapter) {
            Ok(report) => report,
            Err(error) => {
                log::warn!("could not query DXGI adapter memory: {error}");
                None
            }
        };
        let shared_memory = matches!(
            info.device_type,
            DeviceType::IntegratedGpu | DeviceType::Cpu
        );
        let total_ram = calibraw_core::system_memory::total_memory_bytes();
        let ceiling = match configured_ceiling() {
            ConfiguredCeiling::Unlimited => None,
            ConfiguredCeiling::Bytes(bytes) => Some(bytes),
            ConfiguredCeiling::Unset => Some(default_ceiling(
                report.map(|report| report.usable),
                shared_memory,
                total_ram,
            )),
        };
        record_policy(&info, shared_memory, report, ceiling, total_ram);
        Self {
            ceiling,
            adapter,
            shared_memory,
        }
    }

    fn headroom(&self, reserved: u64) -> Option<u64> {
        let ceiling = self.ceiling?;
        let reserve = (ceiling / RESERVE_DIVISOR).max(MIN_RESERVE_BYTES);
        let process_left = ceiling.saturating_sub(reserved);
        let adapter_left = match super::dxgi::memory_for_adapter(self.adapter) {
            Ok(Some(report)) => report.usable.available().saturating_sub(reserve),
            Ok(None) => u64::MAX,
            Err(error) => {
                log::warn!("could not query DXGI adapter memory: {error}");
                u64::MAX
            }
        };
        let system_left = if self.shared_memory {
            calibraw_core::system_memory::available_memory_bytes()
                .map_or(u64::MAX, |available| available / SYSTEM_FREE_SHARE_DIVISOR)
        } else {
            u64::MAX
        };
        Some(process_left.min(adapter_left).min(system_left))
    }
}

#[cfg(target_os = "windows")]
enum ConfiguredCeiling {
    Unset,
    Unlimited,
    Bytes(u64),
}

#[cfg(target_os = "windows")]
fn configured_ceiling() -> ConfiguredCeiling {
    let Ok(raw) = std::env::var(LIMIT_OVERRIDE_ENV) else {
        return ConfiguredCeiling::Unset;
    };
    match raw.trim().parse::<u64>() {
        Ok(0) => ConfiguredCeiling::Unlimited,
        Ok(mib) => ConfiguredCeiling::Bytes(mib.saturating_mul(MIB)),
        Err(_) => {
            log::warn!(
                "ignoring {LIMIT_OVERRIDE_ENV}={raw:?}: it is not a whole number of MiB"
            );
            ConfiguredCeiling::Unset
        }
    }
}

/// The limit for an adapter when no override applies.
#[cfg(target_os = "windows")]
fn default_ceiling(
    dxgi: Option<super::AdapterMemory>,
    shared_memory: bool,
    total_ram: Option<u64>,
) -> u64 {
    let Some(available) = dxgi
        .filter(|memory| memory.available() > 0)
        .map(super::AdapterMemory::available)
    else {
        return fallback_ceiling(total_ram);
    };
    if shared_memory {
        let ram_share = total_ram.map_or_else(
            || fallback_ceiling(None),
            |ram| ram.saturating_mul(SHARED_ADAPTER_RAM_NUMERATOR) / SHARED_ADAPTER_RAM_DENOMINATOR,
        );
        // Never above the RAM share: an integrated adapter's budget is
        // system memory, and the rest of the machine needs it too.
        (available.saturating_mul(SHARED_ADAPTER_FREE_NUMERATOR)
            / SHARED_ADAPTER_FREE_DENOMINATOR)
            .min(ram_share)
    } else {
        // A large card keeps the rest of its VRAM for other processes.
        (available.saturating_mul(DEDICATED_ADAPTER_FREE_NUMERATOR)
            / DEDICATED_ADAPTER_FREE_DENOMINATOR)
            .min(DISCRETE_CEILING_BYTES)
    }
}

#[cfg(target_os = "windows")]
fn fallback_ceiling(total_ram: Option<u64>) -> u64 {
    total_ram.map_or(UNKNOWN_RAM_CEILING_BYTES, |ram| {
        (ram / FALLBACK_RAM_DIVISOR).clamp(FALLBACK_MIN_BYTES, FALLBACK_MAX_BYTES)
    })
}

#[cfg(target_os = "windows")]
fn record_policy(
    info: &wgpu::AdapterInfo,
    shared_memory: bool,
    report: Option<AdapterMemoryReport>,
    ceiling: Option<u64>,
    total_ram: Option<u64>,
) {
    let kind = if shared_memory { "shared" } else { "dedicated" };
    let dxgi = report.map_or_else(
        || "DXGI reported no memory for the adapter".to_owned(),
        |report| {
            format!(
                "DXGI {} segment {} MiB budget / {} MiB used (local {} / {}, non-local {} / {}; dedicated video {} MiB, shared system {} MiB)",
                report.segment.label(),
                report.usable.budget / MIB,
                report.usable.usage / MIB,
                report.local.budget / MIB,
                report.local.usage / MIB,
                report.non_local.budget / MIB,
                report.non_local.usage / MIB,
                report.dedicated_video / MIB,
                report.shared_system / MIB,
            )
        },
    );
    let system = format!(
        "system memory {} MiB free of {} MiB",
        calibraw_core::system_memory::available_memory_bytes().map_or_else(
            || "unknown".to_owned(),
            |bytes| (bytes / MIB).to_string()
        ),
        total_ram.map_or_else(|| "unknown".to_owned(), |bytes| (bytes / MIB).to_string()),
    );
    match ceiling {
        Some(ceiling) => {
            let reserve = (ceiling / RESERVE_DIVISOR).max(MIN_RESERVE_BYTES);
            calibraw_core::diagnostics::record(format!(
                "GPU admission limit {} MiB ({kind} adapter \"{}\", reserve {} MiB): {dxgi}; {system}",
                ceiling / MIB,
                info.name,
                reserve / MIB,
            ));
        }
        None => calibraw_core::diagnostics::record(format!(
            "GPU admission limit disabled by {LIMIT_OVERRIDE_ENV}=0 (adapter \"{}\"): {dxgi}; {system}",
            info.name,
        )),
    }
}
