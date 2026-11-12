//! Adding effects: the Add effect button and the picker it opens, which shows
//! every effect family as an icon tile under its category.

use super::{add_effect_buttons, effect_description, egui, MaskEffect, Ui};
use crate::app::{CalibRawApp, CAMERA_MATCH_CARD_TITLE, FILM_NEGATIVE_CARD_TITLE};
use crate::i18n::{tr, tr_fmt};
use crate::pipeline::{
    effect_places, EffectComponent, FilmSource, MaskEffectCategory, MAX_EFFECT_COMPONENTS,
};

const TILES_PER_ROW: usize = 3;
/// The picker's width when its button is narrower, as the touch Add tab is.
const PICKER_MIN_WIDTH: f32 = 264.0;

/// What an effect picker adds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui::sidebar) enum EffectPick {
    Effect(MaskEffect),
    /// Develops the photo as the positive of a scanned film negative. It is
    /// not an effect, but people look for it among them; only photo-wide
    /// pickers offer it ([`PhotoTiles`]).
    FilmNegative,
    /// Fits the photo's look to its camera's own JPEG preview, which only
    /// photo-wide pickers offer too.
    CameraMatch,
}

impl EffectPick {
    pub(in crate::ui::sidebar) const fn effect(self) -> Option<MaskEffect> {
        match self {
            Self::Effect(effect) => Some(effect),
            Self::FilmNegative | Self::CameraMatch => None,
        }
    }
}

/// How a picker shows a tile that changes the whole photo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui::sidebar) enum PhotoTile {
    /// Mask pickers, which cannot change part of a photo so, and photos the
    /// tile does not apply to.
    Hidden,
    Offered,
    /// The photo has it already.
    Added,
}

/// The tiles of a picker that change the whole photo rather than add an
/// effect: [`EffectPick::FilmNegative`] and [`EffectPick::CameraMatch`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui::sidebar) struct PhotoTiles {
    pub(in crate::ui::sidebar) film_negative: PhotoTile,
    pub(in crate::ui::sidebar) camera_match: PhotoTile,
}

impl PhotoTiles {
    /// The tiles of a mask's picker: none.
    pub(in crate::ui::sidebar) const HIDDEN: Self = Self {
        film_negative: PhotoTile::Hidden,
        camera_match: PhotoTile::Hidden,
    };

    /// The tiles of a photo-wide picker for the open photo.
    pub(in crate::ui::sidebar) fn for_photo(app: &CalibRawApp) -> Self {
        Self {
            film_negative: Self::film_negative(&app.develop.exposure.film),
            camera_match: if app.develop.camera_look.is_some() {
                PhotoTile::Added
            } else if app.camera_match_offered() {
                PhotoTile::Offered
            } else {
                PhotoTile::Hidden
            },
        }
    }

