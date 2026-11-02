use super::{
    effect_card, effect_color, effect_details, effect_position, effect_toggle, float_param_slider,
    EffectFrame, PositionSpace,
};
use crate::i18n::tr;
use crate::pipeline::{effect_params::relight, MaskEffect, RelightEffectSettings};
use eframe::egui::Ui;

pub(crate) fn show(
    ui: &mut Ui,
    settings: &mut RelightEffectSettings,
    enabled: &mut bool,
    remove: &mut bool,
    frame: &EffectFrame,
) -> bool {
    effect_card(
        ui,
        MaskEffect::Relight,
        settings,
        enabled,
        remove,
        |ui, settings| {
            let mut changed = float_param_slider(ui, &mut settings.amount, relight::AMOUNT);
            changed |= effect_position(
                ui,
                tr("Light position"),
                &mut settings.source,
                [relight::SOURCE_X, relight::SOURCE_Y],
                frame,
                PositionSpace::Source,
            );
            changed |= float_param_slider(ui, &mut settings.depth, relight::DEPTH);
            changed |= float_param_slider(ui, &mut settings.ambient, relight::AMBIENT);
            changed |= effect_color(
                ui,
                "relight-color-picker",
                &mut settings.color,
                relight::COLOR,
            );
            changed |= effect_toggle(ui, &mut settings.shadows_enabled, relight::CAST_SHADOWS);
            changed |= effect_details(ui, tr("Light details"), |ui| {
                let mut changed = float_param_slider(ui, &mut settings.reach, relight::REACH);
                changed |= float_param_slider(ui, &mut settings.size, relight::SIZE);
                if settings.shadows_enabled {
                    changed |= float_param_slider(ui, &mut settings.shadows, relight::SHADOWS);
                }
                changed |= float_param_slider(ui, &mut settings.relief, relight::RELIEF);
                changed
            });
            changed
        },
    )
}
