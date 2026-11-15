//! The Develop card of the look fitted to the camera's own JPEG preview,
//! shown while the photo has one: its amount and its removal.

use super::adjustment_cards::CardAction;
use super::*;
use crate::app::CAMERA_MATCH_CARD_TITLE;
use crate::i18n::tr;
use crate::pipeline::camera_look::CameraLook;

/// What the Camera match card asks for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum CameraMatchAction {
    /// 0–1.
    Amount(f32),
    Remove,
    /// Flip the card's switch.
    Toggle,
    /// The default amount, switched on.
    Reset,
}

impl Sidebar {
    /// The Camera match card for a look applied at `amount`, switched `on`.
    pub(super) fn show_camera_match(
        ui: &mut Ui,
        amount: f32,
        on: bool,
        foldable: bool,
    ) -> Option<CameraMatchAction> {
        let mut action = None;
        let card = Self::adjustment_card(ui, CAMERA_MATCH_CARD_TITLE, true, foldable, on, |ui| {
            let mut percent = amount * 100.0;
            if AdjustmentSlider::new(tr("Amount"), &mut percent, 0.0..=100.0)
                .decimals(0)
                .step(1.0)
                .reset_to(CameraLook::DEFAULT_AMOUNT * 100.0)
                .show(ui)
            {
                action = Some(CameraMatchAction::Amount(percent / 100.0));
            }
            if moduwu_design::full_width_button(
                ui,
                format!("{}  {}", egui_phosphor::regular::TRASH, tr("Remove")),
            )
            .clicked()
            {
                action = Some(CameraMatchAction::Remove);
            }
        });
        match card {
            CardAction::None => {}
            CardAction::Toggle => action = Some(CameraMatchAction::Toggle),
            CardAction::Reset => action = Some(CameraMatchAction::Reset),
        }
        action
    }
}
