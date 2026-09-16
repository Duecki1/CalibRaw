use eframe::egui::{self, InnerResponse, Response, Ui};

/// A regular menu action with the same geometry in enabled and disabled states.
pub(crate) fn menu_item(
    ui: &mut Ui,
    enabled: bool,
    label: impl Into<egui::WidgetText>,
) -> Response {
    ui.add_enabled(enabled, egui::Button::new(label))
}

pub(crate) fn destructive_menu_item(ui: &mut Ui, label: impl Into<egui::WidgetText>) -> Response {
    let color = ui.visuals().error_fg_color;
    menu_item(ui, true, label.into().color(color))
}

/// A right-click menu that retains the regular popup/widget styling used by
/// combo boxes instead of egui's compact frameless menu override.
pub(crate) fn context_menu<R>(
    response: &Response,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> Option<InnerResponse<R>> {
    egui::Popup::context_menu(response)
        .style(egui::style::StyleModifier::default())
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(add_contents)
}

/// A click-triggered dropdown that uses the same regular widget styling as
/// combo boxes and context menus instead of egui's compact menu styling.
pub(crate) fn dropdown_menu<R>(
    response: &Response,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> Option<InnerResponse<R>> {
    egui::Popup::menu(response)
        .style(egui::style::StyleModifier::default())
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(add_contents)
}

/// A submenu with the regular dropdown styling used by [`dropdown_menu`].
pub(crate) fn dropdown_submenu<'a, R>(
    ui: &mut Ui,
    label: impl egui::IntoAtoms<'a>,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> Response {
    let (response, _) = egui::menu::SubMenuButton::new(label)
        .config(egui::menu::MenuConfig::new().style(egui::style::StyleModifier::default()))
        .ui(ui, add_contents);
    response
}

pub(crate) fn context_menu_item<'a>(
    ui: &mut Ui,
    enabled: bool,
    label: impl egui::IntoAtoms<'a>,
) -> Response {
    ui.add_enabled(enabled, egui::Button::selectable(false, label))
}
