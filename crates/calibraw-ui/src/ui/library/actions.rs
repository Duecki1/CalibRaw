use super::*;

const SELECTION_BAR_COUNT_LABEL_BREAKPOINT: f32 = 360.0;

#[derive(Clone, Debug)]
pub(crate) enum LibraryAction {
    Review(Vec<LibraryAsset>, super::review::ReviewChange),
    Export(Vec<LibraryAsset>),
    Merge(super::MergeKind, Vec<LibraryAsset>),
    CopyAdjustments(LibraryAsset),
    PasteAdjustments(Vec<LibraryAsset>),
    ApplyPreset(Vec<LibraryAsset>, PathBuf),
    Copy(Vec<LibraryAsset>),
    Cut(Vec<LibraryAsset>),
    PasteIntoAssetFolder(LibraryAsset),
    Duplicate(Vec<LibraryAsset>),
    Rename(LibraryAsset),
    RevealFile(LibraryAsset),
    ResetAdjustments(Vec<LibraryAsset>),
    Delete(Vec<LibraryAsset>),
}

pub(crate) fn library_image_context_menu(
    ui: &mut Ui,
    app: &CalibRawApp,
    context_asset: &LibraryAsset,
    context_assets: &[LibraryAsset],
    show_review: bool,
) -> Option<LibraryAction> {
    let selected_count = context_assets.len();
    let action_enabled = !local_action_in_progress(app)
        && app.library_batch_export_progress().is_none()
        && app.library_ai_mask_refresh_status().is_none()
        && !context_assets.is_empty();
    let mut action = None;
    if show_review {
        ui.add_enabled_ui(action_enabled, |ui| {
            ui.menu_button("Flag", |ui| {
                for (label, flag) in [
                    ("Pick", crate::sidecar::PhotoFlag::Picked),
                    ("Unflag", crate::sidecar::PhotoFlag::Unflagged),
                    ("Reject", crate::sidecar::PhotoFlag::Rejected),
                ] {
                    if moduwu_design::menu_item(ui, true, label).clicked() {
                        action = Some(LibraryAction::Review(
                            context_assets.to_vec(),
                            super::review::ReviewChange::Flag(flag),
                        ));
                        ui.close();
                    }
                }
            });
            ui.menu_button("Rating", |ui| {
                for rating in 0..=5 {
                    let label = if rating == 0 {
                        "Unrated".to_owned()
                    } else {
                        format!("{rating} stars")
                    };
                    if moduwu_design::menu_item(ui, true, label).clicked() {
                        action = Some(LibraryAction::Review(
                            context_assets.to_vec(),
                            super::review::ReviewChange::Rating(rating),
                        ));
                        ui.close();
                    }
                }
            });
        });
        ui.separator();
    }

    if moduwu_design::context_menu_item(
        ui,
        action_enabled,
        if selected_count > 1 {
            "Export selected…"
        } else {
            "Export…"
        },
    )
    .clicked()
    {
        action = Some(LibraryAction::Export(context_assets.to_vec()));
        ui.close();
    }

    if moduwu_design::context_menu_item(
        ui,
        app.caps().is_desktop() && selected_count == 1 && context_asset.desktop_path().is_some(),
        "Show in File Manager",
    )
    .clicked()
    {
        action = Some(LibraryAction::RevealFile(context_asset.clone()));
        ui.close();
    }

    if selected_count >= 2 {
        for (kind, help) in [
            (
                super::MergeKind::Hdr,
                "Auto-align and merge RAW exposures into an editable 32-bit float TIFF",
            ),
            (
                super::MergeKind::FocusStack,
                "Auto-align RAW frames focused at different distances and combine their sharpest parts into an editable 32-bit float TIFF",
            ),
        ] {
            if moduwu_design::context_menu_item(ui, action_enabled, kind.title())
                .on_hover_text(help)
                .clicked()
            {
                action = Some(LibraryAction::Merge(kind, context_assets.to_vec()));
                ui.close();
            }
        }
    }

    ui.separator();
    if moduwu_design::context_menu_item(
        ui,
        action_enabled && selected_count == 1,
        "Copy adjustments",
    )
    .clicked()
    {
        action = Some(LibraryAction::CopyAdjustments(context_asset.clone()));
        ui.close();
    }
    if moduwu_design::context_menu_item(
        ui,
        action_enabled && app.library.has_copied_adjustments(),
        if selected_count > 1 {
            "Paste adjustments to selected"
        } else {
            "Paste adjustments"
        },
    )
    .on_disabled_hover_text("Copy adjustments from an image first")
    .clicked()
    {
        action = Some(LibraryAction::PasteAdjustments(context_assets.to_vec()));
        ui.close();
    }
    if let Some(preset) = apply_preset_menu(ui, app, action_enabled) {
        action = Some(LibraryAction::ApplyPreset(context_assets.to_vec(), preset));
    }

    ui.separator();
    if moduwu_design::context_menu_item(ui, action_enabled, "Copy").clicked() {
        action = Some(LibraryAction::Copy(context_assets.to_vec()));
        ui.close();
    }
    if moduwu_design::context_menu_item(ui, action_enabled, "Cut").clicked() {
        action = Some(LibraryAction::Cut(context_assets.to_vec()));
        ui.close();
    }
    let paste_label = app
        .library
        .image_clipboard
        .as_ref()
        .map_or_else(|| "Paste here".to_owned(), ImageClipboard::paste_label);
    if moduwu_design::context_menu_item(
        ui,
        action_enabled && app.library.image_clipboard.is_some(),
        paste_label,
    )
    .on_disabled_hover_text("Copy or cut photos first")
    .clicked()
    {
        action = Some(LibraryAction::PasteIntoAssetFolder(context_asset.clone()));
        ui.close();
    }
    if moduwu_design::context_menu_item(
        ui,
        action_enabled,
        if selected_count > 1 {
            "Duplicate selected (RAW + sidecars)"
        } else {
            "Duplicate (RAW + sidecar)"
        },
    )
    .clicked()
    {
        action = Some(LibraryAction::Duplicate(context_assets.to_vec()));
        ui.close();
    }
    if moduwu_design::context_menu_item(ui, action_enabled && selected_count == 1, "Rename…")
        .clicked()
    {
        action = Some(LibraryAction::Rename(context_asset.clone()));
        ui.close();
    }
    if moduwu_design::context_menu_item(
        ui,
        action_enabled,
        format!(
            "{}  {}",
            egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE,
            if selected_count > 1 {
                "Reset adjustments for selected"
            } else {
                "Reset all adjustments"
            }
        ),
    )
    .clicked()
    {
        action = Some(LibraryAction::ResetAdjustments(context_assets.to_vec()));
        ui.close();
    }
    ui.separator();
    if moduwu_design::context_menu_item(
        ui,
        action_enabled,
        if selected_count > 1 {
            "Delete selected…"
        } else {
            "Delete…"
        },
    )
    .clicked()
    {
        action = Some(LibraryAction::Delete(context_assets.to_vec()));
        ui.close();
    }
    action
}

