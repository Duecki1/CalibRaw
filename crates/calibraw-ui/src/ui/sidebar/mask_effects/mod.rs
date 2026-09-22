pub(super) mod blur;
pub(super) mod edge_glow;
pub(super) mod fog;
pub(super) mod glow;
pub(super) mod lens_blur;
pub(super) mod light_rays;
pub(super) mod motion_blur;
pub(super) mod neon;
pub(super) mod pixelate;
pub(super) mod radial_blur;
pub(super) mod smoke;
pub(super) mod tilt_shift;

use super::{egui, MaskEffect, Ui};
use crate::pipeline::effect_params::ColorParamSpec;

pub(super) use crate::ui::components::adjustment_slider::float_param_slider as effect_slider;

fn effect_card<Settings>(
    ui: &mut Ui,
    effect: MaskEffect,
    settings: &mut Settings,
    enabled: &mut bool,
    remove: &mut bool,
    body: impl FnOnce(&mut Ui, &mut Settings) -> bool,
) -> bool
where
    Settings: Default,
{
    let mut changed = false;
    let mut reset = false;
    crate::ui::theme::content_card(ui, |ui| {
        ui.push_id(effect.label(), |ui| {
            ui.spacing_mut().interact_size.y = ui.spacing().interact_size.y.max(26.0);
            ui.horizontal(|ui| {
                super::Sidebar::adjustment_card_title(ui, effect.label());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let size = egui::vec2(26.0, 26.0);
                    reset = crate::ui::icons::phosphor_icon_button(
                        ui,
                        egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE,
                        size,
                        &format!("Reset {}", effect.label()),
                    )
                    .clicked();
                    *remove = crate::ui::icons::phosphor_icon_button(
                        ui,
                        egui_phosphor::regular::TRASH,
                        size,
                        &format!("Remove {}", effect.label()),
                    )
                    .clicked();
                    let icon = if *enabled {
                        egui_phosphor::regular::EYE_SLASH
                    } else {
                        egui_phosphor::regular::EYE
                    };
                    if crate::ui::icons::phosphor_icon_toggle_button(
                        ui,
                        icon,
                        !*enabled,
                        size,
                        &format!(
                            "{} {}",
                            if *enabled { "Hide" } else { "Show" },
                            effect.label()
                        ),
                    )
                    .clicked()
                    {
                        *enabled = !*enabled;
                        changed = true;
                    }
                });
            });
            ui.add_enabled_ui(*enabled, |ui| changed |= body(ui, settings));
        });
    });
    crate::ui::theme::card_gap(ui);
    changed
        | apply_card_action(
            if reset {
                super::adjustment_cards::CardAction::Reset
            } else {
                super::adjustment_cards::CardAction::None
            },
            settings,
        )
}

fn apply_card_action<Settings>(
    action: super::adjustment_cards::CardAction,
    settings: &mut Settings,
) -> bool
where
    Settings: Default,
{
    use super::adjustment_cards::CardAction;

    match action {
        CardAction::None | CardAction::Toggle => false,
        CardAction::Reset => {
            *settings = Settings::default();
            true
        }
    }
}

pub(super) fn effect_description(effect: MaskEffect) -> Option<&'static str> {
    match effect {
        MaskEffect::LensBlur => Some(
            "Uses an aperture-shaped scene-linear blur for natural bokeh.",
        ),
        MaskEffect::LightRays => Some(
            "The mask is the light source. Rays converge on the source point and travel beyond the mask.",
        ),
        MaskEffect::Fog => Some(
            "Fog is generated in full-image coordinates and blended through the editable mask.",
        ),
        MaskEffect::Smoke => Some(
            "Smoke is generated in full-image coordinates and blended through the editable mask.",
        ),
        _ => None,
    }
}

