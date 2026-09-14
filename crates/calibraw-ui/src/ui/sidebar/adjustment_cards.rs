use super::*;

#[derive(Clone, Copy, Default)]
pub(super) enum CardAction {
    #[default]
    None,
    Toggle,
    Reset,
}

impl CardAction {
    pub(super) fn apply(self, exposure: &mut ExposureParams, group: AdjustmentGroup) -> bool {
        match self {
            Self::None => return false,
            Self::Toggle => return false,
            Self::Reset => exposure.reset_group(group),
        }
        true
    }

    pub(super) fn apply_local(
        self,
        adjustments: &mut crate::pipeline::LocalAdjustments,
        group: AdjustmentGroup,
    ) -> bool {
        match self {
            Self::None => return false,
            Self::Toggle => return false,
            Self::Reset => adjustments.reset_group(group),
        }
        true
    }
}

impl Sidebar {
    fn adjustment_card_title(ui: &mut Ui, title: &str) {
        let response = ui.strong(title);
        if let Some(help) = MaskEffect::ALL
            .iter()
            .find(|effect| effect.label() == title)
            .and_then(|effect| mask_effects::effect_description(*effect))
        {
            response.on_hover_text(help);
        }
    }

    pub(super) fn card_actions(ui: &mut Ui, title: &str, enabled: bool) -> CardAction {
        let mut action = CardAction::None;
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let size = egui::vec2(26.0, 26.0);
            if crate::ui::icons::phosphor_icon_button(
                ui,
                egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE,
                size,
                &format!("Reset {title}"),
            )
            .clicked()
            {
                action = CardAction::Reset;
            }
            if crate::ui::icons::phosphor_icon_toggle_button(
                ui,
                if enabled {
                    egui_phosphor::regular::EYE
                } else {
                    egui_phosphor::regular::EYE_SLASH
                },
                enabled,
                size,
                &format!("{} {title}", if enabled { "Hide" } else { "Show" }),
            )
            .clicked()
            {
                action = CardAction::Toggle;
            }
        });
        action
    }

    pub(super) fn adjustment_card(
        ui: &mut Ui,
        title: &'static str,
        default_open: bool,
        foldable: bool,
        enabled: bool,
        contents: impl FnOnce(&mut Ui),
    ) -> CardAction {
        Self::adjustment_card_with_enabled(
            ui,
            title,
            default_open,
            foldable,
            enabled,
            enabled,
            contents,
        )
    }

    pub(super) fn adjustment_card_with_enabled(
        ui: &mut Ui,
        title: &'static str,
        default_open: bool,
        foldable: bool,
        enabled: bool,
        controls_enabled: bool,
        contents: impl FnOnce(&mut Ui),
    ) -> CardAction {
        let visible = crate::app::preview_visibility::PreviewVisibility::visible(ui.ctx(), title);
        let controls_enabled = controls_enabled && visible;
        let _ = enabled;
        let mut action = CardAction::None;
        crate::ui::theme::content_card(ui, |ui| {
            ui.push_id(title, |ui| {
                // Reserve the button height before laying out the title.
                ui.spacing_mut().interact_size.y = ui.spacing().interact_size.y.max(26.0);
                let body = |ui: &mut Ui| {
                    ui.add_enabled_ui(controls_enabled, contents);
                };
                if foldable {
                    egui::collapsing_header::CollapsingState::load_with_default_open(
                        ui.ctx(),
                        ui.make_persistent_id("expanded"),
                        default_open,
                    )
                    .show_header(ui, |ui| {
                        Self::adjustment_card_title(ui, title);
                        action = Self::card_actions(ui, title, visible);
                    })
                    .body_unindented(body);
                } else {
                    ui.horizontal(|ui| {
                        Self::adjustment_card_title(ui, title);
                        action = Self::card_actions(ui, title, visible);
                    });
                    body(ui);
                }
            });
        });
        crate::ui::theme::card_gap(ui);
        match action {
            CardAction::Toggle => {
                crate::app::preview_visibility::PreviewVisibility::toggle(ui.ctx(), title);
                CardAction::None
            }
            CardAction::Reset => {
                crate::app::preview_visibility::PreviewVisibility::show(ui.ctx(), title);
                CardAction::Reset
            }
            CardAction::None => CardAction::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::preview_visibility::PreviewVisibility;

    fn text_rect(shapes: &[egui::epaint::ClippedShape], text: &str) -> egui::Rect {
        fn find(shape: &egui::Shape, text: &str) -> Option<egui::Rect> {
            match shape {
                egui::Shape::Text(shape) if shape.galley.text() == text => {
                    Some(egui::Rect::from_min_size(shape.pos, shape.galley.size()))
                }
                egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| find(shape, text)),
                _ => None,
            }
        }
        shapes
            .iter()
            .find_map(|shape| find(&shape.shape, text))
            .expect("header text")
    }

    #[test]
    fn clicking_eye_emits_no_edit_and_reset_only_changes_its_card() {
        for width in [210.0, 280.0, 400.0] {
            for foldable in [false, true] {
                let ctx = egui::Context::default();
                let mut edits = ExposureParams {
                    exposure: 2.0,
                    temperature: 17.0,
                    ..Default::default()
                };
                let original = edits;
                let mut events = Vec::new();
                let mut eye = egui::Pos2::ZERO;
                let mut reset = egui::Pos2::ZERO;
                let mut edit_count = 0;
                for step in 0..8 {
                    let output = ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width, 400.0),
                            )),
                            events: std::mem::take(&mut events),
                            ..Default::default()
                        },
                        |ui| {
                            let mut body_shown = false;
                            let action = Sidebar::adjustment_card(
                                ui,
                                "Light",
                                false,
                                foldable,
                                true,
                                |ui| {
                                    body_shown = true;
                                    assert_eq!(
                                        ui.is_enabled(),
                                        PreviewVisibility::visible(ui.ctx(), "Light")
                                    );
                                    ui.label("Controls");
                                },
                            );
                            assert_eq!(body_shown, !foldable);
                            assert!(!matches!(action, CardAction::Toggle));
                            if action.apply(&mut edits, AdjustmentGroup::Light) {
                                edit_count += 1;
                            }
                        },
                    );
                    if step == 0 {
                        let title = text_rect(&output.shapes, "Light");
                        let eye_rect = text_rect(&output.shapes, egui_phosphor::regular::EYE);
                        let reset_rect = text_rect(
                            &output.shapes,
                            egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE,
                        );
                        assert!((title.center().y - eye_rect.center().y).abs() <= 2.0);
                        assert!((title.center().y - reset_rect.center().y).abs() <= 2.0);
                        assert!(title.right() < eye_rect.left());
                        eye = eye_rect.center();
                        reset = reset_rect.center();
                    }
                    if step == 3 {
                        assert!(!PreviewVisibility::visible(&ctx, "Light"));
                        assert_eq!(edits, original);
                        assert_eq!(edit_count, 0);
                    }
                    let target = if step < 4 { eye } else { reset };
                    events.push(egui::Event::PointerMoved(target));
                    if matches!(step, 1 | 2 | 5 | 6) {
                        events.push(egui::Event::PointerButton {
                            pos: target,
                            button: egui::PointerButton::Primary,
                            pressed: matches!(step, 1 | 5),
                            modifiers: egui::Modifiers::NONE,
                        });
                    }
                }
                assert_eq!(edit_count, 1);
                assert_eq!(edits.exposure, 0.0);
                assert_eq!(edits.temperature, original.temperature);
                assert!(PreviewVisibility::visible(&ctx, "Light"));
            }
        }
    }
}
