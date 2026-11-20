//! The Edit In row and dialog that manage the editors Edit In offers.

use super::Settings;
use crate::app::{CalibRawApp, SettingsDialog};
use crate::i18n::{tr, tr_fmt};
use eframe::egui::{self, Ui};

/// A change made in the editors dialog, applied after it is drawn.
enum EditorChange {
    Move { index: usize, up: bool },
    Remove(usize),
    SendOriginal { index: usize, send_original: bool },
    Add(crate::edit_in::ExternalEditor),
}

impl Settings {
    /// The row that opens the editors dialog.
    pub(super) fn show_editors_row(ui: &mut Ui, app: &mut CalibRawApp) {
        moduwu_design::form_row(ui, tr("Editors"), 190.0, |ui, _| {
            if moduwu_design::secondary_button(ui, tr("Manage…")).clicked() {
                app.ui.settings_dialog = Some(SettingsDialog::Editors);
            }
        });
    }

    /// The dialog that orders, removes and adds editors, and chooses which
    /// take the photo's own file. A change applies at once.
    pub(super) fn show_editors_dialog(ctx: &egui::Context, app: &mut CalibRawApp) {
        if app.ui.settings_dialog != Some(SettingsDialog::Editors) {
            return;
        }
        let editors = app.preferences.edit_in.editors.clone();
        let mut installed = app.installed_editors().unwrap_or_default().to_vec();
        installed.retain(|editor| {
            !editors
                .iter()
                .any(|listed| listed.program == editor.program)
        });
        let picker_open = app.ui.desktop_picker_receiver.is_some();
        let mut close = false;
        let mut choose_program = false;
        let change = moduwu_design::Dialog::new("edit-in-editors", tr("Editors"))
            .show_with_footer(
                ctx,
                |ui| {
                    let mut change = None;
                    if editors.is_empty() {
                        ui.small(tr("No editors added."));
                    }
                    let last = editors.len().saturating_sub(1);
                    for (index, editor) in editors.iter().enumerate() {
                        ui.push_id(("edit-in-editor", index), |ui| {
                            if index > 0 {
                                ui.separator();
                            }
                            if let Some(row) = editor_row(ui, editor, index, last) {
                                change = Some(row);
                            }
                        });
                    }
                    if !installed.is_empty() {
                        moduwu_design::section_separator(ui);
                        ui.label(egui::RichText::new(tr("Installed applications")).strong());
                        for editor in &installed {
                            moduwu_design::property_row(ui, editor.name.as_str(), |ui| {
                                if moduwu_design::secondary_button(ui, tr("Add")).clicked() {
                                    change = Some(EditorChange::Add(editor.clone()));
                                }
                            });
                        }
                    }
                    change
                },
                |ui| {
                    close = moduwu_design::primary_action_button(ui, tr("Done")).clicked()
                        || moduwu_design::dialog_keyboard_action(
                            ui,
                            moduwu_design::DialogKeyboard::CLOSE_ONLY,
                            false,
                        ) == moduwu_design::DialogAction::Cancel;
                    choose_program = moduwu_design::secondary_button_enabled(
                        ui,
                        !picker_open,
                        tr("Choose program…"),
                    )
                    .clicked();
                },
            )
            .inner;
        match change {
            Some(EditorChange::Move { index, up }) => app.move_editor(index, up),
            Some(EditorChange::Remove(index)) => app.remove_editor(index),
            Some(EditorChange::SendOriginal {
                index,
                send_original,
            }) => app.set_editor_sends_original(index, send_original),
            Some(EditorChange::Add(editor)) => app.add_editor(editor),
            None => {}
        }
        if choose_program {
            app.choose_editor_program();
        }
        if close {
            app.ui.settings_dialog = None;
        }
    }
}

/// One editor: its name with its move and remove buttons, where it is
/// started from, and whether it takes the photo's own file.
fn editor_row(
    ui: &mut Ui,
    editor: &crate::edit_in::ExternalEditor,
    index: usize,
    last: usize,
) -> Option<EditorChange> {
    let mut change = None;
    // Right to left: the remove button sits at the row's end.
    moduwu_design::property_row(ui, egui::RichText::new(&editor.name).strong(), |ui| {
        let size = moduwu_design::toolbar_icon_size();
        let name = editor.name.as_str();
        if moduwu_design::ghost_icon_button(
            ui,
            egui_phosphor::regular::TRASH,
            size,
            &tr_fmt("Remove {name}", &[("name", &name)]),
        )
        .clicked()
        {
            change = Some(EditorChange::Remove(index));
        }
        let down = ui.add_enabled_ui(index < last, |ui| {
            moduwu_design::ghost_icon_button(
                ui,
                egui_phosphor::regular::CARET_DOWN,
                size,
                &tr_fmt("Move {name} down", &[("name", &name)]),
            )
        });
        let up = ui.add_enabled_ui(index > 0, |ui| {
            moduwu_design::ghost_icon_button(
                ui,
                egui_phosphor::regular::CARET_UP,
                size,
                &tr_fmt("Move {name} up", &[("name", &name)]),
            )
        });
        if up.inner.clicked() {
            change = Some(EditorChange::Move { index, up: true });
        } else if down.inner.clicked() {
            change = Some(EditorChange::Move { index, up: false });
        }
    });
    ui.add(
        egui::Label::new(
            egui::RichText::new(editor.program.display().to_string())
                .small()
                .monospace()
                .color(ui.visuals().weak_text_color()),
        )
        .wrap(),
    );
    let mut send_original = editor.send_original;
    moduwu_design::toggle_with_help(
        ui,
        &mut send_original,
        tr("Send original file"),
        tr("Opens the photo's own file instead of a rendered copy, for applications that develop RAW files themselves."),
    );
    if send_original != editor.send_original {
        change = Some(EditorChange::SendOriginal {
            index,
            send_original,
        });
    }
    change
}
