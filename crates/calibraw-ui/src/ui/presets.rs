//! The Presets sidebar panel, its dialogs and the "Apply preset" menus.
use crate::app::{CalibRawApp, PresetEditorMode};
use crate::pipeline::AdjustmentGroup;
use crate::sidecar::EditSelection;
use crate::ui::{icons, theme};
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

    theme::section_card_with_help(ui, "Presets", PANEL_HELP, |ui| {
        theme::action_row(ui, |ui| {
            if ui
                .add_enabled_ui(app.can_create_preset(), |ui| {
                    theme::primary_action_button(
                        ui,
                        format!("{}  Create preset…", egui_phosphor::regular::PLUS),
                    )
                })
                .inner
                .on_hover_text("Save settings of this photo as a new preset")
                .on_disabled_hover_text("Open a photo to create a preset from its settings")
                .clicked()
            {
                app.open_new_preset_editor();
            }
            #[cfg(not(target_os = "android"))]
            if theme::secondary_button_enabled(
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
            show_note(
                ui,
                "Presets are unavailable because CalibRaw has no settings folder.",
                ui.visuals().warn_fg_color,
            );
        }
        if !app.presets.load_failures.is_empty() {
            let count = app.presets.load_failures.len();
            show_note(
                ui,
                &format!(
                    "{count} preset {} could not be read.",
                    if count == 1 { "file" } else { "files" }
                ),
                ui.visuals().warn_fg_color,
            )
            .on_hover_text(app.presets.load_failures.join("\n"));
        }
        if list.is_empty() {
            show_note(
                ui,
                "No presets yet. Edit a photo, then choose Create preset to reuse its settings.",
                ui.visuals().weak_text_color(),
            );
        }
    });

    let mut hovered = None;
    for group in &list.groups {
        theme::card_gap(ui);
        theme::section_card(ui, &group.name, |ui| {
            for entry in &group.presets {
                if show_preset_row(ui, entry, &mut action) {
                    hovered = Some(entry.path.clone());
                }
            }
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

fn show_note(ui: &mut Ui, text: &str, color: egui::Color32) -> egui::Response {
    ui.add(egui::Label::new(egui::RichText::new(text).small().color(color)).wrap())
}

/// Draws one preset row. Returns whether the pointer rests on its name.
fn show_preset_row(
    ui: &mut Ui,
    entry: &PresetListEntry,
    action: &mut Option<PresetAction>,
) -> bool {
    ui.push_id(&entry.path, |ui| {
        theme::toolbar_row(ui, |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let menu = icons::phosphor_icon_button(
                    ui,
                    egui_phosphor::regular::DOTS_THREE_VERTICAL,
                    theme::toolbar_icon_size(),
                    "Preset actions",
                );
                theme::dropdown_menu(&menu, |ui| preset_actions_menu(ui, entry, action));
                let response = theme::navigation_row(ui, &entry.name, false, egui::Sense::click())
                    .on_hover_text(format!("{ROW_HELP}\n{}", entry.summary));
                if response.clicked() {
                    *action = Some(PresetAction::Apply(entry.path.clone()));
                }
                theme::context_menu(&response, |ui| preset_actions_menu(ui, entry, action));
                // An open menu means the pointer is choosing an action, not
                // looking at the preset.
                response.hovered() && !egui::Popup::is_any_open(ui.ctx())
            })
            .inner
        })
        .inner
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
            ui.label("Name");
            let response = ui.add(
                theme::dialog_text_edit(&mut editor.name, "preset-editor-name")
                    .hint_text("e.g. Warm matte"),
            );
            theme::request_initial_focus(&response, &mut editor.focus_requested);

            ui.label("Group");
            // A row of fixed height: a bare right-to-left layout would claim
            // the dialog's remaining height and grow the window.
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), theme::CONTROL_HEIGHT),
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    // The button takes its natural width first; the field fills
                    // what is left.
                    let menu = icons::phosphor_icon_button_enabled(
                        ui,
                        !groups.is_empty(),
                        egui_phosphor::regular::CARET_DOWN,
                        theme::toolbar_icon_size(),
                        "Choose an existing group",
                    );
                    theme::dropdown_menu(&menu, |ui| {
                        for group in &groups {
                            if theme::menu_item(ui, true, group).clicked() {
                                editor.group.clone_from(group);
                                ui.close();
                            }
                        }
                    });
                    ui.add(
                        theme::dialog_text_edit(&mut editor.group, "preset-editor-group")
                            .hint_text(crate::presets::DEFAULT_PRESET_GROUP),
                    );
                },
            );

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
    theme::strong_with_help(
        ui,
        "Include settings",
        "Unchecked settings keep the destination photo’s values. Selecting an unchanged group also saves its default values.",
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
            "Each group matches a card in the Edit tab. Unchecked groups keep the destination photo's settings.",
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
        for group in AdjustmentGroup::ALL {
            let mut included = selection.adjustment_groups.contains(group);
            let edited = edits.exposure.group_is_edited(group);
            let label = egui::RichText::new(group.label());
            let label = if edited { label.strong() } else { label };
            let help = if edited {
                "Edited on this photo. Include this group’s current settings."
            } else {
                "Unchanged on this photo. Including this group replaces the destination’s settings with these defaults."
            };
            if theme::checkbox_with_help(ui, &mut included, label, help).changed() {
                selection.adjustment_groups.set(group, included);
            }
        }
    });
}

fn show_additional_selection(ui: &mut Ui, selection: &mut EditSelection) {
    theme::content_card(ui, |ui| {
        ui.strong("Also include");
        ui.add_space(theme::SPACE_XS);
        theme::checkbox_with_help(
        ui,
        &mut selection.camera_profile,
        "Camera profile",
        "The DCP profile chosen for this photo. Photos from other camera models keep their automatic profile.",
    );
        theme::checkbox_with_help(
        ui,
        &mut selection.masks,
        "Masks",
        "Fullscreen, linear and radial masks with their adjustments, and global effects such as fog. They are added next to each photo's own masks.",
    );
        theme::checkbox_with_help(
        ui,
        &mut selection.ai_masks,
        "AI masks",
        "Subject, background, sky, depth and range masks. They are regenerated for each photo the preset is applied to.",
    );
        theme::section_separator(ui);
        theme::checkbox_with_help(
            ui,
            &mut selection.geometry,
            "Crop & geometry",
            "Crop, rotation, straighten, perspective and flips.",
        );
        theme::checkbox_with_help(
            ui,
            &mut selection.lens_correction,
            "Lens correction",
            "Lens correction state and the selected lens profile.",
        );
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
