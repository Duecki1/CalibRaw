use super::*;
use crate::sidecar::{PhotoFlag, PhotoReview};
use egui_phosphor::regular;
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

    /// The ratings the filter shows, such as "5★, 4★", or `None` for all.
    fn rating_summary(self) -> Option<String> {
        self.ratings.iter().any(|selected| *selected).then(|| {
            (0..=5)
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
                .join(", ")
        })
    }

    /// The flags the filter shows, such as "Pick, Reject", or `None` for all.
    fn flag_summary(self) -> Option<String> {
        self.flags.iter().any(|selected| *selected).then(|| {
            FLAG_LABELS
                .into_iter()
                .enumerate()
                .filter(|(index, _)| self.flags[*index])
                .map(|(_, label)| label)
                .collect::<Vec<_>>()
                .join(", ")
        })
    }

    pub(super) fn summary(self) -> String {
        [self.rating_summary(), self.flag_summary()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

/// The flag filter's values, in the order of `LibraryReviewFilter::flags`.
const FLAG_LABELS: [&str; 3] = ["Pick", "Unflagged", "Reject"];

/// Every sort order, with the icon of what it sorts by.
const SORT_ORDERS: [(LibrarySortOrder, &str); 10] = [
    (LibrarySortOrder::NewestFirst, regular::CALENDAR_BLANK),
    (LibrarySortOrder::OldestFirst, regular::CALENDAR_BLANK),
    (LibrarySortOrder::NameAscending, regular::TEXT_AA),
    (LibrarySortOrder::NameDescending, regular::TEXT_AA),
    (LibrarySortOrder::LargestFirst, regular::FILE),
    (LibrarySortOrder::SmallestFirst, regular::FILE),
    (LibrarySortOrder::RatingHighestFirst, regular::STAR),
    (LibrarySortOrder::RatingLowestFirst, regular::STAR),
    (LibrarySortOrder::FlagPickedFirst, regular::FLAG),
    (LibrarySortOrder::FlagRejectedFirst, regular::FLAG),
];

/// Sort & filter, laid out like the other menus and the same on every
/// platform: the sort order and, where the header has no room for its own
/// control, the thumbnail size as lists that close the menu once a choice is
/// made; the RAW + JPEG switch; the rating and flag filters, whose lists stay
/// open for several choices; and Clear filters. Only a list closes the menu,
/// so a sheet gets a close button.
fn sort_filter_menu(
    ui: &mut Ui,
    sort: &mut LibrarySortOrder,
    filter: &mut LibraryReviewFilter,
    stack_raw_companions: &mut bool,
    compact_size: Option<&mut LibraryThumbnailSize>,
) {
    moduwu_design::menu_title_with_close(ui, "Sort & filter");
    let sort_item = MenuItem::new("Sort by")
        .icon(regular::SORT_ASCENDING)
        .value(sort.label());
    moduwu_design::submenu(ui, sort_item, |ui| {
        for (order, icon) in SORT_ORDERS {
            let item = MenuItem::new(order.label())
                .icon(icon)
                .selected(*sort == order);
            if ui.add(item).clicked() {
                *sort = order;
                ui.close();
            }
        }
    });
    if let Some(size) = compact_size {
        let size_item = MenuItem::new("Thumbnail size")
            .icon(regular::SQUARES_FOUR)
            .value(size.label());
        moduwu_design::submenu(ui, size_item, |ui| {
            for value in LibraryThumbnailSize::ALL {
                if ui
                    .add(MenuItem::new(value.label()).selected(*size == value))
                    .clicked()
                {
                    *size = value;
                    ui.close();
                }
            }
        });
    }
    moduwu_design::menu_checkbox_item(ui, stack_raw_companions, "Show only the RAW").on_hover_text(
        "Cameras shooting RAW+JPEG or RAW+HEIC save both files. Show each pair once, \
             as the RAW, and hide the JPEG or HEIC with the same name.",
    );

    ui.separator();
    let rating_item = MenuItem::new("Rating")
        .icon(regular::STAR)
        .value(filter.rating_summary().unwrap_or_else(|| "All".to_owned()));
    moduwu_design::submenu(ui, rating_item, |ui| rating_filter_menu(ui, sort, filter));
    let flag_item = MenuItem::new("Flag")
        .icon(regular::FLAG)
        .value(filter.flag_summary().unwrap_or_else(|| "All".to_owned()));
    moduwu_design::submenu(ui, flag_item, |ui| flag_filter_menu(ui, sort, filter));
    let clear_item = MenuItem::new("Clear filters").icon(regular::ARROW_COUNTER_CLOCKWISE);
    if ui.add_enabled(filter.active(), clear_item).clicked() {
        *filter = LibraryReviewFilter::default();
    }
}

/// The ratings to show; several can be chosen. The first one chosen sorts by
/// rating, unless the photos already are.
fn rating_filter_menu(ui: &mut Ui, sort: &mut LibrarySortOrder, filter: &mut LibraryReviewFilter) {
    let had_ratings = filter.ratings.iter().any(|selected| *selected);
    if ui
        .add(MenuItem::new("All").selected(!had_ratings))
        .clicked()
    {
        filter.ratings = [false; 6];
    }
    for rating in (0..=5).rev() {
        let label = match rating {
            0 => "Unrated".to_owned(),
            1 => "1 star".to_owned(),
            _ => format!("{rating} stars"),
        };
        let selected = &mut filter.ratings[rating];
        if ui.add(MenuItem::new(label).selected(*selected)).clicked() {
            *selected = !*selected;
        }
    }
    if !had_ratings
        && filter.ratings.iter().any(|selected| *selected)
        && !matches!(
            sort,
            LibrarySortOrder::RatingHighestFirst | LibrarySortOrder::RatingLowestFirst
        )
    {
        *sort = LibrarySortOrder::RatingHighestFirst;
    }
}

/// The flags to show; several can be chosen. The first one chosen sorts by
/// flag, unless the photos are sorted by rating or flag already.
fn flag_filter_menu(ui: &mut Ui, sort: &mut LibrarySortOrder, filter: &mut LibraryReviewFilter) {
    let had_flags = filter.flags.iter().any(|selected| *selected);
    if ui.add(MenuItem::new("All").selected(!had_flags)).clicked() {
        filter.flags = [false; 3];
    }
    for (selected, label) in filter.flags.iter_mut().zip(FLAG_LABELS) {
        if ui.add(MenuItem::new(label).selected(*selected)).clicked() {
            *selected = !*selected;
        }
    }
    let has_ratings = filter.ratings.iter().any(|selected| *selected);
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
            regular::SLIDERS_HORIZONTAL,
            if filter.active() { " •" } else { "" }
        ))
        .size(moduwu_design::CONTROL_HEIGHT * 0.55)
        .into()
    } else {
        format!(
            "Sort & filter{} {}",
            if filter.active() { " •" } else { "" },
            regular::CARET_DOWN
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
        sort_filter_menu(ui, sort, filter, stack_raw_companions, compact_size);
    });
}
