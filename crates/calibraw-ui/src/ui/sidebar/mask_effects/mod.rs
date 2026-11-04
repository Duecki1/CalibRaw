mod controls;
pub(super) mod fog;
mod generated;
mod instances;
pub(super) mod light_rays;
mod picker;
pub(super) mod radial_blur;
pub(super) mod rain;
pub(super) mod relight;
pub(super) mod tilt_shift;
pub(super) mod vignette;

use crate::i18n::{tr, tr_fmt};
pub(crate) use controls::EffectFrame;
use controls::{effect_details, effect_position, pattern_seed, PositionSpace};
pub(super) use generated::generated_card;
pub(crate) use instances::InstanceSelection;
pub(super) use picker::{
    add_effect_button, add_effect_controls, effect_picker_popup, EffectPick, FilmNegativeTile,
};

use super::{egui, MaskEffect, Ui};
use crate::pipeline::effect_params::{ColorParamSpec, ToggleSpec};
use crate::pipeline::{effect_places, EffectComponent, EffectSettings, MAX_EFFECT_COMPONENTS};

pub(super) use crate::ui::components::adjustment_slider::{float_param_angle, float_param_slider};

fn effect_card<Settings>(
    ui: &mut Ui,
    effect: MaskEffect,
    settings: &mut Settings,
    enabled: &mut bool,
    remove: &mut bool,
    body: impl FnOnce(&mut Ui, &mut Settings) -> bool,
) -> bool
where
    Settings: EffectSettings,
{
    let body = |ui: &mut Ui, settings: &mut Settings| {
        if effect.is_experimental() {
            ui.weak(tr("Experimental"));
        }
        body(ui, settings)
    };
    let mut changed = false;
    let mut reset = false;
    let card = moduwu_design::content_card(ui, |ui| {
        ui.push_id(effect.label(), |ui| {
            ui.spacing_mut().interact_size.y = ui.spacing().interact_size.y.max(26.0);
            if ui.ctx().content_rect().height() > ui.ctx().content_rect().width() {
                match super::Sidebar::take_pending_vertical_effect_action(ui.ctx(), effect.label())
                {
                    super::adjustment_cards::EffectCardAction::None => {}
                    super::adjustment_cards::EffectCardAction::Reset => reset = true,
                    super::adjustment_cards::EffectCardAction::Toggle => {
                        *enabled = !*enabled;
                        changed = true;
                    }
                    super::adjustment_cards::EffectCardAction::Remove => *remove = true,
                }
                if !*remove {
                    ui.add_enabled_ui(*enabled, |ui| changed |= body(ui, settings));
                    super::Sidebar::register_vertical_effect_actions(ui, effect.label(), *enabled);
                }
            } else {
                let mut header_clicked = false;
                let ctx = ui.ctx().clone();
                let mut header = egui::collapsing_header::CollapsingState::load_with_default_open(
                    ui.ctx(),
                    ui.make_persistent_id("expanded"),
                    true,
                )
                .show_header(ui, |ui| {
                    let available = ui.available_rect_before_wrap();
                    super::Sidebar::adjustment_card_title(ui, effect.label());
                    let buttons = ui
                        .with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let size = egui::vec2(26.0, 26.0);
                            let reset_button = moduwu_design::ghost_icon_button(
                                ui,
                                egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE,
                                size,
                                &tr_fmt("Reset {title}", &[("title", &tr(effect.label()))]),
                            );
                            reset = reset_button.clicked();
                            let remove_button = moduwu_design::ghost_icon_button(
                                ui,
                                egui_phosphor::regular::TRASH,
                                size,
                                &tr_fmt("Remove {title}", &[("title", &tr(effect.label()))]),
                            );
                            *remove = remove_button.clicked();
                            let icon = if *enabled {
                                egui_phosphor::regular::EYE_SLASH
                            } else {
                                egui_phosphor::regular::EYE
                            };
                            let visibility_button = moduwu_design::ghost_icon_toggle_button(
                                ui,
                                icon,
                                !*enabled,
                                size,
                                &if *enabled {
                                    tr_fmt("Hide {title}", &[("title", &tr(effect.label()))])
                                } else {
                                    tr_fmt("Show {title}", &[("title", &tr(effect.label()))])
                                },
                            );
                            if visibility_button.clicked() {
                                *enabled = !*enabled;
                                changed = true;
                            }
                            [
                                visibility_button.rect,
                                remove_button.rect,
                                reset_button.rect,
                            ]
                        })
                        .inner;
                    header_clicked =
                        super::Sidebar::card_header_background_clicked(ui, available, &buttons);
                });
                if header_clicked {
                    header.toggle();
                }
                if header.is_open() {
                    note_open_effect_card(&ctx, effect);
                }
                header.body_unindented(|ui| {
                    ui.add_enabled_ui(*enabled, |ui| changed |= body(ui, settings));
                });
            }
        });
    });
    moduwu_design::scroll_into_view_on_draw(
        ui,
        effect_card_scroll_target(effect),
        card.response.rect,
    );
    moduwu_design::card_gap(ui);
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

