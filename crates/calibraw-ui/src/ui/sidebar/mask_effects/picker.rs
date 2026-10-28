//! Adding effects: the Add effect button and the picker it opens, which shows
//! every effect as an icon tile under its category.

use super::{add_effect_buttons, effect_description, egui, MaskEffect, Ui};
use crate::app::FILM_NEGATIVE_CARD_TITLE;
use crate::pipeline::{EffectComponent, FilmSource, MaskEffectCategory, MAX_EFFECT_COMPONENTS};

const TILES_PER_ROW: usize = 3;
/// Scales with the platform's control height, so tiles stay touch-sized.
const TILE_HEIGHT: f32 = moduwu_design::CONTROL_HEIGHT * 1.6;
const TILE_ICON_SIZE: f32 = 20.0;
const TILE_LABEL_SIZE: f32 = 11.5;
/// The picker's width when its button is narrower, as the touch Add tab is.
const PICKER_MIN_WIDTH: f32 = 264.0;

/// What an effect picker adds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui::sidebar) enum EffectPick {
    Effect(MaskEffect),
    /// Develops the photo as the positive of a scanned film negative. It is
    /// not an effect, but people look for it among them; only photo-wide
    /// pickers offer it ([`FilmNegativeTile`]).
    FilmNegative,
}

impl EffectPick {
    pub(in crate::ui::sidebar) const fn effect(self) -> Option<MaskEffect> {
        match self {
            Self::Effect(effect) => Some(effect),
            Self::FilmNegative => None,
        }
    }
}

/// Whether a picker shows the [`EffectPick::FilmNegative`] tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui::sidebar) enum FilmNegativeTile {
    /// Mask pickers: a mask cannot develop part of a photo as a negative.
    Hidden,
    Offered,
    /// The photo is developed as a negative already.
    Added,
}

impl FilmNegativeTile {
    /// The tile of a photo-wide picker for a photo read as `film`.
    pub(in crate::ui::sidebar) fn for_film(film: &FilmSource) -> Self {
        if film.is_positive() {
            Self::Offered
        } else {
            Self::Added
        }
    }
}

/// The add controls of an Effects card: a quick-add button per effect in
/// `featured`, then [`add_effect_button`]. Returns what to add to
/// `components` or the photo.
pub(in crate::ui::sidebar) fn add_effect_controls(
    ui: &mut Ui,
    featured: &[MaskEffect],
    components: &[EffectComponent],
    film_negative: FilmNegativeTile,
) -> Option<EffectPick> {
    let featured_added = add_effect_buttons(ui, featured, components).map(EffectPick::Effect);
    ui.add_space(moduwu_design::SPACE_XS);
    let picked = add_effect_button(ui, components, film_negative);
    featured_added.or(picked)
}

/// A full-width Add effect button that opens [`effect_picker_popup`].
/// Disabled once `components` is full. Returns what was picked.
pub(in crate::ui::sidebar) fn add_effect_button(
    ui: &mut Ui,
    components: &[EffectComponent],
    film_negative: FilmNegativeTile,
) -> Option<EffectPick> {
    let full = components.len() >= MAX_EFFECT_COMPONENTS;
    let response = ui
        .add_enabled_ui(!full, |ui| {
            moduwu_design::full_width_button(
                ui,
                format!("{}  Add effect", egui_phosphor::regular::PLUS),
            )
        })
        .inner
        .on_disabled_hover_text(format!(
            "At most {MAX_EFFECT_COMPONENTS} effects can be added"
        ));
    effect_picker_popup(&response, components, film_negative)
}

/// Opens [`effect_picker`] when `button` is clicked, as wide as the button
/// and at least [`PICKER_MIN_WIDTH`]. It opens on the side of the button with
/// more room, and scrolls if it is taller than that room. Returns what was
/// picked.
pub(in crate::ui::sidebar) fn effect_picker_popup(
    button: &egui::Response,
    components: &[EffectComponent],
    film_negative: FilmNegativeTile,
) -> Option<EffectPick> {
    let screen = button.ctx.content_rect();
    let gap = moduwu_design::SPACE_XS;
    let room_above = button.rect.top() - screen.top();
    let room_below = screen.bottom() - button.rect.bottom();
    // Chosen here rather than by egui, which learns the popup's height only
    // after showing it once and would move it on the second frame.
    let align = if room_below >= room_above {
        egui::RectAlign::BOTTOM_START
    } else {
        egui::RectAlign::TOP_START
    };
    let size = egui::vec2(
        button.rect.width().max(PICKER_MIN_WIDTH),
        room_above.max(room_below) - 2.0 * gap,
    );
    // Not a menu: menus draw idle buttons flat, and the tiles keep their frame.
    egui::Popup::from_toggle_button_response(button)
        .align(align)
        .align_alternatives(&[])
        .gap(gap)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| effect_picker(ui, components, film_negative, size))
        .and_then(|response| response.inner)
}

