use eframe::egui;
use eframe::egui::Vec2;

use moduwu_design::ScreenLayout;

use crate::platform::PlatformCaps;

pub(crate) const DEVELOP_TOOL_RAIL_ID: &str = "develop_tool_rail";
pub(crate) const DEVELOP_SIDEBAR_ID: &str = "develop_sidebar_right";
pub(crate) const DEVELOP_MASK_STRIP_ID: &str = "develop_horizontal_mask_strip";

/// How the Develop tab places the photo and its tools.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DevelopArrangement {
    /// The toolbar above, the tool rail, sidebar and mask strip docked on the
    /// right, and the photo fitted into the rest: landscape desktop windows
    /// and tablets held sideways.
    Docked,
    /// The photo fills the screen and the tools are drawn over it
    /// ([`crate::ui::develop_viewport`]): portrait screens and phones held
    /// sideways.
    Overlay,
}

impl DevelopArrangement {
    /// Shortest side, in points, from which a touch screen counts as a
    /// tablet: Android's `sw600dp` breakpoint, since a point is a
    /// density-independent pixel there.
    pub(crate) const TABLET_MIN_SHORT_SIDE: f32 = 600.0;

    /// The arrangement for the app's `screen` area in points
    /// (`Context::content_rect`). It includes the system bars, which
    /// Android's smallest width does not subtract either.
    pub(crate) fn of(caps: PlatformCaps, screen: Vec2) -> Self {
        let landscape = ScreenLayout::from_size(screen) == ScreenLayout::Horizontal;
        let roomy = caps.is_desktop() || screen.min_elem() >= Self::TABLET_MIN_SHORT_SIDE;
        if landscape && roomy {
            Self::Docked
        } else {
            Self::Overlay
        }
    }
}

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