pub(crate) fn apply_library_action(
    ui: &mut Ui,
    app: &mut CalibRawApp,
    frame: &eframe::Frame,
    action: LibraryAction,
) {
    match action {
        LibraryAction::Merge(kind, assets) => {
            super::merge::start(app, frame, kind, assets, ui.ctx())
        }
        LibraryAction::Review(assets, change) => super::review::apply_review(app, assets, change),
        LibraryAction::Export(assets) => {
            if !assets.is_empty() {
                app.library.export_dialog = Some(LibraryExportDialog {
                    assets,
                    settings: app.export.settings.clone(),
                    format: app.export.format,
                    overrides: app.export.override_choice.clone(),
                });
            }
        }
        LibraryAction::CopyAdjustments(asset) => {
            app.library.status = match app.copy_library_adjustments(&asset) {
                Ok(()) => format!("Copied adjustments from {}", asset.display_name),
                Err(error) => format!("Could not copy adjustments: {error}"),
            };
        }
        LibraryAction::PasteAdjustments(assets) => {
            let (edited_count, failures) = app.library_adjustment_edit_count(&assets);
            if !failures.is_empty() {
                app.library.status = format!(
                    "Could not inspect selected adjustments. {}",
                    failures.join(" · ")
                );
            } else if edited_count > 0 {
                app.library.adjustment_paste_dialog = Some(LibraryAdjustmentPasteDialog {
                    assets,
                    edited_count,
                });
            } else {
                apply_library_adjustment_paste(
                    app,
                    assets,
                    crate::sidecar::AdjustmentPasteMode::Merge,
                    ui.ctx(),
                    frame,
                );
            }
        }
        LibraryAction::ApplyPreset(assets, preset) => {
            apply_library_preset(app, assets, &preset, ui.ctx(), frame);
        }
        LibraryAction::Copy(assets) => {
            set_library_clipboard(app, ImageClipboardMode::Copy, assets);
        }
        LibraryAction::Cut(assets) => {
            set_library_clipboard(app, ImageClipboardMode::Cut, assets);
        }
        LibraryAction::PasteIntoAssetFolder(asset) => match duplicate_destination(&asset) {
            Ok(destination) => start_image_clipboard_paste(app, destination, ui.ctx()),
            Err(error) => app.library.status = format!("Could not determine paste folder: {error}"),
        },
        LibraryAction::Duplicate(assets) => {
            app.library.clear_selection();
            start_duplicate_assets(app, &assets, ui.ctx());
        }
        LibraryAction::Rename(asset) => {
            app.library.raw_name_dialog = Some(LibraryRawNameDialog {
                name: asset.display_name.clone(),
                asset,
                error: None,
                focus_requested: false,
            });
        }
        LibraryAction::RevealFile(asset) => {
            if let Some(path) = asset.desktop_path() {
                if let Err(error) = app.platform.reveal_file(path) {
                    app.library.status = error;
                }
            }
        }
        LibraryAction::ResetAdjustments(assets) => {
            let current_to_reopen = app.develop.current_path.as_ref().and_then(|current| {
                assets
                    .iter()
                    .find(|asset| asset.desktop_path() == Some(current.as_path()))
                    .and_then(|asset| asset.desktop_path().map(Path::to_path_buf))
            });
            if let Some(path) = current_to_reopen.as_deref() {
                app.detach_current_file_for_library_action(path);
            }

            let total = assets.len();
            let mut changed = 0usize;
            let mut failures = Vec::new();
            for asset in &assets {
                match reset_asset_adjustments(app, asset) {
                    Ok(reset) => {
                        changed += usize::from(reset);
                        app.library.invalidate_adjustment_thumbnail_for_asset(asset);
                    }
                    Err(error) => failures.push(format!("{}: {error}", asset.display_name)),
                }
            }
            app.library.clear_selection();
            app.library.refresh(ui.ctx());
            app.library.status = if failures.is_empty() {
                format!(
                    "Cleared all adjustments for {total} selected {} ({changed} changed)",
                    if total == 1 { "image" } else { "images" }
                )
            } else {
                format!(
                    "Cleared all adjustments for {} of {total} selected images. {}",
                    total.saturating_sub(failures.len()),
                    failures.join(" · ")
                )
            };
            if let Some(path) = current_to_reopen {
                app.reload_desktop_library_document_after_reset(path, frame);
            }
        }
        LibraryAction::Delete(assets) => {
            if !assets.is_empty() {
                app.library.delete_originals_confirmation = Some(assets);
            }
        }
    }
}

