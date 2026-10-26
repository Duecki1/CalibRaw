use eframe::egui::{self, Response, RichText, Ui, Vec2};
use egui_phosphor::regular;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UiIcon {
    Sidebar,
    Filmstrip,
}

impl UiIcon {
    fn glyph(self) -> &'static str {
        match self {
            Self::Sidebar => regular::SIDEBAR_SIMPLE,
            Self::Filmstrip => regular::IMAGE,
        }
    }
}

pub(crate) fn icon_toggle_button(
    ui: &mut Ui,
    icon: UiIcon,
    selected: bool,
    size: Vec2,
    tooltip: &str,
) -> Response {
    moduwu_design::ghost_icon_toggle_button(ui, icon.glyph(), selected, size, tooltip)
}

pub(crate) fn folder_disclosure_size() -> Vec2 {
    egui::vec2(
        if crate::platform::PlatformCaps::CURRENT.is_mobile() {
            30.0
        } else {
            26.0
        },
        moduwu_design::CONTROL_HEIGHT,
    )
}

pub(crate) fn folder_disclosure_button(ui: &mut Ui, expanded: bool) -> Response {
    let glyph = if expanded {
        regular::CARET_DOWN
    } else {
        regular::CARET_RIGHT
    };
    let font_size = if crate::platform::PlatformCaps::CURRENT.is_mobile() {
        13.0
    } else {
        12.0
    };
    ui.add_sized(
        folder_disclosure_size(),
        egui::Button::new(RichText::new(glyph).size(font_size)).frame(false),
    )
    .on_hover_text(if expanded {
        "Collapse folder"
    } else {
        "Expand folder"
    })
}
