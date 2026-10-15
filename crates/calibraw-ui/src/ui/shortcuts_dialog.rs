//! The keyboard and mouse controls reference opened from the toolbar.
//!
//! The list is documentation of controls implemented elsewhere (see the
//! `handle_*_shortcuts` functions, the preview viewport and the tool
//! modules); change both when a control changes. Controls are grouped by
//! what the user is doing, most general first.

use crate::app::CalibRawApp;
use eframe::egui;

/// Primary modifier in the platform's own spelling (egui's `COMMAND`).
const COMMAND: &str = if cfg!(target_os = "macos") {
    "Cmd"
} else {
    "Ctrl"
};

/// One control. Each entry of `inputs` is an alternative way to do it.
struct Control {
    action: &'static str,
    inputs: Vec<String>,
}

struct Group {
    title: &'static str,
    controls: Vec<Control>,
}

fn control(action: &'static str, inputs: &[&str]) -> Control {
    Control {
        action,
        inputs: inputs.iter().map(|input| (*input).to_owned()).collect(),
    }
}

/// `COMMAND` plus `rest`, e.g. `Ctrl+Z`.
fn with_command(rest: &str) -> String {
    format!("{COMMAND}+{rest}")
}

fn groups() -> Vec<Group> {
    vec![
        Group {
            title: "General",
            controls: vec![
                control("Undo", &[&with_command("Z")]),
                control("Redo", &[&with_command("Shift+Z"), &with_command("Y")]),
                control("Save edits now", &[&with_command("S")]),
                control("Previous / next photo in Develop", &["←", "→"]),
            ],
        },
        Group {
            title: "Zoom and pan",
            controls: vec![
                control("Zoom in", &[&with_command("+"), "Scroll up", "Pinch out"]),
                control("Zoom out", &[&with_command("-"), "Scroll down", "Pinch in"]),
                control("Fit photo to view", &[&with_command("0")]),
                control("Zoom to 100%", &[&with_command("1")]),
                control("Switch between fit and 100%", &["Double-click photo"]),
                control("Pan", &["Drag", "Middle-drag", "Space+Drag"]),
            ],
        },
        Group {
            title: "Library",
            controls: vec![
                control("Search filenames", &[&with_command("F")]),
                control("Select all search matches", &["Enter in search"]),
                control("Clear search", &["Esc in search"]),
                control(
                    "Add or remove a photo from the selection",
                    &[&with_command("Click")],
                ),
                control("Select a range", &["Shift+Click"]),
                control(
                    "Extend the selection by a range",
                    &[&with_command("Shift+Click")],
                ),
                control("Photo menu (select, select all)", &["Right-click"]),
            ],
        },
        Group {
            title: "Masks and tools",
            controls: vec![
                control("Bend a path mask while drawing", &["Alt+Drag"]),
                control(
                    "Set the retouch source",
                    &[&with_command("Click"), "Right-click"],
                ),
                control("Snap a band angle to steps", &["Shift+Drag knob"]),
                control("Nudge a focused handle", &["←", "→", "↑", "↓"]),
                control("Nudge a focused handle finely", &["Shift+Arrow"]),
                control(
                    "Change a light's depth",
                    &["Scroll over handle", "Page Up", "Page Down"],
                ),
                control("Reset a handle", &["Double-click"]),
                control("Cancel the point-colour picker", &["Esc"]),
            ],
        },
        Group {
            title: "Sliders, curves and pads",
            controls: vec![
                control("Reset a slider, dial or wheel", &["Double-click"]),
                control("Step a slider or range handle", &["←", "→"]),
                control("Step range handles in larger steps", &["Shift+Arrow"]),
                control("Switch range handle", &["↑", "↓"]),
                control("Move a 2D pad marker", &["←", "→", "↑", "↓"]),
                control("Move a 2D pad marker in larger steps", &["Shift+Arrow"]),
                control("Add a tone-curve point", &["Click"]),
                control("Remove a tone-curve point", &["Right-click point"]),
                control("Reset the reference split", &["Double-click divider"]),
            ],
        },
    ]
}

/// The solid scrollbar the sidebars use, which keeps a gap between the
/// content and the bar.
fn sidebar_scroll_style() -> egui::style::ScrollStyle {
    let mut style = egui::style::ScrollStyle::solid();
    style.bar_width = 7.0;
    style.bar_inner_margin = 7.0;
    style
}

/// Draws one input as a key cap.
fn key_cap(ui: &mut egui::Ui, text: &str) {
    let visuals = ui.visuals();
    egui::Frame::new()
        .fill(visuals.widgets.inactive.weak_bg_fill)
        .stroke(visuals.widgets.noninteractive.bg_stroke)
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).monospace());
        });
}

fn show_group(ui: &mut egui::Ui, group: &Group) {
    ui.add_space(moduwu_design::SPACE_SM);
    ui.strong(group.title);
    ui.add_space(moduwu_design::SPACE_XXS);
    ui.separator();
    for control in &group.controls {
        ui.horizontal_wrapped(|ui| {
            ui.label(control.action);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Right-to-left reverses the order, so add alternatives backwards.
                for (index, input) in control.inputs.iter().enumerate().rev() {
                    key_cap(ui, input);
                    if index > 0 {
                        ui.label("or");
                    }
                }
            });
        });
    }
}

/// Shows the dialog while `app.ui.shortcuts_dialog_open` is set.
pub(crate) fn show(ctx: &egui::Context, app: &mut CalibRawApp) {
    if !app.ui.shortcuts_dialog_open {
        return;
    }
    let mut close = false;
    // The dialog's scroll area reads the style of the window it is created in,
    // so the sidebar scrollbar style is applied around the call.
    let page_scroll = ctx.global_style().spacing.scroll;
    ctx.global_style_mut(|style| style.spacing.scroll = sidebar_scroll_style());
    moduwu_design::dialog_window("Keyboard shortcuts", ctx, moduwu_design::DIALOG_WIDTH_LARGE)
        .show_with_footer(
            ctx,
            |ui| {
                for group in groups() {
                    show_group(ui, &group);
                }
            },
            |ui| {
                moduwu_design::dialog_button_row(ui, |ui| {
                    if moduwu_design::primary_action_button(ui, "Close").clicked() {
                        close = true;
                    }
                });
                if !close
                    && moduwu_design::dialog_keyboard_action(
                        ui,
                        moduwu_design::DialogKeyboard::CLOSE_ONLY,
                        false,
                    ) == moduwu_design::DialogAction::Cancel
                {
                    close = true;
                }
            },
        );
    ctx.global_style_mut(|style| style.spacing.scroll = page_scroll);
    if close {
        app.ui.shortcuts_dialog_open = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_control_has_an_action_and_an_input() {
        for group in groups() {
            assert!(!group.controls.is_empty(), "{} is empty", group.title);
            for control in group.controls {
                assert!(!control.action.is_empty());
                assert!(
                    control.inputs.iter().all(|input| !input.is_empty())
                        && !control.inputs.is_empty(),
                    "{} lacks an input",
                    control.action
                );
            }
        }
    }
}