/// An "Apply preset" submenu listing every preset. Returns the chosen preset.
fn apply_preset_menu(ui: &mut Ui, app: &CalibRawApp, enabled: bool) -> Option<PathBuf> {
    presets_submenu(ui, &crate::ui::presets::PresetList::from_app(app), enabled)
}

fn presets_submenu(
    ui: &mut Ui,
    presets: &crate::ui::presets::PresetList,
    enabled: bool,
) -> Option<PathBuf> {
    let mut chosen = None;
    ui.add_enabled_ui(enabled && !presets.is_empty(), |ui| {
        let menu = ui.menu_button("Apply preset", |ui| presets.show_menu(ui));
        chosen = menu.inner.flatten();
        menu.response
            .on_disabled_hover_text("Create a preset in the editor's Presets tab first");
    });
    chosen
}

fn delete_confirmed_library_assets(ui: &Ui, app: &mut CalibRawApp, assets: Vec<LibraryAsset>) {
    let total = assets.len();
    let mut deleted = 0usize;
    let mut failures = Vec::new();
    for asset in &assets {
        match delete_library_asset(app, asset) {
            Ok(()) => deleted += 1,
            Err(error) => failures.push(format!("{}: {error}", asset.display_name)),
        }
    }
    app.library.clear_selection();
    app.library.refresh(ui.ctx());
    app.library.status = if failures.is_empty() {
        if app.caps().has_folders() {
            format!(
                "Moved {deleted} selected {} to the {}",
                if deleted == 1 {
                    "original"
                } else {
                    "originals"
                },
                crate::platform::trash_name()
            )
        } else {
            format!(
                "Deleted {deleted} selected {}",
                if deleted == 1 {
                    "original"
                } else {
                    "originals"
                }
            )
        }
    } else {
        format!(
            "Removed {deleted} of {total} selected originals. {}",
            failures.join(" · ")
        )
    };
}

