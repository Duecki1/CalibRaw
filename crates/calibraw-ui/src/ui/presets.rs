//! The Presets sidebar panel, its dialogs and the "Apply preset" menus.
use crate::app::{CalibRawApp, GroupDialogMode, PresetEditor, PresetEditorMode};
use crate::pipeline::AdjustmentGroup;
use crate::presets::DeletedGroupPresets;
use crate::sidecar::EditSelection;
use crate::ui::theme;
use eframe::egui::{self, Ui};
use std::path::PathBuf;

#[cfg(not(target_os = "android"))]
const ROW_HELP: &str = "Previewing on the photo. Click to apply, right-click for more.";
#[cfg(target_os = "android")]
const ROW_HELP: &str = "Tap to apply to this photo.";

#[derive(Clone, Debug)]
enum PresetAction {
    Apply(PathBuf),
    Rename(PathBuf),
    #[cfg(not(target_os = "android"))]
    Export(PathBuf),
    Delete(PathBuf),
    NewPreset(String),
    NewGroup,
    RenameGroup(String),
    DeleteGroup(String),
}

/// Presets grouped for display. Collected up front so drawing does not hold
/// a borrow of the app.
#[derive(Clone, Debug, Default)]
pub(crate) struct PresetList {
    groups: Vec<PresetListGroup>,
}

#[derive(Clone, Debug)]
struct PresetListGroup {
    name: String,
    presets: Vec<PresetListEntry>,
}

#[derive(Clone, Debug)]
struct PresetListEntry {
    path: PathBuf,
    name: String,
    summary: String,
}

impl PresetList {
    /// Every group with its presets, including empty groups.
    pub(crate) fn from_app(app: &CalibRawApp) -> Self {
        let groups = app
            .presets
            .groups()
            .iter()
            .map(|group| PresetListGroup {
                name: group.clone(),
                presets: app
                    .presets
                    .all()
                    .iter()
                    .filter(|stored| {
                        crate::presets::find_group(
                            std::slice::from_ref(group),
                            stored.preset.group(),
                        )
                        .is_some()
                    })
                    .map(|stored| PresetListEntry {
                        path: stored.path.clone(),
                        name: stored.preset.name().to_owned(),
                        summary: selection_summary(stored.preset.selection()),
                    })
                    .collect(),
            })
            .collect();
        Self { groups }
    }

    /// Whether there is no preset to apply. Empty groups do not count.
    pub(crate) fn is_empty(&self) -> bool {
        self.groups.iter().all(|group| group.presets.is_empty())
    }

    /// Menu items for every preset, one submenu per group when there are
    /// several. Returns the preset that was chosen.
    pub(crate) fn show_menu(&self, ui: &mut Ui) -> Option<PathBuf> {
        let mut chosen = None;
        let mut show_entries = |ui: &mut Ui, group: &PresetListGroup| {
            for entry in &group.presets {
                if crate::ui::theme::menu_item(ui, true, &entry.name)
                    .on_hover_text(&entry.summary)
                    .clicked()
                {
                    chosen = Some(entry.path.clone());
                    ui.close();
                }
            }
        };
        let groups: Vec<_> = self
            .groups
            .iter()
            .filter(|group| !group.presets.is_empty())
            .collect();
        match groups.as_slice() {
            [group] => show_entries(ui, group),
            groups => {
                for group in groups {
                    ui.menu_button(&group.name, |ui| show_entries(ui, group));
                }
            }
        }
        chosen
    }
}

