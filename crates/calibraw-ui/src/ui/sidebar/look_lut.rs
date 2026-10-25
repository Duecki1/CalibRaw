//! The Develop card that chooses the photo's LUT and its amount.

use super::adjustment_cards::CardAction;
use super::*;
use crate::app::LOOK_LUT_CARD_TITLE;
use crate::luts::{LookLut, LutEntry, LutId};

/// What the LUT card asks for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum LookLutAction {
    Select(Option<LutId>),
    /// 0–1.
    Amount(f32),
    Import,
}

impl Sidebar {
    /// The LUT card: the library's LUTs and Import…, then the amount of the
    /// chosen LUT. A LUT the library does not hold is listed as missing.
    pub(super) fn show_look_lut(
        ui: &mut Ui,
        look: Option<&LookLut>,
        entries: &[LutEntry],
        can_import: bool,
        foldable: bool,
    ) -> Option<LookLutAction> {
        let mut action = None;
        let selected_id = look.map(|look| look.id);
        let card = Self::adjustment_card(ui, LOOK_LUT_CARD_TITLE, true, foldable, true, |ui| {
            let selected_text = match look {
                None => "None".to_owned(),
                Some(look) => entries
                    .iter()
                    .find(|entry| entry.id == look.id)
                    .map_or_else(
                        || format!("{} (missing)", look.name),
                        |entry| entry.name.clone(),
                    ),
            };
            moduwu_design::form_combo(ui, "LUT", "develop-look-lut", selected_text, 240.0, |ui| {
                if ui.selectable_label(look.is_none(), "None").clicked() {
                    action = Some(LookLutAction::Select(None));
                }
                for entry in entries {
                    if ui
                        .selectable_label(selected_id == Some(entry.id), &entry.name)
                        .clicked()
                    {
                        action = Some(LookLutAction::Select(Some(entry.id)));
                    }
                }
                if can_import {
                    ui.separator();
                    if ui.selectable_label(false, "Import…").clicked() {
                        action = Some(LookLutAction::Import);
                    }
                }
            });
            if let Some(look) = look {
                let mut percent = look.amount * 100.0;
                if AdjustmentSlider::new("Amount", &mut percent, 0.0..=100.0)
                    .decimals(0)
                    .step(1.0)
                    .reset_to(LookLut::DEFAULT_AMOUNT * 100.0)
                    .show(ui)
                {
                    action = Some(LookLutAction::Amount(percent / 100.0));
                }
            }
        });
        if matches!(card, CardAction::Reset) {
            action = Some(LookLutAction::Select(None));
        }
        action
    }
}
