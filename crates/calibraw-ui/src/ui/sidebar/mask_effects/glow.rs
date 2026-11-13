//! The Glow's card: its Style, then Amount, Core, Range, Spread and Warmth, whose
//! track runs yellow to red for Halation and cool to warm otherwise, as its
//! tint does (styled_glow.rs in calibraw-core); Star adds its Points, and
//! Motion, Streak and Star their Direction and Softness. Color separation,
//! collapsed, holds Dispersion (but for Motion) and Split with its mode and,
//! for Shift, its direction.

use super::{effect_details, float_param_slider, styled_effect_card, StyleChoice};
use crate::i18n::tr;
use crate::pipeline::{
    effect_params::styled_glow, GlowSplitMode, GlowStyle, MaskEffect, StarPoints,
    StyledGlowEffectSettings,
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

/// The width Points' and Split's segments take where the card is wide
/// enough.
const CHOICE_WIDTH: f32 = 160.0;

/// Its style is the Glow family's Bloom, Diffusion, Halation, Motion, Streak
/// and Star (`style`).
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
            changed |= float_param_slider(ui, &mut settings.core, styled_glow::CORE);
            changed |= float_param_slider(ui, &mut settings.range, styled_glow::RANGE);
            changed |= float_param_slider(ui, &mut settings.spread, styled_glow::SPREAD);
            let warmth = match settings.style {
                GlowStyle::Halation => HALATION_WARMTH,
                GlowStyle::Bloom
                | GlowStyle::Diffusion
                | GlowStyle::Motion
                | GlowStyle::Streak
                | GlowStyle::Star => SliderGradient::Temperature,
            };
            changed |=
                gradient_float_param_slider(ui, &mut settings.warmth, styled_glow::WARMTH, warmth);
            if settings.style == GlowStyle::Star {
                changed |= star_points(ui, &mut settings.points);
            }
            if settings.style.streaks() {
                changed |= float_param_angle(ui, &mut settings.direction, styled_glow::DIRECTION);
                changed |= float_param_slider(ui, &mut settings.softness, styled_glow::SOFTNESS);
            }
            changed |= effect_details(ui, tr(styled_glow::COLOR_SEPARATION), |ui| {
                color_separation(ui, settings)
            });
            changed
        },
    )
}

/// Star's Points: its rays, 4, 6 or 8.
fn star_points(ui: &mut Ui, points: &mut StarPoints) -> bool {
    let options = StarPoints::ALL.map(|option| (option, option.label()));
    moduwu_design::form_row(ui, tr("Points"), CHOICE_WIDTH, |ui, width| {
        moduwu_design::segmented_control(ui, points, &options, width).changed()
    })
    .inner
}

/// Dispersion, then Split with its mode and, for Shift, its direction.
fn color_separation(ui: &mut Ui, settings: &mut StyledGlowEffectSettings) -> bool {
    let mut changed = false;
    if settings.style.disperses() {
        changed |= float_param_slider(ui, &mut settings.dispersion, styled_glow::DISPERSION);
    }
    changed |= float_param_slider(ui, &mut settings.split, styled_glow::SPLIT);
    let options = GlowSplitMode::ALL.map(|mode| (mode, tr(mode.label())));
    changed |= moduwu_design::form_row(ui, tr("Split mode"), CHOICE_WIDTH, |ui, width| {
        moduwu_design::segmented_control(ui, &mut settings.split_mode, &options, width).changed()
    })
    .inner;
    if settings.split_mode == GlowSplitMode::Shift {
        changed |= float_param_angle(
            ui,
            &mut settings.split_direction,
            styled_glow::SPLIT_DIRECTION,
        );
    }
    changed
}