/// A one-line description of what a preset changes, such as
/// "Light, Color, Effects · Masks".
fn selection_summary(selection: EditSelection) -> String {
    let mut parts: Vec<&str> = selection
        .adjustment_groups
        .iter()
        .map(AdjustmentGroup::label)
        .collect();
    let mut extras = Vec::new();
    for (included, label) in [
        (selection.raw_processing, "RAW processing"),
        (selection.camera_profile, "Camera profile"),
        (selection.masks, "Masks"),
        (selection.ai_masks, "AI masks"),
        (selection.geometry, "Crop & geometry"),
        (selection.lens_correction, "Lens correction"),
    ] {
        if included {
            extras.push(label);
        }
    }
    let adjustments = parts.join(", ");
    parts.clear();
    if !adjustments.is_empty() {
        parts.push(&adjustments);
    }
    let extras = extras.join(", ");
    if !extras.is_empty() {
        parts.push(&extras);
    }
    parts.join(" · ")
}

/// The Presets tab's action for the sidebar header, next to the histogram
/// toggle, like the reset buttons of the other tabs. Creating presets and
/// groups happens in the list itself.
#[cfg(not(target_os = "android"))]
pub(crate) fn show_header_actions(ui: &mut Ui, app: &mut CalibRawApp) {
    if crate::ui::icons::phosphor_icon_button_enabled(
        ui,
        app.presets.is_available(),
        egui_phosphor::regular::DOWNLOAD_SIMPLE,
        theme::toolbar_icon_size(),
        "Import presets",
    )
    .clicked()
    {
        app.choose_preset_files_to_import();
    }
}

pub(crate) fn show_panel(ui: &mut Ui, app: &mut CalibRawApp, frame: &eframe::Frame) {
    let list = PresetList::from_app(app);
    let mut action = None;

    let mut notes = Vec::new();
    if !app.presets.is_available() {
        notes.push((
            "Presets are unavailable because CalibRaw has no settings folder.".to_owned(),
            ui.visuals().warn_fg_color,
            None,
        ));
    }
    if !app.presets.load_failures.is_empty() {
        let count = app.presets.load_failures.len();
        notes.push((
            format!(
                "{count} preset {} could not be read.",
                if count == 1 { "file" } else { "files" }
            ),
            ui.visuals().warn_fg_color,
            Some(app.presets.load_failures.join("\n")),
        ));
    }
    if list.groups.is_empty() {
        notes.push((
            "No presets yet. Create a group, then add presets to it from an edited photo."
                .to_owned(),
            ui.visuals().weak_text_color(),
            None,
        ));
    }
    if !notes.is_empty() {
        theme::content_card(ui, |ui| {
            for (text, color, details) in &notes {
                let response = show_note(ui, text, *color);
                if let Some(details) = details {
                    response.on_hover_text(details);
                }
            }
        });
    }

    let can_create_preset = app.can_create_preset();
    let mut hovered = None;
    for (index, group) in list.groups.iter().enumerate() {
        if index > 0 || !notes.is_empty() {
            theme::card_gap(ui);
        }
        theme::content_card(ui, |ui| {
            show_group_title(ui, &group.name, &mut action);
            for entry in &group.presets {
                if show_preset_row(ui, entry, &mut action) {
                    hovered = Some(entry.path.clone());
                }
            }
            if show_add_row(ui, can_create_preset, "New preset")
                .on_hover_text("Save settings of this photo as a preset in this group")
                .on_disabled_hover_text("Open a photo to create a preset from its settings")
                .clicked()
            {
                action = Some(PresetAction::NewPreset(group.name.clone()));
            }
        });
    }
    if !list.groups.is_empty() || !notes.is_empty() {
        theme::card_gap(ui);
    }
    if ui
        .add_enabled_ui(app.presets.is_available(), |ui| {
            theme::full_width_button(ui, format!("{}  New group", egui_phosphor::regular::PLUS))
        })
        .inner
        .clicked()
    {
        action = Some(PresetAction::NewGroup);
    }
    // Touch screens have no hover; a tap applies the preset directly.
    #[cfg(not(target_os = "android"))]
    if let Some(path) = hovered {
        app.presets.hover.request(path);
    }
    #[cfg(target_os = "android")]
    let _ = hovered;

    match action {
        Some(PresetAction::Apply(path)) => app.apply_preset_to_current(&path, frame),
        Some(PresetAction::Rename(path)) => app.open_rename_preset_editor(&path),
        #[cfg(not(target_os = "android"))]
        Some(PresetAction::Export(path)) => app.export_preset(&path),
        Some(PresetAction::Delete(path)) => app.presets.pending_delete = Some(path),
        Some(PresetAction::NewPreset(group)) => app.open_new_preset_editor(group),
        Some(PresetAction::NewGroup) => app.open_group_dialog(GroupDialogMode::Create),
        Some(PresetAction::RenameGroup(group)) => {
            app.open_group_dialog(GroupDialogMode::Rename { group });
        }
        Some(PresetAction::DeleteGroup(group)) => app.presets.pending_group_delete = Some(group),
        None => {}
    }
}

