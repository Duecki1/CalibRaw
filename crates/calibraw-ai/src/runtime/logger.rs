//! ONNX Runtime's log, forwarded to `log`. The WebGPU plugin reports some GPU
//! errors only there, so each one is also counted for placement
//! (`placement::count_logged_gpu_error`) and recorded in diagnostics.

use ort::logging::{LogLevel, LoggerFunction};
use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc,
};

/// Logged WebGPU errors recorded in diagnostics per process: enough to show
/// what failed, few enough that a model failing on every job leaves room for
/// the other events.
const MAX_RECORDED_GPU_ERRORS: u32 = 16;
static RECORDED_GPU_ERRORS: AtomicU32 = AtomicU32::new(0);

/// The logger the environment is created with.
pub(super) fn logger() -> LoggerFunction {
    Arc::new(forward)
}

/// The level ONNX Runtime logs at: `ORT_LOG` (`verbose`, `info`, `warning`,
/// `error` or `fatal`), as without a custom logger, else errors, which
/// include the WebGPU plugin's GPU errors. An environment with a custom
/// logger starts at verbose, which formats a message for every graph
/// transformation, so set this before sessions or plugins are created.
pub(super) fn level() -> LogLevel {
    level_named(std::env::var("ORT_LOG").ok().as_deref())
}

fn level_named(name: Option<&str>) -> LogLevel {
    match name {
        Some("verbose") => LogLevel::Verbose,
        Some("info") => LogLevel::Info,
        Some("warning") => LogLevel::Warning,
        Some("fatal") => LogLevel::Fatal,
        _ => LogLevel::Error,
    }
}

/// Runs on ONNX Runtime's and Dawn's threads. `ort` calls it from a C
/// callback without catching panics, and a panic unwinding out of one aborts
/// the process, so it catches its own.
fn forward(level: LogLevel, _category: &str, _id: &str, _code_location: &str, message: &str) {
    let _ = std::panic::catch_unwind(|| forward_message(level, message));
}

fn forward_message(level: LogLevel, message: &str) {
    match level {
        LogLevel::Verbose => log::debug!(target: "onnxruntime", "{message}"),
        LogLevel::Info => log::info!(target: "onnxruntime", "{message}"),
        LogLevel::Warning => log::warn!(target: "onnxruntime", "{message}"),
        LogLevel::Error | LogLevel::Fatal => {
            log::error!(target: "onnxruntime", "{message}");
            if crate::placement::count_logged_gpu_error(message)
                && RECORDED_GPU_ERRORS.fetch_add(1, Ordering::Relaxed) < MAX_RECORDED_GPU_ERRORS
            {
                calibraw_core::diagnostics::record(format!("ONNX Runtime logged: {message}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ort_log_picks_the_level_as_ort_does() {
        assert_eq!(level_named(None), LogLevel::Error);
        assert_eq!(level_named(Some("verbose")), LogLevel::Verbose);
        assert_eq!(level_named(Some("info")), LogLevel::Info);
        assert_eq!(level_named(Some("warning")), LogLevel::Warning);
        assert_eq!(level_named(Some("error")), LogLevel::Error);
        assert_eq!(level_named(Some("fatal")), LogLevel::Fatal);
        assert_eq!(level_named(Some("loud")), LogLevel::Error);
    }
}
