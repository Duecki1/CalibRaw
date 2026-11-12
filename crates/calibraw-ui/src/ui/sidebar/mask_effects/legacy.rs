//! The card of a legacy effect ([`MaskEffect::is_legacy`]): its name, its
//! switch and Remove, and Replace with new, which picks the style that
//! replaces it from `style` ([`StyleChoice::of`]). Its values stay as saved.

use super::{effect_card_frame, StyleChoice};
use crate::i18n::{tr, tr_fmt};
use crate::pipeline::MaskEffect;
use eframe::egui::Ui;

/// Returns whether its switch changed; a picked replacement is in `style`.
pub(crate) fn show(
    ui: &mut Ui,
    effect: MaskEffect,
    enabled: &mut bool,
    remove: &mut bool,
    style: Option<&mut StyleChoice>,
) -> bool {
    effect_card_frame(ui, effect, enabled, remove, None, |ui| {
        let Some(style) = style else {
            return false;
        };
        let replacement = style.offered.first().copied();
        let label = tr("Replace with new");
        let response = ui
            .add_enabled_ui(replacement.is_some(), |ui| {
                moduwu_design::full_width_button(ui, label)
            })
            .inner
            .on_disabled_hover_text(tr_fmt(
                "{effect} is already added",
                &[(
                    "effect",
                    &tr(effect
                        .replacement()
                        .map_or(effect.label(), |style| style.effect().label())),
                )],
            ));
        if response.clicked() {
            style.picked = replacement;
        }
        false
    })
}
