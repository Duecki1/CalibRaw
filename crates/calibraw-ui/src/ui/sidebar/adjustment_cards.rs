use super::*;
use crate::i18n::{tr, tr_fmt};
use crate::pipeline::CardSwitch;

/// What a card's header asked of its handler.
#[derive(Clone, Copy, Default)]
pub(super) enum CardAction {
    #[default]
    None,
    /// Flip the card's switch.
    Toggle,
    /// Reset the card's settings and switch it on.
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
        on: bool,
        /// Has its own reset; a sub-mask's is its group's.
        reset: bool,
        show_mask_overlay_toggle: bool,
    },
    Effect {
        enabled: bool,
        /// Has a reset; a legacy effect's card has none.
        reset: bool,
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
    /// The card's switch.
    on: bool,
    show_mask_overlay_toggle: bool,
    /// Takes part in closing other cards when one opens ([`single_open_cards`]).
    single_open: bool,
    /// Puts its reset in the shared footer on portrait screens, where cards
    /// have no header; its switch goes there in any case. A card without it
    /// relies on another card's reset.
    footer_reset: bool,
}

impl CardAction {
    /// Applies the action to the photo-wide card `card`; `reset` resets its
    /// settings. Returns whether the edit changed.
    pub(super) fn apply_to_card(
        self,
        exposure: &mut ExposureParams,
        card: CardSwitch,
        reset: impl FnOnce(&mut ExposureParams),
    ) -> bool {
        match self {
            Self::None => false,
            Self::Toggle => exposure.set_card_on(card, !exposure.card_on(card)),
            Self::Reset => {
                reset(exposure);
                exposure.set_card_on(card, true);
                true
            }
        }
    }

    pub(super) fn apply(self, exposure: &mut ExposureParams, group: AdjustmentGroup) -> bool {
        self.apply_to_card(exposure, CardSwitch::of_group(group), |exposure| {
            exposure.reset_group(group)
        })
    }

    /// Resets `group` to the photo-specific starting values in `defaults`.
    pub(super) fn apply_from(
        self,
        exposure: &mut ExposureParams,
        defaults: &ExposureParams,
        group: AdjustmentGroup,
    ) -> bool {
        self.apply_to_card(exposure, CardSwitch::of_group(group), |exposure| {
            exposure.copy_group_from(defaults, group)
        })
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

/// The name an adjustment card shows. `title` stays the card's key (reveal
/// and pending footer actions use it), so only the name is
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
        "Calibration" => tr("Calibration"),
        "Mask" => tr("Mask"),
        "Sub-mask" => tr("Sub-mask"),
        "Subject refinement" => tr("Subject refinement"),
        "Lens Corrections" => tr("Lens Corrections"),
        "Film negative" => tr("Film negative"),
        "Camera match" => tr("Camera match"),
        "LUT" => tr("LUT"),
        _ => tr(title),
    }
}

/// A card's switch, showing `on`. Its title names it for assistive
/// technology; on a header the title shows beside it. A click on the returned
/// response asks to flip it.
pub(super) fn card_switch(ui: &mut Ui, on: bool, title: &str) -> egui::Response {
    let mut on = on;
    moduwu_design::toggle_switch(ui, &mut on, title)
}

