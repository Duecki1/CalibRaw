//! The Keyboard shortcuts section of Settings: every changeable shortcut
//! with its keys, and the fixed mouse and keyboard controls.
//!
//! Clicking a key starts recording its replacement and the add button records
//! another key; the next key press is taken before anything else sees it, and
//! Escape cancels. The fixed list documents controls implemented elsewhere
//! (the library's `keys` module, the preview viewport and the tool modules);
//! change both when a control changes.

use super::Settings;
use crate::app::{
    CalibRawApp, KeyBinding, ShortcutAction, ShortcutGroup, ShortcutRecording, MAX_BINDINGS,
};
use crate::i18n::{tr, tr_fmt};
use eframe::egui::{self, Ui};

/// Primary modifier in the platform's own spelling (egui's `COMMAND`).
const COMMAND: &str = crate::platform::PRIMARY_MODIFIER_NAME;

/// A fixed control. Each entry of `inputs` is an alternative way to do it.
struct FixedControl {
    action: &'static str,
    inputs: Vec<String>,
}

struct FixedGroup {
    title: &'static str,
    controls: Vec<FixedControl>,
}

fn fixed(action: &'static str, inputs: &[&str]) -> FixedControl {
    FixedControl {
        action,
        inputs: inputs.iter().map(|input| (*input).to_owned()).collect(),
    }
}

/// `COMMAND` plus `rest`, e.g. `Ctrl+Click`.
fn with_command(rest: &str) -> String {
    format!("{COMMAND}+{rest}")
}

fn fixed_groups() -> Vec<FixedGroup> {
    vec![
        FixedGroup {
            title: tr("Zoom and pan"),
            controls: vec![
                fixed(tr("Zoom in"), &[tr("Scroll up"), tr("Pinch out")]),
                fixed(tr("Zoom out"), &[tr("Scroll down"), tr("Pinch in")]),
                fixed(
                    tr("Switch between fit and 100%"),
                    &[tr("Double-click photo")],
                ),
                fixed(
                    tr("Pan"),
                    &[tr("Drag"), tr("Middle-drag"), tr("Space+Drag")],
                ),
            ],
        },
        FixedGroup {
            title: tr("Library"),
            controls: vec![
                fixed(tr("Open a photo"), &[tr("Click"), "Enter"]),
                fixed(tr("Move between photos"), &["←", "→", "↑", "↓"]),
                fixed(
                    tr("Jump a screen, or to the first or last photo"),
                    &["Page Up", "Page Down", "Home", "End"],
                ),
                fixed(
                    tr("Add or remove a photo from the selection"),
                    &[&with_command(tr("Click")), tr("Click its check circle")],
                ),
                fixed(tr("Select a range"), &[tr("Shift+Click"), "Shift+Arrow"]),
                fixed(tr("Add or remove the active photo"), &["Space"]),
                fixed(tr("Stop selecting"), &["Esc"]),
                fixed(tr("Photo menu"), &[tr("Right-click")]),
                fixed(tr("Select all search matches"), &[tr("Enter in search")]),
                fixed(tr("Clear search"), &[tr("Esc in search")]),
                fixed(
                    tr("Rate or flag, then go to the next photo"),
                    &[tr("Shift+Key")],
                ),
            ],
        },
        FixedGroup {
            title: tr("Masks and tools"),
            controls: vec![
                fixed(tr("Bend a path mask while drawing"), &[tr("Alt+Drag")]),
                fixed(
                    tr("Set the retouch source"),
                    &[&with_command(tr("Click")), tr("Right-click")],
                ),
                fixed(tr("Snap a band angle to steps"), &[tr("Shift+Drag knob")]),
                fixed(tr("Nudge a focused handle"), &["←", "→", "↑", "↓"]),
                fixed(tr("Nudge a focused handle finely"), &["Shift+Arrow"]),
                fixed(
                    tr("Change a light's depth"),
                    &[tr("Scroll over handle"), "Page Up", "Page Down"],
                ),
                fixed(tr("Reset a handle"), &[tr("Double-click")]),
                fixed(tr("Cancel the point-colour picker"), &["Esc"]),
            ],
        },
        FixedGroup {
            title: tr("Sliders, curves and pads"),
            controls: vec![
                fixed(tr("Reset a slider, dial or wheel"), &[tr("Double-click")]),
                fixed(tr("Step a slider or range handle"), &["←", "→"]),
                fixed(tr("Step range handles in larger steps"), &["Shift+Arrow"]),
                fixed(tr("Switch range handle"), &["↑", "↓"]),
                fixed(tr("Move a 2D pad marker"), &["←", "→", "↑", "↓"]),
                fixed(tr("Move a 2D pad marker in larger steps"), &["Shift+Arrow"]),
                fixed(tr("Add a tone-curve point"), &[tr("Click")]),
                fixed(tr("Remove a tone-curve point"), &[tr("Right-click point")]),
                fixed(
                    tr("Reset the reference split"),
                    &[tr("Double-click divider")],
                ),
            ],
        },
    ]
}

