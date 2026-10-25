//! A list of exclusive choices, such as LUTs or camera profiles, whose rows
//! preview on the photo while a pointer rests on them. The list reports the
//! row that was clicked and the row under a hovering pointer; the caller
//! decides what a choice means and how its preview renders.
use eframe::egui::{self, Ui};

/// One line of a [`preview_list`].
pub(crate) enum PreviewListItem<K> {
    /// A small title above the rows that follow it.
    Heading(&'static str),
    Choice {
        key: K,
        label: String,
        selected: bool,
        /// A disabled row is shown, as a missing choice the photo still uses,
        /// but can be neither clicked nor previewed.
        enabled: bool,
    },
}

impl<K> PreviewListItem<K> {
    pub(crate) fn choice(key: K, label: impl Into<String>, selected: bool) -> Self {
        Self::Choice {
            key,
            label: label.into(),
            selected,
            enabled: true,
        }
    }
}

pub(crate) struct PreviewListResponse<K> {
    pub(crate) clicked: Option<K>,
    /// The row a hovering pointer rests on. Touches never report one: a tap
    /// applies the choice instead.
    pub(crate) hovered: Option<K>,
}

/// Whether the pointer hovers without pressing, so resting on a row can
/// preview it. A touch, a held button and an open menu all mean the pointer is
/// doing something else.
pub(crate) fn pointer_previews(ui: &Ui) -> bool {
    ui.input(|input| !input.any_touches() && !input.pointer.any_down())
        && !egui::Popup::is_any_open(ui.ctx())
}

/// Draws `items` in a scroll area at most `visible_rows` rows high.
pub(crate) fn preview_list<K: Clone>(
    ui: &mut Ui,
    id_salt: impl egui::AsIdSalt,
    visible_rows: f32,
    items: impl IntoIterator<Item = PreviewListItem<K>>,
) -> PreviewListResponse<K> {
    let previews = pointer_previews(ui);
    let row_height = moduwu_design::CONTROL_HEIGHT + ui.spacing().item_spacing.y;
    let mut response = PreviewListResponse {
        clicked: None,
        hovered: None,
    };
    egui::ScrollArea::vertical()
        .id_salt(id_salt)
        .max_height(row_height * visible_rows)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for item in items {
                match item {
                    PreviewListItem::Heading(title) => {
                        ui.label(egui::RichText::new(title).small().weak());
                    }
                    PreviewListItem::Choice {
                        key,
                        label,
                        selected,
                        enabled,
                    } => {
                        let row = ui
                            .add_enabled_ui(enabled, |ui| {
                                moduwu_design::navigation_row(
                                    ui,
                                    label,
                                    selected,
                                    egui::Sense::click(),
                                )
                            })
                            .inner;
                        if row.clicked() {
                            response.clicked = Some(key.clone());
                        }
                        if previews && row.hovered() && enabled {
                            response.hovered = Some(key);
                        }
                    }
                }
            }
        });
    response
}
