use super::*;
use crate::sidecar::{PhotoFlag, PhotoReview};
use moduwu_design::MenuItem;

/// An empty selection means all values; choices within a group are combined with OR.
/// The rating, flag and filename groups are combined with AND.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct LibraryReviewFilter {
    pub(super) ratings: [bool; 6],
    pub(super) flags: [bool; 3],
}

impl LibraryReviewFilter {
    pub(super) fn active(self) -> bool {
        self.ratings.iter().any(|selected| *selected) || self.flags.iter().any(|selected| *selected)
    }

    pub(super) fn matches(self, review: PhotoReview) -> bool {
        let flag = match review.flag {
            PhotoFlag::Picked => 0,
            PhotoFlag::Unflagged => 1,
            PhotoFlag::Rejected => 2,
        };
        (!self.ratings.iter().any(|selected| *selected)
            || self.ratings[usize::from(review.rating.min(5))])
            && (!self.flags.iter().any(|selected| *selected) || self.flags[flag])
    }

    pub(super) fn summary(self) -> String {
        let mut parts = Vec::new();
        if self.ratings.iter().any(|selected| *selected) {
            let ratings = (0..=5)
                .rev()
                .filter(|rating| self.ratings[*rating])
                .map(|rating| {
                    if rating == 0 {
                        "unrated".to_owned()
                    } else {
                        format!("{rating}★")
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");
            parts.push(ratings);
        }
        if self.flags.iter().any(|selected| *selected) {
            parts.push(
                ["Pick", "Unflagged", "Reject"]
                    .into_iter()
                    .enumerate()
                    .filter(|(index, _)| self.flags[*index])
                    .map(|(_, label)| label)
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        parts.join(" · ")
    }
}

const SORT_GROUPS: &[(&str, [(LibrarySortOrder, &str); 2])] = &[
    (
        "Date",
        [
            (LibrarySortOrder::NewestFirst, "Newest first"),
            (LibrarySortOrder::OldestFirst, "Oldest first"),
        ],
    ),
    (
        "Filename",
        [
            (LibrarySortOrder::NameAscending, "A–Z"),
            (LibrarySortOrder::NameDescending, "Z–A"),
        ],
    ),
    (
        "File size",
        [
            (LibrarySortOrder::LargestFirst, "Largest first"),
            (LibrarySortOrder::SmallestFirst, "Smallest first"),
        ],
    ),
    (
        "Rating",
        [
            (LibrarySortOrder::RatingHighestFirst, "Highest first"),
            (LibrarySortOrder::RatingLowestFirst, "Lowest first"),
        ],
    ),
    (
        "Flag",
        [
            (LibrarySortOrder::FlagPickedFirst, "Picks first"),
            (LibrarySortOrder::FlagRejectedFirst, "Rejects first"),
        ],
    ),
];

/// A chip that includes one value in a filter; several can be selected.
fn toggle_chip(ui: &mut Ui, selected: &mut bool, label: impl Into<egui::WidgetText>) {
    if moduwu_design::chip(ui, label, *selected).clicked() {
        *selected = !*selected;
    }
}

/// The index in `SORT_GROUPS` of the group `sort` belongs to.
fn sort_group(sort: LibrarySortOrder) -> usize {
    SORT_GROUPS
        .iter()
        .position(|(_, choices)| choices.iter().any(|(order, _)| *order == sort))
        .unwrap_or(0)
}

/// The sort order, the RAW + JPEG pairs and the rating and flag filters,
/// the same on every platform, then the thumbnail size where the header has
/// no room for its own control.
fn sort_filter_options(
    ui: &mut Ui,
    sort: &mut LibrarySortOrder,
    filter: &mut LibraryReviewFilter,
    stack_raw_companions: &mut bool,
    compact_size: Option<&mut LibraryThumbnailSize>,
) {
    ui.set_min_width(280.0);
    ui.strong("Sort by");
    let group = sort_group(*sort);
    moduwu_design::action_row(ui, |ui| {
        for (index, (label, choices)) in SORT_GROUPS.iter().enumerate() {
            if moduwu_design::chip(ui, *label, index == group).clicked() && index != group {
                *sort = choices[0].0;
            }
        }
    });
    moduwu_design::action_row(ui, |ui| {
        for (order, label) in SORT_GROUPS[sort_group(*sort)].1 {
            if moduwu_design::chip(ui, label, *sort == order).clicked() {
                *sort = order;
            }
        }
    });

    ui.separator();
    ui.strong("RAW + JPEG");
    moduwu_design::toggle(ui, stack_raw_companions, "Show only the RAW").on_hover_text(
        "Cameras shooting RAW+JPEG or RAW+HEIC save both files. Show each pair once, \
             as the RAW, and hide the JPEG or HEIC with the same name.",
    );

    ui.separator();
    ui.strong("Show ratings");
    let had_ratings = filter.ratings.iter().any(|selected| *selected);
    moduwu_design::action_row(ui, |ui| {
        if moduwu_design::chip(ui, "All", !had_ratings).clicked() {
            filter.ratings = [false; 6];
        }
        toggle_chip(ui, &mut filter.ratings[0], "Unrated");
        for rating in (1..=5).rev() {
            toggle_chip(
                ui,
                &mut filter.ratings[rating],
                format!("{rating} {}", egui_phosphor::regular::STAR),
            );
        }
    });
    let has_ratings = filter.ratings.iter().any(|selected| *selected);
    if !had_ratings
        && has_ratings
        && !matches!(
            sort,
            LibrarySortOrder::RatingHighestFirst | LibrarySortOrder::RatingLowestFirst
        )
    {
        *sort = LibrarySortOrder::RatingHighestFirst;
    }

    ui.separator();
    ui.strong("Show flags");
    let had_flags = filter.flags.iter().any(|selected| *selected);
    moduwu_design::action_row(ui, |ui| {
        if moduwu_design::chip(ui, "All", !had_flags).clicked() {
            filter.flags = [false; 3];
        }
        toggle_chip(
            ui,
            &mut filter.flags[0],
            egui::RichText::new(format!("{} Pick", egui_phosphor::regular::FLAG))
                .color(crate::ui::theme::CHANNEL_GREEN),
        );
        toggle_chip(ui, &mut filter.flags[1], "Unflagged");
        toggle_chip(
            ui,
            &mut filter.flags[2],
            egui::RichText::new(format!("{} Reject", egui_phosphor::regular::FLAG))
                .color(crate::ui::theme::CHANNEL_RED),
        );
    });
    if !had_flags
        && filter.flags.iter().any(|selected| *selected)
        && !has_ratings
        && !matches!(
            sort,
            LibrarySortOrder::FlagPickedFirst | LibrarySortOrder::FlagRejectedFirst
        )
    {
        *sort = LibrarySortOrder::FlagPickedFirst;
    }
    ui.add_space(moduwu_design::SPACE_XS);
    ui.weak("Select several values to include them together.");
    let clear = MenuItem::new("Clear filters").icon(egui_phosphor::regular::FUNNEL_X);
    if ui.add_enabled(filter.active(), clear).clicked() {
        *filter = LibraryReviewFilter::default();
    }

    if let Some(size) = compact_size {
        ui.separator();
        ui.strong("Thumbnail size");
        moduwu_design::action_row(ui, |ui| {
            for value in LibraryThumbnailSize::ALL {
                if moduwu_design::chip(ui, value.label(), *size == value).clicked() {
                    *size = value;
                }
            }
        });
    }
}

pub(super) fn sort_filter_popup(
    ui: &mut Ui,
    sort: &mut LibrarySortOrder,
    filter: &mut LibraryReviewFilter,
    stack_raw_companions: &mut bool,
    compact_size: Option<&mut LibraryThumbnailSize>,
    width: f32,
    caps: PlatformCaps,
) {
    let compact = compact_size.is_some();
    let label: egui::WidgetText = if compact {
        egui::RichText::new(format!(
            "{}{}",
            egui_phosphor::regular::SLIDERS_HORIZONTAL,
            if filter.active() { " •" } else { "" }
        ))
        .size(moduwu_design::CONTROL_HEIGHT * 0.55)
        .into()
    } else {
        format!(
            "Sort & filter{} {}",
            if filter.active() { " •" } else { "" },
            egui_phosphor::regular::CARET_DOWN
        )
        .into()
    };
    let response = moduwu_design::toolbar_button(ui, label, width).on_hover_text(format!(
        "{}{}",
        sort.label(),
        if filter.active() {
            format!(" · {}", filter.summary())
        } else {
            String::new()
        }
    ));
    moduwu_design::dropdown_menu(&response, |ui| {
        moduwu_design::menu_title(ui, "Sort & filter");
        // On touch screens the menu is a sheet, which scrolls by itself.
        if caps.is_mobile() {
            sort_filter_options(ui, sort, filter, stack_raw_companions, compact_size);
        } else {
            egui::ScrollArea::vertical()
                .max_height(480.0)
                .show(ui, |ui| {
                    sort_filter_options(ui, sort, filter, stack_raw_companions, compact_size);
                });
        }
    });
}
