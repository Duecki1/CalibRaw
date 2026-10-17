use eframe::egui;
use eframe::egui::Vec2;

use moduwu_design::ScreenLayout;

pub(crate) const DEVELOP_TOOL_RAIL_ID: &str = "develop_tool_rail";
pub(crate) const DEVELOP_SIDEBAR_ID: &str = "develop_sidebar_right";
pub(crate) const DEVELOP_MASK_STRIP_ID: &str = "develop_horizontal_mask_strip";

/// Returns the width explicitly selected with the Develop sidebar resize handle.
///
/// `egui::Panel` also persists content-driven width changes, so this separate
/// value keeps newly revealed controls from overriding the user's choice.
pub(crate) fn develop_sidebar_user_width(
    ctx: &egui::Context,
    panel_id: egui::Id,
    default_width: f32,
    min_width: f32,
    max_width: f32,
) -> f32 {
    moduwu_design::persisted_panel_width(ctx, panel_id, default_width, min_width, max_width)
}

pub(crate) fn develop_sidebar_max_width(viewport: Vec2) -> f32 {
    (viewport.x * 0.48).clamp(
        ScreenLayout::MIN_HORIZONTAL_SIDEBAR_WIDTH,
        ScreenLayout::MAX_HORIZONTAL_SIDEBAR_WIDTH,
    )
}
