use super::{
    effect_card, effect_color, effect_details, effect_position, float_param_slider, EffectFrame,
    PositionSpace,
};
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
                "Light position",
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
            changed |= effect_details(ui, "Light details", |ui| {
                let mut changed = float_param_slider(ui, &mut settings.reach, relight::REACH);
                changed |= float_param_slider(ui, &mut settings.size, relight::SIZE);
                changed |= float_param_slider(ui, &mut settings.shadows, relight::SHADOWS);
                changed |= float_param_slider(ui, &mut settings.relief, relight::RELIEF);
                changed
            });
            changed
        },
    )
}
