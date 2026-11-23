use super::*;
use crate::i18n::{tr, tr_fmt};
use crate::sidecar::{PhotoFlag, PhotoReview};
use egui_phosphor::regular;
use moduwu_design::MenuItem;

/// An empty selection means all values; choices within a group are combined with OR.
/// The rating, flag, edit and filename groups are combined with AND.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct LibraryPhotoFilter {
    pub(super) ratings: [bool; 6],
    pub(super) flags: [bool; 3],
    /// In the order of `edit_labels`: edited, unedited.
    pub(super) edits: [bool; 2],
}

impl LibraryPhotoFilter {
    pub(super) fn active(self) -> bool {
        any_selected(&self.ratings) || any_selected(&self.flags) || any_selected(&self.edits)
    }

    /// Whether a photo with `review`, whose saved edits differ from its start
    /// when `edited`, passes the filter.
    pub(super) fn matches(self, review: PhotoReview, edited: bool) -> bool {
        let flag = match review.flag {
            PhotoFlag::Picked => 0,
            PhotoFlag::Unflagged => 1,
            PhotoFlag::Rejected => 2,
        };
        let edit = if edited { 0 } else { 1 };
        (!any_selected(&self.ratings) || self.ratings[usize::from(review.rating.min(5))])
            && (!any_selected(&self.flags) || self.flags[flag])
            && (!any_selected(&self.edits) || self.edits[edit])
    }

