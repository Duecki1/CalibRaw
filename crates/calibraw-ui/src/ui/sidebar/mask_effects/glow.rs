//! The Glow's card: its Style, then Amount, Range, Spread and Warmth, whose
//! track runs yellow to red for Halation and cool to warm otherwise, as its
//! tint does (styled_glow.rs in calibraw-core); Motion and Streak add their
//! Direction and Softness.

use super::{float_param_slider, styled_effect_card, StyleChoice};
use crate::pipeline::{
    effect_params::styled_glow, GlowStyle, MaskEffect, StyledGlowEffectSettings,
};
use crate::ui::components::adjustment_slider::{
    float_param_angle, gradient_float_param_slider, SliderGradient,
};
use eframe::egui::{Color32, Ui};

/// Halation's Warmth track: yellow, orange and red.
const HALATION_WARMTH: SliderGradient = SliderGradient::ChannelHue {
    left: Color32::from_rgb(255, 210, 40),
    center: Color32::from_rgb(255, 130, 30),
    right: Color32::from_rgb(220, 30, 15),
};

/// Its style is the Glow family's Bloom, Diffusion, Halation, Motion and
/// Streak (`style`).
pub(crate) fn show(
    ui: &mut Ui,
    settings: &mut StyledGlowEffectSettings,
    enabled: &mut bool,
    remove: &mut bool,
    style: Option<&mut StyleChoice>,
) -> bool {
    styled_effect_card(
        ui,
        MaskEffect::StyledGlow,
        settings,
        enabled,
        remove,
        style,
        |ui, settings| {
            let mut changed = float_param_slider(ui, &mut settings.amount, styled_glow::AMOUNT);
            changed |= float_param_slider(ui, &mut settings.range, styled_glow::RANGE);
            changed |= float_param_slider(ui, &mut settings.spread, styled_glow::SPREAD);
            let warmth = match settings.style {
                GlowStyle::Halation => HALATION_WARMTH,
                GlowStyle::Bloom | GlowStyle::Diffusion | GlowStyle::Motion | GlowStyle::Streak => {
                    SliderGradient::Temperature
                }
            };
            changed |=
                gradient_float_param_slider(ui, &mut settings.warmth, styled_glow::WARMTH, warmth);
            if settings.style.streaks() {
                changed |= float_param_angle(ui, &mut settings.direction, styled_glow::DIRECTION);
                changed |= float_param_slider(ui, &mut settings.softness, styled_glow::SOFTNESS);
            }
            changed
        },
    )
}