fn effect_color(
    ui: &mut Ui,
    id_salt: &'static str,
    color: &mut [f32; 3],
    spec: ColorParamSpec,
) -> bool {
    crate::ui::components::color_picker::sidebar_color_picker(
        ui,
        id_salt,
        color,
        spec.label,
        spec.tooltip,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{LocalMask, MaskKind};

    fn icon_rect(shapes: &[egui::epaint::ClippedShape], glyph: &str) -> egui::Rect {
        fn find(shape: &egui::Shape, glyph: &str) -> Option<egui::Rect> {
            match shape {
                egui::Shape::Text(text) if text.galley.text() == glyph => {
                    Some(egui::Rect::from_min_size(text.pos, text.galley.size()))
                }
                egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| find(shape, glyph)),
                _ => None,
            }
        }
        shapes
            .iter()
            .find_map(|shape| find(&shape.shape, glyph))
            .expect("effect card icon")
    }

    #[test]
    fn effect_card_actions_stay_in_the_header_on_wide_and_tall_screens() {
        for size in [egui::vec2(400.0, 180.0), egui::vec2(320.0, 640.0)] {
            let ctx = egui::Context::default();
            crate::ui::theme::install(&ctx);
            let mut settings = crate::pipeline::BlurEffectSettings::default();
            let mut enabled = true;
            let mut remove = false;
            let mut render = |events| {
                let output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        effect_card(
                            ui,
                            MaskEffect::Blur,
                            &mut settings,
                            &mut enabled,
                            &mut remove,
                            |ui, _| {
                                ui.label("Amount");
                                false
                            },
                        );
                    },
                );
                (output.shapes, enabled, remove)
            };
            let (shapes, _, _) = render(Vec::new());
            let eye = icon_rect(&shapes, egui_phosphor::regular::EYE_SLASH);
            let trash = icon_rect(&shapes, egui_phosphor::regular::TRASH);
            let reset = icon_rect(&shapes, egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE);
            assert!(eye.right() <= trash.left());
            assert!(trash.right() <= reset.left());
            assert!((eye.center().y - reset.center().y).abs() < 12.0);

            let click = |position, pressed| {
                vec![
                    egui::Event::PointerMoved(position),
                    egui::Event::PointerButton {
                        pos: position,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ]
            };
            render(click(eye.center(), true));
            let (_, enabled_after, _) = render(click(eye.center(), false));
            assert!(!enabled_after);
            render(click(trash.center(), true));
            let (_, _, removed) = render(click(trash.center(), false));
            assert!(removed);
        }
    }

    #[test]
    fn each_effect_reset_preserves_other_effects_adjustments_and_mask_state() {
        fn modify(value: &mut serde_json::Value) {
            match value {
                serde_json::Value::Number(number) if number.is_f64() => {
                    *value = serde_json::Value::from(0.25)
                }
                serde_json::Value::Array(values) => values.iter_mut().for_each(modify),
                serde_json::Value::Object(values) => values.values_mut().for_each(modify),
                _ => {}
            }
        }
        let mut mask = LocalMask::new(MaskKind::Brush, 1);
        mask.enabled = false;
        mask.adjustments.exposure = 1.75;
        let mut settings = serde_json::to_value(mask.effect_settings).unwrap();
        modify(&mut settings);
        mask.effect_settings = serde_json::from_value(settings).unwrap();
        let before = mask;
        macro_rules! check {
            ($field:ident) => {{
                let mut mask = before.clone();
                assert!(apply_card_action(
                    super::super::adjustment_cards::CardAction::Reset,
                    &mut mask.effect_settings.$field,
                ));
                let mut expected = before.clone();
                expected.effect_settings.$field = Default::default();
                assert_eq!(mask, expected, stringify!($field));
            }};
        }
        check!(blur);
        check!(lens_blur);
        check!(motion_blur);
        check!(radial_blur);
        check!(tilt_shift);
        check!(edge_glow);
        check!(glow);
        check!(light_rays);
        check!(neon);
        check!(pixelate);
        check!(fog);
        check!(smoke);
    }
}