/// One tile of the picker.
struct Tile {
    pick: EffectPick,
    label: &'static str,
    icon: &'static str,
    help: Option<&'static str>,
    added: bool,
    enabled: bool,
}

/// Every effect as a tile under its category's heading, filling a popup of
/// at most `size`, to add one to `components`; the film negative tile leads
/// Film & Finish unless `film_negative` hides it. An effect already in
/// `components` shows a check mark and is disabled; once `components` is
/// full, every effect is disabled. Returns the clicked tile's pick and
/// closes the popup.
fn effect_picker(
    ui: &mut Ui,
    components: &[EffectComponent],
    film_negative: FilmNegativeTile,
    size: egui::Vec2,
) -> Option<EffectPick> {
    let full = components.len() >= MAX_EFFECT_COMPONENTS;
    // The popup frame's margin and border lie outside the contents.
    let size = size
        - ui.spacing().menu_margin.sum()
        - egui::Vec2::splat(2.0 * ui.visuals().window_stroke.width);
    ui.set_width(size.x);
    // The sidebars' solid scrollbar, so a picker taller than its room shows
    // that more effects follow.
    let mut scroll = egui::style::ScrollStyle::solid();
    scroll.bar_width = 7.0;
    scroll.bar_inner_margin = 7.0;
    ui.spacing_mut().scroll = scroll;
    let gap = moduwu_design::SPACE_XS;
    let mut picked = None;
    // A popup offers its contents only the height it had the frame before,
    // which would keep the scroll area at its first height. The minimum lets
    // it grow to its contents, up to `size.y`.
    egui::ScrollArea::vertical()
        .max_height(size.y)
        .min_scrolled_height(size.y)
        .show(ui, |ui| {
            // Narrower than the popup while the scrollbar shows.
            let tile_size = egui::vec2(
                (ui.available_width() - gap * (TILES_PER_ROW - 1) as f32) / TILES_PER_ROW as f32,
                TILE_HEIGHT,
            );
            ui.spacing_mut().item_spacing = egui::vec2(gap, gap);
            for (index, category) in MaskEffectCategory::ALL.into_iter().enumerate() {
                if index > 0 {
                    ui.add_space(moduwu_design::SPACE_SM);
                }
                category_heading(ui, category.label());
                let mut tiles = Vec::new();
                if category == MaskEffectCategory::FilmAndFinish
                    && film_negative != FilmNegativeTile::Hidden
                {
                    let added = film_negative == FilmNegativeTile::Added;
                    tiles.push(Tile {
                        pick: EffectPick::FilmNegative,
                        label: FILM_NEGATIVE_CARD_TITLE,
                        icon: egui_phosphor::regular::FILM_STRIP,
                        help: Some("Develop a scanned film negative as its positive."),
                        added,
                        enabled: !added,
                    });
                }
                tiles.extend(
                    MaskEffect::ALL
                        .into_iter()
                        .filter(|effect| effect.category() == Some(category))
                        .map(|effect| {
                            let added = components
                                .iter()
                                .any(|component| component.effect == effect);
                            Tile {
                                pick: EffectPick::Effect(effect),
                                label: effect.label(),
                                icon: effect_icon(effect),
                                help: effect_description(effect),
                                added,
                                enabled: !added && !full,
                            }
                        }),
                );
                for row in tiles.chunks(TILES_PER_ROW) {
                    ui.horizontal(|ui| {
                        for tile in row {
                            let mut response = effect_tile(ui, tile, tile_size)
                                .on_disabled_hover_text(if tile.added {
                                    format!("{} is already added", tile.label)
                                } else {
                                    format!("At most {MAX_EFFECT_COMPONENTS} effects can be added")
                                });
                            if let Some(help) = tile.help {
                                response = response.on_hover_text(help);
                            }
                            if response.clicked() {
                                picked = Some(tile.pick);
                            }
                        }
                    });
                }
            }
        });
    if picked.is_some() {
        ui.close();
    }
    picked
}

