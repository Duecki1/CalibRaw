//! Keeps backend warnings and errors in the persistent report as well as the
//! platform logger. In particular, wgpu's error chain must survive a restart.

use super::{send_to_writer, state, Message};
use log::{Level, LevelFilter, Log, Metadata, Record, SetLoggerError};
use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const MAX_MESSAGE_BYTES: usize = 32 * 1024;
type RecentErrors = VecDeque<(Instant, String)>;
static GPU_ERRORS: Mutex<RecentErrors> = Mutex::new(VecDeque::new());

struct DiagnosticLogger {
    platform: Box<dyn Log>,
}

/// Installs the platform logger with persistent warning/error capture. Normal
/// console filtering stays with `platform`; errors reach the file even when
/// the console's filter would suppress them. Call once, before GPU setup.
pub fn install_logger(
    platform: impl Log + 'static,
    level: LevelFilter,
) -> Result<(), SetLoggerError> {
    static LOGGER: OnceLock<DiagnosticLogger> = OnceLock::new();
    let logger = LOGGER.get_or_init(|| DiagnosticLogger {
        platform: Box::new(platform),
    });
    log::set_logger(logger)?;
    log::set_max_level(level.max(LevelFilter::Warn));
    Ok(())
}

impl Log for DiagnosticLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= Level::Warn || self.platform.enabled(metadata)
    }

    fn log(&self, record: &Record<'_>) {
        if record.level() <= Level::Warn {
            if record.level() == Level::Error
                && (record.target().starts_with("wgpu") || record.target().starts_with("egui_wgpu"))
            {
                // The full backtrace stays in the file. A short native error
                // and operation also fit in the independent recovery dialog.
                let detail = record.args().to_string();
                let detail = detail
                    .split("; backend call stack:")
                    .next()
                    .unwrap_or(&detail);
                let detail: String = detail.chars().take(1000).collect();
                if let Ok(mut errors) = GPU_ERRORS.try_lock() {
                    if errors.len() == 4 {
                        errors.pop_front();
                    }
                    errors.push_back((Instant::now(), format!("{}: {detail}", record.target())));
                }
            }
            let thread = std::thread::current();
            let message = bounded(format!(
                "{} {} thread={}/{:?} {}:{}: {}",
                record.level(),
                record.target(),
                thread.name().unwrap_or("unnamed"),
                thread.id(),
                record.file().unwrap_or("unknown"),
                record.line().unwrap_or(0),
                record.args(),
            ));
            // Logging may happen in a panic hook or while `record` owns the
            // report lock. Never recurse into log! or wait for that lock.
            if let Ok(mut state) = state().try_lock() {
                state.push(&message);
            } else {
                send_to_writer(Message::Line(message));
            }
        }
        self.platform.log(record);
    }

    fn flush(&self) {
        self.platform.flush();
        super::flush();
    }
}

/// Recent backend failures for a native GPU-loss notice. Their full versions,
/// including error-path backtraces, remain in the persistent log. These are
/// nearby observations, not a claim about which operation caused the loss.
pub fn recent_gpu_errors() -> Option<String> {
    let errors = GPU_ERRORS.lock().ok()?;
    let details: Vec<&str> = errors
        .iter()
        .filter(|(at, _)| at.elapsed() <= Duration::from_secs(10))
        .map(|(_, detail)| detail.as_str())
        .collect();
    (!details.is_empty()).then(|| details.join("\n"))
}

pub(super) fn bounded(mut message: String) -> String {
    if message.len() > MAX_MESSAGE_BYTES {
        let mut end = MAX_MESSAGE_BYTES;
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        message.truncate(end);
        message.push_str(" … [truncated]");
    }
    message
}
