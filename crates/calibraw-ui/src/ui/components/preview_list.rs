//! Exclusive choices, such as LUTs or camera profiles, whose rows preview on
//! the photo while a pointer rests on them: an inline [`preview_list`] or a
//! [`preview_combo`] dropdown. Both report the row that was clicked and the
//! row under a hovering pointer; the caller decides what a choice means and
//! how its preview renders.
use eframe::egui::{self, Ui};

/// One row of a [`preview_list`] or [`preview_combo`].
pub(crate) struct PreviewListItem<K> {
    pub(crate) key: K,
    pub(crate) label: String,
    pub(crate) selected: bool,
    /// A disabled row is shown, as a missing choice the photo still uses, but
    /// can be neither clicked nor previewed.
    pub(crate) enabled: bool,
}

impl<K> PreviewListItem<K> {
    pub(crate) fn choice(key: K, label: impl Into<String>, selected: bool) -> Self {
        Self {
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

impl<K> Default for PreviewListResponse<K> {
    fn default() -> Self {
        Self {
            clicked: None,
            hovered: None,
        }
    }
}

/// How tall a [`preview_list`] is at most; shorter when its rows all fit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ListHeight {
    Rows(f32),
    Points(f32),
}

/// Whether the pointer hovers without pressing. A touch or a held button
/// means the pointer is doing something else.
fn pointer_hovers(ui: &Ui) -> bool {
    ui.input(|input| !input.any_touches() && !input.pointer.any_down())
}

/// Whether the pointer hovers without pressing, so resting on a row can
/// preview it. A touch, a held button and an open menu all mean the pointer is
/// doing something else.
pub(crate) fn pointer_previews(ui: &Ui) -> bool {
    pointer_hovers(ui) && !egui::Popup::is_any_open(ui.ctx())
}

/// Narrowest cell a [`preview_list`] splits its width into; a narrower list
/// shows fewer columns.
const MIN_CELL_WIDTH: f32 = 120.0;
/// Vertical gap between two rows of a [`preview_list`].
pub(crate) const ROW_GAP: f32 = moduwu_design::SPACE_XS;

/// Draws `items` in a grid of up to `columns` columns, filled row by row, in
/// a scroll area `height` tall, or shorter when they all fit. A row of
/// [`ListHeight::Rows`] is one grid row.
pub(crate) fn preview_list<K: Clone>(
    ui: &mut Ui,
    id_salt: impl egui::AsIdSalt,
    height: ListHeight,
    columns: usize,
    items: impl IntoIterator<Item = PreviewListItem<K>>,
) -> PreviewListResponse<K> {
    let previews = pointer_previews(ui);
    let row_height = moduwu_design::Metrics::of(ui.ctx()).control_height;
    let height = match height {
        ListHeight::Rows(rows) => (row_height + ROW_GAP) * rows,
        ListHeight::Points(points) => points,
    };
    let items: Vec<_> = items.into_iter().collect();
    egui::ScrollArea::vertical()
        .id_salt(id_salt)
        .max_height(height)
        // A scroll area never grows past the room left in the one around it,
        // which below the fold of the Develop panel is none; without this the
        // list would shrink to egui's 64 px minimum there.
        .min_scrolled_height(height)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = ROW_GAP;
            let gap = ui.spacing().item_spacing.x;
            let width = ui.available_width().max(1.0);
            let fitting = ((width + gap) / (MIN_CELL_WIDTH + gap)).floor() as usize;
            let columns = fitting.clamp(1, columns.max(1));
            let cell = egui::vec2(
                ((width - gap * (columns - 1) as f32) / columns as f32).max(1.0),
                row_height,
            );
            let mut response = PreviewListResponse::default();
            let mut items = items.into_iter().peekable();
            while items.peek().is_some() {
                ui.horizontal(|ui| {
                    for item in items.by_ref().take(columns) {
                        ui.allocate_ui(cell, |ui| {
                            show_item(ui, item, previews, &mut response, |ui, label, selected| {
                                let truncated = label_width(ui, &label)
                                    > cell.x - ui.spacing().button_padding.x * 2.0;
                                let row = moduwu_design::navigation_row(
                                    ui,
                                    label.as_str(),
                                    selected,
                                    egui::Sense::click(),
                                );
                                if truncated {
                                    row.on_hover_text(label)
                                } else {
                                    row
                                }
                            });
                        });
                    }
                });
            }
            response
        })
        .inner
}

/// Width of `label` on one line in a button's font.
fn label_width(ui: &Ui, label: &str) -> f32 {
    let font = egui::TextStyle::Button.resolve(ui.style());
    ui.painter()
        .layout_no_wrap(label.to_owned(), font, egui::Color32::PLACEHOLDER)
        .size()
        .x
}

/// A full-width dropdown of `items` whose closed field reads the selected
/// row. Resting the pointer on a row of the open menu reports it as hovered.
pub(crate) fn preview_combo<K: Clone>(
    ui: &mut Ui,
    id_salt: impl egui::AsIdSalt,
    items: Vec<PreviewListItem<K>>,
) -> PreviewListResponse<K> {
    let selected_text = items
        .iter()
        .find(|item| item.selected)
        .map(|item| item.label.clone())
        .unwrap_or_default();
    // The open menu is this dropdown's own, so only the pointer decides.
    let previews = pointer_hovers(ui);
    let mut response = PreviewListResponse::default();
    moduwu_design::combo_box(id_salt, selected_text, ui.available_width()).show_ui(ui, |ui| {
        response = show_items(ui, items, previews, |ui, label, selected| {
            ui.selectable_label(selected, label)
        });
    });
    response
}

/// Draws `items` one below another, each with `row`, and reports the clicked
/// row and, when `previews`, the hovered one.
fn show_items<K: Clone>(
    ui: &mut Ui,
    items: impl IntoIterator<Item = PreviewListItem<K>>,
    previews: bool,
    mut row: impl FnMut(&mut Ui, String, bool) -> egui::Response,
) -> PreviewListResponse<K> {
    let mut response = PreviewListResponse::default();
    for item in items {
        show_item(ui, item, previews, &mut response, &mut row);
    }
    response
}

/// Draws `item` with `row` and records it in `response` when it is clicked
/// or, when `previews`, hovered.
fn show_item<K: Clone>(
    ui: &mut Ui,
    item: PreviewListItem<K>,
    previews: bool,
    response: &mut PreviewListResponse<K>,
    row: impl FnOnce(&mut Ui, String, bool) -> egui::Response,
) {
    let PreviewListItem {
        key,
        label,
        selected,
        enabled,
    } = item;
    let shown = ui
        .add_enabled_ui(enabled, |ui| row(ui, label, selected))
        .inner;
    if shown.clicked() {
        response.clicked = Some(key.clone());
    }
    if previews && shown.hovered() && enabled {
        response.hovered = Some(key);
    }
}
