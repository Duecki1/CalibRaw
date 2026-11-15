use super::{
    effect_position, float_param_slider, styled_effect_card, EffectFrame, PositionSpace,
    StyleChoice,
};
use crate::i18n::tr;
use crate::pipeline::{effect_params::radial_blur, MaskEffect, RadialBlurEffectSettings};
use eframe::egui::Ui;

/// Its mode is the Blur family's Zoom and Spin styles (`style`).
pub(crate) fn show(
    ui: &mut Ui,
    settings: &mut RadialBlurEffectSettings,
    enabled: &mut bool,
    remove: &mut bool,
    frame: &EffectFrame,
    style: Option<&mut StyleChoice>,
) -> bool {
    styled_effect_card(
        ui,
        MaskEffect::RadialBlur,
        settings,
        enabled,
        remove,
        style,
        |ui, settings| {
            let mut changed = float_param_slider(ui, &mut settings.amount, radial_blur::AMOUNT);
            changed |= float_param_slider(ui, &mut settings.strength, radial_blur::STRENGTH);
            changed |= effect_position(
                ui,
                tr("Blur center"),
                &mut settings.center,
                [radial_blur::CENTER_X, radial_blur::CENTER_Y],
                frame,
                PositionSpace::Source,
            );
            changed
        },
    )
}
