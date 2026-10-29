//! The keyboard and mouse controls reference opened from the toolbar.
//!
//! The list is documentation of controls implemented elsewhere (see the
//! `handle_*_shortcuts` functions, the library's `keys` and `review`
//! modules, the preview viewport and the tool modules); change both when a
//! control changes. Controls are grouped by
//! what the user is doing, most general first.

use crate::app::CalibRawApp;
use crate::i18n::tr;
use eframe::egui;

/// Primary modifier in the platform's own spelling (egui's `COMMAND`).
const COMMAND: &str = crate::platform::PRIMARY_MODIFIER_NAME;

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
            title: tr("General"),
            controls: vec![
                control(tr("Undo"), &[&with_command("Z")]),
                control(tr("Redo"), &[&with_command("Shift+Z"), &with_command("Y")]),
                control(tr("Save edits now"), &[&with_command("S")]),
                control(tr("Previous / next photo in Develop"), &["←", "→"]),
            ],
        },
        Group {
            title: tr("Zoom and pan"),
            controls: vec![
                control(
                    tr("Zoom in"),
                    &[&with_command("+"), tr("Scroll up"), tr("Pinch out")],
                ),
                control(
                    tr("Zoom out"),
                    &[&with_command("-"), tr("Scroll down"), tr("Pinch in")],
                ),
                control(tr("Fit photo to view"), &[&with_command("0")]),
                control(tr("Zoom to 100%"), &[&with_command("1")]),
                control(
                    tr("Switch between fit and 100%"),
                    &[tr("Double-click photo")],
                ),
                control(
                    tr("Pan"),
                    &[tr("Drag"), tr("Middle-drag"), tr("Space+Drag")],
                ),
            ],
        },
        Group {
            title: tr("Library"),
            controls: vec![
                control(tr("Open a photo"), &[tr("Click"), "Enter"]),
                control(tr("Move between photos"), &["←", "→", "↑", "↓"]),
                control(
                    tr("Jump a screen, or to the first or last photo"),
                    &["Page Up", "Page Down", "Home", "End"],
                ),
                control(
                    tr("Add or remove a photo from the selection"),
                    &[&with_command(tr("Click")), tr("Click its check circle")],
                ),
                control(tr("Select a range"), &[tr("Shift+Click"), "Shift+Arrow"]),
                control(tr("Add or remove the active photo"), &["Space"]),
                control(tr("Select all"), &[&with_command("A")]),
                control(tr("Stop selecting"), &["Esc"]),
                control(tr("Photo menu"), &[tr("Right-click")]),
                control(tr("Copy adjustments"), &[&with_command("Shift+C")]),
                control(tr("Paste adjustments"), &[&with_command("Shift+V")]),
                control(tr("Export"), &[&with_command("Shift+E")]),
                control(tr("Delete"), &["Delete", "Backspace"]),
                control(tr("Search filenames"), &[&with_command("F")]),
                control(tr("Select all search matches"), &[tr("Enter in search")]),
                control(tr("Clear search"), &[tr("Esc in search")]),
            ],
        },
        Group {
            title: tr("Rating and flags"),
            controls: vec![
                control(tr("Rate 1 to 5 stars"), &["1", "2", "3", "4", "5"]),
                control(tr("Clear the rating"), &["0"]),
                control(tr("Lower or raise the rating"), &["[", "]"]),
                control(tr("Pick, reject or unflag"), &["P", "X", "U"]),
                control(tr("Switch between picked and unflagged"), &["`"]),
                control(
                    tr("Rate or flag, then go to the next photo"),
                    &[tr("Shift+Key")],
                ),
            ],
        },
        Group {
            title: tr("Masks and tools"),
            controls: vec![
                control(tr("Bend a path mask while drawing"), &[tr("Alt+Drag")]),
                control(
                    tr("Set the retouch source"),
                    &[&with_command(tr("Click")), tr("Right-click")],
                ),
                control(tr("Snap a band angle to steps"), &[tr("Shift+Drag knob")]),
                control(tr("Nudge a focused handle"), &["←", "→", "↑", "↓"]),
                control(tr("Nudge a focused handle finely"), &["Shift+Arrow"]),
                control(
                    tr("Change a light's depth"),
                    &[tr("Scroll over handle"), "Page Up", "Page Down"],
                ),
                control(tr("Reset a handle"), &[tr("Double-click")]),
                control(tr("Cancel the point-colour picker"), &["Esc"]),
            ],
        },
        Group {
            title: tr("Sliders, curves and pads"),
            controls: vec![
                control(tr("Reset a slider, dial or wheel"), &[tr("Double-click")]),
                control(tr("Step a slider or range handle"), &["←", "→"]),
                control(tr("Step range handles in larger steps"), &["Shift+Arrow"]),
                control(tr("Switch range handle"), &["↑", "↓"]),
                control(tr("Move a 2D pad marker"), &["←", "→", "↑", "↓"]),
                control(tr("Move a 2D pad marker in larger steps"), &["Shift+Arrow"]),
                control(tr("Add a tone-curve point"), &[tr("Click")]),
                control(tr("Remove a tone-curve point"), &[tr("Right-click point")]),
                control(
                    tr("Reset the reference split"),
                    &[tr("Double-click divider")],
                ),
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
                        ui.label(tr("or"));
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
    moduwu_design::Dialog::new("keyboard-shortcuts", tr("Keyboard shortcuts"))
        .size(moduwu_design::DialogSize::Wide)
        .show_with_footer(
            ctx,
            |ui| {
                for group in groups() {
                    show_group(ui, &group);
                }
            },
            |ui| {
                if moduwu_design::primary_action_button(ui, tr("Close")).clicked() {
                    close = true;
                }
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
