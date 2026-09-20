use eframe::egui::{self, InnerResponse, Response, Ui};

use super::theme::SPACE_SM;
use super::widgets::buttons::{destructive_button, primary_action_button, secondary_button};

pub(crate) const DIALOG_WIDTH_NARROW: f32 = 360.0;
pub(crate) const DIALOG_WIDTH_FORM: f32 = 420.0;
pub(crate) const DIALOG_WIDTH_DEFAULT: f32 = 440.0;
pub(crate) const DIALOG_WIDTH_WIDE: f32 = 480.0;
pub(crate) const DIALOG_WIDTH_LARGE: f32 = 520.0;
pub(crate) const DIALOG_TEXT_FIELD_WIDTH: f32 = 320.0;
const DIALOG_COMPACT_WIDTH_BREAKPOINT: f32 = 560.0;
const DIALOG_VIEWPORT_PADDING: f32 = 24.0;
pub(crate) const DIALOG_MARGIN: i8 = if cfg!(target_os = "android") { 16 } else { 12 };

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum DialogAction {
    #[default]
    None,
    Cancel,
    Confirm,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DialogKeyboard {
    pub(crate) escape_closes: bool,
    pub(crate) enter_confirms: bool,
}

impl DialogKeyboard {
    pub(crate) const CLOSE_ONLY: Self = Self {
        escape_closes: true,
        enter_confirms: false,
    };
    pub(crate) const CONFIRM_ON_ENTER: Self = Self {
        escape_closes: true,
        enter_confirms: true,
    };
}

/// Fallback keyboard handling; call after dialog controls have processed input.
pub(crate) fn dialog_keyboard_action(
    ui: &Ui,
    keyboard: DialogKeyboard,
    confirm_enabled: bool,
) -> DialogAction {
    // A disabled dialog body must not consume application-level keys. Likewise,
    // a modal shown above this window owns keyboard input, and an open popup
    // gets first refusal for Escape so it can close without dismissing the
    // underlying dialog.
    if !ui.is_enabled()
        || !ui
            .ctx()
            .memory(|memory| memory.allows_interaction(ui.layer_id()))
    {
        return DialogAction::None;
    }
    if egui::Popup::is_any_open(ui.ctx()) {
        if keyboard.escape_closes
            && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            egui::Popup::close_all(ui.ctx());
        }
        return DialogAction::None;
    }
    if keyboard.escape_closes
        && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
    {
        return DialogAction::Cancel;
    }
    if keyboard.enter_confirms
        && confirm_enabled
        && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Enter))
    {
        return DialogAction::Confirm;
    }
    DialogAction::None
}

pub(super) fn take_initial_focus_request(focus_requested: &mut bool) -> bool {
    if *focus_requested {
        false
    } else {
        *focus_requested = true;
        true
    }
}

pub(crate) fn request_initial_focus(response: &Response, focus_requested: &mut bool) {
    if take_initial_focus_request(focus_requested) {
        response.request_focus();
    }
}

pub(crate) fn dialog_button_row<R>(
    ui: &mut Ui,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> InnerResponse<R> {
    ui.add_space(SPACE_SM);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = SPACE_SM;
        add_contents(ui)
    })
}

fn dialog_confirmation_keyboard(keyboard: DialogKeyboard, destructive: bool) -> DialogKeyboard {
    DialogKeyboard {
        enter_confirms: keyboard.enter_confirms && !destructive,
        ..keyboard
    }
}

pub(crate) fn dialog_confirmation_buttons(
    ui: &mut Ui,
    cancel_label: impl Into<egui::WidgetText>,
    confirm_label: impl Into<egui::WidgetText>,
    confirm_enabled: bool,
    destructive: bool,
    keyboard: DialogKeyboard,
) -> DialogAction {
    let keyboard = dialog_confirmation_keyboard(keyboard, destructive);
    let mut action = DialogAction::None;
    let cancel_label = cancel_label.into();
    let confirm_label = confirm_label.into();
    dialog_button_row(ui, |ui| {
        if secondary_button(ui, cancel_label).clicked() {
            action = DialogAction::Cancel;
        }
        let confirm = ui
            .add_enabled_ui(confirm_enabled, |ui| {
                if destructive {
                    destructive_button(ui, confirm_label)
                } else {
                    primary_action_button(ui, confirm_label)
                }
            })
            .inner;
        if confirm.clicked() {
            action = DialogAction::Confirm;
        }
    });
    if action == DialogAction::None {
        action = dialog_keyboard_action(ui, keyboard, confirm_enabled);
    }
    action
}

