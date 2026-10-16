//! Recovery from GPU failures that end the session: a lost GPU device and a
//! previous run that died during GPU AI work.

use super::*;

/// Set in a process started by `restart_command`, so a GPU that fails again
/// right away does not restart CalibRaw in a loop.
const RESTARTED_AFTER_GPU_LOSS: &str = "CALIBRAW_RESTARTED_AFTER_GPU_LOSS";

impl CalibRawApp {
    /// Turns GPU AI off when the previous run ended during `job` on the GPU,
    /// as `calibraw_ai::set_gpu_job_record_path` reports it.
    pub(in crate::app) fn handle_interrupted_gpu_ai_job(&mut self, job: &str) {
        calibraw_core::diagnostics::record(format!("previous run ended during GPU AI job: {job}"));
        self.set_ai_gpu_acceleration(false);
        self.report_error(
            ErrorKind::GpuAi,
            format!(
                "CalibRaw closed unexpectedly last time while {job} was running. AI tools now use the CPU; you can turn GPU acceleration back on in Settings."
            ),
        );
    }

    /// Saves and restarts after the GPU device was lost, usually a driver
    /// reset. Nothing can be drawn on a lost device, so the notice is a native
    /// dialog. A GPU AI job that was running switches AI to the CPU first.
    pub(in crate::app) fn handle_gpu_device_loss(&mut self, ctx: &egui::Context) {
        if calibraw_gpu::take_gpu_device_lost().is_none() {
            return;
        }
        let ai_job = calibraw_ai::gpu_job_in_progress()
            .or_else(|| calibraw_ai::take_ai_gpu_failure().map(|_| "an AI model".to_owned()));
        if ai_job.is_some() {
            self.set_ai_gpu_acceleration(false);
        }
        self.flush_sidecar_on_exit();

        let restart = restart_command();
        let mut message =
            "The graphics driver reset the GPU, so CalibRaw cannot continue. Your edits were saved."
                .to_owned();
        if let Some(job) = &ai_job {
            message.push_str(&format!(
                " This happened while {job} was running, so AI tools now use the CPU. You can turn GPU acceleration back on in Settings."
            ));
        }
        message.push_str(if restart.is_some() {
            " CalibRaw will restart."
        } else {
            " Please start CalibRaw again."
        });
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title("CalibRaw has to restart")
            .set_description(message)
            .set_buttons(rfd::MessageButtons::Ok)
            .show();

        if let Some(mut command) = restart {
            if let Err(error) = command.spawn() {
                log::error!("could not restart CalibRaw after the GPU was lost: {error}");
            }
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

/// This program with its arguments, or `None` where a restart cannot work:
/// after a restart that already followed a GPU loss, and inside Flatpak,
/// whose sandbox ends with this process.
fn restart_command() -> Option<std::process::Command> {
    if std::env::var_os(RESTARTED_AFTER_GPU_LOSS).is_some()
        || std::env::var_os("FLATPAK_ID").is_some()
    {
        return None;
    }
    // An AppImage runs from a temporary mount; restart the image itself.
    let program = match std::env::var_os("APPIMAGE") {
        Some(image) => PathBuf::from(image),
        None => std::env::current_exe().ok()?,
    };
    let mut command = std::process::Command::new(program);
    command
        .args(std::env::args_os().skip(1))
        .env(RESTARTED_AFTER_GPU_LOSS, "1");
    Some(command)
}
