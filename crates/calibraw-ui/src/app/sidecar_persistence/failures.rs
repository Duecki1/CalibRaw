//! Reporting sidecar save failures and recovering unsupported sidecars.

use super::*;

impl CalibRawApp {
    pub(crate) fn report_mask_persistence_limit(
        &mut self,
        action: &str,
        error: &crate::sidecar::SidecarError,
    ) {
        let message = format!(
            "{action} was not applied because the resulting edit could not be saved: {error}"
        );
        calibraw_core::diagnostics::record(&message);
        self.report_error(ErrorKind::EditNotApplied, message);
    }

    pub(in crate::app) fn report_sidecar_save_failure(
        &mut self,
        revision: Option<u64>,
        detail: impl AsRef<str>,
    ) {
        self.persistence.sidecar_save_feedback_until = None;
        if let Some(revision) = revision {
            self.persistence.sidecar_failed_revision = Some(revision);
        }

        let message = format!("Could not save edits: {}", detail.as_ref());
        self.ui.notice = Some(message.clone());
        self.persistence.sidecar_save_error_dialog = Some(message.clone());
        self.persistence.sidecar_recovery = None;
        calibraw_core::diagnostics::record(format!("Edit save failed: {}", detail.as_ref()));
        log::error!("{message}");
        self.egui_ctx.request_repaint();
    }

    pub(in crate::app) fn show_sidecar_save_error_dialog(&mut self, ctx: &egui::Context) {
        let Some(message) = self.persistence.sidecar_save_error_dialog.clone() else {
            return;
        };
        let can_retry = self.can_save_edits()
            && !self.sidecar_save_in_progress()
            && self.persistence.sidecar_failed_revision == Some(self.edit_commit_revision());
        #[cfg(not(target_os = "android"))]
        let can_recover =
            self.persistence.sidecar_recovery.is_some() && !self.sidecar_save_in_progress();
        let mut retry = false;
        let mut close = false;
        #[cfg(not(target_os = "android"))]
        let mut recover = false;
        moduwu_design::dialog_window(
            "Could not save edits",
            ctx,
            moduwu_design::DIALOG_WIDTH_WIDE,
        )
        .resizable(true)
        .show(ctx, |ui| {
            ui.label("CalibRaw was unable to write the edit sidecar.");
            ui.add_space(6.0);
            ui.add(
                egui::Label::new(egui::RichText::new(&message).monospace())
                    .wrap()
                    .selectable(true),
            );
            ui.add_space(6.0);
            ui.small("This error was added to the log in Settings → Diagnostics.");
            #[cfg(not(target_os = "android"))]
            if self.persistence.sidecar_recovery.is_some() {
                ui.add_space(8.0);
                ui.label("This sidecar uses an unsupported format or version. You can back up its exact contents and create a new sidecar with the edits currently in memory. Its previous review rating will not be carried over.");
                if moduwu_design::secondary_button_enabled(ui, can_recover, "Back up sidecar and create new one").clicked() {
                    recover = true;
                }
            }
            match moduwu_design::dialog_confirmation_buttons(
                ui,
                "Close",
                "Try again",
                can_retry,
                false,
                moduwu_design::DialogKeyboard::CLOSE_ONLY,
            ) {
                moduwu_design::DialogAction::Cancel => close = true,
                moduwu_design::DialogAction::Confirm => retry = true,
                moduwu_design::DialogAction::None => {}
            }
        });
        #[cfg(not(target_os = "android"))]
        let should_recover = recover;
        #[cfg(target_os = "android")]
        let should_recover = false;
        if retry {
            self.persistence.sidecar_save_error_dialog = None;
            self.persistence.sidecar_recovery = None;
            self.save_edits_now();
        } else if should_recover {
            #[cfg(not(target_os = "android"))]
            self.recover_unsupported_sidecar();
        } else if close {
            self.persistence.sidecar_save_error_dialog = None;
            self.persistence.sidecar_recovery = None;
        }
    }

    #[cfg(not(target_os = "android"))]
    fn recover_unsupported_sidecar(&mut self) {
        let Some(request) = self.persistence.sidecar_recovery.clone() else {
            return;
        };
        let crate::sidecar::SidecarTarget::Desktop { raw_path } = &request.target;
        let current = self.persistence.sidecar_target.as_ref() == Some(&request.target);
        let edits = if current {
            self.capture_sidecar_edit_state()
        } else {
            request.edits
        };
        let editing_time_ms = if current {
            self.raw_editing_time_ms()
        } else {
            request.editing_time_ms
        };
        match crate::sidecar::backup_and_replace_desktop_sidecar(raw_path, edits, editing_time_ms) {
            Ok(backup) => {
                self.persistence.sidecar_save_error_dialog = None;
                self.persistence.sidecar_recovery = None;
                self.ui.notice = Some(format!(
                    "Sidecar backed up to {}. Edits saved in a new sidecar.",
                    backup.display()
                ));
                if current {
                    self.persistence.sidecar_failed_revision = None;
                    let revision = self.edit_commit_revision();
                    self.persistence.sidecar_saved_revision = Some(revision);
                    self.queue_developed_thumbnail_refresh(
                        self.persistence.document_generation,
                        revision,
                    );
                }
            }
            Err(error) => {
                let message = format!("Could not back up and replace sidecar: {error}");
                log::error!("{message}");
                self.persistence.sidecar_save_error_dialog = Some(message.clone());
                self.ui.notice = Some(message);
            }
        }
    }
}
