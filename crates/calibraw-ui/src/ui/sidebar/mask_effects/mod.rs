pub(super) mod blur;
mod controls;
pub(super) mod edge_glow;
pub(super) mod fog;
pub(super) mod glow;
pub(super) mod grain;
pub(super) mod halation;
pub(super) mod lens_blur;
pub(super) mod light_rays;
pub(super) mod motion_blur;
pub(super) mod neon;
mod picker;
pub(super) mod pixelate;
pub(super) mod radial_blur;
pub(super) mod relight;
pub(super) mod sharpen;
pub(super) mod smoke;
pub(super) mod tilt_shift;
pub(super) mod vignette;

pub(crate) use controls::EffectFrame;
use controls::{effect_details, effect_position, image_lights_toggle, pattern_seed, PositionSpace};
pub(super) use picker::{
    add_effect_button, add_effect_controls, effect_picker_popup, EffectPick, FilmNegativeTile,
};

use super::{egui, MaskEffect, Ui};
use crate::pipeline::effect_params::ColorParamSpec;
use crate::pipeline::{EffectComponent, InitialEffectSettings, MAX_EFFECT_COMPONENTS};

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
    Settings: InitialEffectSettings,
{
    let body = |ui: &mut Ui, settings: &mut Settings| {
        if effect.is_experimental() {
            ui.weak("Experimental");
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
                                &format!("Reset {}", effect.label()),
                            );
                            reset = reset_button.clicked();
                            let remove_button = moduwu_design::ghost_icon_button(
                                ui,
                                egui_phosphor::regular::TRASH,
                                size,
                                &format!("Remove {}", effect.label()),
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
                                &format!(
                                    "{} {}",
                                    if *enabled { "Hide" } else { "Show" },
                                    effect.label()
                                ),
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
    Settings: InitialEffectSettings,
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
    let full = components.len() >= MAX_EFFECT_COMPONENTS;
    let mut added = None;
    ui.columns(effects.len().clamp(1, COLUMNS), |columns| {
        let column_count = columns.len();
        for (index, &effect) in effects.iter().enumerate() {
            let ui = &mut columns[index % column_count];
            let present = components
                .iter()
                .any(|component| component.effect == effect);
            let label = format!("{}  {}", egui_phosphor::regular::PLUS, effect.label());
            let response = ui
                .add_enabled_ui(!present && !full, |ui| {
                    moduwu_design::full_width_button(ui, label)
                })
                .inner
                .on_hover_text(if effect.is_experimental() {
                    format!("Add {} (experimental)", effect.label())
                } else {
                    format!("Add {}", effect.label())
                })
                .on_disabled_hover_text(if present {
                    format!("{} is already added", effect.label())
                } else {
                    format!("At most {MAX_EFFECT_COMPONENTS} effects can be added")
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
    match effect {
        MaskEffect::Blur => Some(
            "Soften distracting detail. The mask controls where the blurred image is blended in.",
        ),
        MaskEffect::LensBlur => Some(
            "Defocus backgrounds with aperture-shaped bokeh. The mask controls the blend; use a soft edge for a gradual transition.",
        ),
        MaskEffect::MotionBlur => Some(
            "Add directional movement. Distance sets the trail length; the mask controls where the blur appears.",
        ),
        MaskEffect::RadialBlur => Some(
            "Zoom toward a center or spin around it. Position is relative to the full image; the mask controls the blend.",
        ),
        MaskEffect::TiltShift => Some(
            "Keep a band in focus and soften its surroundings. Position is relative to the full image. Use a Fullscreen mask for an uninterrupted focus transition.",
        ),
        MaskEffect::Sharpen => Some(
            "Sharpen fine detail inside the mask with its own radius, detail and masking. It adds to the Detail card's sharpening.",
        ),
        MaskEffect::EdgeGlow => Some(
            "Light up image contours with a soft halo. Both the outlines and their glow stay inside the mask.",
        ),
        MaskEffect::Glow => Some(
            "Spread colored light from bright pixels. The mask selects the highlights that emit light; their glow can extend beyond the selection.",
        ),
        MaskEffect::LightRays => Some(
            "Place the source relative to the full image, then paint its openings. The mask shapes emitting regions; light shafts extend beyond them.",
        ),
        MaskEffect::Relight => Some(
            "Add a light that shapes the scene in depth. Position it like Light Rays; scene depth is generated automatically. Turn on Shadows to let nearer objects block the light. The sky and anything the depth map places at its far limit stay unlit. The mask controls where the light falls.",
        ),
        MaskEffect::Neon => Some(
            "Trace image edges with colored light inside the mask. Original image keeps the photo visible behind the lines; the glow also stays inside the mask.",
        ),
        MaskEffect::Pixelate => Some(
            "Create a graphic mosaic. Block size sets the tile scale; the mask controls where the result appears.",
        ),
        MaskEffect::Fog => Some(
            "Add atmospheric haze through the mask. Scene depth builds fog with distance; turn it off for an even-distance veil.",
        ),
        MaskEffect::Smoke => Some(
            "Layer drifting plumes through the mask. The pattern stays anchored to the full image as you edit the selection.",
        ),
        MaskEffect::Grain => Some(
            "Add photographic texture. Grain stays anchored to the image; the mask controls its local strength.",
        ),
        MaskEffect::Halation => Some(
            "Give bright highlights a warm film halo inside the mask. Nearby highlights outside the selection can cast a halo into it. Use Fullscreen for a whole-image finish.",
        ),
        MaskEffect::Vignette => Some(
            "Darken edges with negative amounts or brighten them with positive amounts. The center follows the cropped, rotated frame; the mask controls the blend.",
        ),
        MaskEffect::Adjustment => None,
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
