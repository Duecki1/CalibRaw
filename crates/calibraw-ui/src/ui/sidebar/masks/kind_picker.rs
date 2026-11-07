//! Picking the kind of a new mask group or sub-mask: icon tiles in three
//! sizes, the most used kinds largest.

use super::*;
use crate::i18n::tr;
use egui_phosphor::regular;
use moduwu_design::{IconTile, TileSize};

/// The picker's width when its button is narrower, as the strip's are.
pub(super) const KIND_PICKER_MIN_WIDTH: f32 = 300.0;

/// Each tier's kinds in order, with tiles per row and tile size. Every
/// [`MaskKind`] appears exactly once.
const TIERS: [(&[MaskKind], usize, TileSize); 3] = [
    (
        &[MaskKind::Subject, MaskKind::Background],
        2,
        TileSize::LARGE,
    ),
    (
        &[MaskKind::Linear, MaskKind::Radial, MaskKind::Brush],
        3,
        TileSize::MEDIUM,
    ),
    (
        &[
            MaskKind::Sky,
            MaskKind::Person,
            MaskKind::Object,
            MaskKind::Path,
            MaskKind::LuminanceRange,
            MaskKind::ColorRange,
            MaskKind::DepthRange,
            MaskKind::Fullscreen,
        ],
        4,
        TileSize::SMALL,
    ),
];

pub(super) fn mask_kind_icon(kind: MaskKind) -> &'static str {
    match kind {
        MaskKind::Subject => regular::USER_FOCUS,
        MaskKind::Background => regular::MOUNTAINS,
        MaskKind::Linear => regular::GRADIENT,
        MaskKind::Radial => regular::CIRCLE_DASHED,
        MaskKind::Brush => regular::PAINT_BRUSH,
        MaskKind::Sky => regular::CLOUD,
        MaskKind::Person => regular::USERS_THREE,
        MaskKind::Object => regular::CUBE,
        MaskKind::Path => regular::PEN_NIB,
        MaskKind::LuminanceRange => regular::CIRCLE_HALF,
        MaskKind::ColorRange => regular::EYEDROPPER,
        MaskKind::DepthRange => regular::STACK,
        MaskKind::Fullscreen => regular::FRAME_CORNERS,
    }
}

/// The short name a tile shows; [`MaskKind::label`] names new masks.
fn mask_kind_tile_label(kind: MaskKind) -> &'static str {
    match kind {
        MaskKind::Subject => "Subject",
        MaskKind::Background => "Background",
        MaskKind::Linear => "Linear",
        MaskKind::Radial => "Radial",
        MaskKind::Brush => "Brush",
        MaskKind::Sky => "Sky",
        MaskKind::Person => "People",
        MaskKind::Object => "Object",
        MaskKind::Path => "Path",
        MaskKind::LuminanceRange => "Luminance",
        MaskKind::ColorRange => "Color",
        MaskKind::DepthRange => "Depth",
        MaskKind::Fullscreen => "Fullscreen",
    }
}

/// Every available kind as a tile, filling the available width. An
/// experimental kind carries a flask badge. Returns the clicked kind.
pub(super) fn mask_kind_tiles(ui: &mut Ui) -> Option<MaskKind> {
    let gap = moduwu_design::SPACE_XS;
    ui.spacing_mut().item_spacing = egui::vec2(gap, gap);
    let mut picked = None;
    for (index, (kinds, per_row, size)) in TIERS.into_iter().enumerate() {
        if index > 0 {
            ui.add_space(moduwu_design::SPACE_XXS);
        }
        let width = moduwu_design::tile_width(ui, per_row);
        let kinds: Vec<MaskKind> = kinds
            .iter()
            .copied()
            .filter(|kind| kind.is_available())
            .collect();
        for row in kinds.chunks(per_row) {
            ui.horizontal(|ui| {
                for &kind in row {
                    let mut tile =
                        IconTile::new(mask_kind_icon(kind), tr(mask_kind_tile_label(kind)))
                            .size(size)
                            .width(width);
                    if kind.is_experimental() {
                        tile = tile.badge(regular::FLASK);
                    }
                    let mut response = ui.add(tile);
                    if kind.is_experimental() {
                        response = response.on_hover_text(tr("Experimental"));
                    }
                    if response.clicked() {
                        picked = Some(kind);
                    }
                }
            });
        }
    }
    picked
}