/// The forced mask overlay's eye, which the mask cards keep.
fn mask_overlay_toggle(ui: &mut Ui, size: egui::Vec2) -> egui::Response {
    let forced = crate::app::preview_visibility::PreviewVisibility::mask_overlay_forced(ui.ctx());
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
        crate::app::preview_visibility::PreviewVisibility::toggle_mask_overlay(ui.ctx());
    }
    response
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

    /// Whether the header row `available` was clicked outside its `buttons`,
    /// which may come in any order. The click areas are added after the
    /// buttons, so egui would give them a click on a button they overlap;
    /// they only span the gaps between buttons.
    pub(super) fn card_header_background_clicked(
        ui: &mut Ui,
        available: egui::Rect,
        buttons: &[egui::Rect],
    ) -> bool {
        let button_top = buttons[0].top();
        let button_bottom = buttons[0].bottom();
        // Right-to-left headers add their buttons rightmost first.
        let mut buttons = buttons.to_vec();
        buttons.sort_by(|a, b| a.left().total_cmp(&b.left()));
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
        on: bool,
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
            let overlay = show_mask_overlay_toggle.then(|| mask_overlay_toggle(ui, size));
            let switch = card_switch(ui, on, card_title_text(title));
            if switch.clicked() {
                action = CardAction::Toggle;
            }
            match overlay {
                Some(overlay) => ([switch.rect, overlay.rect, reset.rect], 3),
                None => ([switch.rect, reset.rect, egui::Rect::NOTHING], 2),
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
        on: bool,
        reset: bool,
        show_mask_overlay_toggle: bool,
    ) {
        Self::register_vertical_actions(
            ui,
            title,
            VerticalCardKind::Adjustment {
                on,
                reset,
                show_mask_overlay_toggle,
            },
        );
    }

    pub(super) fn register_vertical_effect_actions(
        ui: &Ui,
        title: &'static str,
        enabled: bool,
        reset: bool,
    ) {
        Self::register_vertical_actions(ui, title, VerticalCardKind::Effect { enabled, reset });
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
            let title = card_title_text(entry.title);
            let reset = match entry.kind {
                VerticalCardKind::Adjustment { reset, .. } => reset,
                VerticalCardKind::Effect { reset, .. } => reset,
            };
            if reset
                && moduwu_design::ghost_icon_button(
                    ui,
                    egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE,
                    size,
                    &tr_fmt("Reset {title}", &[("title", &title)]),
                )
                .clicked()
            {
                match entry.kind {
                    VerticalCardKind::Adjustment { .. } => Self::queue_vertical_card_action(
                        ui.ctx(),
                        entry.scope,
                        entry.title,
                        CardAction::Reset,
                    ),
                    VerticalCardKind::Effect { .. } => Self::queue_vertical_effect_action(
                        ui.ctx(),
                        entry.scope,
                        entry.title,
                        EffectCardAction::Reset,
                    ),
                }
            }

            match entry.kind {
                VerticalCardKind::Effect { enabled, .. } => {
                    if moduwu_design::ghost_icon_button(
                        ui,
                        egui_phosphor::regular::TRASH,
                        size,
                        &tr_fmt("Remove {title}", &[("title", &title)]),
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
                    if card_switch(ui, enabled, title).clicked() {
                        Self::queue_vertical_effect_action(
                            ui.ctx(),
                            entry.scope,
                            entry.title,
                            EffectCardAction::Toggle,
                        );
                    }
                }
                VerticalCardKind::Adjustment {
                    on,
                    show_mask_overlay_toggle,
                    ..
                } => {
                    if show_mask_overlay_toggle {
                        mask_overlay_toggle(ui, size);
                    }
                    if card_switch(ui, on, title).clicked() {
                        Self::queue_vertical_card_action(
                            ui.ctx(),
                            entry.scope,
                            entry.title,
                            CardAction::Toggle,
                        );
                    }
                }
            }
        }

        crate::app::preview_visibility::PreviewVisibility::set_mask_scope(ui.ctx(), None);
    }

    /// A card with a switch, `on`, in its header; off greys out its body.
    /// Its handler applies the returned action, flipping the switch and
    /// resetting.
    pub(super) fn adjustment_card(
        ui: &mut Ui,
        title: &'static str,
        default_open: bool,
        foldable: bool,
        on: bool,
        contents: impl FnOnce(&mut Ui),
    ) -> CardAction {
        Self::adjustment_card_controls(
            ui,
            title,
            default_open,
            foldable,
            CardHeaderControls {
                on,
                show_mask_overlay_toggle: false,
                single_open: true,
                footer_reset: true,
            },
            contents,
        )
    }

    /// The selected mask group's card, switched by whether the mask applies.
    /// On portrait screens, where cards have no header, its footer reset
    /// also stands for the sub-mask card.
    pub(super) fn mask_group_card(
        ui: &mut Ui,
        on: bool,
        foldable: bool,
        contents: impl FnOnce(&mut Ui),
    ) -> CardAction {
        Self::adjustment_card_controls(
            ui,
            "Mask",
            true,
            foldable,
            CardHeaderControls {
                on,
                show_mask_overlay_toggle: true,
                single_open: false,
                footer_reset: true,
            },
            contents,
        )
    }

    /// The selected sub-mask's card, below its group's, switched by whether
    /// the sub-mask takes part in the mask.
    pub(super) fn submask_card(
        ui: &mut Ui,
        on: bool,
        foldable: bool,
        contents: impl FnOnce(&mut Ui),
    ) -> CardAction {
        Self::adjustment_card_controls(
            ui,
            "Sub-mask",
            true,
            foldable,
            CardHeaderControls {
                on,
                show_mask_overlay_toggle: false,
                single_open: false,
                footer_reset: false,
            },
            contents,
        )
    }

    /// Whether cards drop their header and put their actions in the shared
    /// footer: on portrait screens.
    pub(super) fn cards_use_footer(ctx: &egui::Context) -> bool {
        let viewport = ctx.content_rect();
        viewport.height() > viewport.width()
    }

    /// The switch only greys out the card body; the header (fold, reset, switch) stays
    /// interactive so a card that is off can be switched on again.
    fn adjustment_card_controls(
        ui: &mut Ui,
        title: &'static str,
        default_open: bool,
        foldable: bool,
        header_controls: CardHeaderControls,
        contents: impl FnOnce(&mut Ui),
    ) -> CardAction {
        let CardHeaderControls {
            on,
            show_mask_overlay_toggle,
            single_open,
            footer_reset,
        } = header_controls;
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
                    ui.add_enabled_ui(on, contents);
                };
                if Self::cards_use_footer(ui.ctx()) {
                    // Vertical-screen cards deliberately have no header. Their switch and reset
                    // are registered here and rendered outside the cards in the shared sidebar
                    // footer alongside the other mobile actions.
                    body(ui);
                    Self::register_vertical_card_actions(
                        ui,
                        title,
                        on,
                        footer_reset,
                        show_mask_overlay_toggle,
                    );
                    action = Self::take_pending_vertical_card_action(ui.ctx(), title);
                } else if foldable {
                    let mut header_clicked = false;
                    let ctx = ui.ctx().clone();
                    let mut header =
                        egui::collapsing_header::CollapsingState::load_with_default_open(
                            ui.ctx(),
                            ui.make_persistent_id("expanded"),
                            default_open,
                        );
                    let was_open = header.is_open();
                    if reveal {
                        header.set_open(true);
                    }
                    let mut header = header.show_header(ui, |ui| {
                        let available = ui.available_rect_before_wrap();
                        Self::adjustment_card_title(ui, title);
                        let card_actions =
                            Self::card_actions(ui, title, on, show_mask_overlay_toggle);
                        action = card_actions.action;
                        let buttons = card_actions.button_rects();
                        // The built-in arrow already toggles.
                        header_clicked =
                            Self::card_header_background_clicked(ui, available, buttons);
                    });
                    if header_clicked {
                        header.toggle();
                    }
                    if single_open {
                        let open = single_open_cards::stays_open(
                            &ctx,
                            card_reveal_id(title),
                            was_open,
                            reveal,
                            header.is_open(),
                        );
                        header.set_open(open);
                    }
                    header.body_unindented(body);
                } else {
                    ui.horizontal(|ui| {
                        Self::adjustment_card_title(ui, title);
                        action = Self::card_actions(ui, title, on, show_mask_overlay_toggle).action;
                    });
                    body(ui);
                }
            });
        });
        single_open_cards::keep_in_view(ui, card_reveal_id(title), card.response.rect);
        moduwu_design::scroll_into_view_on_draw(ui, card_reveal_id(title), card.response.rect);
        moduwu_design::card_gap(ui);
        action
    }
}
