use super::{effect_card, effect_color, effect_slider};
use crate::pipeline::{effect_params::fog, FogEffectSettings, MaskEffect};
use eframe::egui::Ui;

pub(crate) fn show(
    ui: &mut Ui,
    settings: &mut FogEffectSettings,
    enabled: &mut bool,
    remove: &mut bool,
) -> bool {
    effect_card(
        ui,
        MaskEffect::Fog,
        settings,
        enabled,
        remove,
        |ui, settings| {
            let mut changed = false;
            changed |= ui
                .toggle_value(&mut settings.depth_enabled, "Depth")
                .changed();
            changed |= effect_slider(ui, &mut settings.amount, fog::AMOUNT);
            changed |= effect_slider(ui, &mut settings.density, fog::DENSITY);
            if settings.depth_enabled {
                changed |= effect_slider(ui, &mut settings.start, fog::START);
                changed |= effect_slider(ui, &mut settings.depth_influence, fog::DEPTH_INFLUENCE);
            }
            changed |= effect_slider(ui, &mut settings.scale, fog::SCALE);
            changed |= effect_slider(ui, &mut settings.softness, fog::SOFTNESS);
            changed |= effect_slider(ui, &mut settings.variation, fog::VARIATION);
            changed |= effect_slider(ui, &mut settings.seed, fog::SEED);
            changed |= effect_color(ui, "fog-color-picker", &mut settings.color, fog::COLOR);
            changed
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui;

    #[test]
    fn depth_button_hides_sliders_and_preserves_their_values() {
        fn text_rect(shapes: &[egui::epaint::ClippedShape], label: &str) -> Option<egui::Rect> {
            fn find(shape: &egui::Shape, label: &str) -> Option<egui::Rect> {
                match shape {
                    egui::Shape::Text(text) if text.galley.text() == label => {
                        Some(egui::Rect::from_min_size(text.pos, text.galley.size()))
                    }
                    egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| find(shape, label)),
                    _ => None,
                }
            }
            shapes.iter().find_map(|shape| find(&shape.shape, label))
        }
        let ctx = egui::Context::default();
        crate::ui::theme::install(&ctx);
        let mut settings = FogEffectSettings {
            start: 35.0,
            depth_influence: 80.0,
            ..Default::default()
        };
        let mut enabled = true;
        let mut remove = false;
        let mut render = |events| {
            let mut changed = false;
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(400.0, 900.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| changed = show(ui, &mut settings, &mut enabled, &mut remove),
            );
            (output.shapes, changed, settings)
        };
        let (shapes, _, _) = render(Vec::new());
        assert!(text_rect(&shapes, fog::START.label).is_some());
        assert!(text_rect(&shapes, fog::DEPTH_INFLUENCE.label).is_some());
        let button = text_rect(&shapes, "Depth").unwrap();
        let click = |pressed| {
            vec![
                egui::Event::PointerMoved(button.center()),
                egui::Event::PointerButton {
                    pos: button.center(),
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ]
        };
        render(click(true));
        let (_, changed, settings) = render(click(false));
        assert!(changed);
        assert!(!settings.depth_enabled);
        let (shapes, _, _) = render(Vec::new());
        assert!(text_rect(&shapes, fog::START.label).is_none());
        assert!(text_rect(&shapes, fog::DEPTH_INFLUENCE.label).is_none());
        assert!(text_rect(&shapes, fog::DENSITY.label).is_some());
        render(click(true));
        let (_, changed, settings) = render(click(false));
        assert!(changed);
        assert!(settings.depth_enabled);
        assert_eq!(settings.start, 35.0);
        assert_eq!(settings.depth_influence, 80.0);
        let (shapes, _, _) = render(Vec::new());
        assert!(text_rect(&shapes, fog::START.label).is_some());
        assert!(text_rect(&shapes, fog::DEPTH_INFLUENCE.label).is_some());
    }
}
