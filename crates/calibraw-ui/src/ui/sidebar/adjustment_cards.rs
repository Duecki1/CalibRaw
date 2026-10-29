use super::*;
use crate::i18n::{tr, tr_fmt};

#[derive(Clone, Copy, Default)]
pub(super) enum CardAction {
    #[default]
    None,
    Toggle,
    Reset,
}

struct CardActionLayout {
    action: CardAction,
    buttons: [egui::Rect; 3],
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
    kind: VerticalCardKind,
    scope: Option<usize>,
}

#[derive(Clone, Copy)]
enum VerticalCardKind {
    Adjustment {
        show_visibility: bool,
        show_mask_overlay_toggle: bool,
    },
    Effect {
        enabled: bool,
    },
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum EffectCardAction {
    #[default]
    None,
    Reset,
    Toggle,
    Remove,
}

#[derive(Clone, Copy)]
struct CardHeaderControls {
    show_visibility: bool,
    show_mask_overlay_toggle: bool,
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

    /// Resets `group` to the photo-specific starting values in `defaults`.
    pub(super) fn apply_from(
        self,
        exposure: &mut ExposureParams,
        defaults: &ExposureParams,
        group: AdjustmentGroup,
    ) -> bool {
        self.apply_reset(|| exposure.copy_group_from(defaults, group))
    }

    pub(super) fn apply_local(
        self,
        adjustments: &mut crate::pipeline::LocalAdjustments,
        group: AdjustmentGroup,
    ) -> bool {
        self.apply_reset(|| adjustments.reset_group(group))
    }
}

fn card_reveal_id(title: &str) -> egui::Id {
    egui::Id::new("calibraw-reveal-adjustment-card").with(title)
}

/// Unfolds the adjustment card titled `title` and scrolls it into view. Call
/// it in the pass that draws the card, before drawing it.
pub(super) fn reveal_adjustment_card(ctx: &egui::Context, title: &'static str) {
    let target = card_reveal_id(title);
    ctx.data_mut(|data| data.insert_temp(target, true));
    moduwu_design::request_scroll_into_view(ctx, target, Some(egui::Align::TOP));
}

fn visibility_icon(visible: bool) -> &'static str {
    if visible {
        egui_phosphor::regular::EYE_SLASH
    } else {
        egui_phosphor::regular::EYE
    }
}