fn set_library_clipboard(
    app: &mut CalibRawApp,
    mode: ImageClipboardMode,
    assets: Vec<LibraryAsset>,
) {
    let count = assets.len();
    app.library.image_clipboard = Some(ImageClipboard { mode, assets });
    app.library.folder_clipboard = None;
    app.library.clear_selection();
    app.library.status = format!(
        "{} {count} photo{}. Choose Paste in a Library folder.",
        if mode == ImageClipboardMode::Copy {
            "Copied"
        } else {
            "Cut"
        },
        if count == 1 { "" } else { "s" }
    );
}

pub(super) fn selection_bar_action_button(
    ui: &mut Ui,
    enabled: bool,
    compact: bool,
    glyph: &'static str,
    label: &'static str,
) -> egui::Response {
    if compact {
        ui.add_enabled_ui(enabled, |ui| {
            moduwu_design::ghost_icon_button(ui, glyph, moduwu_design::toolbar_icon_size(), label)
        })
        .inner
    } else {
        moduwu_design::secondary_button_enabled(ui, enabled, format!("{glyph}  {label}"))
            .on_hover_text(label)
    }
}

pub(super) fn selection_bar_more_menu<R>(
    ui: &mut Ui,
    enabled: bool,
    compact: bool,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> egui::InnerResponse<Option<R>> {
    let label = if compact {
        egui::RichText::new(egui_phosphor::regular::DOTS_THREE)
            .size(moduwu_design::CONTROL_HEIGHT * 0.55)
    } else {
        egui::RichText::new(format!("{}  More", egui_phosphor::regular::DOTS_THREE))
    };
    ui.add_enabled_ui(enabled, |ui| ui.menu_button(label, add_contents))
        .inner
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SelectionBarCommand {
    Export,
    Merge(super::MergeKind),
    CopyAdjustments,
    PasteAdjustments,
    ApplyPreset(PathBuf),
    Copy,
    Cut,
    Duplicate,
    Rename,
    ResetAdjustments,
    Delete,
}

pub(super) fn selection_bar_actions(
    ui: &mut Ui,
    selected_count: usize,
    action_enabled: bool,
    can_paste_adjustments: bool,
    presets: &crate::ui::presets::PresetList,
    compact: bool,
    caps: PlatformCaps,
) -> Option<SelectionBarCommand> {
    let mut action = None;
    if selection_bar_action_button(
        ui,
        action_enabled,
        compact,
        egui_phosphor::regular::EXPORT,
        "Export",
    )
    .clicked()
    {
        action = Some(SelectionBarCommand::Export);
    }
    if selected_count >= 2 {
        for (kind, glyph) in [
            (super::MergeKind::Hdr, egui_phosphor::regular::STACK),
            (
                super::MergeKind::FocusStack,
                egui_phosphor::regular::CROSSHAIR_SIMPLE,
            ),
        ] {
            if selection_bar_action_button(ui, action_enabled, compact, glyph, kind.title())
                .clicked()
            {
                action = Some(SelectionBarCommand::Merge(kind));
            }
        }
    }
    if selected_count == 1
        && selection_bar_action_button(
            ui,
            action_enabled,
            compact,
            egui_phosphor::regular::SLIDERS_HORIZONTAL,
            "Copy adjustments",
        )
        .clicked()
    {
        action = Some(SelectionBarCommand::CopyAdjustments);
    }
    if selection_bar_action_button(
        ui,
        action_enabled && can_paste_adjustments,
        compact,
        egui_phosphor::regular::CLIPBOARD_TEXT,
        "Paste adjustments",
    )
    .on_disabled_hover_text("Copy adjustments from an image first")
    .clicked()
    {
        action = Some(SelectionBarCommand::PasteAdjustments);
    }
    // Copying and cutting work on photos in folders.
    if caps.has_folders()
        && selection_bar_action_button(
            ui,
            action_enabled,
            compact,
            egui_phosphor::regular::COPY,
            "Copy",
        )
        .clicked()
    {
        action = Some(SelectionBarCommand::Copy);
    }
    selection_bar_more_menu(ui, action_enabled, compact, |ui| {
        if let Some(preset) = presets_submenu(ui, presets, true) {
            action = Some(SelectionBarCommand::ApplyPreset(preset));
        }
        if caps.has_folders() && moduwu_design::menu_item(ui, true, "Cut").clicked() {
            action = Some(SelectionBarCommand::Cut);
            ui.close();
        }
        if moduwu_design::menu_item(
            ui,
            true,
            if selected_count > 1 {
                "Duplicate selected (RAW + sidecars)"
            } else {
                "Duplicate (RAW + sidecar)"
            },
        )
        .clicked()
        {
            action = Some(SelectionBarCommand::Duplicate);
            ui.close();
        }
        if selected_count == 1 && moduwu_design::menu_item(ui, true, "Rename…").clicked() {
            action = Some(SelectionBarCommand::Rename);
            ui.close();
        }
        if moduwu_design::menu_item(
            ui,
            true,
            if selected_count > 1 {
                "Reset adjustments for selected"
            } else {
                "Reset all adjustments"
            },
        )
        .clicked()
        {
            action = Some(SelectionBarCommand::ResetAdjustments);
            ui.close();
        }
        ui.separator();
        if moduwu_design::menu_item(
            ui,
            true,
            if selected_count > 1 {
                "Delete selected…"
            } else {
                "Delete…"
            },
        )
        .clicked()
        {
            action = Some(SelectionBarCommand::Delete);
            ui.close();
        }
    })
    .response
    .on_hover_text("More selection actions");
    action
}

pub(super) fn library_selection_action(
    command: SelectionBarCommand,
    assets: &[LibraryAsset],
) -> Option<LibraryAction> {
    match command {
        SelectionBarCommand::Export => Some(LibraryAction::Export(assets.to_vec())),
        SelectionBarCommand::Merge(kind) => {
            (assets.len() >= 2).then(|| LibraryAction::Merge(kind, assets.to_vec()))
        }
        SelectionBarCommand::CopyAdjustments if assets.len() == 1 => {
            assets.first().cloned().map(LibraryAction::CopyAdjustments)
        }
        SelectionBarCommand::CopyAdjustments => None,
        SelectionBarCommand::PasteAdjustments => {
            Some(LibraryAction::PasteAdjustments(assets.to_vec()))
        }
        SelectionBarCommand::ApplyPreset(preset) => {
            Some(LibraryAction::ApplyPreset(assets.to_vec(), preset))
        }
        SelectionBarCommand::Copy => Some(LibraryAction::Copy(assets.to_vec())),
        SelectionBarCommand::Cut => Some(LibraryAction::Cut(assets.to_vec())),
        SelectionBarCommand::Duplicate => Some(LibraryAction::Duplicate(assets.to_vec())),
        SelectionBarCommand::Rename if assets.len() == 1 => {
            assets.first().cloned().map(LibraryAction::Rename)
        }
        SelectionBarCommand::Rename => None,
        SelectionBarCommand::ResetAdjustments => {
            Some(LibraryAction::ResetAdjustments(assets.to_vec()))
        }
        SelectionBarCommand::Delete => Some(LibraryAction::Delete(assets.to_vec())),
    }
}

pub(super) fn show_library_selection_action_bar(
    ui: &Ui,
    app: &mut CalibRawApp,
    selected: &[LibraryAsset],
    library_action: &mut Option<LibraryAction>,
) {
    if selected.is_empty() {
        return;
    }
    let bounds = ui.max_rect();
    let compact = !moduwu_design::ResponsiveWidth::from_width(bounds.width()).is_wide();
    let count = selected.len();
    let mut clear_selection = false;
    egui::Area::new(egui::Id::new("library-selection-action-bar"))
        .order(egui::Order::Foreground)
        .pivot(egui::Align2::CENTER_BOTTOM)
        .fixed_pos(egui::pos2(bounds.center().x, bounds.bottom() - 12.0))
        .constrain_to(bounds)
        .movable(false)
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style())
                .inner_margin(egui::Margin::symmetric(moduwu_design::SPACE_SM as i8, 6))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.x = if compact {
                        moduwu_design::SPACE_XS
                    } else {
                        6.0
                    };
                    ui.spacing_mut().interact_size.y = moduwu_design::CONTROL_HEIGHT;
                    ui.horizontal(|ui| {
                        let count_label =
                            if compact && bounds.width() < SELECTION_BAR_COUNT_LABEL_BREAKPOINT {
                                count.to_string()
                            } else {
                                format!("{count} selected")
                            };
                        ui.strong(count_label).on_hover_text(format!(
                            "{count} selected {}",
                            if count == 1 { "photo" } else { "photos" }
                        ));
                        ui.separator();
                        let action_enabled = !local_action_in_progress(app)
                            && app.library_batch_export_progress().is_none()
                            && app.library_ai_mask_refresh_status().is_none();
                        let presets = crate::ui::presets::PresetList::from_app(app);
                        if let Some(action) = selection_bar_actions(
                            ui,
                            count,
                            action_enabled,
                            app.library.has_copied_adjustments(),
                            &presets,
                            compact,
                            app.caps(),
                        )
                        .and_then(|command| library_selection_action(command, selected))
                        {
                            *library_action = Some(action);
                        }
                        ui.separator();
                        if moduwu_design::ghost_icon_button(
                            ui,
                            egui_phosphor::regular::X,
                            moduwu_design::toolbar_icon_size(),
                            "Clear selection",
                        )
                        .clicked()
                        {
                            clear_selection = true;
                        }
                    });
                });
        });
    if clear_selection {
        app.library.clear_selection();
    }
}

