use super::*;

#[derive(Clone, Copy, Default)]
pub(super) enum CardAction {
    #[default]
    None,
    Toggle,
    Reset,
}

struct CardActionLayout {
    action: CardAction,
    buttons: [egui::Rect; 2],
    button_count: usize,
}

impl CardActionLayout {
    fn button_rects(&self) -> &[egui::Rect] {
        &self.buttons[..self.button_count]
    }
}

#[derive(Clone, Copy)]
struct VerticalCardActions {
    title: &'static str,
    show_visibility: bool,
    scope: Option<usize>,
}

impl CardAction {
    fn apply_reset(self, reset: impl FnOnce()) -> bool {
        match self {
            Self::None | Self::Toggle => false,
            Self::Reset => {
                reset();
                true
            }
        }
    }

    pub(super) fn apply(self, exposure: &mut ExposureParams, group: AdjustmentGroup) -> bool {
        self.apply_reset(|| exposure.reset_group(group))
    }

    pub(super) fn apply_local(
        self,
        adjustments: &mut crate::pipeline::LocalAdjustments,
        group: AdjustmentGroup,
    ) -> bool {
        self.apply_reset(|| adjustments.reset_group(group))
    }
}

fn visibility_icon(visible: bool) -> &'static str {
    if visible {
        egui_phosphor::regular::EYE_SLASH
    } else {
        egui_phosphor::regular::EYE
    }
}