/// The name an adjustment card shows. `title` stays the card's key (reveal,
/// preview visibility and pending actions use it), so only the name is
/// translated. Names listed here are literals so the catalog extraction
/// finds them; any other title, such as a mask effect name from
/// `calibraw-core`, is looked up by its English text.
pub(super) fn card_title_text(title: &'static str) -> &'static str {
    match title {
        "Light" => tr("Light"),
        "Tone Curve" => tr("Tone Curve"),
        "Color" => tr("Color"),
        "Color Grading" => tr("Color Grading"),
        "Detail" => tr("Detail"),
        "Effects" => tr("Effects"),
        "Color Mixer" => tr("Color Mixer"),
        "Mask Properties" => tr("Mask Properties"),
        "Subject refinement" => tr("Subject refinement"),
        "Lens Corrections" => tr("Lens Corrections"),
        "Film negative" => tr("Film negative"),
        "LUT" => tr("LUT"),
        _ => tr(title),
    }
}

fn visibility_label(visible: bool, title: &'static str) -> String {
    if visible {
        tr_fmt("Hide {title}", &[("title", &card_title_text(title))])
    } else {
        tr_fmt("Show {title}", &[("title", &card_title_text(title))])
    }
}

impl Sidebar {
    pub(super) fn adjustment_card_title(ui: &mut Ui, title: &'static str) {
        let response = ui.strong(card_title_text(title));
        if let Some(help) = MaskEffect::ALL
            .iter()
            .find(|effect| effect.label() == title)
            .and_then(|effect| mask_effects::effect_description(*effect))
        {
            response.on_hover_text(help);
        }
    }

    pub(super) fn card_header_background_clicked(
        ui: &mut Ui,
        available: egui::Rect,
        buttons: &[egui::Rect],
    ) -> bool {
        let button_top = buttons[0].top();
        let button_bottom = buttons[0].bottom();
        let mut left = available.left();
        let mut clicked = false;
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
                clicked |= ui
                    .interact(rect, ui.id().with(("header", index)), egui::Sense::click())
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked();
            }
            if let Some(button) = buttons.get(index) {
                left = button.right();
            }
        }
        clicked
    }

    fn card_actions(
        ui: &mut Ui,
        title: &'static str,
        visible: bool,
        show_visibility: bool,
        show_mask_overlay_toggle: bool,
    ) -> CardActionLayout {
        let mut action = CardAction::None;
        let buttons = ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let size = egui::vec2(26.0, 26.0);
            let reset = moduwu_design::ghost_icon_button(
                ui,
                egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE,
                size,
                &tr_fmt("Reset {title}", &[("title", &card_title_text(title))]),
            );
            if reset.clicked() {
                action = CardAction::Reset;
            }
            let overlay = show_mask_overlay_toggle.then(|| {
                let forced = crate::app::preview_visibility::PreviewVisibility::mask_overlay_forced(
                    ui.ctx(),
                );
                let response = moduwu_design::ghost_icon_toggle_button(
                    ui,
                    egui_phosphor::regular::EYE,
                    forced,
                    size,
                    if forced {
                        tr("Use automatic mask overlay visibility")
                    } else {
                        tr("Always show the selected mask overlay")
                    },
                );
                if response.clicked() {
                    crate::app::preview_visibility::PreviewVisibility::toggle_mask_overlay(
                        ui.ctx(),
                    );
                }
                response
            });
            if show_visibility {
                // Like the topbar eye, highlight the button when edits are bypassed.
                let eye = moduwu_design::ghost_icon_toggle_button(
                    ui,
                    visibility_icon(visible),
                    !visible,
                    size,
                    &visibility_label(visible, title),
                );
                if eye.clicked() {
                    action = CardAction::Toggle;
                }
                if let Some(overlay) = overlay {
                    ([eye.rect, overlay.rect, reset.rect], 3)
                } else {
                    ([eye.rect, reset.rect, egui::Rect::NOTHING], 2)
                }
            } else if let Some(overlay) = overlay {
                ([overlay.rect, reset.rect, egui::Rect::NOTHING], 2)
            } else {
                ([reset.rect, egui::Rect::NOTHING, egui::Rect::NOTHING], 1)
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

    fn pending_vertical_effect_action_id(scope: Option<usize>, title: &'static str) -> egui::Id {
        egui::Id::new(("develop-vertical-effect-action", scope, title))
    }

    pub(super) fn begin_vertical_card_actions(ctx: &egui::Context) {
        ctx.data_mut(|data| {
            data.remove::<Vec<VerticalCardActions>>(Self::vertical_card_actions_id())
        });
    }

    fn register_vertical_card_actions(
        ui: &Ui,
        title: &'static str,
        show_visibility: bool,
        show_mask_overlay_toggle: bool,
    ) {
        Self::register_vertical_actions(
            ui,
            title,
            VerticalCardKind::Adjustment {
                show_visibility,
                show_mask_overlay_toggle,
            },
        );
    }

    pub(super) fn register_vertical_effect_actions(ui: &Ui, title: &'static str, enabled: bool) {
        Self::register_vertical_actions(ui, title, VerticalCardKind::Effect { enabled });
    }

    fn register_vertical_actions(ui: &Ui, title: &'static str, kind: VerticalCardKind) {
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
                actions.push(VerticalCardActions { title, kind, scope });
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

    pub(super) fn take_pending_vertical_effect_action(
        ctx: &egui::Context,
        title: &'static str,
    ) -> EffectCardAction {
        let scope = crate::app::preview_visibility::PreviewVisibility::current_scope(ctx);
        let id = Self::pending_vertical_effect_action_id(scope, title);
        ctx.data_mut(|data| {
            let action = data.get_temp::<EffectCardAction>(id).unwrap_or_default();
            data.remove::<EffectCardAction>(id);
            action
        })
    }

    fn queue_vertical_effect_action(
        ctx: &egui::Context,
        scope: Option<usize>,
        title: &'static str,
        action: EffectCardAction,
    ) {
        ctx.data_mut(|data| {
            data.insert_temp(
                Self::pending_vertical_effect_action_id(scope, title),
                action,
            )
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
        let size = moduwu_design::toolbar_icon_size();

        for entry in actions {
            crate::app::preview_visibility::PreviewVisibility::set_mask_scope(
                ui.ctx(),
                entry.scope,
            );

            if moduwu_design::ghost_icon_button(
                ui,
                egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE,
                size,
                &tr_fmt("Reset {title}", &[("title", &card_title_text(entry.title))]),
            )
            .clicked()
            {
                match entry.kind {
                    VerticalCardKind::Adjustment {
                        show_visibility, ..
                    } => {
                        if show_visibility {
                            crate::app::preview_visibility::PreviewVisibility::show(
                                ui.ctx(),
                                entry.title,
                            );
                        }
                        Self::queue_vertical_card_action(
                            ui.ctx(),
                            entry.scope,
                            entry.title,
                            CardAction::Reset,
                        );
                    }
                    VerticalCardKind::Effect { .. } => Self::queue_vertical_effect_action(
                        ui.ctx(),
                        entry.scope,
                        entry.title,
                        EffectCardAction::Reset,
                    ),
                }
            }

            if let VerticalCardKind::Effect { enabled } = entry.kind {
                if moduwu_design::ghost_icon_button(
                    ui,
                    egui_phosphor::regular::TRASH,
                    size,
                    &tr_fmt(
                        "Remove {title}",
                        &[("title", &card_title_text(entry.title))],
                    ),
                )
                .clicked()
                {
                    Self::queue_vertical_effect_action(
                        ui.ctx(),
                        entry.scope,
                        entry.title,
                        EffectCardAction::Remove,
                    );
                }
                if moduwu_design::ghost_icon_toggle_button(
                    ui,
                    visibility_icon(enabled),
                    !enabled,
                    size,
                    &visibility_label(enabled, entry.title),
                )
                .clicked()
                {
                    Self::queue_vertical_effect_action(
                        ui.ctx(),
                        entry.scope,
                        entry.title,
                        EffectCardAction::Toggle,
                    );
                }
                continue;
            }

            let VerticalCardKind::Adjustment {
                show_visibility,
                show_mask_overlay_toggle,
            } = entry.kind
            else {
                unreachable!()
            };
            if show_mask_overlay_toggle {
                let forced = crate::app::preview_visibility::PreviewVisibility::mask_overlay_forced(
                    ui.ctx(),
                );
                if moduwu_design::ghost_icon_toggle_button(
                    ui,
                    egui_phosphor::regular::EYE,
                    forced,
                    size,
                    if forced {
                        tr("Use automatic mask overlay visibility")
                    } else {
                        tr("Always show the selected mask overlay")
                    },
                )
                .clicked()
                {
                    crate::app::preview_visibility::PreviewVisibility::toggle_mask_overlay(
                        ui.ctx(),
                    );
                }
            }

            if show_visibility {
                let visible = crate::app::preview_visibility::PreviewVisibility::visible(
                    ui.ctx(),
                    entry.title,
                );
                if moduwu_design::ghost_icon_toggle_button(
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
            CardHeaderControls {
                show_visibility: true,
                show_mask_overlay_toggle: false,
            },
            contents,
        )
    }

    /// An adjustment card without the preview eye, for adjustments the
    /// preview cannot temporarily hide.
    pub(super) fn adjustment_card_without_eye(
        ui: &mut Ui,
        title: &'static str,
        default_open: bool,
        foldable: bool,
        contents: impl FnOnce(&mut Ui),
    ) -> CardAction {
        Self::adjustment_card_controls(
            ui,
            title,
            default_open,
            foldable,
            true,
            CardHeaderControls {
                show_visibility: false,
                show_mask_overlay_toggle: false,
            },
            contents,
        )
    }

    pub(super) fn mask_properties_card(
        ui: &mut Ui,
        default_open: bool,
        foldable: bool,
        controls_enabled: bool,
        contents: impl FnOnce(&mut Ui),
    ) -> CardAction {
        Self::adjustment_card_controls(
            ui,
            "Mask Properties",
            default_open,
            foldable,
            controls_enabled,
            CardHeaderControls {
                show_visibility: false,
                show_mask_overlay_toggle: true,
            },
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
        header_controls: CardHeaderControls,
        contents: impl FnOnce(&mut Ui),
    ) -> CardAction {
        let CardHeaderControls {
            show_visibility,
            show_mask_overlay_toggle,
        } = header_controls;
        let visible = !show_visibility
            || crate::app::preview_visibility::PreviewVisibility::visible(ui.ctx(), title);
        let controls_enabled = controls_enabled && visible;
        let reveal = ui
            .ctx()
            .data_mut(|data| data.remove_temp::<bool>(card_reveal_id(title)))
            .unwrap_or(false);
        let mut action = CardAction::None;
        let card = moduwu_design::content_card(ui, |ui| {
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
                    Self::register_vertical_card_actions(
                        ui,
                        title,
                        show_visibility,
                        show_mask_overlay_toggle,
                    );
                    action = Self::take_pending_vertical_card_action(ui.ctx(), title);
                } else if foldable {
                    let mut header_clicked = false;
                    let mut header =
                        egui::collapsing_header::CollapsingState::load_with_default_open(
                            ui.ctx(),
                            ui.make_persistent_id("expanded"),
                            default_open,
                        );
                    if reveal {
                        header.set_open(true);
                    }
                    let mut header = header.show_header(ui, |ui| {
                        let available = ui.available_rect_before_wrap();
                        Self::adjustment_card_title(ui, title);
                        let card_actions = Self::card_actions(
                            ui,
                            title,
                            visible,
                            show_visibility,
                            show_mask_overlay_toggle,
                        );
                        action = card_actions.action;
                        let buttons = card_actions.button_rects();
                        // The built-in arrow already toggles.
                        header_clicked =
                            Self::card_header_background_clicked(ui, available, buttons);
                    });
                    if header_clicked {
                        header.toggle();
                    }
                    header.body_unindented(body);
                } else {
                    ui.horizontal(|ui| {
                        Self::adjustment_card_title(ui, title);
                        action = Self::card_actions(
                            ui,
                            title,
                            visible,
                            show_visibility,
                            show_mask_overlay_toggle,
                        )
                        .action;
                    });
                    body(ui);
                }
            });
        });
        moduwu_design::scroll_into_view_on_draw(ui, card_reveal_id(title), card.response.rect);
        moduwu_design::card_gap(ui);
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
