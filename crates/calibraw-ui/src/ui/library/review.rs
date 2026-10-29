use super::hover::BUTTON_HIGHLIGHT_RADIUS;
use super::*;
use crate::i18n::{tr, tr_fmt};
use crate::sidecar::{PhotoFlag, PhotoReview};
use crate::ui::theme;

const STAR_COLOR: Color32 = Color32::from_rgb(234, 195, 112);
const STAR_PREVIEW_COLOR: Color32 = Color32::from_rgb(255, 232, 183);
const MUTED: Color32 = Color32::from_rgb(137, 145, 155);
const EDITOR_WIDTH: f32 = 196.0;

/// How a rating or flag control or key changes a photo's review.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReviewChange {
    /// Sets the rating; 0 clears it.
    Rating(u8),
    /// Raises or lowers the rating by one star, within 0–5.
    StepRating(i8),
    Flag(PhotoFlag),
    /// Picks a photo that is not picked and unflags a picked one.
    TogglePick,
}

impl ReviewChange {
    pub(crate) fn apply(self, mut review: PhotoReview) -> PhotoReview {
        match self {
            Self::Rating(rating) => review.rating = rating.min(5),
            Self::StepRating(step) => {
                review.rating = review.rating.saturating_add_signed(step).min(5)
            }
            Self::Flag(flag) => review.flag = flag,
            Self::TogglePick => {
                review.flag = if review.flag == PhotoFlag::Picked {
                    PhotoFlag::Unflagged
                } else {
                    PhotoFlag::Picked
                };
            }
        }
        review
    }
}

/// A rating or flag key, as in Lightroom: 0–5, [ and ], P, X, U and `.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ReviewKey {
    pub(crate) change: ReviewChange,
    /// Shift was held: after the change, move on to the next photo.
    pub(crate) advance: bool,
}

/// Takes this frame's first rating or flag key press. Digits, brackets and
/// the backtick also match by position, so they work with Shift held and on
/// layouts that need Shift for digits; letters match the key's own label.
pub(crate) fn take_review_key(ctx: &egui::Context) -> Option<ReviewKey> {
    ctx.input_mut(|input| {
        let position = input
            .events
            .iter()
            .position(|event| review_key(event).is_some())?;
        review_key(&input.events.remove(position))
    })
}

fn review_key(event: &egui::Event) -> Option<ReviewKey> {
    use egui::Key;
    let egui::Event::Key {
        key,
        physical_key,
        pressed: true,
        repeat: false,
        modifiers,
    } = event
    else {
        return None;
    };
    if modifiers.command || modifiers.ctrl || modifiers.alt {
        return None;
    }
    let at = |target: Key| *key == target || *physical_key == Some(target);
    let digits = [
        Key::Num0,
        Key::Num1,
        Key::Num2,
        Key::Num3,
        Key::Num4,
        Key::Num5,
    ];
    let change = if let Some(rating) = digits.into_iter().position(at) {
        ReviewChange::Rating(rating as u8)
    } else if at(Key::OpenBracket) {
        ReviewChange::StepRating(-1)
    } else if at(Key::CloseBracket) {
        ReviewChange::StepRating(1)
    } else if at(Key::Backtick) {
        ReviewChange::TogglePick
    } else {
        match key {
            Key::P => ReviewChange::Flag(PhotoFlag::Picked),
            Key::X => ReviewChange::Flag(PhotoFlag::Rejected),
            Key::U => ReviewChange::Flag(PhotoFlag::Unflagged),
            _ => return None,
        }
    };
    Some(ReviewKey {
        change,
        advance: modifiers.shift,
    })
}

/// The confirmation shown briefly after a rating or flag key.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ReviewFeedback {
    /// For one photo, the rating or flag it now has.
    change: ReviewChange,
    count: usize,
    shown_at: f64,
}

impl ReviewFeedback {
    pub(crate) fn new(
        change: ReviewChange,
        count: usize,
        single_result: Option<PhotoReview>,
        now: f64,
    ) -> Self {
        let change = match (change, single_result) {
            (ReviewChange::StepRating(_), Some(review)) => ReviewChange::Rating(review.rating),
            (ReviewChange::TogglePick, Some(review)) => ReviewChange::Flag(review.flag),
            _ => change,
        };
        Self {
            change,
            count,
            shown_at: now,
        }
    }
}

