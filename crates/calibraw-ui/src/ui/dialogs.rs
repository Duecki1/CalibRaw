use eframe::egui::{self, InnerResponse, Response, Ui};

use super::theme::{CONTROL_HEIGHT, SPACE_SM, SPACE_XS};
use super::widgets::buttons::{destructive_button, primary_action_button, secondary_button};

pub(crate) const DIALOG_WIDTH_NARROW: f32 = 360.0;
pub(crate) const DIALOG_WIDTH_FORM: f32 = 420.0;
pub(crate) const DIALOG_WIDTH_DEFAULT: f32 = 440.0;
pub(crate) const DIALOG_WIDTH_WIDE: f32 = 480.0;
pub(crate) const DIALOG_WIDTH_LARGE: f32 = 520.0;
pub(crate) const DIALOG_TEXT_FIELD_WIDTH: f32 = 320.0;
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
    ui.add_space(SPACE_XS);
    ui.separator();
    ui.add_space(SPACE_XS);
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width().max(1.0), CONTROL_HEIGHT),
        egui::Layout::right_to_left(egui::Align::Center).with_main_wrap(true),
        |ui| {
            ui.spacing_mut().item_spacing.x = SPACE_SM;
            add_contents(ui)
        },
    )
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
        if secondary_button(ui, cancel_label).clicked() {
            action = DialogAction::Cancel;
        }
    });
    if action == DialogAction::None {
        action = dialog_keyboard_action(ui, keyboard, confirm_enabled);
    }
    action
}

pub(crate) struct DialogWindow {
    window: egui::Window<'static>,
    title: String,
    max_body_height: f32,
}

impl DialogWindow {
    pub(crate) fn id(mut self, id: egui::Id) -> Self {
        self.window = self.window.id(id);
        self
    }

    pub(crate) fn movable(mut self, movable: bool) -> Self {
        self.window = self.window.movable(movable);
        self
    }

    pub(crate) fn resizable(mut self, resizable: bool) -> Self {
        self.window = self.window.resizable(resizable);
        self
    }

    pub(crate) fn show<R>(
        self,
        ctx: &egui::Context,
        add_contents: impl FnOnce(&mut Ui) -> R,
    ) -> Option<InnerResponse<Option<R>>> {
        let Self {
            window,
            title,
            max_body_height,
        } = self;
        window.show(ctx, |ui| {
            ui.heading(title);
            ui.add_space(SPACE_SM);
            egui::ScrollArea::vertical()
                .max_height(max_body_height)
                .auto_shrink([false, true])
                .show(ui, add_contents)
                .inner
        })
    }

    pub(crate) fn show_with_footer<R>(
        self,
        ctx: &egui::Context,
        add_body: impl FnOnce(&mut Ui) -> R,
        add_footer: impl FnOnce(&mut Ui),
    ) -> Option<InnerResponse<Option<R>>> {
        let Self {
            window,
            title,
            max_body_height,
        } = self;
        // Two consent actions wrap into separate rows on narrow screens.
        let footer_reserve = if ctx.content_rect().width() < 320.0 {
            CONTROL_HEIGHT * 2.0 + 32.0
        } else {
            CONTROL_HEIGHT + 16.0
        };
        window.show(ctx, |ui| {
            ui.heading(title);
            ui.add_space(SPACE_SM);
            let body = egui::ScrollArea::vertical()
                .max_height((max_body_height - footer_reserve).max(1.0))
                .auto_shrink([false, true])
                .show(ui, add_body)
                .inner;
            add_footer(ui);
            body
        })
    }
}

