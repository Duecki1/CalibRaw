//! The Presets sidebar panel, its dialogs and the "Apply preset" menus.
use crate::app::{CalibRawApp, PresetEditorMode};
use crate::pipeline::AdjustmentGroup;
use crate::sidecar::EditSelection;
use eframe::egui::{self, Ui};
use std::path::PathBuf;

#[cfg(not(target_os = "android"))]
const ROW_HELP: &str = "Previewing on the photo. Click to apply.";
#[cfg(target_os = "android")]
const ROW_HELP: &str = "Tap to apply to this photo.";

const PANEL_HELP: &str = "Click a preset to apply it to this photo. A preset changes only the settings it includes, and adds its masks next to the photo's own.";

#[derive(Clone, Debug)]
enum PresetAction {
    Apply(PathBuf),
    Rename(PathBuf),
    #[cfg(not(target_os = "android"))]
    Export(PathBuf),
    Delete(PathBuf),
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
    pub(crate) fn from_app(app: &CalibRawApp) -> Self {
        let mut groups: Vec<PresetListGroup> = Vec::new();
        for stored in app.presets.all() {
            let entry = PresetListEntry {
                path: stored.path.clone(),
                name: stored.preset.name().to_owned(),
                summary: selection_summary(stored.preset.selection()),
            };
            match groups.last_mut() {
                Some(group) if group.name == stored.preset.group() => group.presets.push(entry),
                _ => groups.push(PresetListGroup {
                    name: stored.preset.group().to_owned(),
                    presets: vec![entry],
                }),
            }
        }
        Self { groups }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.groups.is_empty()
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
        match self.groups.as_slice() {
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

pub(crate) fn show_panel(ui: &mut Ui, app: &mut CalibRawApp, frame: &eframe::Frame) {
    let list = PresetList::from_app(app);
    let mut action = None;

    crate::ui::theme::content_card(ui, |ui| {
        crate::ui::theme::heading_with_help(ui, "Presets", PANEL_HELP);
        crate::ui::theme::action_row(ui, |ui| {
            let create_enabled = app.can_create_preset();
            if ui
                .add_enabled_ui(create_enabled, |ui| {
                    crate::ui::theme::primary_action_button(
                        ui,
                        format!("{}  Create preset…", egui_phosphor::regular::PLUS),
                    )
                })
                .inner
                .on_hover_text("Save settings of this photo as a new preset")
                .clicked()
            {
                app.open_new_preset_editor();
            }
            #[cfg(not(target_os = "android"))]
            if crate::ui::theme::secondary_button_enabled(
                ui,
                app.presets.is_available(),
                format!("{}  Import…", egui_phosphor::regular::DOWNLOAD_SIMPLE),
            )
            .on_hover_text("Add .calibraw-preset files")
            .clicked()
            {
                app.choose_preset_files_to_import();
            }
        });
        if !app.presets.is_available() {
            ui.label(
                egui::RichText::new(
                    "Presets are unavailable because CalibRaw has no settings folder.",
                )
                .color(ui.visuals().warn_fg_color),
            );
        }
        if !app.presets.load_failures.is_empty() {
            let count = app.presets.load_failures.len();
            ui.label(
                egui::RichText::new(format!(
                    "{count} preset {} could not be read.",
                    if count == 1 { "file" } else { "files" }
                ))
                .small()
                .color(ui.visuals().warn_fg_color),
            )
            .on_hover_text(app.presets.load_failures.join("\n"));
        }
        if list.is_empty() {
            ui.label(
                egui::RichText::new(
                    "No presets yet. Edit a photo, then choose Create preset to reuse its look on other photos.",
                )
                .weak(),
            );
        }
    });

    let mut hovered = None;
    for group in &list.groups {
        crate::ui::theme::card_gap(ui);
        crate::ui::theme::content_card(ui, |ui| {
            egui::CollapsingHeader::new(egui::RichText::new(&group.name).strong())
                .id_salt(("preset-group", &group.name))
                .default_open(true)
                .show(ui, |ui| {
                    for entry in &group.presets {
                        if show_preset_row(ui, entry, &mut action) {
                            hovered = Some(entry.path.clone());
                        }
                    }
                });
        });
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
        None => {}
    }
}

/// Draws one preset row. Returns whether the pointer rests on its name.
fn show_preset_row(
    ui: &mut Ui,
    entry: &PresetListEntry,
    action: &mut Option<PresetAction>,
) -> bool {
    ui.horizontal(|ui| {
        let menu_width = crate::ui::theme::CONTROL_HEIGHT;
        let row_width = (ui.available_width() - menu_width - ui.spacing().item_spacing.x).max(1.0);
        let response = ui
            .allocate_ui(
                egui::vec2(row_width, crate::ui::theme::CONTROL_HEIGHT),
                |ui| crate::ui::theme::navigation_row(ui, &entry.name, false, egui::Sense::click()),
            )
            .inner
            .on_hover_text(format!("{ROW_HELP}\n{}", entry.summary));
        if response.clicked() {
            *action = Some(PresetAction::Apply(entry.path.clone()));
        }
        crate::ui::theme::context_menu(&response, |ui| preset_actions_menu(ui, entry, action));
        ui.menu_button(egui_phosphor::regular::DOTS_THREE_VERTICAL, |ui| {
            preset_actions_menu(ui, entry, action);
        })
        .response
        .on_hover_text("Preset actions");
        // An open context menu means the pointer is choosing an action, not
        // looking at the preset.
        response.hovered() && !egui::Popup::is_any_open(ui.ctx())
    })
    .inner
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
    show_delete_confirmation(ctx, app);
}

fn show_editor(ctx: &egui::Context, app: &mut CalibRawApp) {
    let groups: Vec<String> = app
        .presets
        .groups()
        .into_iter()
        .map(str::to_owned)
        .collect();
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
    let mut choice = crate::ui::theme::DialogAction::None;
    crate::ui::theme::dialog_window(
        if creating {
            "New preset"
        } else {
            "Rename preset"
        },
        ctx,
        crate::ui::theme::DIALOG_WIDTH_FORM,
    )
    .id(egui::Id::new("preset-editor-dialog"))
    .show(ctx, |ui| {
        ui.label("Name");
        let response = ui.add(
            crate::ui::theme::singleline_text_edit(&mut editor.name)
                .desired_width(crate::ui::theme::DIALOG_TEXT_FIELD_WIDTH)
                .hint_text("e.g. Warm matte")
                .id_source("preset-editor-name"),
        );
        crate::ui::theme::request_initial_focus(&response, &mut editor.focus_requested);

        ui.label("Group");
        ui.horizontal(|ui| {
            ui.add(
                crate::ui::theme::singleline_text_edit(&mut editor.group)
                    .desired_width(
                        crate::ui::theme::DIALOG_TEXT_FIELD_WIDTH
                            - crate::ui::theme::CONTROL_HEIGHT
                            - ui.spacing().item_spacing.x,
                    )
                    .hint_text(crate::presets::DEFAULT_PRESET_GROUP)
                    .id_source("preset-editor-group"),
            );
            ui.add_enabled_ui(!groups.is_empty(), |ui| {
                ui.menu_button(egui_phosphor::regular::CARET_DOWN, |ui| {
                    for group in &groups {
                        if crate::ui::theme::menu_item(ui, true, group).clicked() {
                            editor.group.clone_from(group);
                            ui.close();
                        }
                    }
                })
                .response
                .on_hover_text("Choose an existing group");
            });
        });

        if let PresetEditorMode::Create { edits } = &editor.mode {
            ui.add_space(crate::ui::theme::SPACE_SM);
            show_selection_controls(ui, &mut editor.selection, edits);
        }

        if replaces_existing {
            ui.label(
                egui::RichText::new(
                    "A preset with this name exists in this group and will be replaced.",
                )
                .small()
                .color(ui.visuals().warn_fg_color),
            );
        }
        if let Some(error) = &editor.error {
            ui.label(
                egui::RichText::new(error)
                    .small()
                    .color(ui.visuals().error_fg_color),
            );
        }

        let confirm_enabled =
            !editor.name.trim().is_empty() && (!creating || !editor.selection.is_empty());
        let confirm_label = match (creating, replaces_existing) {
            (true, true) => "Replace",
            (true, false) => "Save",
            (false, _) => "Rename",
        };
        choice = crate::ui::theme::dialog_confirmation_buttons(
            ui,
            "Cancel",
            confirm_label,
            confirm_enabled,
            false,
            crate::ui::theme::DialogKeyboard::CONFIRM_ON_ENTER,
        );
    });

    match choice {
        crate::ui::theme::DialogAction::Cancel => app.presets.editor = None,
        crate::ui::theme::DialogAction::Confirm => app.confirm_preset_editor(),
        crate::ui::theme::DialogAction::None => {}
    }
}

fn show_selection_controls(
    ui: &mut Ui,
    selection: &mut EditSelection,
    edits: &crate::sidecar::EditState,
) {
    ui.horizontal(|ui| {
        crate::ui::theme::strong_with_help(
            ui,
            "Adjustments",
            "Each group matches a card in the Edit tab. Unchecked groups keep the destination photo's settings.",
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("None").clicked() {
                selection.adjustment_groups = crate::pipeline::AdjustmentGroupSet::EMPTY;
            }
            if ui.small_button("All").clicked() {
                selection.adjustment_groups = crate::pipeline::AdjustmentGroupSet::ALL;
            }
        });
    });
    for group in AdjustmentGroup::ALL {
        let mut included = selection.adjustment_groups.contains(group);
        let edited = edits.exposure.group_is_edited(group);
        let label = if edited {
            group.label().to_owned()
        } else {
            format!("{} (unchanged)", group.label())
        };
        if ui.checkbox(&mut included, label).changed() {
            selection.adjustment_groups.set(group, included);
        }
    }

    ui.add_space(crate::ui::theme::SPACE_XS);
    ui.strong("Also include");
    crate::ui::theme::checkbox_with_help(
        ui,
        &mut selection.camera_profile,
        "Camera profile",
        "The DCP profile chosen for this photo. Photos from other camera models keep their automatic profile.",
    );
    crate::ui::theme::checkbox_with_help(
        ui,
        &mut selection.masks,
        "Masks",
        "Fullscreen, linear and radial masks with their adjustments, and global effects such as fog. They are added next to each photo's own masks.",
    );
    crate::ui::theme::checkbox_with_help(
        ui,
        &mut selection.ai_masks,
        "AI masks",
        "Subject, background, sky, depth and range masks. They are regenerated for each photo the preset is applied to.",
    );
    crate::ui::theme::checkbox_with_help(
        ui,
        &mut selection.geometry,
        "Crop & geometry",
        "Crop, rotation, straighten, perspective and flips.",
    );
    crate::ui::theme::checkbox_with_help(
        ui,
        &mut selection.lens_correction,
        "Lens correction",
        "Lens correction state and the selected lens profile.",
    );

    let has_photo_specific_masks = edits
        .masks
        .masks
        .iter()
        .flat_map(|mask| &mask.components)
        .any(|component| !crate::presets::is_portable_mask_kind(component.kind));
    if has_photo_specific_masks && (selection.masks || selection.ai_masks) {
        ui.label(
            egui::RichText::new(
                "Brush, path and object masks follow this photo's content and are not saved in presets.",
            )
            .small()
            .weak(),
        );
    }
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