/// The fill and outline of a key; `recording` while it waits for a press.
fn key_colors(ui: &Ui, recording: bool) -> (egui::Color32, egui::Stroke) {
    let visuals = ui.visuals();
    if recording {
        (
            visuals.selection.bg_fill,
            egui::Stroke::new(1.0, visuals.selection.stroke.color),
        )
    } else {
        (
            visuals.widgets.inactive.weak_bg_fill,
            visuals.widgets.noninteractive.bg_stroke,
        )
    }
}

/// Draws a fixed input as a key.
fn fixed_key(ui: &mut Ui, text: &str) {
    let (fill, stroke) = key_colors(ui, false);
    egui::Frame::new()
        .fill(fill)
        .stroke(stroke)
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).monospace());
        });
}

/// A key that can be clicked, labelled with `text`; `recording` while it
/// waits for its replacement.
fn key_button(ui: &mut Ui, text: &str, recording: bool, tooltip: &str) -> egui::Response {
    let (fill, stroke) = key_colors(ui, recording);
    ui.add(
        egui::Button::new(egui::RichText::new(text).monospace())
            .fill(fill)
            .stroke(stroke)
            .corner_radius(4.0)
            .selected(recording),
    )
    .on_hover_text(tooltip)
}

impl Settings {
    pub(super) fn show_shortcuts_section(ui: &mut Ui, app: &mut CalibRawApp) {
        for group in ShortcutGroup::ALL {
            ui.add_space(moduwu_design::SPACE_SM);
            ui.strong(group.label());
            ui.add_space(moduwu_design::SPACE_XXS);
            for action in ShortcutAction::ALL
                .into_iter()
                .filter(|action| action.group() == group)
            {
                Self::shortcut_row(ui, app, action);
            }
        }

        ui.add_space(moduwu_design::SPACE_SM);
        moduwu_design::action_row(ui, |ui| {
            if moduwu_design::secondary_button_enabled(
                ui,
                !app.preferences.shortcuts.is_default(),
                tr("Reset all shortcuts"),
            )
            .clicked()
            {
                app.reset_all_shortcuts();
            }
        });

        moduwu_design::section_separator(ui);
        egui::CollapsingHeader::new(tr("Mouse and other controls"))
            .id_salt("settings-fixed-controls")
            .default_open(false)
            .show(ui, |ui| {
                for group in fixed_groups() {
                    Self::fixed_group(ui, &group);
                }
            });
    }

