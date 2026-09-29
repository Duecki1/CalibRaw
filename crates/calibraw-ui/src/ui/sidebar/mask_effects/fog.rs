use super::{effect_card, effect_color, effect_slider};
use crate::app::CalibRawApp;
use crate::pipeline::{effect_params::fog, FogEffectSettings, MaskEffect};
use eframe::egui::Ui;

pub(crate) struct SceneDepthControls {
    available: bool,
    can_generate: bool,
    requested: bool,
}

impl SceneDepthControls {
    pub(crate) fn new(app: &CalibRawApp) -> Self {
        Self {
            available: app.masks.stack.scene_depth_image().is_some(),
            can_generate: !app.foreground_operation_active() && !app.ai.consent.is_open(),
            requested: false,
        }
    }

    fn show(&mut self, ui: &mut Ui) {
        if self.available {
            ui.weak("Scene depth ready")
                .on_hover_text("Global and local fog share the existing full-image depth map.");
        } else {
            ui.weak("Use scene depth to build fog with distance.");
            self.requested |= ui
                .add_enabled(
                    self.can_generate,
                    eframe::egui::Button::new("Generate scene depth"),
                )
                .on_hover_text("Generate depth locally with Depth Anything 3 for all fog effects.")
                .clicked();
        }
        ui.add_space(crate::ui::theme::SPACE_XS);
    }

    pub(crate) fn apply_request(self, app: &mut CalibRawApp, frame: &eframe::Frame) {
        if self.requested {
            app.request_depth_mask(frame);
        }
    }
}

pub(crate) fn show(
    ui: &mut Ui,
    settings: &mut FogEffectSettings,
    enabled: &mut bool,
    remove: &mut bool,
    scene_depth: &mut SceneDepthControls,
) -> bool {
    effect_card(
        ui,
        MaskEffect::Fog,
        settings,
        enabled,
        remove,
        |ui, settings| {
            scene_depth.show(ui);
            let mut changed = false;
            changed |= effect_slider(ui, &mut settings.amount, fog::AMOUNT);
            changed |= effect_slider(ui, &mut settings.density, fog::DENSITY);
            changed |= effect_slider(ui, &mut settings.start, fog::START);
            changed |= effect_slider(ui, &mut settings.depth_influence, fog::DEPTH_INFLUENCE);
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
    use crate::pipeline::{EffectComponent, MaskImage, MaskKind, MaskStack};
    use crate::ui::sidebar::Sidebar;
    use eframe::egui;

    fn label_rect(shapes: &[egui::epaint::ClippedShape], label: &str) -> Option<egui::Rect> {
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

    #[test]
    fn global_and_local_fog_request_depth_in_both_layouts_only_on_click() {
        for vertical in [false, true] {
            for local in [false, true] {
                let ctx = egui::Context::default();
                crate::ui::theme::install(&ctx);
                let mut stack = MaskStack::default();
                let fog = EffectComponent::new(MaskEffect::Fog);
                if local {
                    stack.add_mask(MaskKind::Fullscreen).unwrap();
                    stack.masks[0].effect_components.push(fog);
                } else {
                    stack.global_effects.push(fog);
                }
                let render = |stack: &mut MaskStack, events, can_generate| {
                    let mut controls = SceneDepthControls {
                        available: stack.scene_depth_image().is_some(),
                        can_generate,
                        requested: false,
                    };
                    let output = ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                if vertical {
                                    egui::vec2(360.0, 1000.0)
                                } else {
                                    egui::vec2(1100.0, 1000.0)
                                },
                            )),
                            events,
                            ..Default::default()
                        },
                        |ui| {
                            ui.set_max_width(340.0);
                            let components = if local {
                                &mut stack.masks[0].effect_components
                            } else {
                                &mut stack.global_effects
                            };
                            let changed = if vertical {
                                Sidebar::show_selected_effect_component(
                                    ui,
                                    components,
                                    &mut Some(MaskEffect::Fog),
                                    true,
                                    &mut controls,
                                )
                            } else {
                                Sidebar::show_effect_components(ui, components, true, &mut controls)
                            };
                            assert!(!changed);
                        },
                    );
                    (output.shapes, controls.requested)
                };

                for _ in 0..3 {
                    let (shapes, requested) = render(&mut stack, Vec::new(), true);
                    assert!(label_rect(&shapes, "Generate scene depth").is_some());
                    assert!(!requested);
                }
                let (shapes, _) = render(&mut stack, Vec::new(), true);
                let button = label_rect(&shapes, "Generate scene depth")
                    .unwrap()
                    .center();
                // Busy operations disable the button; only an enabled click requests depth.
                for can_generate in [false, true] {
                    render(&mut stack, Vec::new(), can_generate);
                    for pressed in [true, false] {
                        let (_, requested) = render(
                            &mut stack,
                            vec![
                                egui::Event::PointerMoved(button),
                                egui::Event::PointerButton {
                                    pos: button,
                                    button: egui::PointerButton::Primary,
                                    pressed,
                                    modifiers: egui::Modifiers::NONE,
                                },
                            ],
                            can_generate,
                        );
                        assert_eq!(requested, can_generate && !pressed);
                    }
                }
                stack.scene_depth = Some(MaskImage::new(2, 2, vec![0, 85, 170, 255]).unwrap());
                let (shapes, requested) = render(&mut stack, Vec::new(), true);
                assert!(!requested);
                assert!(label_rect(&shapes, "Scene depth ready").is_some());
                assert!(label_rect(&shapes, "Generate scene depth").is_none());
            }
        }
    }
}