const FEEDBACK_SECONDS: f64 = 1.2;
const FEEDBACK_FADE_SECONDS: f64 = 0.3;

/// Paints the confirmation over the middle of the window until it expires.
pub(crate) fn show_review_feedback(ctx: &egui::Context, feedback: &mut Option<ReviewFeedback>) {
    let Some(shown) = *feedback else {
        return;
    };
    let remaining = FEEDBACK_SECONDS - (ctx.input(|input| input.time) - shown.shown_at);
    if remaining <= 0.0 {
        *feedback = None;
        return;
    }
    egui::Area::new(egui::Id::new("library-review-feedback"))
        .order(egui::Order::Tooltip)
        .interactable(false)
        .anchor(Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.set_opacity((remaining / FEEDBACK_FADE_SECONDS).clamp(0.0, 1.0) as f32);
            egui::Frame::popup(ui.style())
                .inner_margin(egui::Margin::symmetric(14, 8))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = moduwu_design::SPACE_XS;
                        paint_feedback_change(ui, shown.change);
                        if shown.count > 1 {
                            ui.label(tr_fmt("· {count} photos", &[("count", &shown.count)]));
                        }
                    });
                });
        });
    ctx.request_repaint();
}

fn paint_feedback_change(ui: &mut Ui, change: ReviewChange) {
    const ICON: f32 = 18.0;
    let icon = |ui: &mut Ui, paint: &dyn Fn(&egui::Painter, egui::Pos2)| {
        let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(ICON), Sense::hover());
        paint(ui.painter(), rect.center());
    };
    let star = |ui: &mut Ui| {
        icon(ui, &|painter, center| {
            paint_star(painter, center, ICON, STAR_COLOR, true)
        });
    };
    match change {
        ReviewChange::Rating(0) => {
            ui.label(tr("No rating"));
        }
        ReviewChange::Rating(rating) => {
            for _ in 0..rating {
                star(ui);
            }
        }
        ReviewChange::StepRating(step) => {
            star(ui);
            ui.label(if step > 0 { "+1" } else { "−1" });
        }
        ReviewChange::Flag(flag) => {
            icon(ui, &|painter, center| {
                paint_flag(
                    painter,
                    center,
                    ICON,
                    flag_color(flag),
                    flag != PhotoFlag::Unflagged,
                )
            });
            ui.label(flag_label(flag));
        }
        ReviewChange::TogglePick => {
            icon(ui, &|painter, center| {
                paint_flag(painter, center, ICON, flag_color(PhotoFlag::Picked), true)
            });
            ui.label(tr("Pick toggled"));
        }
    }
}

fn flag_label(flag: PhotoFlag) -> &'static str {
    match flag {
        PhotoFlag::Picked => tr("Pick"),
        PhotoFlag::Rejected => tr("Reject"),
        PhotoFlag::Unflagged => tr("Unflagged"),
    }
}

// Review badge look, in points at full size.
const BADGE_MARGIN: f32 = 8.0;
const BADGE_HEIGHT: f32 = 22.0;
const BADGE_RADIUS: f32 = 6.0;
/// Also the library hover card's fill.
pub(super) const BADGE_FILL: Color32 = Color32::from_black_alpha(150);
/// Between the centres of neighbouring icons.
const BADGE_ICON_STEP: f32 = 15.0;
/// From a badge's edge to the centre of its first icon.
const BADGE_ICON_INSET: f32 = 13.5;
const BADGE_STAR_SIZE: f32 = 11.0;
const BADGE_FLAG_SIZE: f32 = 12.0;

/// The largest star and flag icon on rating and flag buttons, in points.
pub(super) const REVIEW_ICON_SIZE: f32 = 15.0;

