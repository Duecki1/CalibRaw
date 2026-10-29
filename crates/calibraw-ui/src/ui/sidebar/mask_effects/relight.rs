use super::{
    effect_card, effect_color, effect_details, effect_position, float_param_slider, EffectFrame,
    PositionSpace,
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
            if moduwu_design::toggle_with_help(ui, &mut settings.shadows_enabled, tr("Cast shadows"), tr("Let nearer objects block the light. Turn off for light without shadows; the Shadows strength is preserved."))
                .changed()
            {
                changed = true;
            }
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
