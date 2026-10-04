use super::*;
use crate::sidecar::{PhotoFlag, PhotoReview};

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

pub(super) fn show_sort_filter_options(
    ui: &mut Ui,
    sort: &mut LibrarySortOrder,
    filter: &mut LibraryReviewFilter,
    stack_raw_companions: &mut bool,
) {
    #[cfg(target_os = "android")]
    let _ = filter;
    ui.set_min_width(280.0);
    ui.strong("Sort by");
    let mut group = SORT_GROUPS
        .iter()
        .position(|(_, choices)| choices.iter().any(|(order, _)| order == sort))
        .unwrap_or(0);
    let previous = group;
    moduwu_design::dropdown_submenu(ui, SORT_GROUPS[group].0, |ui| {
        ui.set_min_width(220.0);
        for (index, (label, _)) in SORT_GROUPS.iter().enumerate() {
            ui.selectable_value(&mut group, index, *label);
        }
    });
    if group != previous {
        *sort = SORT_GROUPS[group].1[0].0;
    }
    ui.horizontal(|ui| {
        for (order, label) in SORT_GROUPS[group].1 {
            ui.selectable_value(sort, order, label);
        }
    });

    ui.separator();
    ui.strong("RAW + JPEG");
    ui.checkbox(stack_raw_companions, "Show only the RAW")
        .on_hover_text(
            "Cameras shooting RAW+JPEG or RAW+HEIC save both files. Show each pair once, \
             as the RAW, and hide the JPEG or HEIC with the same name.",
        );

    #[cfg(not(target_os = "android"))]
    {
        ui.separator();
        ui.strong("Show ratings");
        let had_ratings = filter.ratings.iter().any(|selected| *selected);
        ui.horizontal(|ui| {
            if ui.selectable_label(!had_ratings, "All ratings").clicked() {
                filter.ratings = [false; 6];
            }
            ui.toggle_value(&mut filter.ratings[0], "Unrated");
        });
        ui.horizontal(|ui| {
            for rating in (1..=5).rev() {
                ui.toggle_value(
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
        if ui.selectable_label(!had_flags, "All flags").clicked() {
            filter.flags = [false; 3];
        }
        ui.horizontal(|ui| {
            ui.toggle_value(
                &mut filter.flags[0],
                egui::RichText::new(format!("{} Pick", egui_phosphor::regular::FLAG))
                    .color(crate::ui::theme::CHANNEL_GREEN),
            );
            ui.toggle_value(&mut filter.flags[1], "Unflagged");
            ui.toggle_value(
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
        if moduwu_design::menu_item(ui, filter.active(), "Clear filters").clicked() {
            *filter = LibraryReviewFilter::default();
        }
    }
}

pub(super) fn sort_filter_popup(
    ui: &mut Ui,
    sort: &mut LibrarySortOrder,
    filter: &mut LibraryReviewFilter,
    stack_raw_companions: &mut bool,
    compact_size: Option<&mut LibraryThumbnailSize>,
    width: f32,
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
        egui::ScrollArea::vertical()
            .max_height(480.0)
            .show(ui, |ui| {
                show_sort_filter_options(ui, sort, filter, stack_raw_companions);
                if let Some(size) = compact_size {
                    ui.separator();
                    ui.strong("Thumbnail size");
                    ui.horizontal_wrapped(|ui| {
                        for value in LibraryThumbnailSize::ALL {
                            ui.selectable_value(size, value, value.label());
                        }
                    });
                }
            });
    });
}