// TODO: Evaluate migrating standard confirmation/form dialogs to `egui::Modal` so
// background interaction is structurally blocked while preserving desktop/Android behavior.
pub(crate) fn dialog_window(
    title: impl Into<String>,
    ctx: &egui::Context,
    preferred_width: f32,
) -> DialogWindow {
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
    let title = title.into();
    let window = egui::Window::new(title.clone())
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .default_width(preferred_width.min(available.x))
        .max_width(available.x)
        .max_height(available.y)
        .vscroll(false);
    #[cfg(target_os = "android")]
    let window = window.order(egui::Order::Foreground);
    // Leave room for the in-body title and window margins. The body grows with
    // short content and becomes scrollable only when it reaches this limit.
    DialogWindow {
        window,
        title,
        max_body_height: (available.y - f32::from(DIALOG_MARGIN) * 2.0 - 48.0).max(32.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dialog_window_shrinks_to_short_content() {
        let ctx = egui::Context::default();
        // Reuse the id of the old title-bar dialog, which started at 420 px high.
        let _ = ctx.run_ui(egui::RawInput::default(), |root| {
            egui::Window::new("Compact dialog")
                .default_size([DIALOG_WIDTH_FORM, 420.0])
                .resizable(false)
                .show(root.ctx(), |ui| {
                    ui.label("A short message");
                });
        });
        for _ in 0..3 {
            let mut height = None;
            let _ = ctx.run_ui(egui::RawInput::default(), |root| {
                height = dialog_window("Compact dialog", root.ctx(), DIALOG_WIDTH_FORM)
                    .show(root.ctx(), |ui| {
                        ui.label("A short message");
                        dialog_button_row(ui, |ui| {
                            secondary_button(ui, "Cancel");
                        });
                    })
                    .map(|response| response.response.rect.height());
            });
            assert!(
                height.is_some_and(|height| height < 200.0),
                "height: {height:?}"
            );
        }
    }

    #[test]
    fn long_dialog_keeps_actions_inside_window() {
        let ctx = egui::Context::default();
        let mut geometry = None;
        for _ in 0..3 {
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900.0, 700.0),
                    )),
                    ..Default::default()
                },
                |root| {
                    let dialog = dialog_window(
                        "Download subject-selection model?",
                        root.ctx(),
                        DIALOG_WIDTH_LARGE,
                    )
                    .show(root.ctx(), |ui| {
                        for _ in 0..7 {
                            ui.label("A long model description that wraps across the dialog width and explains the download and local processing details.");
                        }
                        dialog_button_row(ui, |ui| {
                            primary_action_button(ui, "Download and continue")
                        })
                        .inner
                        .rect
                    });
                    geometry = dialog.and_then(|result| {
                        result.inner.map(|button| (result.response.rect, button))
                    });
                },
            );
        }
        let (window, button) = geometry.expect("dialog and action should render");
        assert!(window.height() > 200.0, "window {window:?}");
        assert!(
            window.contains_rect(button),
            "window {window:?}, button {button:?}"
        );
    }

    #[test]
    fn long_dialog_can_reach_actions_in_short_landscape_window() {
        let ctx = egui::Context::default();
        let mut geometry = None;
        for frame in 0..5 {
            let events = if frame == 0 {
                Vec::new()
            } else {
                vec![
                    egui::Event::PointerMoved(egui::pos2(350.0, 100.0)),
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(0.0, -180.0),
                        phase: egui::TouchPhase::Move,
                        modifiers: egui::Modifiers::NONE,
                    },
                ]
            };
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(720.0, 166.0),
                    )),
                    events,
                    ..Default::default()
                },
                |root| {
                    let dialog = dialog_window(
                        "Download subject-selection model?",
                        root.ctx(),
                        DIALOG_WIDTH_LARGE,
                    )
                    .show(root.ctx(), |ui| {
                        for _ in 0..7 {
                            ui.label("A long model description that wraps across the dialog width and explains the download and local processing details.");
                        }
                        dialog_button_row(ui, |ui| {
                            primary_action_button(ui, "Download and continue")
                        })
                        .inner
                        .rect
                    });
                    geometry = dialog.and_then(|result| {
                        result.inner.map(|button| (result.response.rect, button))
                    });
                },
            );
        }
        let (window, button) = geometry.expect("dialog and action should render");
        assert!(
            window.contains_rect(button),
            "window {window:?}, button {button:?}"
        );
    }

    #[test]
    fn consent_footer_stays_visible_when_details_scroll() {
        let ctx = egui::Context::default();
        for (width, height) in [(720.0, 700.0), (720.0, 260.0), (180.0, 260.0)] {
            let mut geometry = None;
            for _ in 0..3 {
                let _ = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, height),
                        )),
                        ..Default::default()
                    },
                    |root| {
                        let mut footer_rect = None;
                        let dialog = dialog_window(
                            "Download subject-selection model?",
                            root.ctx(),
                            DIALOG_WIDTH_LARGE,
                        )
                        .show_with_footer(
                            root.ctx(),
                            |ui| {
                                for _ in 0..12 {
                                    ui.label("Expanded model and privacy details that need to scroll in a short window.");
                                }
                            },
                            |ui| {
                                footer_rect = Some(
                                    dialog_button_row(ui, |ui| {
                                        primary_action_button(ui, "Download and continue");
                                        secondary_button(ui, "Cancel");
                                    })
                                    .response
                                    .rect,
                                );
                            },
                        );
                        geometry = dialog.and_then(|result| {
                            footer_rect.map(|footer| (result.response.rect, footer))
                        });
                    },
                );
            }
            let (window, footer) = geometry.expect("dialog footer should render");
            assert!(
                window.contains_rect(footer),
                "viewport {width}x{height}, window {window:?}, footer {footer:?}"
            );
            let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, height));
            assert!(
                viewport.contains_rect(footer),
                "viewport {width}x{height}, footer {footer:?}"
            );
        }
    }

    #[test]
    fn dialog_actions_wrap_within_narrow_width() {
        let ctx = egui::Context::default();
        let mut footer_height = None;
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(180.0, 500.0),
                )),
                ..Default::default()
            },
            |root| {
                dialog_window("Narrow dialog", root.ctx(), DIALOG_WIDTH_DEFAULT).show(
                    root.ctx(),
                    |ui| {
                        footer_height = Some(
                            dialog_button_row(ui, |ui| {
                                primary_action_button(ui, "Replace");
                                secondary_button(ui, "Merge");
                                secondary_button(ui, "Cancel");
                            })
                            .response
                            .rect
                            .height(),
                        );
                    },
                );
            },
        );
        assert!(footer_height.is_some_and(|height| height > CONTROL_HEIGHT));
    }

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
                    dialog_window("Parent", root.ctx(), DIALOG_WIDTH_FORM).show(root.ctx(), |ui| {
                        if frame == 1 {
                            egui::Popup::open_id(ui.ctx(), popup_id);
                        }
                        let anchor = ui.button("Options");
                        popup_rendered |=
                            egui::Popup::new(popup_id, ui.ctx().clone(), &anchor, ui.layer_id())
                                .open_memory(None)
                                .show(|ui| {
                                    ui.label("Choice");
                                })
                                .is_some();
                        action = dialog_keyboard_action(ui, DialogKeyboard::CLOSE_ONLY, true);
                    });
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