/// A category's name followed by a hairline to the right edge. Painted into
/// one rect of the text's height: a row of widgets lays out at another height
/// in the popup's sizing pass, which would misplace a popup opening upwards.
fn category_heading(ui: &mut Ui, label: &str) {
    let galley = egui::WidgetText::from(egui::RichText::new(label).small()).into_galley(
        ui,
        Some(egui::TextWrapMode::Extend),
        f32::INFINITY,
        egui::TextStyle::Small,
    );
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), galley.size().y),
        egui::Sense::hover(),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, label));
    let line_start = rect.left() + galley.size().x + ui.spacing().item_spacing.x;
    let painter = ui.painter();
    painter.hline(
        line_start..=rect.right(),
        rect.center().y,
        ui.visuals().widgets.noninteractive.bg_stroke,
    );
    painter.galley(rect.left_top(), galley, ui.visuals().weak_text_color());
}

/// A button showing `tile`'s icon above its name, framed like the quick-add
/// buttons. An added tile shows a check mark and reports itself selected to
/// assistive technology.
fn effect_tile(ui: &mut Ui, tile: &Tile, size: egui::Vec2) -> egui::Response {
    ui.add_enabled_ui(tile.enabled, |ui| {
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::WidgetType::Button,
                ui.is_enabled(),
                tile.added,
                tile.label,
            )
        });
        if !ui.is_rect_visible(rect) {
            return response;
        }
        let visuals = moduwu_design::interaction_visuals(ui, &response, false);
        let painter = ui.painter();
        painter.rect(
            rect,
            moduwu_design::CARD_RADIUS,
            visuals.weak_fill,
            visuals.stroke,
            egui::StrokeKind::Inside,
        );

        let padding = ui.spacing().button_padding.x;
        let label = egui::WidgetText::from(egui::RichText::new(tile.label).size(TILE_LABEL_SIZE))
            .into_galley(
                ui,
                Some(egui::TextWrapMode::Truncate),
                rect.width() - 2.0 * padding,
                egui::TextStyle::Button,
            );
        // The icon and the name are centred as one block.
        let icon_gap = moduwu_design::SPACE_XXS;
        let top = rect.center().y - (TILE_ICON_SIZE + icon_gap + label.size().y) * 0.5;
        painter.text(
            egui::pos2(rect.center().x, top + TILE_ICON_SIZE * 0.5),
            egui::Align2::CENTER_CENTER,
            tile.icon,
            egui::FontId::proportional(TILE_ICON_SIZE),
            visuals.foreground,
        );
        painter.galley(
            egui::pos2(
                rect.center().x - label.size().x * 0.5,
                top + TILE_ICON_SIZE + icon_gap,
            ),
            label,
            visuals.foreground,
        );
        if tile.added {
            painter.text(
                rect.right_top() + egui::vec2(-padding, padding),
                egui::Align2::RIGHT_TOP,
                egui_phosphor::regular::CHECK,
                egui::FontId::proportional(TILE_LABEL_SIZE),
                visuals.foreground,
            );
        }
        response
    })
    .inner
}

fn effect_icon(effect: MaskEffect) -> &'static str {
    use egui_phosphor::regular;

    match effect {
        MaskEffect::Adjustment => regular::SLIDERS_HORIZONTAL,
        MaskEffect::Blur => regular::DROP_HALF,
        MaskEffect::LensBlur => regular::APERTURE,
        MaskEffect::MotionBlur => regular::ARROW_FAT_LINES_RIGHT,
        MaskEffect::RadialBlur => regular::SPIRAL,
        MaskEffect::Sharpen => regular::TRIANGLE,
        MaskEffect::TiltShift => regular::ARROWS_IN_LINE_VERTICAL,
        MaskEffect::Glow => regular::SUN,
        MaskEffect::LightRays => regular::SUN_HORIZON,
        MaskEffect::Neon => regular::LIGHTNING,
        MaskEffect::Relight => regular::LIGHTBULB,
        MaskEffect::EdgeGlow => regular::SCRIBBLE,
        MaskEffect::Pixelate => regular::CHECKERBOARD,
        MaskEffect::Fog => regular::CLOUD_FOG,
        MaskEffect::Smoke => regular::WIND,
        MaskEffect::Grain => regular::DOTS_NINE,
        MaskEffect::Halation => regular::CIRCLE_DASHED,
        MaskEffect::Vignette => regular::VIGNETTE,
    }
}