fn show_note(ui: &mut Ui, text: &str, color: egui::Color32) -> egui::Response {
    ui.add(egui::Label::new(egui::RichText::new(text).small().color(color)).wrap())
}

/// A row of control height: the title or button fills it, and on Android a
/// menu button sits at its right end. A bare right-to-left layout would
/// claim the remaining height of the panel.
fn show_menu_row<R>(
    ui: &mut Ui,
    menu_id: impl egui::AsIdSalt,
    add_menu: impl FnOnce(&mut Ui),
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> R {
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), theme::CONTROL_HEIGHT),
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| {
            // Touch screens have no right-click, so the menu gets a button.
            #[cfg(target_os = "android")]
            ui.push_id(menu_id, |ui| {
                let button = crate::ui::icons::phosphor_icon_button(
                    ui,
                    egui_phosphor::regular::DOTS_THREE_VERTICAL,
                    theme::toolbar_icon_size(),
                    "More actions",
                );
                theme::dropdown_menu(&button, add_menu);
            });
            #[cfg(not(target_os = "android"))]
            let _ = (menu_id, add_menu);
            ui.with_layout(
                egui::Layout::left_to_right(egui::Align::Center),
                add_contents,
            )
            .inner
        },
    )
    .inner
}

/// The group's card title. Its actions are on a right-click menu, like the
/// mask list, and on a menu button on Android.
fn show_group_title(ui: &mut Ui, group: &str, action: &mut Option<PresetAction>) {
    let menu = |ui: &mut Ui, action: &mut Option<PresetAction>| {
        if theme::menu_item(ui, true, "Rename group…").clicked() {
            *action = Some(PresetAction::RenameGroup(group.to_owned()));
            ui.close();
        }
        ui.separator();
        if theme::destructive_menu_item(ui, "Delete group…").clicked() {
            *action = Some(PresetAction::DeleteGroup(group.to_owned()));
            ui.close();
        }
    };
    let mut android_action = None;
    let title = show_menu_row(
        ui,
        ("preset-group-menu", group),
        |ui| menu(ui, &mut android_action),
        |ui| {
            ui.add(
                egui::Label::new(egui::RichText::new(group).strong())
                    .truncate()
                    .sense(egui::Sense::click()),
            )
        },
    );
    theme::context_menu(&title, |ui| menu(ui, action));
    if android_action.is_some() {
        *action = android_action;
    }
}

/// The button that ends a group card, styled like "New group" below the
/// cards.
fn show_add_row(ui: &mut Ui, enabled: bool, label: &str) -> egui::Response {
    ui.add_enabled_ui(enabled, |ui| {
        theme::full_width_button(ui, format!("{}  {label}", egui_phosphor::regular::PLUS))
    })
    .inner
}

