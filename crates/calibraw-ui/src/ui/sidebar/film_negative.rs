//! The Develop card of a film negative, shown while the photo is developed as
//! one: its kind of film, its base and a fresh estimate.

use super::adjustment_cards::CardAction;
use super::*;
use crate::app::FILM_NEGATIVE_CARD_TITLE;
use crate::pipeline::{FilmNegative, FilmStock};

/// What the Film negative card asks for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum FilmNegativeAction {
    Stock(FilmStock),
    /// Starts or stops picking the film base on the photo.
    PickBase(bool),
    /// Estimates the negative anew from what the crop shows.
    Auto,
    /// Develops the photo as it is again: the card's reset.
    Remove,
}

impl Sidebar {
    /// The Film negative card for `negative`. `picking_base` is whether the
    /// film base picker is active.
    pub(super) fn show_film_negative(
        ui: &mut Ui,
        negative: &FilmNegative,
        picking_base: bool,
        foldable: bool,
    ) -> Option<FilmNegativeAction> {
        let mut action = None;
        let card =
            Self::adjustment_card(ui, FILM_NEGATIVE_CARD_TITLE, true, foldable, true, |ui| {
                let mut stock = negative.stock;
                if moduwu_design::segmented_control(
                    ui,
                    &mut stock,
                    &[
                        (FilmStock::Color, "Color"),
                        (FilmStock::BlackAndWhite, "B&W"),
                    ],
                    ui.available_width(),
                )
                .changed()
                {
                    action = Some(FilmNegativeAction::Stock(stock));
                }
                ui.horizontal(|ui| {
                    ui.label("Film base");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let size = egui::vec2(
                            moduwu_design::TOOLBAR_ICON_EDGE,
                            moduwu_design::CONTROL_HEIGHT,
                        );
                        if moduwu_design::ghost_icon_toggle_button(
                            ui,
                            egui_phosphor::regular::EYEDROPPER,
                            picking_base,
                            size,
                            "Pick unexposed film, such as the border between frames",
                        )
                        .clicked()
                        {
                            action = Some(FilmNegativeAction::PickBase(!picking_base));
                        }
                        if moduwu_design::ghost_icon_button(
                            ui,
                            egui_phosphor::regular::MAGIC_WAND,
                            size,
                            "Estimate the base from the whole scan and the colors from the crop",
                        )
                        .clicked()
                        {
                            action = Some(FilmNegativeAction::Auto);
                        }
                    });
                });
            });
        if matches!(card, CardAction::Reset) {
            action = Some(FilmNegativeAction::Remove);
        }
        action
    }
}
