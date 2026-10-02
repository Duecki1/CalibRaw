use super::{effect_card, effect_color, effect_details, float_param_slider};
use crate::pipeline::{effect_params::neon, MaskEffect, NeonEffectSettings};
use eframe::egui::Ui;

pub(crate) fn show(
    ui: &mut Ui,
    settings: &mut NeonEffectSettings,
    enabled: &mut bool,
    remove: &mut bool,
) -> bool {
    effect_card(
        ui,
        MaskEffect::Neon,
        settings,
        enabled,
        remove,
        |ui, settings| {
            let mut changed = float_param_slider(ui, &mut settings.amount, neon::AMOUNT);
            changed |= float_param_slider(ui, &mut settings.edge_width, neon::EDGE_WIDTH);
            changed |= effect_color(ui, "neon-color-picker", &mut settings.color, neon::COLOR);
            changed |= effect_details(ui, "Neon details", |ui| {
                float_param_slider(ui, &mut settings.glow, neon::GLOW)
                    | float_param_slider(ui, &mut settings.detail, neon::DETAIL)
                    | float_param_slider(ui, &mut settings.background, neon::BACKGROUND)
            });
            changed
        },
    )
}