/// Draws one preset as a framed button with its name on the left. Returns
/// whether the pointer rests on it.
fn show_preset_row(
    ui: &mut Ui,
    entry: &PresetListEntry,
    action: &mut Option<PresetAction>,
) -> bool {
    let mut android_action = None;
    let response = show_menu_row(
        ui,
        ("preset-menu", &entry.path),
        |ui| preset_actions_menu(ui, entry, &mut android_action),
        |ui| {
            ui.add_sized(
                [ui.available_width(), theme::CONTROL_HEIGHT],
                egui::Button::new(())
                    .left_text(entry.name.as_str())
                    .truncate()
                    .corner_radius(theme::CARD_RADIUS),
            )
        },
    )
    .on_hover_text(format!("{ROW_HELP}\n{}", entry.summary));
    theme::context_menu(&response, |ui| preset_actions_menu(ui, entry, action));
    if response.clicked() {
        *action = Some(PresetAction::Apply(entry.path.clone()));
    }
    if android_action.is_some() {
        *action = android_action;
    }
    // An open menu means the pointer is choosing an action, not looking at
    // the preset.
    response.hovered() && !egui::Popup::is_any_open(ui.ctx())
}

fn preset_actions_menu(ui: &mut Ui, entry: &PresetListEntry, action: &mut Option<PresetAction>) {
    if crate::ui::theme::menu_item(ui, true, "Apply to this photo").clicked() {
        *action = Some(PresetAction::Apply(entry.path.clone()));
        ui.close();
    }
    if crate::ui::theme::menu_item(ui, true, "Rename…").clicked() {
        *action = Some(PresetAction::Rename(entry.path.clone()));
        ui.close();
    }
    #[cfg(not(target_os = "android"))]
    if crate::ui::theme::menu_item(ui, true, "Export…").clicked() {
        *action = Some(PresetAction::Export(entry.path.clone()));
        ui.close();
    }
    ui.separator();
    if crate::ui::theme::destructive_menu_item(ui, "Delete…").clicked() {
        *action = Some(PresetAction::Delete(entry.path.clone()));
        ui.close();
    }
}

/// The preset editor and delete confirmation. Shown on top of every tab.
pub(crate) fn show_dialogs(ctx: &egui::Context, app: &mut CalibRawApp) {
    show_editor(ctx, app);
    // Drawn after the editor so it opens on top of it.
    show_group_dialog(ctx, app);
    show_group_delete_confirmation(ctx, app);
    show_delete_confirmation(ctx, app);
}

fn show_editor(ctx: &egui::Context, app: &mut CalibRawApp) {
    let groups = app.presets.groups().to_vec();
    // The group dialog on top owns the keyboard and pointer.
    let group_dialog_open = app.presets.group_dialog.is_some();
    let replaces_existing = app.presets.editor.as_ref().is_some_and(|editor| {
        matches!(editor.mode, PresetEditorMode::Create { .. })
            && app
                .presets
                .find_named(&editor.name, &editor.group, None)
                .is_some()
    });
    let Some(editor) = app.presets.editor.as_mut() else {
        return;
    };

    let creating = matches!(editor.mode, PresetEditorMode::Create { .. });
    let mut open_group_dialog = false;
    // The body updates eligibility before the fixed footer is drawn, without
    // making either closure hold a second borrow of the editor.
    let confirm_enabled = std::cell::Cell::new(false);
    let mut choice = theme::DialogAction::None;
    theme::dialog_window(
        if creating {
            "New preset"
        } else {
            "Rename preset"
        },
        ctx,
        if creating {
            theme::DIALOG_WIDTH_LARGE
        } else {
            theme::DIALOG_WIDTH_FORM
        },
    )
    .id(egui::Id::new("preset-editor-dialog"))
    .show_with_footer(
        ctx,
        |ui| {
            if group_dialog_open {
                ui.disable();
            }
            ui.label("Name");
            let response = theme::dialog_text_field(
                ui,
                &mut editor.name,
                "preset-editor-name",
                "e.g. Warm matte",
            );
            theme::request_initial_focus(&response, &mut editor.focus_requested);

            ui.label("Group");
            open_group_dialog |= show_group_dropdown(ui, editor, &groups);

            if let PresetEditorMode::Create { edits } = &editor.mode {
                theme::card_gap(ui);
                show_selection_controls(ui, &mut editor.selection, edits);
            }

            if replaces_existing {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(
                            "A preset with this name exists in this group and will be replaced.",
                        )
                        .small()
                        .color(ui.visuals().warn_fg_color),
                    )
                    .wrap(),
                );
            }
            if let Some(error) = &editor.error {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(error)
                            .small()
                            .color(ui.visuals().error_fg_color),
                    )
                    .wrap(),
                );
            }

            confirm_enabled
                .set(!editor.name.trim().is_empty() && (!creating || !editor.selection.is_empty()));
        },
        |ui| {
            if group_dialog_open {
                ui.disable();
            }
            let confirm_label = match (creating, replaces_existing) {
                (true, true) => "Replace",
                (true, false) => "Save preset",
                (false, _) => "Rename",
            };
            choice = theme::dialog_confirmation_buttons(
                ui,
                "Cancel",
                confirm_label,
                confirm_enabled.get(),
                false,
                theme::DialogKeyboard::CONFIRM_ON_ENTER,
            );
        },
    );

    if open_group_dialog {
        app.open_group_dialog(GroupDialogMode::Create);
    }
    match choice {
        crate::ui::theme::DialogAction::Cancel => app.presets.editor = None,
        crate::ui::theme::DialogAction::Confirm => app.confirm_preset_editor(),
        crate::ui::theme::DialogAction::None => {}
    }
}