pub(crate) fn paint_review_badge(ui: &Ui, rect: egui::Rect, review: PhotoReview) {
    const BADGE_PADDING: f32 = 12.0;
    const BADGE_GAP: f32 = 4.0;
    const COMPACT_RATING_WIDTH: f32 = 43.0;

    let has_rating = review.rating > 0;
    let has_flag = review.flag != PhotoFlag::Unflagged;
    if !has_rating && !has_flag {
        return;
    }

    let full_rating_width = f32::from(review.rating) * BADGE_ICON_STEP + BADGE_PADDING;
    let flag_width = BADGE_ICON_STEP + BADGE_PADDING;
    let required_width = |rating_width: f32| {
        let rating = if has_rating { rating_width } else { 0.0 };
        let flag = match (has_flag, has_rating) {
            (true, true) => flag_width + BADGE_GAP,
            (true, false) => flag_width,
            (false, _) => 0.0,
        };
        rating + flag + BADGE_MARGIN * 2.0
    };
    let compact_rating = review.rating >= 3 && required_width(full_rating_width) > rect.width();
    let rating_width = if compact_rating {
        COMPACT_RATING_WIDTH
    } else {
        full_rating_width
    };
    let scale = (rect.width() / required_width(rating_width)).clamp(0.0, 1.0);
    let margin = BADGE_MARGIN * scale;
    let badge_y = rect.bottom() - margin - BADGE_HEIGHT * scale;
    let painter = ui.painter_at(rect);

    if has_rating {
        let rating_badge = egui::Rect::from_min_size(
            egui::pos2(rect.left() + margin, badge_y),
            egui::vec2(rating_width * scale, BADGE_HEIGHT * scale),
        );
        painter.rect_filled(rating_badge, BADGE_RADIUS * scale, BADGE_FILL);
        let painted_stars = if compact_rating { 1 } else { review.rating };
        for index in 0..painted_stars {
            paint_star(
                &painter,
                egui::pos2(
                    rating_badge.left()
                        + (BADGE_ICON_INSET + f32::from(index) * BADGE_ICON_STEP) * scale,
                    rating_badge.center().y,
                ),
                BADGE_STAR_SIZE * scale,
                STAR_COLOR,
                true,
            );
        }
        if compact_rating {
            painter.text(
                egui::pos2(rating_badge.left() + 29.5 * scale, rating_badge.center().y),
                Align2::CENTER_CENTER,
                review.rating.to_string(),
                FontId::proportional(11.0 * scale),
                Color32::WHITE,
            );
        }
    }

    if has_flag {
        let flag_badge = egui::Rect::from_min_size(
            egui::pos2(rect.right() - margin - flag_width * scale, badge_y),
            egui::vec2(flag_width * scale, BADGE_HEIGHT * scale),
        );
        painter.rect_filled(flag_badge, BADGE_RADIUS * scale, BADGE_FILL);
        paint_flag(
            &painter,
            flag_badge.center(),
            BADGE_FLAG_SIZE * scale,
            flag_color(review.flag),
            true,
        );
    }
}

fn flag_color(flag: PhotoFlag) -> Color32 {
    match flag {
        PhotoFlag::Picked => theme::CHANNEL_GREEN,
        PhotoFlag::Rejected => theme::CHANNEL_RED,
        PhotoFlag::Unflagged => MUTED,
    }
}

/// What review buttons sit on, which sets their idle look.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ReviewSurface {
    /// The Develop toolbar, on its button fill.
    Toolbar,
    /// A library photo, on its hover card.
    Photo,
}