    /// The ratings the filter shows, such as "5★, 4★", or `None` for all.
    fn rating_summary(self) -> Option<String> {
        any_selected(&self.ratings).then(|| {
            (0..=5)
                .rev()
                .filter(|rating| self.ratings[*rating])
                .map(|rating| {
                    if rating == 0 {
                        tr("unrated").to_owned()
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
        any_selected(&self.flags).then(|| {
            flag_labels()
                .into_iter()
                .enumerate()
                .filter(|(index, _)| self.flags[*index])
                .map(|(_, label)| label)
                .collect::<Vec<_>>()
                .join(", ")
        })
    }

    /// The edit states the filter shows, such as "Edited", or `None` for all.
    fn edit_summary(self) -> Option<String> {
        any_selected(&self.edits).then(|| {
            edit_labels()
                .into_iter()
                .zip(self.edits)
                .filter(|(_, selected)| *selected)
                .map(|(label, _)| label)
                .collect::<Vec<_>>()
                .join(", ")
        })
    }

    pub(super) fn summary(self) -> String {
        [
            self.rating_summary(),
            self.flag_summary(),
            self.edit_summary(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ")
    }
}

/// The flag filter's labels, in the order of `LibraryPhotoFilter::flags`.
fn flag_labels() -> [&'static str; 3] {
    [tr("Pick"), tr("Unflagged"), tr("Reject")]
}

/// The edit filter's labels, in the order of `LibraryPhotoFilter::edits`.
fn edit_labels() -> [&'static str; 2] {
    [tr("Edited"), tr("Unedited")]
}

fn any_selected(choices: &[bool]) -> bool {
    choices.iter().any(|selected| *selected)
}

/// The icon of what a sort key sorts by.
fn sort_key_icon(key: LibrarySortKey) -> &'static str {
    match key {
        LibrarySortKey::Date => regular::CALENDAR_BLANK,
        LibrarySortKey::Name => regular::TEXT_AA,
        LibrarySortKey::Size => regular::FILE,
        LibrarySortKey::Rating => regular::STAR,
        LibrarySortKey::Flag => regular::FLAG,
        LibrarySortKey::Edits => regular::SLIDERS_HORIZONTAL,
    }
}

/// Sort & filter, laid out like the other menus and the same on every
/// platform: what to sort by as a list that closes the menu once a choice is
/// made, and the direction as a switch below it; the RAW + JPEG switch; the
/// rating, flag and edit filters, whose lists stay open for several choices;
/// Clear filters; and, where the header has no room for the View menu, its
/// options. Only a list closes the menu, so a sheet gets a close button.
fn sort_filter_menu(
    ui: &mut Ui,
    sort: &mut LibrarySortOrder,
    filter: &mut LibraryPhotoFilter,
    stack_raw_companions: &mut bool,
    compact_view: Option<&mut LibraryView>,
) {
    moduwu_design::menu_title_with_close(ui, tr("Sort & filter"));
    let sort_icon = match sort.direction {
        LibrarySortDirection::Ascending => regular::SORT_ASCENDING,
        LibrarySortDirection::Descending => regular::SORT_DESCENDING,
    };
    let sort_item = MenuItem::new(tr("Sort by"))
        .icon(sort_icon)
        .value(sort.key.label());
    moduwu_design::submenu(ui, sort_item, |ui| {
        for key in LibrarySortKey::ALL {
            let item = MenuItem::new(key.label())
                .icon(sort_key_icon(key))
                .selected(sort.key == key);
            if ui.add(item).clicked() {
                if sort.key != key {
                    *sort = LibrarySortOrder::by(key);
                }
                ui.close();
            }
        }
    });
    // The key's usual order on the left, its reverse on the right.
    let usual = sort.key.usual_direction();
    let reversed = match usual {
        LibrarySortDirection::Ascending => LibrarySortDirection::Descending,
        LibrarySortDirection::Descending => LibrarySortDirection::Ascending,
    };
    ui.add_space(moduwu_design::SPACE_XS);
    let width = menu_choice_width(ui);
    moduwu_design::segmented_control(
        ui,
        &mut sort.direction,
        &[
            (usual, sort.key.direction_label(usual)),
            (reversed, sort.key.direction_label(reversed)),
        ],
        width,
    );
    moduwu_design::menu_checkbox_item(ui, stack_raw_companions, tr("Show only the RAW"))
        .on_hover_text(tr(
            "Cameras shooting RAW+JPEG or RAW+HEIC save both files. Show each pair once, \
             as the RAW, and hide the JPEG or HEIC with the same name.",
        ));

    ui.separator();
    let rating_item = MenuItem::new(tr("Rating")).icon(regular::STAR).value(
        filter
            .rating_summary()
            .unwrap_or_else(|| tr("All").to_owned()),
    );
    moduwu_design::submenu(ui, rating_item, |ui| rating_filter_menu(ui, sort, filter));
    let flag_item = MenuItem::new(tr("Flag")).icon(regular::FLAG).value(
        filter
            .flag_summary()
            .unwrap_or_else(|| tr("All").to_owned()),
    );
    moduwu_design::submenu(ui, flag_item, |ui| flag_filter_menu(ui, sort, filter));
    let edit_item = MenuItem::new(tr("Edits"))
        .icon(regular::SLIDERS_HORIZONTAL)
        .value(
            filter
                .edit_summary()
                .unwrap_or_else(|| tr("All").to_owned()),
        );
    moduwu_design::submenu(ui, edit_item, |ui| edit_filter_menu(ui, filter));
    let clear_item = MenuItem::new(tr("Clear filters")).icon(regular::ARROW_COUNTER_CLOCKWISE);
    if ui.add_enabled(filter.active(), clear_item).clicked() {
        *filter = LibraryPhotoFilter::default();
    }
    if let Some(view) = compact_view {
        ui.separator();
        view_menu_items(ui, view);
    }
}

/// How wide a menu's segmented choices are in a popup; a sheet gives them
/// its own width.
const MENU_CHOICE_WIDTH: f32 = 320.0;

fn menu_choice_width(ui: &Ui) -> f32 {
    if moduwu_design::menu_sheet_open(ui.ctx()) {
        ui.available_width().max(1.0)
    } else {
        MENU_CHOICE_WIDTH
    }
}

/// The View menu's options, each a segmented control like the app's tabs
/// under a static label. Choosing keeps the menu open, so several options can
/// be changed at once.
fn view_menu_items(ui: &mut Ui, view: &mut LibraryView) {
    let width = menu_choice_width(ui);
    let sizes = LibraryThumbnailSize::ALL.map(|size| (size, size.label()));
    view_choice(
        ui,
        tr("Thumbnail size"),
        &mut view.thumbnail_size,
        &sizes,
        width,
    );
    view_choice(
        ui,
        tr("Thumbnail shape"),
        &mut view.square_thumbnails,
        &[(false, tr("Original")), (true, tr("Square"))],
        width,
    );
    view_choice(
        ui,
        tr("Folders"),
        &mut view.include_subfolders,
        &[(false, tr("This folder")), (true, tr("Include subfolders"))],
        width,
    );
}

fn view_choice<T: PartialEq + Copy>(
    ui: &mut Ui,
    heading: &str,
    value: &mut T,
    options: &[(T, &str)],
    width: f32,
) {
    ui.add_space(moduwu_design::SPACE_XS);
    ui.weak(heading);
    moduwu_design::segmented_control(ui, value, options, width);
}

/// The header's View menu, where it has room for one.
pub(super) fn view_popup(ui: &mut Ui, view: &mut LibraryView, width: f32) {
    let label = format!("{} {}", tr("View"), regular::CARET_DOWN);
    let response = moduwu_design::toolbar_button(ui, label, width);
    moduwu_design::dropdown_menu(&response, |ui| {
        moduwu_design::menu_title_with_close(ui, tr("View"));
        view_menu_items(ui, view);
    });
}

/// The ratings to show; several can be chosen. The first one chosen sorts by
/// rating, unless the photos already are.
fn rating_filter_menu(ui: &mut Ui, sort: &mut LibrarySortOrder, filter: &mut LibraryPhotoFilter) {
    let had_ratings = any_selected(&filter.ratings);
    if ui
        .add(MenuItem::new(tr("All")).selected(!had_ratings))
        .clicked()
    {
        filter.ratings = [false; 6];
    }
    for rating in (0..=5).rev() {
        let label = match rating {
            0 => tr("Unrated").to_owned(),
            1 => tr("1 star").to_owned(),
            _ => tr_fmt("{rating} stars", &[("rating", &rating)]),
        };
        let selected = &mut filter.ratings[rating];
        if ui.add(MenuItem::new(label).selected(*selected)).clicked() {
            *selected = !*selected;
        }
    }
    if !had_ratings && any_selected(&filter.ratings) && sort.key != LibrarySortKey::Rating {
        *sort = LibrarySortOrder::by(LibrarySortKey::Rating);
    }
}

/// The flags to show; several can be chosen. The first one chosen sorts by
/// flag, unless the photos are sorted by rating or flag already.
fn flag_filter_menu(ui: &mut Ui, sort: &mut LibrarySortOrder, filter: &mut LibraryPhotoFilter) {
    let had_flags = any_selected(&filter.flags);
    if ui
        .add(MenuItem::new(tr("All")).selected(!had_flags))
        .clicked()
    {
        filter.flags = [false; 3];
    }
    for (selected, label) in filter.flags.iter_mut().zip(flag_labels()) {
        if ui.add(MenuItem::new(label).selected(*selected)).clicked() {
            *selected = !*selected;
        }
    }
    if !had_flags
        && any_selected(&filter.flags)
        && !any_selected(&filter.ratings)
        && sort.key != LibrarySortKey::Flag
    {
        *sort = LibrarySortOrder::by(LibrarySortKey::Flag);
    }
}

/// The edit states to show. Unlike ratings and flags, choosing one does not
/// sort by it: the photos shown then all share it.
fn edit_filter_menu(ui: &mut Ui, filter: &mut LibraryPhotoFilter) {
    if ui
        .add(MenuItem::new(tr("All")).selected(!any_selected(&filter.edits)))
        .clicked()
    {
        filter.edits = [false; 2];
    }
    for (selected, label) in filter.edits.iter_mut().zip(edit_labels()) {
        if ui.add(MenuItem::new(label).selected(*selected)).clicked() {
            *selected = !*selected;
        }
    }
}

pub(super) fn sort_filter_popup(
    ui: &mut Ui,
    sort: &mut LibrarySortOrder,
    filter: &mut LibraryPhotoFilter,
    stack_raw_companions: &mut bool,
    compact_view: Option<&mut LibraryView>,
    width: f32,
) {
    let compact = compact_view.is_some();
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
            "{}{} {}",
            tr("Sort & filter"),
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
        sort_filter_menu(ui, sort, filter, stack_raw_companions, compact_view);
    });
}