/// The group dropdown: existing groups, the editor's group when it is new,
/// and "New group…". Returns whether "New group…" was chosen.
fn show_group_dropdown(ui: &mut Ui, editor: &mut PresetEditor, groups: &[String]) -> bool {
    let mut choices = groups.to_vec();
    let current = editor.group.to_lowercase();
    if !choices.iter().any(|group| group.to_lowercase() == current) {
        choices.insert(0, editor.group.clone());
    }
    let mut open_new_group = false;
    theme::combo_box(
        "preset-editor-group",
        editor.group.clone(),
        ui.available_width(),
    )
    .show_ui(ui, |ui| {
        for group in &choices {
            ui.selectable_value(&mut editor.group, group.clone(), group);
        }
        ui.separator();
        if ui
            .selectable_label(
                false,
                format!("{}  New group…", egui_phosphor::regular::PLUS),
            )
            .clicked()
        {
            open_new_group = true;
        }
    });
    open_new_group
}

fn show_group_dialog(ctx: &egui::Context, app: &mut CalibRawApp) {
    let Some(dialog) = app.presets.group_dialog.as_mut() else {
        return;
    };
    let (title, confirm_label) = match dialog.mode {
        GroupDialogMode::Create => ("New group", "Add group"),
        GroupDialogMode::Rename { .. } => ("Rename group", "Rename"),
    };
    let mut choice = theme::DialogAction::None;
    theme::dialog_window(title, ctx, theme::DIALOG_WIDTH_FORM)
        .id(egui::Id::new("preset-group-dialog"))
        .show(ctx, |ui| {
            ui.label("Group name");
            let response =
                theme::dialog_text_field(ui, &mut dialog.name, "preset-group-name", "e.g. Film");
            theme::request_initial_focus(&response, &mut dialog.focus_requested);
            if let Some(error) = &dialog.error {
                show_note(ui, error, ui.visuals().error_fg_color);
            }
            choice = theme::dialog_confirmation_buttons(
                ui,
                "Cancel",
                confirm_label,
                !dialog.name.trim().is_empty(),
                false,
                theme::DialogKeyboard::CONFIRM_ON_ENTER,
            );
        });
    match choice {
        theme::DialogAction::Cancel => app.presets.group_dialog = None,
        theme::DialogAction::Confirm => app.confirm_group_dialog(),
        theme::DialogAction::None => {}
    }
}

