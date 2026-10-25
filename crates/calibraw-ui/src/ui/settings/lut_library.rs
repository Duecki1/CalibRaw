//! The LUTs card and the dialog that imports, renames and removes the
//! library's LUTs.

use super::Settings;
use crate::app::{CalibRawApp, SettingsDialog};
use eframe::egui::{self, Ui};

impl Settings {
    pub(super) fn show_luts_card(ui: &mut Ui, app: &mut CalibRawApp) {
        moduwu_design::heading_with_help(
            ui,
            "LUTs",
            "3D LUTs in the .cube format, for the LUT card in Develop and for batch exports. Imported files are copied into CalibRaw, so the originals can be moved or deleted.",
        );
        if let Some(failure) = app.luts.failure() {
            ui.colored_label(ui.visuals().error_fg_color, failure);
        }
        let count = app.luts.entries().len();
        let summary = match count {
            0 => "No LUTs imported".to_owned(),
            1 => "1 LUT".to_owned(),
            count => format!("{count} LUTs"),
        };
        moduwu_design::property_row(ui, summary, |ui| {
            if moduwu_design::secondary_button_enabled(ui, app.luts.is_available(), "Manage…")
                .clicked()
            {
                app.ui.settings_dialog = Some(SettingsDialog::Luts);
            }
        });
    }

    /// The dialog listing the library's LUTs, each with rename and remove
    /// buttons, and Import… in its footer. Renames and removals open their
    /// own dialogs above it.
    pub(super) fn show_lut_library_dialog(ctx: &egui::Context, app: &mut CalibRawApp) {
        if app.ui.settings_dialog != Some(SettingsDialog::Luts) {
            return;
        }
        let mut rename = None;
        let mut remove = None;
        let mut import = false;
        let mut close = false;
        moduwu_design::Dialog::new("lut-library", "LUTs")
            .size(moduwu_design::DialogSize::Wide)
            .show_with_footer(
                ctx,
                |ui| {
                    if app.luts.entries().is_empty() {
                        ui.small("No LUTs imported.");
                    }
                    for entry in app.luts.entries() {
                        // Right to left: remove sits at the row's end.
                        moduwu_design::property_row(ui, &entry.name, |ui| {
                            let size = moduwu_design::toolbar_icon_size();
                            if moduwu_design::ghost_icon_button(
                                ui,
                                egui_phosphor::regular::TRASH,
                                size,
                                &format!("Remove {}", entry.name),
                            )
                            .clicked()
                            {
                                remove = Some(entry.id);
                            }
                            if moduwu_design::ghost_icon_button(
                                ui,
                                egui_phosphor::regular::PENCIL_SIMPLE,
                                size,
                                &format!("Rename {}", entry.name),
                            )
                            .clicked()
                            {
                                rename = Some(entry.id);
                            }
                            ui.small(format!("{0}×{0}×{0}", entry.size));
                        });
                    }
                },
                |ui| {
                    close = moduwu_design::primary_action_button(ui, "Done").clicked()
                        || moduwu_design::dialog_keyboard_action(
                            ui,
                            moduwu_design::DialogKeyboard::CLOSE_ONLY,
                            false,
                        ) == moduwu_design::DialogAction::Cancel;
                    import = moduwu_design::secondary_button_enabled(
                        ui,
                        app.luts.is_available(),
                        format!("{}  Import…", egui_phosphor::regular::DOWNLOAD_SIMPLE),
                    )
                    .clicked();
                },
            );
        if let Some(id) = rename {
            app.start_lut_rename(id);
        }
        if remove.is_some() {
            app.luts.pending_remove = remove;
        }
        if import {
            app.choose_lut_files_to_import(false);
        }
        if close {
            app.ui.settings_dialog = None;
        }
    }
}