/// The scroll-into-view target of `effect`'s card
/// ([`moduwu_design::scroll_into_view_on_draw`]). Each effect has at most one
/// card in the effect list the sidebar shows.
pub(super) fn effect_card_scroll_target(effect: MaskEffect) -> egui::Id {
    egui::Id::new("calibraw-effect-card-scroll").with(effect.shader_id())
}

fn open_effect_card_id(effect: MaskEffect) -> egui::Id {
    egui::Id::new("calibraw-open-effect-card").with(effect.shader_id())
}

/// Records that `effect`'s card is drawn expanded in this pass. Each effect
/// has at most one card in the effect list the sidebar shows.
fn note_open_effect_card(ctx: &egui::Context, effect: MaskEffect) {
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|data| data.insert_temp(open_effect_card_id(effect), pass));
}

/// Whether the sidebar drew `effect`'s card expanded earlier in this pass.
/// The desktop sidebar panel is drawn before the preview, so on-canvas
/// controls can follow the card without a frame of lag.
pub(crate) fn effect_card_open(ctx: &egui::Context, effect: MaskEffect) -> bool {
    ctx.data(|data| data.get_temp::<u64>(open_effect_card_id(effect)))
        == Some(ctx.cumulative_pass_nr())
}

fn apply_card_action<Settings>(
    action: super::adjustment_cards::CardAction,
    settings: &mut Settings,
) -> bool
where
    Settings: EffectSettings,
{
    use super::adjustment_cards::CardAction;

    match action {
        CardAction::None | CardAction::Toggle => false,
        CardAction::Reset => {
            *settings = Settings::initial();
            true
        }
    }
}

/// Effects that once were Effects-card sliders. The cards offer them as
/// quick-add buttons ([`add_effect_controls`]).
pub(super) const FINISH_EFFECTS: [MaskEffect; 4] = [
    MaskEffect::Glow,
    MaskEffect::Halation,
    MaskEffect::Grain,
    MaskEffect::Vignette,
];

/// One button per effect in `effects`, in a grid of up to two columns, that
/// adds the effect to `components`. Returns the effect whose button was
/// clicked. An effect already in `components`, or any effect once
/// `components` is full, has its button disabled.
fn add_effect_buttons(
    ui: &mut Ui,
    effects: &[MaskEffect],
    components: &[EffectComponent],
) -> Option<MaskEffect> {
    const COLUMNS: usize = 2;
    let full = effect_places(components) >= MAX_EFFECT_COMPONENTS;
    let mut added = None;
    ui.columns(effects.len().clamp(1, COLUMNS), |columns| {
        let column_count = columns.len();
        for (index, &effect) in effects.iter().enumerate() {
            let ui = &mut columns[index % column_count];
            let present = components
                .iter()
                .any(|component| component.effect == effect);
            let label = format!("{}  {}", egui_phosphor::regular::PLUS, tr(effect.label()));
            let response = ui
                .add_enabled_ui(!present && !full, |ui| {
                    moduwu_design::full_width_button(ui, label)
                })
                .inner
                .on_hover_text(if effect.is_experimental() {
                    tr_fmt(
                        "Add {effect} (experimental)",
                        &[("effect", &tr(effect.label()))],
                    )
                } else {
                    tr_fmt("Add {effect}", &[("effect", &tr(effect.label()))])
                })
                .on_disabled_hover_text(if present {
                    tr_fmt(
                        "{effect} is already added",
                        &[("effect", &tr(effect.label()))],
                    )
                } else {
                    tr_fmt(
                        "At most {max} effects can be added",
                        &[("max", &MAX_EFFECT_COMPONENTS)],
                    )
                });
            if response.clicked() {
                added = Some(effect);
            }
        }
    });
    added
}

/// Tooltip for an effect card's title.
pub(super) fn effect_description(effect: MaskEffect) -> Option<&'static str> {
    effect.description().map(tr)
}

fn effect_color(
    ui: &mut Ui,
    id_salt: impl egui::AsIdSalt,
    color: &mut [f32; 3],
    spec: ColorParamSpec,
) -> bool {
    crate::ui::components::color_picker::sidebar_color_picker(
        ui,
        id_salt,
        color,
        tr(spec.label),
        tr(spec.tooltip),
    )
}

/// A switch with its help as the tooltip.
fn effect_toggle(ui: &mut Ui, value: &mut bool, spec: ToggleSpec) -> bool {
    moduwu_design::toggle_with_help(ui, value, tr(spec.label), tr(spec.help)).changed()
}