    fn film_negative(film: &FilmSource) -> PhotoTile {
        if film.is_positive() {
            PhotoTile::Offered
        } else {
            PhotoTile::Added
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
    photo_tiles: PhotoTiles,
) -> Option<EffectPick> {
    let featured_added = add_effect_buttons(ui, featured, components).map(EffectPick::Effect);
    ui.add_space(moduwu_design::SPACE_XS);
    let picked = add_effect_button(ui, components, photo_tiles);
    featured_added.or(picked)
}

/// A full-width Add effect button that opens [`effect_picker_popup`].
/// Disabled once `components` is full. Returns what was picked.
pub(in crate::ui::sidebar) fn add_effect_button(
    ui: &mut Ui,
    components: &[EffectComponent],
    photo_tiles: PhotoTiles,
) -> Option<EffectPick> {
    let full = effect_places(components) >= MAX_EFFECT_COMPONENTS;
    let response = ui
        .add_enabled_ui(!full, |ui| {
            moduwu_design::full_width_button(
                ui,
                format!("{}  {}", egui_phosphor::regular::PLUS, tr("Add effect")),
            )
        })
        .inner
        .on_disabled_hover_text(tr_fmt(
            "At most {max} effects can be added",
            &[("max", &MAX_EFFECT_COMPONENTS)],
        ));
    effect_picker_popup(&response, components, photo_tiles)
}

/// Opens [`effect_picker`] when `button` is clicked, as wide as the button
/// and at least [`PICKER_MIN_WIDTH`], on the side of the button with more
/// room. Returns what was picked.
pub(in crate::ui::sidebar) fn effect_picker_popup(
    button: &egui::Response,
    components: &[EffectComponent],
    photo_tiles: PhotoTiles,
) -> Option<EffectPick> {
    moduwu_design::picker_popup(
        button,
        PICKER_MIN_WIDTH,
        moduwu_design::PickerPlacement::BelowOrAbove,
        |ui| effect_picker(ui, components, photo_tiles),
    )
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

/// Every effect family as a tile under its category's heading, to add one of
/// its effects to `components`: the first style whose effect `components`
/// lacks; the film negative and Camera match tiles lead Film & Finish unless
/// `photo_tiles` hides them. A family whose every effect `components` holds
/// shows a check mark and is disabled; once `components` is full, every
/// family is disabled. Returns the clicked tile's pick.
fn effect_picker(
    ui: &mut Ui,
    components: &[EffectComponent],
    photo_tiles: PhotoTiles,
) -> Option<EffectPick> {
    let full = effect_places(components) >= MAX_EFFECT_COMPONENTS;
    let gap = moduwu_design::SPACE_XS;
    let mut picked = None;
    ui.spacing_mut().item_spacing = egui::vec2(gap, gap);
    // Narrower than the popup while the scrollbar shows.
    let tile_width = moduwu_design::tile_width(ui, TILES_PER_ROW);
    for (index, category) in MaskEffectCategory::ALL.into_iter().enumerate() {
        if index > 0 {
            ui.add_space(moduwu_design::SPACE_SM);
        }
        category_heading(ui, tr(category.label()));
        let mut tiles = Vec::new();
        if category == MaskEffectCategory::FilmAndFinish {
            let photo_wide = [
                (
                    photo_tiles.film_negative,
                    EffectPick::FilmNegative,
                    FILM_NEGATIVE_CARD_TITLE,
                    egui_phosphor::regular::FILM_STRIP,
                    tr("Develop a scanned film negative as its positive."),
                ),
                (
                    photo_tiles.camera_match,
                    EffectPick::CameraMatch,
                    CAMERA_MATCH_CARD_TITLE,
                    egui_phosphor::regular::CAMERA,
                    tr("Match the colors and tones of the camera's own JPEG."),
                ),
            ];
            for (state, pick, title, icon, help) in photo_wide {
                if state == PhotoTile::Hidden {
                    continue;
                }
                let added = state == PhotoTile::Added;
                tiles.push(Tile {
                    pick,
                    label: super::super::adjustment_cards::card_title_text(title),
                    icon,
                    help: Some(help),
                    added,
                    enabled: !added,
                });
            }
        }
        tiles.extend(
            MaskEffect::ALL
                .into_iter()
                // Each family once, where its first effect is listed.
                .filter_map(|effect| {
                    effect
                        .family()
                        .filter(|family| family.first_effect() == effect)
                })
                .filter(|family| family.category() == Some(category))
                .filter_map(|family| {
                    // Effects switched off for good or replaced are never offered.
                    family.styles().find(|style| style.effect().is_offered())?;
                    let to_add = family.styles_to_add(components).next();
                    let effect = to_add.map_or(family.first_effect(), |style| style.effect());
                    let added = to_add.is_none();
                    Some(Tile {
                        pick: EffectPick::Effect(effect),
                        label: tr(family.label()),
                        icon: effect_icon(family.first_effect()),
                        help: effect_description(effect),
                        added,
                        enabled: !added && !full,
                    })
                }),
        );
        for row in tiles.chunks(TILES_PER_ROW) {
            ui.horizontal(|ui| {
                for tile in row {
                    let mut response =
                        effect_tile(ui, tile, tile_width).on_disabled_hover_text(if tile.added {
                            tr_fmt("{label} is already added", &[("label", &tile.label)])
                        } else {
                            tr_fmt(
                                "At most {max} effects can be added",
                                &[("max", &MAX_EFFECT_COMPONENTS)],
                            )
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

/// `tile` as an icon tile `width` wide. An added tile shows a check mark and
/// reports itself selected to assistive technology.
fn effect_tile(ui: &mut Ui, tile: &Tile, width: f32) -> egui::Response {
    let mut icon_tile = moduwu_design::IconTile::new(tile.icon, tile.label)
        .size(moduwu_design::TileSize::MEDIUM)
        .width(width)
        .selected(tile.added);
    if tile.added {
        icon_tile = icon_tile.badge(egui_phosphor::regular::CHECK);
    }
    ui.add_enabled(tile.enabled, icon_tile)
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
        MaskEffect::StyledGlow => regular::SUN,
        MaskEffect::Glow => regular::SPARKLE,
        MaskEffect::LightShafts | MaskEffect::LightRays => regular::SUN_HORIZON,
        MaskEffect::Neon => regular::LIGHTNING,
        MaskEffect::Relight => regular::LIGHTBULB,
        MaskEffect::EdgeGlow => regular::SCRIBBLE,
        MaskEffect::Pixelate => regular::CHECKERBOARD,
        MaskEffect::Fog => regular::CLOUD_FOG,
        MaskEffect::Smoke => regular::WIND,
        MaskEffect::Rain => regular::CLOUD_RAIN,
        MaskEffect::WetSurfaces => regular::DROP,
        MaskEffect::Grain => regular::DOTS_NINE,
        MaskEffect::Halation => regular::CIRCLE_DASHED,
        MaskEffect::LegacyHighlightGlow => regular::SUN_DIM,
        MaskEffect::Vignette => regular::VIGNETTE,
    }
}