    /// Takes the key press the recording shortcut waits for. Call it before
    /// any control of the page is drawn, so none sees the press. Escape
    /// cancels, and so does folding the Keyboard shortcuts section.
    pub(super) fn take_recorded_key(ui: &mut Ui, app: &mut CalibRawApp) {
        let Some(recording) = app.ui.shortcut_recording else {
            return;
        };
        if app.ui.settings_section != Some(crate::app::SettingsSection::KeyboardShortcuts) {
            app.cancel_shortcut_recording();
            return;
        }
        let Some(binding) = ui.input_mut(KeyBinding::take_press) else {
            return;
        };
        if binding.is_escape() {
            app.cancel_shortcut_recording();
        } else {
            app.record_shortcut(recording, binding);
        }
    }

    fn shortcut_row(ui: &mut Ui, app: &mut CalibRawApp, action: ShortcutAction) {
        let recording = app
            .ui
            .shortcut_recording
            .filter(|recording| recording.action == action);
        let bindings = app.preferences.shortcuts.bindings(action).to_vec();
        let changed = app.preferences.shortcuts.is_changed(action);
        let size = moduwu_design::toolbar_icon_size();
        let press_a_key = tr("Press a key…");

        // Right to left: reset sits at the row's end, the keys before it.
        moduwu_design::property_row(ui, action.label(), |ui| {
            ui.add_enabled_ui(changed, |ui| {
                if moduwu_design::ghost_icon_button(
                    ui,
                    egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE,
                    size,
                    &tr_fmt("Reset {name} to its default", &[("name", &action.label())]),
                )
                .clicked()
                {
                    app.reset_shortcut(action);
                }
            });

            let adding = recording.is_some_and(|recording| recording.replace.is_none());
            if adding {
                if key_button(ui, press_a_key, true, tr("Press Esc to cancel.")).clicked() {
                    app.cancel_shortcut_recording();
                }
            } else if bindings.len() < MAX_BINDINGS
                && moduwu_design::ghost_icon_button(
                    ui,
                    egui_phosphor::regular::PLUS,
                    size,
                    &tr_fmt("Add a shortcut for {name}", &[("name", &action.label())]),
                )
                .clicked()
            {
                app.start_shortcut_recording(ShortcutRecording {
                    action,
                    replace: None,
                });
            }

            for (index, binding) in bindings.iter().enumerate().rev() {
                if index + 1 < bindings.len() {
                    ui.add_space(moduwu_design::SPACE_XXS);
                }
                let replacing = recording.is_some_and(|recording| recording.replace == Some(index));
                if replacing {
                    if moduwu_design::ghost_icon_button(
                        ui,
                        egui_phosphor::regular::TRASH,
                        size,
                        &tr_fmt("Remove {shortcut}", &[("shortcut", &binding.label())]),
                    )
                    .clicked()
                    {
                        app.remove_shortcut(action, index);
                    }
                    if key_button(ui, press_a_key, true, tr("Press Esc to cancel.")).clicked() {
                        app.cancel_shortcut_recording();
                    }
                } else if key_button(ui, &binding.label(), false, tr("Click to change")).clicked() {
                    app.start_shortcut_recording(ShortcutRecording {
                        action,
                        replace: Some(index),
                    });
                }
            }
            if bindings.is_empty() && !adding {
                ui.label(egui::RichText::new(tr("None")).color(ui.visuals().weak_text_color()));
            }
        });

        if let Some((_, message)) = app
            .ui
            .shortcut_feedback
            .as_ref()
            .filter(|(feedback_action, _)| *feedback_action == action)
        {
            ui.colored_label(ui.visuals().warn_fg_color, message);
        }
    }

    fn fixed_group(ui: &mut Ui, group: &FixedGroup) {
        ui.add_space(moduwu_design::SPACE_SM);
        ui.strong(group.title);
        ui.add_space(moduwu_design::SPACE_XXS);
        for control in &group.controls {
            ui.horizontal_wrapped(|ui| {
                ui.label(control.action);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Right-to-left reverses the order, so add alternatives backwards.
                    for (index, input) in control.inputs.iter().enumerate().rev() {
                        fixed_key(ui, input);
                        if index > 0 {
                            ui.label(tr("or"));
                        }
                    }
                });
            });
        }
    }
}
