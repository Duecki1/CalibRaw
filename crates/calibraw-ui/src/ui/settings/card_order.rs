//! The Interface row and dialog that order the Develop cards.

use super::Settings;
use crate::app::{CalibRawApp, DevelopCardOrder, SettingsDialog};
use crate::i18n::{tr, tr_fmt};
use eframe::egui::{self, Ui};

impl Settings {
    /// The row that opens the Develop card order dialog.
    pub(super) fn show_develop_card_order(ui: &mut Ui, app: &mut CalibRawApp) {
        moduwu_design::form_row_with_help(
            ui,
            tr("Develop card order"),
            160.0,
            tr("The order of the Edit cards in the sidebar, and of the Edit tabs on phones in portrait."),
            |ui, _| {
                if moduwu_design::secondary_button(ui, tr("Reorder…")).clicked() {
                    app.ui.settings_dialog = Some(SettingsDialog::DevelopCardOrder);
                }
            },
        );
    }

    /// The dialog that moves the Develop cards one step at a time with each
    /// card's arrows. A change applies at once.
    pub(super) fn show_develop_card_order_dialog(ctx: &egui::Context, app: &mut CalibRawApp) {
        if app.ui.settings_dialog != Some(SettingsDialog::DevelopCardOrder) {
            return;
        }
        let mut order = app.preferences.develop_card_order.clone();
        let mut close = false;
        let mut reset = false;
        let moved = moduwu_design::Dialog::new("develop-card-order", tr("Develop card order"))
            .show_with_footer(
                ctx,
                |ui| {
                    let mut moved = None;
                    let last = order.cards().len().saturating_sub(1);
                    for (index, card) in order.cards().iter().enumerate() {
                        // Right to left: the down arrow sits at the row's end.
                        moduwu_design::property_row(ui, card.label(), |ui| {
                            let size = moduwu_design::toolbar_icon_size();
                            let down = ui.add_enabled_ui(index < last, |ui| {
                                moduwu_design::ghost_icon_button(
                                    ui,
                                    egui_phosphor::regular::CARET_DOWN,
                                    size,
                                    &tr_fmt("Move {name} down", &[("name", &card.label())]),
                                )
                            });
                            let up = ui.add_enabled_ui(index > 0, |ui| {
                                moduwu_design::ghost_icon_button(
                                    ui,
                                    egui_phosphor::regular::CARET_UP,
                                    size,
                                    &tr_fmt("Move {name} up", &[("name", &card.label())]),
                                )
                            });
                            if up.inner.clicked() {
                                moved = Some((index, true));
                            } else if down.inner.clicked() {
                                moved = Some((index, false));
                            }
                        });
                    }
                    moved
                },
                |ui| {
                    close = moduwu_design::primary_action_button(ui, tr("Done")).clicked()
                        || moduwu_design::dialog_keyboard_action(
                            ui,
                            moduwu_design::DialogKeyboard::CLOSE_ONLY,
                            false,
                        ) == moduwu_design::DialogAction::Cancel;
                    reset = moduwu_design::secondary_button_enabled(
                        ui,
                        !order.is_default(),
                        tr("Reset order"),
                    )
                    .clicked();
                },
            )
            .inner;
        if reset {
            order = DevelopCardOrder::default();
        } else if let Some((index, up)) = moved {
            order.move_card(index, up);
        }
        app.set_develop_card_order(order);
        if close {
            app.ui.settings_dialog = None;
        }
    }
}
