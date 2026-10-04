//! Presenting a running export: the progress indicator, the task dialog and the Android notification.

use super::*;

impl CalibRawApp {
    pub(crate) fn show_export_task_indicator(&mut self, ui: &mut egui::Ui) {
        let Some(task) = self.export.task.as_ref() else {
            return;
        };
        if !task.minimized {
            return;
        }
        let replay = task.kind == ExportTaskKind::Replay;
        let label = if replay {
            format!("Replay {:.0}%", task.progress.clamp(0.0, 1.0) * 100.0)
        } else if task.total > 1 {
            format!(
                "Exporting {} / {}",
                task.completed.min(task.total),
                task.total
            )
        } else {
            format!("Exporting {:.0}%", task.progress.clamp(0.0, 1.0) * 100.0)
        };
        if ui
            .small_button(label)
            .on_hover_text("Show export progress")
            .clicked()
        {
            self.restore_export_task();
        }
    }

    #[cfg(target_os = "android")]
    pub(crate) fn sync_android_export_notification(&self) {
        let Some(task) = self.export.task.as_ref() else {
            if let Err(error) =
                calibraw_ffi::clear_background_task_notification(&self.android.android_app)
            {
                log::warn!("{error}");
            }
            return;
        };
        let title = if task.kind == ExportTaskKind::LibraryBatch {
            "CalibRaw batch export"
        } else {
            "CalibRaw export"
        };
        let detail = (task.total > 1).then(|| {
            format!(
                "{} / {} images complete",
                task.completed.min(task.total),
                task.total
            )
        });
        let percent = (task.progress.clamp(0.0, 1.0) * 100.0).round() as i32;
        if let Err(error) = calibraw_ffi::update_background_task_notification(
            &self.android.android_app,
            title,
            &task.phase,
            detail.as_deref(),
            percent,
            task.total_tiles == 0 && task.progress <= 0.0,
            0,
        ) {
            log::warn!("{error}");
        }
    }

    pub(crate) fn show_export_task_dialog(&mut self, ctx: &egui::Context) {
        let Some(task) = self.export.task.as_ref() else {
            return;
        };
        if task.minimized {
            return;
        }
        let progress = task.progress.clamp(0.0, 1.0);
        let phase = task.phase.clone();
        let completed = task.completed;
        let total = task.total;
        let cancelling = task.cancelling;
        let mut minimize = false;
        let mut cancel = false;
        let replay = task.kind == ExportTaskKind::Replay;
        let window_title = if replay {
            "Creating Edit Replay"
        } else {
            "Exporting"
        };
        moduwu_design::dialog_window(window_title, ctx, moduwu_design::DIALOG_WIDTH_DEFAULT)
            .id(egui::Id::new("active-export-progress"))
            .show(ctx, |ui| {
                if replay {
                    ui.label(egui::RichText::new("Creating edit replay").strong());
                } else if total > 1 {
                    ui.label(
                        egui::RichText::new(format!(
                            "{} / {} images complete",
                            completed.min(total),
                            total
                        ))
                        .strong(),
                    );
                } else {
                    ui.label(egui::RichText::new("Exporting image").strong());
                }
                ui.label(&phase);
                ui.add_space(6.0);
                ui.add(
                    egui::ProgressBar::new(progress)
                        .show_percentage()
                        .animate(!cancelling),
                );
                if cancelling {
                    ui.label(
                        egui::RichText::new("Stopping at the next safe point…")
                            .small()
                            .color(ui.visuals().weak_text_color()),
                    );
                }
                moduwu_design::dialog_button_row(ui, |ui| {
                    cancel |= ui
                        .add_enabled_ui(!cancelling, |ui| {
                            moduwu_design::secondary_button(ui, "Cancel")
                        })
                        .inner
                        .clicked();
                    if moduwu_design::secondary_button(ui, "Minimize").clicked() {
                        minimize = true;
                    }
                });
                if !cancel
                    && !cancelling
                    && moduwu_design::dialog_keyboard_action(
                        ui,
                        moduwu_design::DialogKeyboard::CLOSE_ONLY,
                        false,
                    ) == moduwu_design::DialogAction::Cancel
                {
                    cancel = true;
                }
            });
        if minimize {
            self.minimize_export_task();
        }
        if cancel {
            self.cancel_export_task();
        }
        ctx.request_repaint_after(Duration::from_millis(50));
    }
}