pub(crate) fn show_library_action_overlays(
    ui: &mut Ui,
    app: &mut CalibRawApp,
    frame: &eframe::Frame,
) {
    let delete_choice = app
        .library
        .delete_originals_confirmation
        .as_ref()
        .and_then(|assets| show_delete_originals_confirmation(ui, assets, app.caps()));
    if let Some(confirmed) = delete_choice {
        if let Some(assets) = app.library.delete_originals_confirmation.take() {
            if confirmed {
                delete_confirmed_library_assets(ui, app, assets);
            }
        }
    }

    show_library_raw_name_dialog(ui, app, frame);

    let paste_choice = app
        .library
        .adjustment_paste_dialog
        .as_ref()
        .and_then(|dialog| {
            show_adjustment_paste_choice(
                ui,
                "library-adjustment-paste-conflict-dialog",
                dialog.edited_count,
                dialog.assets.len(),
            )
        });
    if let Some(choice) = paste_choice {
        if let Some(dialog) = app.library.adjustment_paste_dialog.take() {
            let mode = match choice {
                AdjustmentPasteChoice::Merge => Some(crate::sidecar::AdjustmentPasteMode::Merge),
                AdjustmentPasteChoice::Replace => {
                    Some(crate::sidecar::AdjustmentPasteMode::Replace)
                }
                AdjustmentPasteChoice::Cancel => None,
            };
            if let Some(mode) = mode {
                apply_library_adjustment_paste(app, dialog.assets, mode, ui.ctx(), frame);
            }
        }
    }

    let can_regenerate = app.can_start_library_ai_mask_refresh();
    let refresh_choice = app
        .library
        .ai_mask_refresh_prompt
        .as_ref()
        .and_then(|prompt| {
            show_ai_mask_refresh_choice(
                ui,
                "library-ai-mask-refresh-prompt",
                prompt.assets.len(),
                can_regenerate,
            )
        });
    if let Some(choice) = refresh_choice {
        if let Some(prompt) = app.library.ai_mask_refresh_prompt.take() {
            if choice == AiMaskRefreshChoice::Regenerate {
                start_library_ai_mask_refresh_for_assets(app, prompt.assets, frame);
            }
        }
    }

    if let Some((completed, total, failed, current_name)) = app.library_ai_mask_refresh_status() {
        if show_ai_mask_refresh_progress(ui, completed, total, failed, current_name.as_deref()) {
            app.cancel_library_ai_mask_refresh();
        }
    }

    let mut close_export_dialog = false;
    let mut confirm_export = false;
    let mut selected_export_format = None;
    // Folder exports ask where to save; media-library exports are published.
    let publishes = app.caps().has_media_library();
    let presets = app.presets.all();
    let luts = app.luts.entries();
    if let Some(dialog) = app.library.export_dialog.as_mut() {
        let count = dialog.assets.len();
        let export_picker_directory = dialog
            .assets
            .first()
            .and_then(LibraryAsset::desktop_path)
            .and_then(Path::parent)
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(Path::to_path_buf);
        let title = if count == 1 {
            "Export image".to_owned()
        } else {
            format!("Export {count} images")
        };
        let help = if publishes {
            "Exports are saved to Pictures/CalibRaw. File names are generated from each photo's name."
        } else if count > 1 {
            "A destination folder will be selected for the batch. File names are generated from each photo's name."
        } else {
            "Choose the output file after pressing Export."
        };
        let label = match (count, publishes) {
            (1, false) => "Export 1 image…".to_owned(),
            (1, true) => "Export 1 image".to_owned(),
            (_, false) => format!("Export {count} images…"),
            (_, true) => format!("Export {count} images"),
        };
        moduwu_design::Dialog::new("library-export-dialog", title)
            .size(moduwu_design::DialogSize::Wide)
            .show_with_footer(
                ui.ctx(),
                |ui| {
                    let format_changed = show_library_export_settings_controls(
                        ui,
                        &mut dialog.format,
                        &mut dialog.settings,
                        export_picker_directory.as_deref(),
                    );
                    if format_changed {
                        selected_export_format = Some(dialog.format);
                    }
                    show_export_override_controls(ui, &mut dialog.overrides, presets, luts);
                },
                |ui| {
                    if moduwu_design::primary_action_button(ui, label)
                        .on_hover_text(help)
                        .clicked()
                    {
                        confirm_export = true;
                    }
                    if moduwu_design::secondary_button(ui, "Cancel").clicked() {
                        close_export_dialog = true;
                    }
                    if !close_export_dialog
                        && !confirm_export
                        && moduwu_design::dialog_keyboard_action(
                            ui,
                            moduwu_design::DialogKeyboard::CLOSE_ONLY,
                            false,
                        ) == moduwu_design::DialogAction::Cancel
                    {
                        close_export_dialog = true;
                    }
                },
            );
    }

    if let Some(format) = selected_export_format {
        app.export.format = format;
        app.persist_performance_settings();
    }

    if confirm_export {
        if let Some(dialog) = app.library.export_dialog.clone() {
            let export_override = app.export_override(&dialog.overrides);
            app.export.override_choice = dialog.overrides;
            if start_local_library_export(
                app,
                &dialog.assets,
                dialog.settings,
                dialog.format,
                export_override,
                frame,
            ) {
                app.library.clear_selection();
                app.library.export_dialog = None;
            }
        }
    } else if close_export_dialog {
        app.library.export_dialog = None;
    }
}
