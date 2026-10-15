use super::{effect_card, effect_color, effect_details, float_param_slider};
use crate::pipeline::{effect_params::glow, GlowEffectSettings, MaskEffect};
use eframe::egui::Ui;

pub(crate) fn show(
    ui: &mut Ui,
    settings: &mut GlowEffectSettings,
    enabled: &mut bool,
    remove: &mut bool,
) -> bool {
    effect_card(
        ui,
        MaskEffect::Glow,
        settings,
        enabled,
        remove,
        |ui, settings| {
            let mut changed = false;
            if moduwu_design::toggle_with_help(ui, &mut settings.self_illuminating, "Self-illuminating", "The mask emits the glow color itself, so it also shows on dark surfaces. Turn off to let only bright areas inside the mask glow.")
                .changed()
            {
                changed = true;
            }
            changed |= float_param_slider(ui, &mut settings.amount, glow::AMOUNT);
            changed |= float_param_slider(ui, &mut settings.radius, glow::RADIUS);
            changed |= effect_color(ui, "glow-color-picker", &mut settings.color, glow::COLOR);
            changed |= effect_details(ui, "Light details", |ui| {
                float_param_slider(ui, &mut settings.core, glow::CORE)
            });
            changed
        },
    )
}
