use eframe::egui::{Ui, Vec2};

use super::theme::{CARD_GAP, CONTENT_MARGIN, SPACE_SM};

const COMPACT_PORTRAIT_CARD_GAP: f32 = SPACE_SM;
const COMPACT_PORTRAIT_CONTENT_MARGIN: i8 = SPACE_SM as i8;

pub(crate) fn is_compact_portrait(ui: &Ui) -> bool {
    compact_portrait_for_platform(ui.ctx().content_rect().size(), cfg!(target_os = "android"))
}

pub(super) fn compact_portrait_for_platform(viewport: Vec2, android: bool) -> bool {
    android && viewport.x < viewport.y
}

pub(super) fn content_margin(ui: &Ui) -> i8 {
    if is_compact_portrait(ui) {
        COMPACT_PORTRAIT_CONTENT_MARGIN
    } else {
        CONTENT_MARGIN
    }
}

pub(crate) fn card_gap(ui: &mut Ui) {
    let gap = if is_compact_portrait(ui) {
        COMPACT_PORTRAIT_CARD_GAP
    } else {
        CARD_GAP
    };
    let explicit_space = (gap - ui.spacing().item_spacing.y).max(0.0);
    ui.add_space(explicit_space);
}