// TODO: Evaluate migrating standard confirmation/form dialogs to `egui::Modal` so
// background interaction is structurally blocked while preserving desktop/Android behavior.
pub(crate) fn dialog_window<'a>(
    window: egui::Window<'a>,
    ctx: &egui::Context,
    preferred_width: f32,
) -> egui::Window<'a> {
    // egui popups close on Escape without consuming the key. Handle an existing
    // popup before rendering the window's controls so their keyboard fallback
    // cannot also cancel the parent dialog in the same frame.
    if egui::Popup::is_any_open(ctx)
        && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
    {
        egui::Popup::close_all(ctx);
    }
    let available =
        ctx.content_rect().size() - egui::vec2(DIALOG_VIEWPORT_PADDING, DIALOG_VIEWPORT_PADDING);
    let available = egui::vec2(available.x.max(1.0), available.y.max(1.0));
    let compact_portrait =
        available.x < DIALOG_COMPACT_WIDTH_BREAKPOINT && available.y > available.x;
    let window = window
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .default_width(preferred_width.min(available.x))
        .max_width(available.x)
        .max_height(available.y)
        .vscroll(compact_portrait);
    #[cfg(target_os = "android")]
    let window = window.order(egui::Order::Foreground);
    window
}

#[cfg(test)]
mod tests {
    use super::*;

    fn escape_event() -> egui::Event {
        egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn disabled_dialog_body_does_not_consume_keyboard_actions() {
        let ctx = egui::Context::default();
        let mut action = DialogAction::None;
        let mut remains = false;
        let _ = ctx.run_ui(
            egui::RawInput {
                events: vec![escape_event()],
                ..Default::default()
            },
            |ui| {
                ui.disable();
                action = dialog_keyboard_action(ui, DialogKeyboard::CLOSE_ONLY, true);
                remains = ui
                    .input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
            },
        );
        assert_eq!(action, DialogAction::None);
        assert!(remains);
    }

    #[test]
    fn escape_closes_popup_before_parent_dialog() {
        let ctx = egui::Context::default();
        egui::Popup::open_id(&ctx, egui::Id::new("dialog-test-popup"));
        let mut action = DialogAction::None;
        let _ = ctx.run_ui(
            egui::RawInput {
                events: vec![escape_event()],
                ..Default::default()
            },
            |ui| {
                action = dialog_keyboard_action(ui, DialogKeyboard::CLOSE_ONLY, true);
            },
        );
        assert_eq!(action, DialogAction::None);
        assert!(!egui::Popup::is_any_open(&ctx));
    }

    #[test]
    fn escape_from_rendered_popup_does_not_cancel_its_window() {
        let ctx = egui::Context::default();
        let popup_id = egui::Id::new("rendered-dialog-popup");
        let mut action = DialogAction::None;
        let mut popup_rendered = false;
        for frame in 0..3 {
            let _ = ctx.run_ui(
                egui::RawInput {
                    events: if frame == 2 {
                        vec![escape_event()]
                    } else {
                        vec![]
                    },
                    ..Default::default()
                },
                |root| {
                    dialog_window(egui::Window::new("Parent"), root.ctx(), DIALOG_WIDTH_FORM).show(
                        root.ctx(),
                        |ui| {
                            if frame == 1 {
                                egui::Popup::open_id(ui.ctx(), popup_id);
                            }
                            let anchor = ui.button("Options");
                            popup_rendered |= egui::Popup::new(
                                popup_id,
                                ui.ctx().clone(),
                                &anchor,
                                ui.layer_id(),
                            )
                            .open_memory(None)
                            .show(|ui| {
                                ui.label("Choice");
                            })
                            .is_some();
                            action = dialog_keyboard_action(ui, DialogKeyboard::CLOSE_ONLY, true);
                        },
                    );
                },
            );
        }
        assert!(popup_rendered);
        assert!(!egui::Popup::is_any_open(&ctx));
        assert_eq!(action, DialogAction::None);
    }

    #[test]
    fn only_the_owning_modal_handles_dialog_keyboard_actions() {
        let ctx = egui::Context::default();
        let mut background = DialogAction::None;
        let mut modal = DialogAction::None;
        for frame in 0..3 {
            let _ = ctx.run_ui(
                egui::RawInput {
                    events: if frame == 2 {
                        vec![escape_event()]
                    } else {
                        vec![]
                    },
                    ..Default::default()
                },
                |ui| {
                    background = dialog_keyboard_action(ui, DialogKeyboard::CLOSE_ONLY, true);
                    egui::Modal::new(egui::Id::new("keyboard-owner")).show(ui.ctx(), |ui| {
                        modal = dialog_keyboard_action(ui, DialogKeyboard::CLOSE_ONLY, true);
                    });
                },
            );
        }
        assert_eq!(background, DialogAction::None);
        assert_eq!(modal, DialogAction::Cancel);
    }
}