/// The Develop toolbar's rating and flag editor in `bounds`: five stars, a
/// divider, then Pick and Reject.
fn review_editor(
    ui: &Ui,
    bounds: egui::Rect,
    id_salt: impl std::hash::Hash + std::fmt::Debug + Copy,
    review: PhotoReview,
) -> Option<ReviewChange> {
    // The same surface treatment as the app's standard toolbar buttons, so
    // the review controls read as one compact group.
    let painter = ui.painter_at(bounds);
    paint_button_frame(&painter, bounds, &ui.visuals().widgets.inactive);
    let edge = (bounds.width() - 24.0) / 7.0;
    let button = |index: usize| {
        let divider = if index >= 5 { 12.0 } else { 0.0 };
        egui::Rect::from_min_size(
            egui::pos2(
                bounds.left() + 6.0 + edge * index as f32 + divider,
                bounds.top(),
            ),
            egui::vec2(edge, bounds.height()),
        )
    };
    let icon_size = (edge * 0.62).min(REVIEW_ICON_SIZE);
    let surface = ReviewSurface::Toolbar;
    let rating = star_buttons(
        ui,
        std::array::from_fn(button),
        icon_size,
        id_salt,
        review,
        surface,
    );
    let flag = flag_buttons(
        ui,
        [button(5), button(6)],
        icon_size,
        id_salt,
        review,
        surface,
    );
    let x = bounds.left() + 6.0 + edge * 5.0 + 6.0;
    painter.line_segment(
        [
            egui::pos2(x, bounds.top() + 9.0),
            egui::pos2(x, bounds.bottom() - 9.0),
        ],
        Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
    );
    rating.or(flag)
}

/// Five rating stars at `buttons`. Returns the rating the clicked star sets;
/// clicking the current rating clears it.
pub(super) fn star_buttons(
    ui: &Ui,
    buttons: [egui::Rect; 5],
    icon_size: f32,
    id_salt: impl std::hash::Hash + std::fmt::Debug + Copy,
    review: PhotoReview,
    surface: ReviewSurface,
) -> Option<ReviewChange> {
    let hovered_rating = buttons
        .iter()
        .position(|button| ui.rect_contains_pointer(*button))
        .map(|index| index as u8 + 1);
    let mut change = None;
    for (index, button) in buttons.into_iter().enumerate() {
        let rating = index as u8 + 1;
        let label = if rating == 1 {
            tr("1 star").to_owned()
        } else {
            tr_fmt("{rating} stars", &[("rating", &rating)])
        };
        let response = review_button(ui, button, ("photo-review", id_salt, index), surface, false);
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::WidgetType::RadioButton,
                true,
                review.rating == rating,
                &label,
            )
        });
        let active = review.rating >= rating;
        paint_star(
            ui.painter(),
            button.center(),
            icon_size,
            if active {
                STAR_COLOR
            } else if hovered_rating.is_some_and(|hovered| hovered >= rating) {
                STAR_PREVIEW_COLOR
            } else {
                match surface {
                    ReviewSurface::Toolbar => ui.visuals().weak_text_color(),
                    ReviewSurface::Photo => MUTED,
                }
            },
            active,
        );
        if response.clicked() {
            change = Some(ReviewChange::Rating(if review.rating == rating {
                0
            } else {
                rating
            }));
        }
    }
    change
}

/// Pick, then Reject, at `buttons`. Returns the flag the clicked button sets;
/// clicking the photo's own flag clears it.
pub(super) fn flag_buttons(
    ui: &Ui,
    buttons: [egui::Rect; 2],
    icon_size: f32,
    id_salt: impl std::hash::Hash + std::fmt::Debug + Copy,
    review: PhotoReview,
    surface: ReviewSurface,
) -> Option<ReviewChange> {
    let flags = [PhotoFlag::Picked, PhotoFlag::Rejected];
    let mut change = None;
    for (index, (flag, button)) in flags.into_iter().zip(buttons).enumerate() {
        let active = review.flag == flag;
        // The toolbar keeps its set flag highlighted.
        let marked = surface == ReviewSurface::Toolbar && active;
        // Numbered after the stars.
        let id_salt = ("photo-review", id_salt, 5 + index);
        let response = review_button(ui, button, id_salt, surface, marked);
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, active, flag_label(flag))
        });
        paint_flag(
            ui.painter(),
            button.center(),
            icon_size,
            flag_color(flag),
            active,
        );
        if response.clicked() {
            change = Some(ReviewChange::Flag(if active {
                PhotoFlag::Unflagged
            } else {
                flag
            }));
        }
    }
    change
}