fn show_group_delete_confirmation(ctx: &egui::Context, app: &mut CalibRawApp) {
    let Some(group) = app.presets.pending_group_delete.clone() else {
        return;
    };
    let count = PresetList::from_app(app)
        .groups
        .iter()
        .find(|listed| listed.name == group)
        .map_or(0, |listed| listed.presets.len());
    let can_move = crate::presets::find_group(
        std::slice::from_ref(&group),
        crate::presets::DEFAULT_PRESET_GROUP,
    )
    .is_none();

    let mut choice = None;
    let mut cancel = false;
    theme::dialog_window("Delete group?", ctx, theme::DIALOG_WIDTH_DEFAULT)
        .id(egui::Id::new("preset-group-delete-confirmation"))
        .show(ctx, |ui| {
            ui.add(
                egui::Label::new(match count {
                    0 => format!("“{group}” will be deleted."),
                    1 => format!("“{group}” contains 1 preset."),
                    count => format!("“{group}” contains {count} presets."),
                })
                .wrap(),
            );
            theme::dialog_button_row(ui, |ui| {
                let delete_label = if count == 0 {
                    "Delete"
                } else {
                    "Delete with presets"
                };
                if ui
                    .add(
                        egui::Button::new(
                            egui::RichText::new(delete_label).color(ui.visuals().error_fg_color),
                        )
                        .min_size(egui::vec2(0.0, theme::CONTROL_HEIGHT)),
                    )
                    .clicked()
                {
                    choice = Some(DeletedGroupPresets::Delete);
                }
                if count > 0
                    && can_move
                    && theme::primary_action_button(
                        ui,
                        format!("Move presets to {}", crate::presets::DEFAULT_PRESET_GROUP),
                    )
                    .clicked()
                {
                    choice = Some(DeletedGroupPresets::MoveToDefaultGroup);
                }
                cancel |= theme::secondary_button(ui, "Cancel").clicked();
            });
            cancel |= choice.is_none()
                && theme::dialog_keyboard_action(ui, theme::DialogKeyboard::CLOSE_ONLY, false)
                    == theme::DialogAction::Cancel;
        });
    if let Some(presets) = choice {
        app.presets.pending_group_delete = None;
        app.delete_preset_group(&group, presets);
    } else if cancel {
        app.presets.pending_group_delete = None;
    }
}

fn show_selection_controls(
    ui: &mut Ui,
    selection: &mut EditSelection,
    edits: &crate::sidecar::EditState,
) {
    theme::strong_with_help(
        ui,
        "Include settings",
        "Settings that are not selected keep the destination photo’s values. Selecting an unchanged group also saves its default values.",
    );
    ui.add_space(theme::SPACE_XS);
    // Each column retains a readable checkbox width; small viewports stack the
    // same cards inside the dialog's bounded scroll area.
    if ui.available_width() >= 440.0 {
        ui.columns(2, |columns| {
            show_adjustment_selection(&mut columns[0], selection, edits);
            show_additional_selection(&mut columns[1], selection);
        });
    } else {
        show_adjustment_selection(ui, selection, edits);
        theme::card_gap(ui);
        show_additional_selection(ui, selection);
    }

    let selected_count = selection.adjustment_groups.iter().count()
        + [
            selection.raw_processing,
            selection.camera_profile,
            selection.masks,
            selection.ai_masks,
            selection.geometry,
            selection.lens_correction,
        ]
        .into_iter()
        .filter(|included| *included)
        .count();
    ui.add_space(theme::SPACE_XS);
    ui.add(
        egui::Label::new(
            egui::RichText::new(if selected_count == 0 {
                "Choose at least one setting to save this preset.".to_owned()
            } else {
                format!(
                    "{selected_count} {} selected",
                    if selected_count == 1 {
                        "setting"
                    } else {
                        "settings"
                    }
                )
            })
            .small()
            .weak(),
        )
        .wrap(),
    );

    let has_photo_specific_masks = edits
        .masks
        .masks
        .iter()
        .flat_map(|mask| &mask.components)
        .any(|component| !crate::presets::is_portable_mask_kind(component.kind));
    if has_photo_specific_masks && (selection.masks || selection.ai_masks) {
        ui.add(egui::Label::new(
            egui::RichText::new(
                "Brush, path and object masks follow this photo's content and are not saved in presets.",
            )
            .small()
            .weak(),
        ).wrap());
    }
}

