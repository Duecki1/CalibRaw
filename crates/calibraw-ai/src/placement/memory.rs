//! CPU admission against iOS's process memory allowance, and OpenVINO
//! admission against the RAM the system has available. A failed allocation
//! can terminate the process before ONNX Runtime can return an error.

use super::session::SessionOptions;
use anyhow::Result;

const MIB: u64 = 1 << 20;
// CPU peaks are measured with the built-in runtime and include session load.
// Keep room for editor allocations while inference runs. The GPU's percentage
// margin also covers other providers and is too large for this process budget.
const CPU_MEMORY_RESERVE: u64 = 256 * MIB;

/// Copies of a model's weights an OpenVINO session holds while it compiles,
/// beyond the one ONNX Runtime's CPU provider holds: the serialized model
/// handed to OpenVINO, OpenVINO's constants and its CPU plugin's repacked
/// weights. An estimate: OpenVINO's peaks are not measured yet.
const OPENVINO_EXTRA_WEIGHT_COPIES: u64 = 3;

const fn needed_memory(peak: u64) -> u64 {
    peak.saturating_add(CPU_MEMORY_RESERVE)
}

pub(super) fn ensure_cpu_memory_available(options: &SessionOptions) -> Result<()> {
    let free = options
        .cpu_memory_bytes
        .and_then(|_| available_process_memory());
    if let (Some(peak), Some(free)) = (options.cpu_memory_bytes, free) {
        calibraw_core::diagnostics::record(format!(
            "AI {} CPU memory: {} MiB available, {} MiB needed",
            options.model_name,
            free / MIB,
            needed_memory(peak) / MIB
        ));
    }
    check_cpu_memory(options.model_name, options.cpu_memory_bytes, free)
}

/// Fails unless the system has the RAM free that an OpenVINO session of
/// `options` for a model file of `model_bytes` needs: the measured CPU peak
/// (or, where it is not measured, the weights), the extra weight copies
/// OpenVINO compiles with, and the reserve. Unknown free RAM does not keep a
/// session off OpenVINO. A session that fails here runs on ONNX Runtime's
/// CPU provider, which needs less.
pub(super) fn ensure_openvino_memory_available(
    options: &SessionOptions,
    model_bytes: u64,
) -> Result<()> {
    let Some(free) = calibraw_core::system_memory::available_memory_bytes() else {
        return Ok(());
    };
    let needed = needed_memory(
        options
            .cpu_memory_bytes
            .unwrap_or(model_bytes)
            .saturating_add(model_bytes.saturating_mul(OPENVINO_EXTRA_WEIGHT_COPIES)),
    );
    calibraw_core::diagnostics::record(format!(
        "AI {} OpenVINO memory: {} MiB of RAM available, {} MiB needed",
        options.model_name,
        free / MIB,
        needed / MIB
    ));
    anyhow::ensure!(
        free >= needed,
        "{} MiB of RAM available, {} MiB needed",
        free / MIB,
        needed / MIB
    );
    Ok(())
}

fn check_cpu_memory(model_name: &str, peak: Option<u64>, free: Option<u64>) -> Result<()> {
    if let (Some(peak), Some(free)) = (peak, free) {
        let needed = needed_memory(peak);
        anyhow::ensure!(
            free >= needed,
            "Not enough memory to run {model_name}: {} MiB available, {} MiB required. Try a lower model quality or a device with more available memory.",
            free / MIB,
            needed / MIB
        );
    }
    Ok(())
}

#[cfg(target_os = "ios")]
fn available_process_memory() -> Option<u64> {
    unsafe extern "C" {
        fn os_proc_available_memory() -> usize;
    }
    // SAFETY: this iOS system function takes no pointers, transfers no
    // ownership and is callable on any thread. It reports bytes this process
    // may still allocate, including its device-specific memory limit.
    Some(unsafe { os_proc_available_memory() } as u64)
}

#[cfg(not(target_os = "ios"))]
fn available_process_memory() -> Option<u64> {
    None
}
