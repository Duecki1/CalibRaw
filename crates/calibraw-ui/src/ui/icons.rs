use eframe::egui::{self, Response, Sense, Ui, Vec2};
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

/// One folder of a folder tree, `depth` levels below its top folder.
/// `expanded` is `None` for a folder without subfolders.
pub(crate) fn folder_tree_row(
    ui: &mut Ui,
    id_salt: impl egui::AsIdSalt,
    name: &str,
    depth: usize,
    expanded: Option<bool>,
    selected: bool,
    sense: Sense,
) -> moduwu_design::TreeRowResponse {
    let open = expanded == Some(true);
    let mut response = moduwu_design::TreeRow::new(id_salt, name)
        .depth(depth)
        .expanded(expanded)
        .icon(if open {
            regular::FOLDER_OPEN
        } else {
            regular::FOLDER
        })
        .selected(selected)
        .sense(sense)
        .show(ui);
    response.disclosure = response.disclosure.map(|disclosure| {
        disclosure.on_hover_text(if open {
            "Collapse folder"
        } else {
            "Expand folder"
        })
    });
    response
}