fn show_adjustment_selection(
    ui: &mut Ui,
    selection: &mut EditSelection,
    edits: &crate::sidecar::EditState,
) {
    theme::content_card(ui, |ui| {
        theme::strong_with_help(
            ui,
            "Adjustments",
            "Each group matches a card in the Edit tab. Groups that are not selected keep the destination photo's settings. Edited groups are shown in bold.",
        );
        theme::action_row(ui, |ui| {
            if theme::secondary_button(ui, "All")
                .on_hover_text("Include every adjustment group, including unchanged values")
                .clicked()
            {
                selection.adjustment_groups = crate::pipeline::AdjustmentGroupSet::ALL;
            }
            if theme::secondary_button(ui, "None")
                .on_hover_text("Clear the adjustment groups; other settings stay selected")
                .clicked()
            {
                selection.adjustment_groups = crate::pipeline::AdjustmentGroupSet::EMPTY;
            }
        });
        ui.add_space(theme::SPACE_XS);
        theme::action_row(ui, |ui| {
            for group in AdjustmentGroup::ALL {
                let included = selection.adjustment_groups.contains(group);
                let edited = edits.exposure.group_is_edited(group);
                let label = egui::RichText::new(group.label());
                let label = if edited { label.strong() } else { label };
                let help = if edited {
                    "Edited on this photo. Include this group’s current settings."
                } else {
                    "Unchanged on this photo. Including this group replaces the destination’s settings with these defaults."
                };
                if theme::toggle_button(ui, label, included)
                    .on_hover_text(help)
                    .clicked()
                {
                    selection.adjustment_groups.set(group, !included);
                }
            }
        });
    });
}

fn show_additional_selection(ui: &mut Ui, selection: &mut EditSelection) {
    theme::content_card(ui, |ui| {
        ui.strong("Also include");
        ui.add_space(theme::SPACE_XS);
        theme::action_row(ui, |ui| {
            for (included, label, help) in [
                (
                    &mut selection.camera_profile,
                    "Camera profile",
                    "The DCP profile chosen for this photo. Photos from other camera models keep their automatic profile.",
                ),
                (
                    &mut selection.masks,
                    "Masks",
                    "Fullscreen, linear and radial masks with their adjustments, and global effects such as fog. They are added next to each photo's own masks.",
                ),
                (
                    &mut selection.ai_masks,
                    "AI masks",
                    "Subject, background, sky, depth and range masks. They are regenerated for each photo the preset is applied to.",
                ),
                (
                    &mut selection.geometry,
                    "Crop & geometry",
                    "Crop, rotation, straighten, perspective and flips.",
                ),
                (
                    &mut selection.lens_correction,
                    "Lens correction",
                    "Lens correction state and the selected lens profile.",
                ),
            ] {
                if theme::toggle_button(ui, label, *included)
                    .on_hover_text(help)
                    .clicked()
                {
                    *included = !*included;
                }
            }
        });
    });
}

