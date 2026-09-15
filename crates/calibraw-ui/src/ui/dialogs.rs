use eframe::egui::{self, InnerResponse, Response, Ui};

use super::theme::SPACE_SM;
use super::widgets::buttons::{
    destructive_button, primary_action_button, secondary_button,
};

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

fn dialog_confirmation_keyboard(
    keyboard: DialogKeyboard,
    destructive: bool,
) -> DialogKeyboard {
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
                    destructive_button(ui, confirm_label.clone())
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
    let available = ctx.content_rect().size()
        - egui::vec2(DIALOG_VIEWPORT_PADDING, DIALOG_VIEWPORT_PADDING);
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
