//! Exclusive choices, such as LUTs or camera profiles, whose rows preview on
//! the photo while a pointer rests on them: an inline [`preview_list`] or a
//! [`preview_combo`] dropdown. Both report the row that was clicked and the
//! row under a hovering pointer; the caller decides what a choice means and
//! how its preview renders.
use eframe::egui::{self, Ui};

/// One line of a [`preview_list`] or [`preview_combo`].
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

/// Draws `items` in a scroll area `height` tall, or shorter when they all
/// fit.
pub(crate) fn preview_list<K: Clone>(
    ui: &mut Ui,
    id_salt: impl egui::AsIdSalt,
    height: ListHeight,
    items: impl IntoIterator<Item = PreviewListItem<K>>,
) -> PreviewListResponse<K> {
    let previews = pointer_previews(ui);
    let height = match height {
        ListHeight::Rows(rows) => {
            (moduwu_design::CONTROL_HEIGHT + ui.spacing().item_spacing.y) * rows
        }
        ListHeight::Points(points) => points,
    };
    egui::ScrollArea::vertical()
        .id_salt(id_salt)
        .max_height(height)
        // A scroll area never grows past the room left in the one around it,
        // which below the fold of the Develop panel is none; without this the
        // list would shrink to egui's 64 px minimum there.
        .min_scrolled_height(height)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            show_items(ui, items, previews, |ui, label, selected| {
                moduwu_design::navigation_row(ui, label, selected, egui::Sense::click())
            })
        })
        .inner
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
        .find_map(|item| match item {
            PreviewListItem::Choice {
                label,
                selected: true,
                ..
            } => Some(label.clone()),
            _ => None,
        })
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

/// Draws `items` one below another, each choice with `row`, and reports the
/// clicked row and, when `previews`, the hovered one.
fn show_items<K: Clone>(
    ui: &mut Ui,
    items: impl IntoIterator<Item = PreviewListItem<K>>,
    previews: bool,
    mut row: impl FnMut(&mut Ui, String, bool) -> egui::Response,
) -> PreviewListResponse<K> {
    let mut response = PreviewListResponse::default();
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
        }
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(count: usize) -> Vec<PreviewListItem<usize>> {
        (0..count)
            .map(|index| PreviewListItem::choice(index, format!("Row {index}"), index == 0))
            .collect()
    }

    /// The height `preview_list` takes for `count` rows of up to `height`,
    /// drawn below the fold of a scrolling panel.
    fn height_below_the_fold(count: usize, height: ListHeight) -> f32 {
        let mut measured = 0.0;
        egui::__run_test_ui(|ui| {
            egui::ScrollArea::vertical()
                .max_height(200.0)
                .show(ui, |ui| {
                    ui.add_space(400.0);
                    let top = ui.cursor().top();
                    preview_list(ui, "list", height, rows(count));
                    measured = ui.cursor().top() - top - ui.spacing().item_spacing.y;
                });
        });
        measured
    }

    #[test]
    fn a_list_below_the_fold_keeps_its_visible_rows() {
        let row_height = moduwu_design::CONTROL_HEIGHT + egui::Spacing::default().item_spacing.y;
        let height = height_below_the_fold(20, ListHeight::Rows(7.5));
        assert!(
            (height - row_height * 7.5).abs() < 1.0,
            "{height} for 7.5 rows of {row_height}"
        );
    }

    #[test]
    fn a_short_list_shrinks_to_its_rows() {
        let row_height = moduwu_design::CONTROL_HEIGHT + egui::Spacing::default().item_spacing.y;
        assert!(height_below_the_fold(2, ListHeight::Rows(7.5)) < row_height * 2.5);
        assert!(height_below_the_fold(2, ListHeight::Points(500.0)) < row_height * 2.5);
    }

    #[test]
    fn a_list_takes_the_points_it_is_given() {
        let height = height_below_the_fold(40, ListHeight::Points(300.0));
        assert!((height - 300.0).abs() < 1.0, "{height}");
    }
}
