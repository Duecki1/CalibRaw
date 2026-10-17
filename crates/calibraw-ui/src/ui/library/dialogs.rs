use super::*;

fn show_dialog_error(ui: &mut Ui, error: Option<&str>) {
    if let Some(error) = error {
        ui.label(
            egui::RichText::new(error)
                .small()
                .color(ui.visuals().error_fg_color),
        );
    }
}

pub(super) fn show_delete_originals_confirmation(
    ui: &mut Ui,
    assets: &[LibraryAsset],
    caps: PlatformCaps,
) -> Option<bool> {
    if assets.is_empty() {
        return Some(false);
    }
    let count = assets.len();
    let title = if count == 1 {
        "Delete original?".to_owned()
    } else {
        format!("Delete {count} originals?")
    };
    // Folder photos go to the desktop trash; the media library deletes them.
    let recycles = caps.has_folders();
    let confirm_label = if recycles {
        format!("Move to {}", crate::platform::trash_name())
    } else {
        "Delete permanently".to_owned()
    };
    let mut action = moduwu_design::DialogAction::None;
    moduwu_design::Dialog::new("library-delete-originals-confirmation", title).show_with_footer(
        ui.ctx(),
        |ui| {
            if count == 1 {
                ui.label(format!("Remove {} from the Library?", assets[0].display_name));
            } else {
                ui.label(format!(
                    "Remove the {count} selected originals from the Library?"
                ));
            }
            if recycles {
            ui.label(if count == 1 {
                format!(
                    "The original photo and its saved adjustments will be moved to the system {}.",
                    crate::platform::trash_name()
                )
            } else {
                format!(
                    "The selected originals and their saved adjustments will be moved to the system {}.",
                    crate::platform::trash_name()
                )
            });
            } else {
            ui.label(
                egui::RichText::new(
                    "CalibRaw's Android library does not support recycling. This cannot be undone.",
                )
                .strong()
                .color(ui.visuals().warn_fg_color),
            );
            }
        },
        |ui| {
            action = moduwu_design::dialog_confirmation_actions(
                ui,
                "Cancel",
                confirm_label,
                true,
                true,
                moduwu_design::DialogKeyboard::CLOSE_ONLY,
            );
        },
    );
    match action {
        moduwu_design::DialogAction::Cancel => Some(false),
        moduwu_design::DialogAction::Confirm => Some(true),
        moduwu_design::DialogAction::None => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AdjustmentPasteChoice {
    Cancel,
    Merge,
    Replace,
}

pub(super) fn show_adjustment_paste_choice(
    ui: &mut Ui,
    id: &'static str,
    edited_count: usize,
    target_count: usize,
) -> Option<AdjustmentPasteChoice> {
    let mut choice = None;
    moduwu_design::Dialog::new(id, "Paste adjustments").show_with_footer(
        ui.ctx(),
        |ui| {
            ui.label(format!(
                "{} of the {} selected {} already contain edits.",
                edited_count,
                target_count,
                if target_count == 1 { "image" } else { "images" }
            ));
        },
        |ui| {
            if moduwu_design::primary_action_button(ui, "Replace")
                .on_hover_text("Clear the destination edits, then apply the copied settings.")
                .clicked()
            {
                choice = Some(AdjustmentPasteChoice::Replace);
            }
            if moduwu_design::secondary_button(ui, "Merge")
                .on_hover_text("Overwrite only the copied settings and keep everything else.")
                .clicked()
            {
                choice = Some(AdjustmentPasteChoice::Merge);
            }
            if moduwu_design::secondary_button(ui, "Cancel").clicked() {
                choice = Some(AdjustmentPasteChoice::Cancel);
            }
            if choice.is_none()
                && moduwu_design::dialog_keyboard_action(
                    ui,
                    moduwu_design::DialogKeyboard::CLOSE_ONLY,
                    false,
                ) == moduwu_design::DialogAction::Cancel
            {
                choice = Some(AdjustmentPasteChoice::Cancel);
            }
        },
    );
    choice
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AiMaskRefreshChoice {
    Dismiss,
    Regenerate,
}

pub(super) fn show_ai_mask_refresh_choice(
    ui: &mut Ui,
    id: &'static str,
    target_count: usize,
    can_regenerate: bool,
) -> Option<AiMaskRefreshChoice> {
    let mut action = moduwu_design::DialogAction::None;
    moduwu_design::Dialog::new(id, "Regenerate AI masks?")
        .size(moduwu_design::DialogSize::Wide)
        .show_with_footer(
            ui.ctx(),
            |ui| {
                ui.label(format!(
                    "{} pasted {} contain content-aware masks that belong to the source image.",
                    target_count,
                    if target_count == 1 { "image" } else { "images" }
                ));
                ui.label(
                    "Regenerate them now for each destination image? Mask groups, settings, object strokes, and local adjustments are preserved.",
                );
                if !can_regenerate {
                    ui.label(
                        egui::RichText::new(
                            "Waiting for the current photo load or edit save to finish…",
                        )
                        .small()
                        .color(ui.visuals().weak_text_color()),
                    );
                }
            },
            |ui| {
                action = moduwu_design::dialog_confirmation_actions(
                    ui,
                    "Not now",
                    "Regenerate",
                    can_regenerate,
                    false,
                    moduwu_design::DialogKeyboard::CLOSE_ONLY,
                );
            },
        );
    match action {
        moduwu_design::DialogAction::Cancel => Some(AiMaskRefreshChoice::Dismiss),
        moduwu_design::DialogAction::Confirm => Some(AiMaskRefreshChoice::Regenerate),
        moduwu_design::DialogAction::None => None,
    }
}

/// The blocking progress of regenerating AI masks: the refresh opens each
/// photo in the editor in turn. Returns whether Cancel was chosen.
pub(super) fn show_ai_mask_refresh_progress(
    ui: &mut Ui,
    completed: usize,
    total: usize,
    failed: usize,
    current_name: Option<&str>,
) -> bool {
    let fraction = if total == 0 {
        0.0
    } else {
        (completed as f32 / total as f32).clamp(0.0, 1.0)
    };
    let mut cancel = false;
    moduwu_design::Dialog::new("library-ai-mask-refresh-progress", "Regenerating AI masks")
        .show_with_footer(
            ui.ctx(),
            |ui| {
                ui.label(
                    egui::RichText::new(format!("{completed} / {total} AI masks updated")).strong(),
                );
                ui.add(
                    egui::ProgressBar::new(fraction)
                        .show_percentage()
                        .animate(completed < total),
                );
                if let Some(name) = current_name {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(format!("Refreshing {name}…"));
                    });
                }
                if failed > 0 {
                    ui.label(
                        egui::RichText::new(format!(
                            "{failed} {} failed",
                            if failed == 1 { "image" } else { "images" }
                        ))
                        .small()
                        .color(ui.visuals().warn_fg_color),
                    );
                }
            },
            |ui| {
                cancel |= moduwu_design::secondary_button(ui, "Cancel").clicked();
                cancel |= !cancel
                    && moduwu_design::dialog_keyboard_action(
                        ui,
                        moduwu_design::DialogKeyboard::CLOSE_ONLY,
                        false,
                    ) == moduwu_design::DialogAction::Cancel;
            },
        );
    cancel
}

pub(super) fn show_android_library_folder_dialog(ui: &mut Ui, app: &mut CalibRawApp) {
    let mut close = false;
    let mut create = None;
    if let Some(dialog) = app.library.media.folder_name_dialog.as_mut() {
        let mut action = moduwu_design::DialogAction::None;
        moduwu_design::Dialog::new("android-library-folder-name-dialog", "New folder")
            .show_with_footer(
                ui.ctx(),
                |ui| {
                    ui.label("Folder name");
                    let response = moduwu_design::dialog_text_field(
                        ui,
                        &mut dialog.name,
                        "android-library-folder-name-input",
                        "",
                    );
                    moduwu_design::request_initial_focus(&response, &mut dialog.focus_requested);
                    show_dialog_error(ui, dialog.error.as_deref());
                },
                |ui| {
                    action = moduwu_design::dialog_confirmation_actions(
                        ui,
                        "Cancel",
                        "Create",
                        true,
                        false,
                        moduwu_design::DialogKeyboard::CONFIRM_ON_ENTER,
                    );
                },
            );
        match action {
            moduwu_design::DialogAction::Cancel => close = true,
            moduwu_design::DialogAction::Confirm => {
                create = Some((dialog.parent.clone(), dialog.name.clone()));
            }
            moduwu_design::DialogAction::None => {}
        }
    }
    if close {
        app.library.media.folder_name_dialog = None;
    }
    if let Some((parent, name)) = create {
        match app.library.platform.create_library_folder(&parent, &name) {
            Ok(folder) => {
                app.library.media.folder_name_dialog = None;
                app.library.media.expanded_folders.insert(parent);
                app.library.status = format!("Created folder {folder}");
                app.library.refresh(ui.ctx());
            }
            Err(error) => {
                if let Some(dialog) = app.library.media.folder_name_dialog.as_mut() {
                    dialog.error = Some(error);
                }
            }
        }
    }
}

pub(super) fn show_library_folder_dialogs(ui: &mut Ui, app: &mut CalibRawApp) {
    let mut close_name_dialog = false;
    let mut name_operation = None;
    if let Some(dialog) = app.library.folder_name_dialog.as_mut() {
        let title = match dialog.kind {
            LibraryFolderNameDialogKind::Create { .. } => "New folder",
            LibraryFolderNameDialogKind::Rename { .. } => "Rename folder",
        };
        let confirm_label = match dialog.kind {
            LibraryFolderNameDialogKind::Create { .. } => "Create",
            LibraryFolderNameDialogKind::Rename { .. } => "Rename",
        };
        let mut action = moduwu_design::DialogAction::None;
        moduwu_design::Dialog::new("library-folder-name-dialog", title).show_with_footer(
            ui.ctx(),
            |ui| {
                ui.label("Folder name");
                let response = moduwu_design::dialog_text_field(
                    ui,
                    &mut dialog.name,
                    "library-folder-name-input",
                    "",
                );
                moduwu_design::request_initial_focus(&response, &mut dialog.focus_requested);
                show_dialog_error(ui, dialog.error.as_deref());
            },
            |ui| {
                action = moduwu_design::dialog_confirmation_actions(
                    ui,
                    "Cancel",
                    confirm_label,
                    true,
                    false,
                    moduwu_design::DialogKeyboard::CONFIRM_ON_ENTER,
                );
            },
        );
        match action {
            moduwu_design::DialogAction::Cancel => close_name_dialog = true,
            moduwu_design::DialogAction::Confirm => match validate_folder_name(&dialog.name) {
                Ok(_) => match app.library.root_folder.clone() {
                    None => close_name_dialog = true,
                    Some(root) => match &dialog.kind {
                        LibraryFolderNameDialogKind::Create { parent } => {
                            name_operation = Some(LibraryFolderOperation::Create {
                                root,
                                parent: parent.clone(),
                                name: dialog.name.clone(),
                            });
                            close_name_dialog = true;
                        }
                        LibraryFolderNameDialogKind::Rename { source } => {
                            if let Some(parent) = source.parent() {
                                name_operation = Some(LibraryFolderOperation::Move {
                                    root,
                                    source: source.clone(),
                                    destination_parent: parent.to_path_buf(),
                                    new_name: Some(dialog.name.clone()),
                                });
                                close_name_dialog = true;
                            } else {
                                dialog.error = Some("This folder has no parent folder.".to_owned());
                            }
                        }
                    },
                },
                Err(error) => dialog.error = Some(error),
            },
            moduwu_design::DialogAction::None => {}
        }
    }
    if close_name_dialog {
        app.library.folder_name_dialog = None;
    }
    if let Some(operation) = name_operation {
        app.library.start_folder_operation(operation, ui.ctx());
    }

    let delete_target = app.library.folder_delete_confirmation.clone();
    let mut close_delete = false;
    let mut confirm_delete = false;
    if let Some(target) = delete_target.as_ref() {
        let mut action = moduwu_design::DialogAction::None;
        moduwu_design::Dialog::new("library-folder-delete-confirmation", "Delete folder?")
            .show_with_footer(
                ui.ctx(),
                |ui| {
                    ui.label(format!(
                        "Delete {} and everything inside it?",
                        target.display()
                    ));
                    ui.label(
                        egui::RichText::new("This cannot be undone.")
                            .strong()
                            .color(ui.visuals().warn_fg_color),
                    );
                },
                |ui| {
                    action = moduwu_design::dialog_confirmation_actions(
                        ui,
                        "Cancel",
                        "Delete Folder",
                        true,
                        true,
                        moduwu_design::DialogKeyboard::CLOSE_ONLY,
                    );
                },
            );
        match action {
            moduwu_design::DialogAction::Cancel => close_delete = true,
            moduwu_design::DialogAction::Confirm => {
                confirm_delete = true;
                close_delete = true;
            }
            moduwu_design::DialogAction::None => {}
        }
    }
    if close_delete {
        app.library.folder_delete_confirmation = None;
    }
    if confirm_delete {
        if let (Some(root), Some(target)) = (app.library.root_folder.clone(), delete_target) {
            if let Some(current) = app
                .develop
                .current_path
                .clone()
                .filter(|current| current.starts_with(&target))
            {
                app.detach_current_file_for_library_action(&current);
                app.develop.current_path = None;
            }
            app.library
                .start_folder_operation(LibraryFolderOperation::Delete { root, target }, ui.ctx());
        }
    }
}

pub(super) fn validate_library_item_name(name: &str, raw: bool) -> Result<(), String> {
    if name.is_empty()
        || name.trim() != name
        || name.contains(['/', '\\'])
        || name.contains('"')
        || name.chars().any(char::is_control)
    {
        return Err("Enter a single safe name without leading or trailing spaces.".to_owned());
    }
    if raw && !crate::pipeline::is_supported_image_path(Path::new(name)) {
        return Err("Keep a supported photo filename extension.".to_owned());
    }
    Ok(())
}

pub(super) fn show_library_raw_name_dialog(
    ui: &mut Ui,
    app: &mut CalibRawApp,
    frame: &eframe::Frame,
) {
    let mut close = false;
    let mut rename = None;
    if let Some(dialog) = app.library.raw_name_dialog.as_mut() {
        let mut action = moduwu_design::DialogAction::None;
        moduwu_design::Dialog::new("library-raw-name-dialog", "Rename Photo").show_with_footer(
            ui.ctx(),
            |ui| {
                ui.label("Filename");
                let response = moduwu_design::dialog_text_field(
                    ui,
                    &mut dialog.name,
                    "library-raw-name-input",
                    "",
                );
                moduwu_design::request_initial_focus(&response, &mut dialog.focus_requested);
                show_dialog_error(ui, dialog.error.as_deref());
            },
            |ui| {
                action = moduwu_design::dialog_confirmation_actions(
                    ui,
                    "Cancel",
                    "Rename",
                    true,
                    false,
                    moduwu_design::DialogKeyboard::CONFIRM_ON_ENTER,
                );
            },
        );
        match action {
            moduwu_design::DialogAction::Cancel => close = true,
            moduwu_design::DialogAction::Confirm => {
                match validate_library_item_name(&dialog.name, true) {
                    Ok(()) => rename = Some((dialog.asset.clone(), dialog.name.clone())),
                    Err(error) => dialog.error = Some(error),
                }
            }
            moduwu_design::DialogAction::None => {}
        }
    }
    if close {
        app.library.raw_name_dialog = None;
    }

    let Some((asset, name)) = rename else {
        return;
    };

    let current_path = asset.desktop_path().and_then(|path| {
        (app.develop.current_path.as_deref() == Some(path)).then(|| path.to_path_buf())
    });
    if let Some(path) = current_path.as_deref() {
        app.detach_current_file_for_library_action(path);
        app.develop.current_path = None;
    }

    match rename_asset(app, &asset, &name) {
        Ok(renamed_asset) => {
            if let Some(clipboard) = app.library.image_clipboard.as_mut() {
                for clipboard_asset in &mut clipboard.assets {
                    if clipboard_asset.id == asset.id {
                        *clipboard_asset = renamed_asset.clone();
                    }
                }
            }
            app.library.raw_name_dialog = None;
            app.library.clear_selection();
            app.library.refresh(ui.ctx());
            app.library.status = format!("Renamed photo to {name}.");

            if current_path.is_some() {
                if let Some(destination) = renamed_asset.desktop_path().map(Path::to_path_buf) {
                    app.open_document(
                        crate::app::DocumentSource::desktop(destination, name),
                        frame,
                    );
                }
            }
        }
        Err(error) => {
            if let Some(dialog) = app.library.raw_name_dialog.as_mut() {
                dialog.error = Some(error);
            }
            if let Some(source) = current_path.filter(|path| path.is_file()) {
                let label = source
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("local RAW")
                    .to_owned();
                app.open_document(crate::app::DocumentSource::desktop(source, label), frame);
            }
        }
    }
}