/// A rating or flag button's click area, highlighted while hovered, focused,
/// pressed or `marked`.
fn review_button(
    ui: &Ui,
    button: egui::Rect,
    id_salt: impl std::hash::Hash + std::fmt::Debug,
    surface: ReviewSurface,
    marked: bool,
) -> egui::Response {
    let response = ui.interact(button, ui.make_persistent_id(id_salt), Sense::click());
    let engaged = response.has_focus() || response.is_pointer_button_down_on();
    if !(marked || engaged || response.hovered()) {
        return response;
    }
    let painter = ui.painter();
    match surface {
        ReviewSurface::Toolbar => paint_button_frame(
            painter,
            button.shrink2(egui::vec2(1.0, 4.0)),
            ui.style().interact(&response),
        ),
        // Fixed colours, as the card under the button is dark in every theme.
        ReviewSurface::Photo => {
            let fill = Color32::from_white_alpha(if engaged { 56 } else { 32 });
            painter.rect_filled(button, BUTTON_HIGHLIGHT_RADIUS, fill);
        }
    }
    response
}

/// A standard button's fill and outline in `visuals`.
fn paint_button_frame(
    painter: &egui::Painter,
    rect: egui::Rect,
    visuals: &egui::style::WidgetVisuals,
) {
    painter.rect_filled(rect, visuals.corner_radius, visuals.weak_bg_fill);
    painter.rect_stroke(
        rect,
        visuals.corner_radius,
        visuals.bg_stroke,
        StrokeKind::Inside,
    );
}

fn paint_star(
    painter: &egui::Painter,
    center: egui::Pos2,
    size: f32,
    color: Color32,
    filled: bool,
) {
    let points = (0..10)
        .map(|index| {
            let angle = std::f32::consts::TAU * index as f32 / 10.0 - std::f32::consts::FRAC_PI_2;
            let radius = size * if index % 2 == 0 { 0.5 } else { 0.23 };
            center + egui::vec2(angle.cos(), angle.sin()) * radius
        })
        .collect::<Vec<_>>();
    if filled {
        let mut mesh = egui::Mesh::default();
        mesh.colored_vertex(center, color);
        for point in &points {
            mesh.colored_vertex(*point, color);
        }
        for index in 0..10 {
            mesh.add_triangle(0, index + 1, (index + 1) % 10 + 1);
        }
        painter.add(egui::Shape::mesh(mesh));
    }
    painter.add(egui::Shape::closed_line(points, Stroke::new(1.0, color)));
}

fn paint_flag(
    painter: &egui::Painter,
    center: egui::Pos2,
    size: f32,
    color: Color32,
    filled: bool,
) {
    let origin = center - egui::vec2(size * 0.35, size * 0.45);
    painter.line_segment(
        [origin, origin + egui::vec2(0.0, size)],
        Stroke::new(1.4, color),
    );
    let points = vec![
        origin,
        origin + egui::vec2(size * 0.75, size * 0.13),
        origin + egui::vec2(size * 0.75, size * 0.63),
        origin + egui::vec2(0.0, size * 0.5),
    ];
    painter.add(egui::Shape::convex_polygon(
        points,
        if filled { color } else { Color32::TRANSPARENT },
        Stroke::new(1.2, color),
    ));
}

pub(super) const COMPACT_REVIEW_EDITOR_WIDTH: f32 = 92.0;

/// A button showing the rating and flag that opens the full editor.
pub(super) fn compact_review_editor(
    ui: &mut Ui,
    id_salt: impl std::hash::Hash + std::fmt::Debug + Copy,
    review: PhotoReview,
) -> Option<ReviewChange> {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(COMPACT_REVIEW_EDITOR_WIDTH, moduwu_design::CONTROL_HEIGHT),
        Sense::click(),
    );
    let painter = ui.painter_at(rect);
    paint_button_frame(&painter, rect, ui.style().interact(&response));
    paint_star(
        &painter,
        rect.left_center() + egui::vec2(14.0, 0.0),
        14.0,
        if review.rating > 0 {
            STAR_COLOR
        } else {
            ui.visuals().weak_text_color()
        },
        review.rating > 0,
    );
    painter.text(
        rect.left_center() + egui::vec2(31.0, 0.0),
        Align2::CENTER_CENTER,
        review.rating.to_string(),
        FontId::proportional(12.0),
        ui.visuals().text_color(),
    );
    paint_flag(
        &painter,
        rect.left_center() + egui::vec2(56.0, 0.0),
        14.0,
        if review.flag == PhotoFlag::Unflagged {
            ui.visuals().weak_text_color()
        } else {
            flag_color(review.flag)
        },
        review.flag != PhotoFlag::Unflagged,
    );
    painter.text(
        rect.right_center() - egui::vec2(13.0, 0.0),
        Align2::CENTER_CENTER,
        egui_phosphor::regular::CARET_DOWN,
        FontId::proportional(12.0),
        ui.visuals().weak_text_color(),
    );
    let mut change = None;
    moduwu_design::dropdown_menu(&response, |ui| {
        change = menu_review_row(ui, id_salt, review);
        if change.is_some() {
            ui.close();
        }
    });
    change
}