fn visibility_label(visible: bool, title: &str) -> String {
    format!("{} {title}", if visible { "Hide" } else { "Show" })
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

    fn card_actions(
        ui: &mut Ui,
        title: &str,
        visible: bool,
        show_visibility: bool,
    ) -> CardActionLayout {
        let mut action = CardAction::None;
        let buttons = ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let size = egui::vec2(26.0, 26.0);
            let reset = crate::ui::icons::phosphor_icon_button(
                ui,
                egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE,
                size,
                &format!("Reset {title}"),
            );
            if reset.clicked() {
                action = CardAction::Reset;
            }
            if show_visibility {
                // Like the topbar eye, highlight the button when edits are bypassed.
                let eye = crate::ui::icons::phosphor_icon_toggle_button(
                    ui,
                    visibility_icon(visible),
                    !visible,
                    size,
                    &visibility_label(visible, title),
                );
                if eye.clicked() {
                    action = CardAction::Toggle;
                }
                ([eye.rect, reset.rect], 2)
            } else {
                ([reset.rect, egui::Rect::NOTHING], 1)
            }
        });
        CardActionLayout {
            action,
            buttons: buttons.inner.0,
            button_count: buttons.inner.1,
        }
    }

    fn vertical_card_actions_id() -> egui::Id {
        egui::Id::new("develop-vertical-card-actions")
    }

    fn pending_vertical_card_action_id(scope: Option<usize>, title: &'static str) -> egui::Id {
        egui::Id::new(("develop-vertical-card-action", scope, title))
    }

    pub(super) fn begin_vertical_card_actions(ctx: &egui::Context) {
        ctx.data_mut(|data| {
            data.remove::<Vec<VerticalCardActions>>(Self::vertical_card_actions_id())
        });
    }

    fn register_vertical_card_actions(ui: &Ui, title: &'static str, show_visibility: bool) {
        let scope = crate::app::preview_visibility::PreviewVisibility::current_scope(ui.ctx());
        let id = Self::vertical_card_actions_id();
        ui.ctx().data_mut(|data| {
            let mut actions = data
                .get_temp::<Vec<VerticalCardActions>>(id)
                .unwrap_or_default();
            if !actions
                .iter()
                .any(|entry| entry.title == title && entry.scope == scope)
            {
                actions.push(VerticalCardActions {
                    title,
                    show_visibility,
                    scope,
                });
            }
            data.insert_temp(id, actions);
        });
    }

    fn take_pending_vertical_card_action(ctx: &egui::Context, title: &'static str) -> CardAction {
        let scope = crate::app::preview_visibility::PreviewVisibility::current_scope(ctx);
        let id = Self::pending_vertical_card_action_id(scope, title);
        ctx.data_mut(|data| {
            let action = data.get_temp::<CardAction>(id).unwrap_or_default();
            data.remove::<CardAction>(id);
            action
        })
    }

    fn queue_vertical_card_action(
        ctx: &egui::Context,
        scope: Option<usize>,
        title: &'static str,
        action: CardAction,
    ) {
        ctx.data_mut(|data| {
            data.insert_temp(Self::pending_vertical_card_action_id(scope, title), action)
        });
        ctx.request_repaint();
    }

    pub(super) fn show_vertical_card_footer_actions(ui: &mut Ui) {
        let actions = ui
            .ctx()
            .data(|data| {
                data.get_temp::<Vec<VerticalCardActions>>(Self::vertical_card_actions_id())
            })
            .unwrap_or_default();
        let size = crate::ui::theme::toolbar_icon_size();

        for entry in actions {
            crate::app::preview_visibility::PreviewVisibility::set_mask_scope(
                ui.ctx(),
                entry.scope,
            );

            if crate::ui::icons::phosphor_icon_button(
                ui,
                egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE,
                size,
                &format!("Reset {}", entry.title),
            )
            .clicked()
            {
                if entry.show_visibility {
                    crate::app::preview_visibility::PreviewVisibility::show(ui.ctx(), entry.title);
                }
                Self::queue_vertical_card_action(
                    ui.ctx(),
                    entry.scope,
                    entry.title,
                    CardAction::Reset,
                );
            }

            if entry.show_visibility {
                let visible = crate::app::preview_visibility::PreviewVisibility::visible(
                    ui.ctx(),
                    entry.title,
                );
                if crate::ui::icons::phosphor_icon_toggle_button(
                    ui,
                    visibility_icon(visible),
                    !visible,
                    size,
                    &visibility_label(visible, entry.title),
                )
                .clicked()
                {
                    crate::app::preview_visibility::PreviewVisibility::toggle(
                        ui.ctx(),
                        entry.title,
                    );
                }
            }
        }

        crate::app::preview_visibility::PreviewVisibility::set_mask_scope(ui.ctx(), None);
    }

    pub(super) fn adjustment_card(
        ui: &mut Ui,
        title: &'static str,
        default_open: bool,
        foldable: bool,
        controls_enabled: bool,
        contents: impl FnOnce(&mut Ui),
    ) -> CardAction {
        Self::adjustment_card_controls(
            ui,
            title,
            default_open,
            foldable,
            controls_enabled,
            true,
            contents,
        )
    }

    pub(super) fn adjustment_card_without_visibility(
        ui: &mut Ui,
        title: &'static str,
        default_open: bool,
        foldable: bool,
        controls_enabled: bool,
        contents: impl FnOnce(&mut Ui),
    ) -> CardAction {
        Self::adjustment_card_controls(
            ui,
            title,
            default_open,
            foldable,
            controls_enabled,
            false,
            contents,
        )
    }

    /// `controls_enabled` only greys out the card body; the header (fold, reset, preview eye)
    /// stays interactive so a disabled card can still be re-enabled. Pass an already-`&&`ed
    /// expression – there is deliberately no second “card enabled” flag to keep in sync.
    fn adjustment_card_controls(
        ui: &mut Ui,
        title: &'static str,
        default_open: bool,
        foldable: bool,
        controls_enabled: bool,
        show_visibility: bool,
        contents: impl FnOnce(&mut Ui),
    ) -> CardAction {
        let visible = !show_visibility
            || crate::app::preview_visibility::PreviewVisibility::visible(ui.ctx(), title);
        let controls_enabled = controls_enabled && visible;
        let mut action = CardAction::None;
        crate::ui::theme::content_card(ui, |ui| {
            ui.push_id(title, |ui| {
                // Reserve the button height before laying out the title.
                ui.spacing_mut().interact_size.y = ui.spacing().interact_size.y.max(26.0);
                let body = |ui: &mut Ui| {
                    ui.add_enabled_ui(controls_enabled, contents);
                };
                let viewport = ui.ctx().content_rect();
                let vertical_screen = viewport.height() > viewport.width();
                if vertical_screen {
                    // Vertical-screen cards deliberately have no header. Their preview/reset
                    // actions are registered here and rendered outside the cards in the shared
                    // sidebar footer alongside the other mobile actions.
                    body(ui);
                    Self::register_vertical_card_actions(ui, title, show_visibility);
                    action = Self::take_pending_vertical_card_action(ui.ctx(), title);
                } else if foldable {
                    let mut header_clicked = false;
                    let mut header =
                        egui::collapsing_header::CollapsingState::load_with_default_open(
                            ui.ctx(),
                            ui.make_persistent_id("expanded"),
                            default_open,
                        )
                        .show_header(ui, |ui| {
                            let available = ui.available_rect_before_wrap();
                            Self::adjustment_card_title(ui, title);
                            let card_actions =
                                Self::card_actions(ui, title, visible, show_visibility);
                            action = card_actions.action;
                            let buttons = card_actions.button_rects();
                            // The built-in arrow already toggles. Make the rest of the
                            // header clickable, excluding each action button's bounds.
                            let button_top = buttons[0].top();
                            let button_bottom = buttons[0].bottom();
                            let mut left = available.left();
                            for (index, right) in buttons
                                .iter()
                                .map(egui::Rect::left)
                                .chain(std::iter::once(available.right()))
                                .enumerate()
                            {
                                if right > left {
                                    let rect = egui::Rect::from_min_max(
                                        egui::pos2(left, button_top),
                                        egui::pos2(right, button_bottom),
                                    );
                                    header_clicked |= ui
                                        .interact(
                                            rect,
                                            ui.id().with(("header", index)),
                                            egui::Sense::click(),
                                        )
                                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                                        .clicked();
                                }
                                if let Some(button) = buttons.get(index) {
                                    left = button.right();
                                }
                            }
                        });
                    if header_clicked {
                        header.toggle();
                    }
                    header.body_unindented(body);
                } else {
                    ui.horizontal(|ui| {
                        Self::adjustment_card_title(ui, title);
                        action = Self::card_actions(ui, title, visible, show_visibility).action;
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
                if show_visibility {
                    crate::app::preview_visibility::PreviewVisibility::show(ui.ctx(), title);
                }
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

    fn optional_text_rect(shapes: &[egui::epaint::ClippedShape], text: &str) -> Option<egui::Rect> {
        fn find(shape: &egui::Shape, text: &str) -> Option<egui::Rect> {
            match shape {
                egui::Shape::Text(shape) if shape.galley.text() == text => {
                    Some(egui::Rect::from_min_size(shape.pos, shape.galley.size()))
                }
                egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| find(shape, text)),
                _ => None,
            }
        }
        shapes.iter().find_map(|shape| find(&shape.shape, text))
    }

    fn text_rect(shapes: &[egui::epaint::ClippedShape], text: &str) -> egui::Rect {
        optional_text_rect(shapes, text).expect("header text")
    }

    #[test]
    fn structural_card_has_reset_without_preview_eye() {
        let ctx = egui::Context::default();
        PreviewVisibility::set_mask_scope(&ctx, Some(0));
        PreviewVisibility::toggle(&ctx, "Mask Properties");
        let output = ctx.run_ui(Default::default(), |ui| {
            Sidebar::adjustment_card_without_visibility(
                ui,
                "Mask Properties",
                true,
                false,
                true,
                |ui| {
                    assert!(ui.is_enabled());
                    ui.label("Controls");
                },
            );
        });

        assert!(optional_text_rect(
            &output.shapes,
            egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE
        )
        .is_some());
        assert!(optional_text_rect(&output.shapes, egui_phosphor::regular::EYE).is_none());
        assert!(optional_text_rect(&output.shapes, egui_phosphor::regular::EYE_SLASH).is_none());
    }

    #[test]
    fn header_folds_from_title_and_empty_space_but_not_action_buttons() {
        for width in [210.0, 280.0, 400.0] {
            for title in ["Light", "Color Grading", "Mask Properties"] {
                let ctx = egui::Context::default();
                ctx.style_mut_of(egui::Theme::Dark, |style| style.animation_time = 0.0);
                let render = |events| {
                    let mut body_shown = false;
                    let mut reset_count = 0;
                    let output = ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width, 180.0),
                            )),
                            events,
                            ..Default::default()
                        },
                        |ui| {
                            let action =
                                Sidebar::adjustment_card(ui, title, false, true, true, |ui| {
                                    body_shown = true;
                                    ui.label("Controls");
                                });
                            assert!(!matches!(action, CardAction::Toggle));
                            reset_count += usize::from(matches!(action, CardAction::Reset));
                        },
                    );
                    (output, body_shown, reset_count)
                };
                let (output, shown, _) = render(Vec::new());
                assert!(!shown);
                let label = text_rect(&output.shapes, title);
                let eye = text_rect(&output.shapes, egui_phosphor::regular::EYE_SLASH).center();
                let reset = text_rect(
                    &output.shapes,
                    egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE,
                )
                .center();
                let blank = egui::pos2((label.right() + eye.x - 13.0) / 2.0, eye.y);
                let arrow = egui::pos2(
                    label.left() - ctx.style_of(egui::Theme::Dark).spacing.indent / 2.0,
                    eye.y,
                );
                let button_gap = eye.lerp(reset, 0.5);
                for (target, expected_open, expected_visible, expected_resets) in [
                    (label.center(), true, true, 0),
                    (eye, true, false, 0),
                    (reset, true, true, 1),
                    (blank, false, true, 0),
                    (eye, false, false, 0),
                    (reset, false, true, 1),
                    (arrow, true, true, 0),
                    (label.center(), false, true, 0),
                    (button_gap, true, true, 0),
                    (arrow, false, true, 0),
                ] {
                    let mut resets = 0;
                    for step in 0..4 {
                        let mut events = vec![egui::Event::PointerMoved(target)];
                        if matches!(step, 1 | 2) {
                            events.push(egui::Event::PointerButton {
                                pos: target,
                                button: egui::PointerButton::Primary,
                                pressed: step == 1,
                                modifiers: egui::Modifiers::NONE,
                            });
                        }
                        let (output, shown, count) = render(events);
                        resets += count;
                        if step == 3 {
                            assert_eq!(shown, expected_open, "{title}, width {width}, {target:?}");
                            assert_eq!(PreviewVisibility::visible(&ctx, title), expected_visible);
                            text_rect(
                                &output.shapes,
                                if expected_visible {
                                    egui_phosphor::regular::EYE_SLASH
                                } else {
                                    egui_phosphor::regular::EYE
                                },
                            );
                        }
                    }
                    assert_eq!(resets, expected_resets);
                }
            }
        }
    }

    #[test]
    fn clicking_eye_emits_no_edit_and_reset_only_changes_its_card() {
        for width in [210.0, 280.0, 400.0] {
            for foldable in [false, true] {
                let ctx = egui::Context::default();
                crate::ui::theme::install(&ctx);
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
                                egui::vec2(width, 180.0),
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
                        let eye_rect = text_rect(&output.shapes, egui_phosphor::regular::EYE_SLASH);
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
