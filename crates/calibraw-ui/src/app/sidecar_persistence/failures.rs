//! Reporting sidecar save failures and recovering unsupported sidecars.

use super::*;
use crate::i18n::{tr, tr_fmt};

impl CalibRawApp {
    pub(crate) fn report_mask_persistence_limit(
        &mut self,
        action: &str,
        error: &crate::sidecar::SidecarError,
    ) {
        let message = tr_fmt(
            "{action} was not applied because the resulting edit could not be saved: {error}",
            &[("action", &action), ("error", &error)],
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

        let message = tr_fmt(
            "Could not save edits: {detail}",
            &[("detail", &detail.as_ref())],
        );
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
        // Only folder sidecars can be backed up beside the photo and replaced.
        let has_recovery = self.caps().has_folders() && self.persistence.sidecar_recovery.is_some();
        let can_recover = has_recovery && !self.sidecar_save_in_progress();
        let mut retry = false;
        let mut close = false;
        let mut recover = false;
        let mut action = moduwu_design::DialogAction::None;
        moduwu_design::Dialog::new("sidecar-save-error", "Could not save edits")
            .size(moduwu_design::DialogSize::Wide)
            .show_with_footer(
                ctx,
                |ui| {
                    ui.label(tr("CalibRaw was unable to write the edit sidecar."));
                    ui.add(
                        egui::Label::new(egui::RichText::new(&message).monospace())
                            .wrap()
                            .selectable(true),
                    );
                    ui.small(tr("This error was added to the log in Settings → Diagnostics."));
                    if has_recovery {
                        ui.label(tr("This sidecar uses an unsupported format or version. You can back up its exact contents and create a new sidecar with the edits currently in memory. Its previous review rating will not be carried over."));
                    }
                },
                |ui| {
                    action = moduwu_design::dialog_confirmation_actions(
                        ui,
                        tr("Close"),
                        tr("Try again"),
                        can_retry,
                        false,
                        moduwu_design::DialogKeyboard::CLOSE_ONLY,
                    );
                    if has_recovery
                        && moduwu_design::secondary_button_enabled(
                            ui,
                            can_recover,
                            tr("Back up sidecar and create new one"),
                        )
                        .clicked()
                    {
                        recover = true;
                    }
                },
            );
        match action {
            moduwu_design::DialogAction::Cancel => close = true,
            moduwu_design::DialogAction::Confirm => retry = true,
            moduwu_design::DialogAction::None => {}
        }
        let should_recover = recover;
        if retry {
            self.persistence.sidecar_save_error_dialog = None;
            self.persistence.sidecar_recovery = None;
            self.save_edits_now();
        } else if should_recover {
            self.recover_unsupported_sidecar();
        } else if close {
            self.persistence.sidecar_save_error_dialog = None;
            self.persistence.sidecar_recovery = None;
        }
    }

    fn recover_unsupported_sidecar(&mut self) {
        let Some(request) = self.persistence.sidecar_recovery.clone() else {
            return;
        };
        let crate::sidecar::SidecarTarget::Desktop { raw_path } = &request.target else {
            return;
        };
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
                self.ui.notice = Some(tr_fmt(
                    "Sidecar backed up to {backup}. Edits saved in a new sidecar.",
                    &[("backup", &backup.display())],
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
                let message = tr_fmt(
                    "Could not back up and replace sidecar: {error}",
                    &[("error", &error)],
                );
                log::error!("{message}");
                self.persistence.sidecar_save_error_dialog = Some(message.clone());
                self.ui.notice = Some(message);
            }
        }
    }
}