/// The width of a menu's rating and flag row on touch screens: seven
/// finger-sized buttons.
const TOUCH_MENU_EDITOR_WIDTH: f32 = 7.0 * 44.0 + 24.0;

/// The rating and flag editor as a menu row, wider on touch screens.
pub(super) fn menu_review_row(
    ui: &mut Ui,
    id_salt: impl std::hash::Hash + std::fmt::Debug + Copy,
    review: PhotoReview,
) -> Option<ReviewChange> {
    let width = if PlatformCaps::CURRENT.is_mobile() {
        TOUCH_MENU_EDITOR_WIDTH.min(ui.available_width())
    } else {
        EDITOR_WIDTH
    };
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(width, moduwu_design::CONTROL_HEIGHT),
        Sense::hover(),
    );
    review_editor(ui, rect, id_salt, review)
}

/// The open photo's rating and flag editor; `false` when no photo is open.
pub(crate) fn show_current_photo_review(ui: &mut Ui, app: &mut CalibRawApp, compact: bool) -> bool {
    if current_document_id(app).is_none() {
        return false;
    }
    const ID: &str = "develop-photo-review";
    let review = app.develop.review;
    let change = if !compact && app.caps().has_folders() && ui.available_width() >= EDITOR_WIDTH {
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(EDITOR_WIDTH, moduwu_design::CONTROL_HEIGHT),
            Sense::hover(),
        );
        review_editor(ui, rect, ID, review)
    } else {
        compact_review_editor(ui, ID, review)
    };
    if let Some(change) = change {
        if let Some(asset) = current_photo_asset(app) {
            app.review_photos(vec![asset], change);
        }
    }
    true
}

/// The library identity of the photo open in Develop.
pub(super) fn current_document_id(app: &CalibRawApp) -> Option<LibraryAssetId> {
    match app.persistence.sidecar_target.as_ref() {
        Some(crate::sidecar::SidecarTarget::Android { raw_uri, .. }) => {
            Some(LibraryAssetId::Android(raw_uri.clone()))
        }
        _ => app
            .develop
            .current_path
            .clone()
            .map(LibraryAssetId::Desktop),
    }
}

/// The photo open in Develop as a library asset: its library entry, or one
/// made for a photo opened from elsewhere.
pub(crate) fn current_photo_asset(app: &CalibRawApp) -> Option<LibraryAsset> {
    let asset_id = current_document_id(app)?;
    if let Some(index) = app.library.entry_indices.get(&asset_id) {
        return Some(app.library.entries[*index].asset.clone());
    }
    Some(match (asset_id, app.persistence.sidecar_target.as_ref()) {
        (
            LibraryAssetId::Android(uri),
            Some(crate::sidecar::SidecarTarget::Android { display_name, .. }),
        ) => LibraryAsset::from_android_document(crate::platform::LibraryDocument {
            uri,
            display_name: display_name.clone(),
            display_path: display_name.clone(),
            bytes: 0,
            modified_seconds: 0,
        }),
        (LibraryAssetId::Desktop(path), _) => LibraryAsset::from_desktop_path(path, 0, 0, None),
        // `current_document_id` gives an Android id only for an Android target.
        (LibraryAssetId::Android(_), _) => return None,
    })
}
