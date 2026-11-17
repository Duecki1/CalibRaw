//! Recovery from GPU failures that end the session: a lost GPU device and a
//! previous run that died during GPU AI work.

use super::*;
use crate::i18n::{tr, tr_fmt};

/// When AI had `activity` on the GPU, for a message: "while … was running".
fn during(activity: &calibraw_ai::GpuAiActivity) -> String {
    match activity {
        calibraw_ai::GpuAiActivity::Job(job) => tr_fmt("while {job} was running", &[("job", &job)]),
        calibraw_ai::GpuAiActivity::IdleModel(model) => {
            tr_fmt("while {model} was loaded", &[("model", &model)])
        }
    }
}

impl CalibRawApp {
    /// Turns GPU AI off when the previous run ended during `activity` on the
    /// GPU, as `calibraw_ai::configure` reports it.
    pub(in crate::app) fn handle_interrupted_gpu_ai(
        &mut self,
        activity: &calibraw_ai::GpuAiActivity,
    ) {
        calibraw_core::diagnostics::record(match activity {
            calibraw_ai::GpuAiActivity::Job(job) => {
                format!("previous run ended during GPU AI job: {job}")
            }
            calibraw_ai::GpuAiActivity::IdleModel(model) => {
                format!("previous run ended with an AI model loaded on the GPU: {model}")
            }
        });
        self.set_ai_gpu_acceleration(false);
        self.report_error(
            ErrorKind::GpuAi,
            tr_fmt(
                "CalibRaw closed unexpectedly last time {when}. AI tools now use the CPU; you can turn GPU acceleration back on in Settings.",
                &[("when", &during(activity))],
            ),
        );
    }

    /// Tells the user how the previous run ended when it ended abnormally for a
    /// reason the log shows: a GPU reset nobody was told about, a panic, or an
    /// export cut short. The full log is in Settings.
    pub(in crate::app) fn report_previous_session_end(&mut self) {
        let Some(previous) = calibraw_core::diagnostics::previous_session() else {
            return;
        };
        if !previous.is_noteworthy() {
            return;
        }
        let Some(cause) = previous.cause() else {
            return;
        };
        self.report_error(
            ErrorKind::PreviousSession,
            tr_fmt(
                "CalibRaw closed unexpectedly last time. {cause}. The events leading up to it are in Settings under Diagnostics; please include them when you report this.",
                &[("cause", &cause)],
            ),
        );
    }

    /// Saves and restarts after the GPU device was lost, usually a driver
    /// reset. Nothing can be drawn on a lost device, so the notice is a native
    /// dialog. Where the platform cannot restart CalibRaw, or this run already
    /// followed such a restart, the window closes after the notice. A GPU AI
    /// job that was running, or a model loaded on the GPU, switches AI to the
    /// CPU first: a loaded model's memory alone can drive the GPU into paging
    /// and a reset.
    pub(in crate::app) fn handle_gpu_device_loss(&mut self, ctx: &egui::Context) {
        let Some(reason) = calibraw_gpu::take_gpu_device_lost() else {
            return;
        };
        let ai_activity = calibraw_ai::gpu_ai_activity().or_else(|| {
            calibraw_ai::take_ai_gpu_device_loss()
                .then(|| calibraw_ai::GpuAiActivity::Job("an AI model".to_owned()))
        });
        if ai_activity.is_some() {
            self.set_ai_gpu_acceleration(false);
        }
        let save_failure = self.save_edits_before_exit();

        let recovery = self.platform.gpu_loss_recovery();
        let mut message =
            tr("The graphics driver reset the GPU, so CalibRaw cannot continue.").to_owned();
        match &save_failure {
            None => message.push_str(tr(" Your edits were saved.")),
            Some(error) => message.push_str(&format!(" {error}.")),
        }
        if let Some(activity) = &ai_activity {
            message.push_str(&tr_fmt(
                " This happened {when}, so AI tools now use the CPU. You can turn GPU acceleration back on in Settings.",
                &[("when", &during(activity))],
            ));
        }
        message.push_str(if recovery.restarts() {
            tr(" CalibRaw will restart.")
        } else {
            tr(" Please start CalibRaw again.")
        });
        message.push_str("\n\n");
        message.push_str(&reason);
        // Recovery ends the old process directly, before eframe can paint or
        // destroy its lost swapchain. Run the normal persistence/shutdown
        // work first, while the application still owns its state.
        eframe::App::on_exit(self);
        // This loss has been reported; the next start must not find the record
        // of a job that is still running and report it again.
        calibraw_ai::end_gpu_job_records();
        calibraw_core::diagnostics::mark_reported();
        if let Err(error) = recovery.finish(tr("CalibRaw has to restart"), &message) {
            log::error!("could not recover after the GPU was lost: {error}");
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    /// Saves the open photo's edits, returning the failure if they could not
    /// be saved. Save failures are normally shown in an egui dialog, which a
    /// lost device cannot draw, so the caller reports it instead.
    fn save_edits_before_exit(&mut self) -> Option<String> {
        self.persistence.sidecar_save_error_dialog = None;
        self.flush_sidecar_on_exit();
        self.persistence.sidecar_save_error_dialog.take()
    }
}