fn show_delete_confirmation(ctx: &egui::Context, app: &mut CalibRawApp) {
    let Some(path) = app.presets.pending_delete.clone() else {
        return;
    };
    let Some(name) = app.preset_at(&path).map(|preset| preset.name().to_owned()) else {
        app.presets.pending_delete = None;
        return;
    };
    let mut choice = crate::ui::theme::DialogAction::None;
    crate::ui::theme::dialog_window(
        "Delete preset?",
        ctx,
        crate::ui::theme::DIALOG_WIDTH_DEFAULT,
    )
    .id(egui::Id::new("preset-delete-confirmation"))
    .show(ctx, |ui| {
        ui.label(format!(
            "“{name}” will be deleted. Photos it was applied to keep their adjustments."
        ));
        choice = crate::ui::theme::dialog_confirmation_buttons(
            ui,
            "Cancel",
            "Delete",
            true,
            true,
            crate::ui::theme::DialogKeyboard::CLOSE_ONLY,
        );
    });
    match choice {
        crate::ui::theme::DialogAction::Cancel => app.presets.pending_delete = None,
        crate::ui::theme::DialogAction::Confirm => {
            app.presets.pending_delete = None;
            app.delete_preset(&path);
        }
        crate::ui::theme::DialogAction::None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_dialogs_keep_their_size_across_frames() {
        let folder = std::env::temp_dir().join(format!(
            "calibraw-preset-dialog-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
        ));
        let selection = EditSelection {
            adjustment_groups: [AdjustmentGroup::Light].into_iter().collect(),
            ..EditSelection::default()
        };
        let preset = crate::presets::Preset::new(
            "Warm matte",
            "Film",
            selection,
            &crate::sidecar::default_edit_state(),
        )
        .unwrap();
        let path = crate::presets::save_new_preset(&folder, &preset).unwrap();

        let ctx = egui::Context::default();
        crate::ui::theme::install(&ctx);
        let mut app = CalibRawApp::empty(&ctx);
        app.ui.onboarding_step = None;
        app.presets = crate::app::PresetState::load(Some(folder.clone()));
        app.open_rename_preset_editor(&path);

        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1600.0, 1000.0),
            )),
            ..Default::default()
        };
        let mut sizes = Vec::new();
        for _ in 0..12 {
            let _ = ctx.run_ui(input(), |root| show_dialogs(root.ctx(), &mut app));
            let size = ctx
                .memory(|memory| memory.area_rect(egui::Id::new("preset-editor-dialog")))
                .expect("the rename dialog is open")
                .size();
            sizes.push(size);
        }
        let settled = sizes[2];
        assert!(
            sizes[2..]
                .iter()
                .all(|size| (*size - settled).length() < 0.5),
            "dialog size changed between frames: {sizes:?}"
        );
        assert!(settled.x < 600.0, "dialog grew to {} wide", settled.x);
        assert!(settled.y < 400.0, "dialog grew to {} high", settled.y);

        // "New group" opens on top of the editor and keeps its size too.
        app.open_group_dialog(GroupDialogMode::Create);
        let mut sizes = Vec::new();
        for _ in 0..12 {
            let _ = ctx.run_ui(input(), |root| show_dialogs(root.ctx(), &mut app));
            sizes.push(
                ctx.memory(|memory| memory.area_rect(egui::Id::new("preset-group-dialog")))
                    .expect("the new group dialog is open")
                    .size(),
            );
        }
        let settled = sizes[2];
        assert!(
            sizes[2..]
                .iter()
                .all(|size| (*size - settled).length() < 0.5),
            "new group dialog size changed between frames: {sizes:?}"
        );
        // Both dialogs are centered, so whatever is hit at the new group
        // dialog's center is the dialog drawn on top.
        let center = ctx
            .memory(|memory| memory.area_rect(egui::Id::new("preset-group-dialog")))
            .unwrap()
            .center();
        let on_top = ctx.layer_id_at(center).map(|layer| layer.id)
            == Some(egui::Id::new("preset-group-dialog"));
        assert!(
            on_top,
            "the new group dialog must be drawn above the editor"
        );

        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn selection_summaries_list_groups_then_other_categories() {
        let selection = EditSelection {
            adjustment_groups: [AdjustmentGroup::Light, AdjustmentGroup::Effects]
                .into_iter()
                .collect(),
            masks: true,
            camera_profile: true,
            ..EditSelection::default()
        };
        assert_eq!(
            selection_summary(selection),
            "Light, Effects · Camera profile, Masks"
        );
        let masks_only = EditSelection {
            ai_masks: true,
            ..EditSelection::default()
        };
        assert_eq!(selection_summary(masks_only), "AI masks");
    }
}
