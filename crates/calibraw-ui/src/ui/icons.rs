use eframe::egui::{self, Response, RichText, Ui, Vec2};
use egui_phosphor::regular;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UiIcon {
    #[cfg(not(target_os = "android"))]
    Sidebar,
    #[cfg(not(target_os = "android"))]
    Filmstrip,
    RotateLeft,
    RotateRight,
}

impl UiIcon {
    fn glyph(self) -> &'static str {
        match self {
            #[cfg(not(target_os = "android"))]
            Self::Sidebar => regular::SIDEBAR_SIMPLE,
            #[cfg(not(target_os = "android"))]
            Self::Filmstrip => regular::IMAGE,
            Self::RotateLeft => regular::ARROW_COUNTER_CLOCKWISE,
            Self::RotateRight => regular::ARROW_CLOCKWISE,
        }
    }
}

#[cfg(not(target_os = "android"))]
pub(crate) fn icon_toggle_button(
    ui: &mut Ui,
    icon: UiIcon,
    selected: bool,
    size: Vec2,
    tooltip: &str,
) -> Response {
    glyph_toggle_button(ui, icon.glyph(), selected, size, tooltip)
}

/// [`icon_toggle_button`] for any Phosphor glyph. Only the selected state is framed.
#[cfg(not(target_os = "android"))]
pub(crate) fn glyph_toggle_button(
    ui: &mut Ui,
    glyph: &str,
    selected: bool,
    size: Vec2,
    tooltip: &str,
) -> Response {
    ui.add_sized(
        size,
        egui::Button::new(RichText::new(glyph).size(size.y * 0.52))
            .selected(selected)
            .frame(selected),
    )
    .on_hover_text(tooltip)
}

pub(crate) fn icon_button(ui: &mut Ui, icon: UiIcon, size: Vec2, tooltip: &str) -> Response {
    ui.add_sized(
        size,
        egui::Button::new(RichText::new(icon.glyph()).size(size.y * 0.52)).frame(true),
    )
    .on_hover_text(tooltip)
}

#[cfg(not(target_os = "android"))]
pub(crate) use moduwu_design::icon_toggle_button_enabled as phosphor_icon_toggle_button_enabled;
pub(crate) use moduwu_design::{
    icon_button as phosphor_icon_button, icon_button_enabled as phosphor_icon_button_enabled,
    icon_toggle_button as phosphor_icon_toggle_button,
};

pub(crate) fn folder_disclosure_size() -> Vec2 {
    egui::vec2(
        if cfg!(target_os = "android") {
            30.0
        } else {
            26.0
        },
        crate::ui::theme::CONTROL_HEIGHT,
    )
}

pub(crate) fn folder_disclosure_button(ui: &mut Ui, expanded: bool) -> Response {
    let glyph = if expanded {
        regular::CARET_DOWN
    } else {
        regular::CARET_RIGHT
    };
    let font_size = if cfg!(target_os = "android") {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conditional_icon_buttons_preserve_geometry_and_block_disabled_clicks() {
        for design in crate::ui::theme::UiDesign::ALL {
            for enabled in [false, true] {
                let ctx = egui::Context::default();
                crate::ui::theme::install(&ctx);
                crate::ui::theme::apply(&ctx, design);
                let mut rect = egui::Rect::NOTHING;
                let mut clicked = false;
                let mut show = |events| {
                    let _ = ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(400.0, 200.0),
                            )),
                            events,
                            ..Default::default()
                        },
                        |ui| {
                            let size = egui::vec2(32.0, 20.0);
                            let conditional = phosphor_icon_button_enabled(
                                ui,
                                enabled,
                                regular::X,
                                size,
                                "Close",
                            );
                            let regular = phosphor_icon_button(ui, regular::X, size, "Close");
                            assert_eq!(conditional.rect.size(), regular.rect.size());
                            assert!(conditional.rect.height() >= crate::ui::theme::CONTROL_HEIGHT);
                            assert_eq!(conditional.enabled(), enabled);
                            rect = conditional.rect;
                            clicked |= conditional.clicked();
                        },
                    );
                    rect
                };
                let position = show(Vec::new()).center();
                for pressed in [true, false] {
                    show(vec![
                        egui::Event::PointerMoved(position),
                        egui::Event::PointerButton {
                            pos: position,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ]);
                }
                assert_eq!(clicked, enabled);
            }
        }
    }
}
