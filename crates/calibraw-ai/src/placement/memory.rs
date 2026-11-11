//! CPU admission against iOS's process memory allowance. A failed allocation
//! can terminate the process before ONNX Runtime can return an error.

use super::session::SessionOptions;
use anyhow::Result;

const MIB: u64 = 1 << 20;
// CPU peaks are measured with the built-in runtime and include session load.
// Keep room for editor allocations while inference runs. The GPU's percentage
// margin also covers other providers and is too large for this process budget.
const CPU_MEMORY_RESERVE: u64 = 256 * MIB;

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
