use super::*;
use crate::sidecar::{PhotoFlag, PhotoReview};
use crate::ui::theme;

const STAR_COLOR: Color32 = Color32::from_rgb(234, 195, 112);
const STAR_PREVIEW_COLOR: Color32 = Color32::from_rgb(255, 232, 183);
const MUTED: Color32 = Color32::from_rgb(137, 145, 155);
const HOVER_ANIMATION_SECONDS: f32 = 0.18;
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
                            ui.label(format!("· {} photos", shown.count));
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
    match change {
        ReviewChange::Rating(0) => {
            ui.label("No rating");
        }
        ReviewChange::Rating(rating) => {
            for _ in 0..rating {
                icon(ui, &|painter, center| {
                    paint_star(painter, center, ICON, STAR_COLOR, true)
                });
            }
        }
        ReviewChange::StepRating(step) => {
            icon(ui, &|painter, center| {
                paint_star(painter, center, ICON, STAR_COLOR, true)
            });
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
            ui.label("Pick toggled");
        }
    }
}

fn flag_label(flag: PhotoFlag) -> &'static str {
    match flag {
        PhotoFlag::Picked => "Pick",
        PhotoFlag::Rejected => "Reject",
        PhotoFlag::Unflagged => "Unflagged",
    }
}

/// One hover surface owns both photo details and review controls, including when
/// the pointer moves from the image onto a star or flag.
pub(super) fn thumbnail_hover_overlay(
    ui: &mut Ui,
    rect: egui::Rect,
    entry: &LibraryEntry,
    stacked_formats: &[crate::pipeline::RenderedImageFormat],
) -> Option<LibraryAction> {
    let asset = &entry.asset;
    let hovered = ui.rect_contains_pointer(rect);
    let progress = ui.ctx().animate_bool_with_time_and_easing(
        ui.make_persistent_id(("thumbnail-hover", &asset.id)),
        hovered,
        HOVER_ANIMATION_SECONDS,
        egui::emath::easing::cubic_out,
    );
    if progress <= 0.0 {
        paint_review_badge(ui, rect, entry.review());
        return None;
    }

    // Full-photo scrim with centered, gently sliding text.
    let mut painter = ui.painter_at(rect);
    painter.set_opacity(progress);
    painter.rect_filled(
        rect,
        moduwu_design::CARD_RADIUS,
        Color32::from_black_alpha(156),
    );
    let scale = ((rect.height() - 16.0) / 124.0).clamp(0.4, 1.0);
    let center = rect.center() + egui::vec2(0.0, 7.0 * (1.0 - progress));
    let detail = entry.thumbnail_error.clone().unwrap_or_else(|| {
        if entry.developed_thumbnail_pending {
            if entry.thumbnail_queued {
                "Rendering edits…"
            } else {
                "Preview awaiting saved edits"
            }
            .to_owned()
        } else {
            thumbnail_hover_details(asset, stacked_formats)
        }
    });
    for (text, size, offset, alpha) in hover_text_lines(ui, rect, asset, detail) {
        painter.text(
            center + egui::vec2(0.0, offset * scale),
            Align2::CENTER_CENTER,
            text,
            FontId::proportional(size),
            Color32::from_white_alpha(alpha),
        );
    }
    let edge = (26.0 * scale).min((rect.width() - 16.0).max(1.0) / 5.0);
    let controls = egui::Rect::from_min_size(
        center + egui::vec2(-edge * 2.5, 20.0 * scale - edge * 0.5),
        egui::vec2(edge * 5.0, edge * 2.0),
    );
    let change = ui
        .scope(|ui| {
            ui.set_opacity(progress);
            review_editor(ui, controls, &asset.id, entry.review(), true)
        })
        .inner;
    change
        .filter(|_| hovered)
        .map(|change| LibraryAction::Review(vec![asset.clone()], change))
}

/// Never squeeze complete metadata into ellipses or unreadably small text.
fn hover_text_lines(
    ui: &Ui,
    rect: egui::Rect,
    asset: &LibraryAsset,
    detail: String,
) -> Vec<(String, f32, f32, u8)> {
    if rect.width() < 64.0 || rect.height() < 84.0 {
        return Vec::new();
    }
    let fits = |text: &str, size: f32| {
        !text.is_empty()
            && ui
                .painter()
                .layout_no_wrap(text.to_owned(), FontId::proportional(size), Color32::WHITE)
                .size()
                .x
                <= rect.width() - 24.0
    };
    let capture = thumbnail_capture_details(asset);
    if rect.width() >= 180.0 && rect.height() >= 124.0 {
        let lines = [
            (asset.display_name.clone(), 14.5, -48.0, 255),
            (capture.clone(), 11.5, -27.0, 205),
            (detail, 10.5, -8.0, 175),
        ];
        let visible: Vec<_> = lines
            .into_iter()
            .filter(|(text, size, _, _)| fits(text, *size))
            .collect();
        if !visible.is_empty() {
            return visible;
        }
    }
    // Portrait thumbnails get at most one readable line. Prefer the name, then
    // a complete capture field (aperture first) when the name cannot fit.
    let candidates = std::iter::once(asset.display_name.clone())
        .chain(std::iter::once(capture.clone()))
        .chain(
            capture
                .split("  ·  ")
                .skip(2)
                .take(1)
                .filter(|text| !text.contains('—'))
                .map(str::to_owned),
        )
        .chain(
            capture
                .split("  ·  ")
                .filter(|text| !text.contains('—'))
                .map(str::to_owned),
        );
    candidates
        .into_iter()
        .find(|text| fits(text, 11.5))
        .map(|text| vec![(text, 11.5, -20.0, 255)])
        .unwrap_or_default()
}

pub(crate) fn paint_review_badge(ui: &Ui, rect: egui::Rect, review: PhotoReview) {
    const ICON_STEP: f32 = 15.0;
    const BADGE_PADDING: f32 = 12.0;
    const BADGE_HEIGHT: f32 = 22.0;
    const BADGE_MARGIN: f32 = 8.0;
    const BADGE_GAP: f32 = 4.0;
    const COMPACT_RATING_WIDTH: f32 = 43.0;

    let has_rating = review.rating > 0;
    let has_flag = review.flag != PhotoFlag::Unflagged;
    if !has_rating && !has_flag {
        return;
    }

    let full_rating_width = f32::from(review.rating) * ICON_STEP + BADGE_PADDING;
    let flag_width = ICON_STEP + BADGE_PADDING;
    let full_required_width = if has_rating { full_rating_width } else { 0.0 }
        + if has_flag {
            flag_width + if has_rating { BADGE_GAP } else { 0.0 }
        } else {
            0.0
        }
        + BADGE_MARGIN * 2.0;
    let compact_rating = review.rating >= 3 && full_required_width > rect.width();
    let rating_width = if compact_rating {
        COMPACT_RATING_WIDTH
    } else {
        full_rating_width
    };
    let required_width = if has_rating { rating_width } else { 0.0 }
        + if has_flag {
            flag_width + if has_rating { BADGE_GAP } else { 0.0 }
        } else {
            0.0
        }
        + BADGE_MARGIN * 2.0;
    let scale = (rect.width() / required_width).clamp(0.0, 1.0);
    let margin = 8.0 * scale;
    let badge_y = rect.bottom() - margin - BADGE_HEIGHT * scale;
    let painter = ui.painter_at(rect);

    if has_rating {
        let rating_badge = egui::Rect::from_min_size(
            egui::pos2(rect.left() + margin, badge_y),
            egui::vec2(rating_width * scale, BADGE_HEIGHT * scale),
        );
        painter.rect_filled(rating_badge, 6.0 * scale, Color32::from_black_alpha(150));
        let painted_stars = if compact_rating { 1 } else { review.rating };
        for index in 0..painted_stars {
            paint_star(
                &painter,
                egui::pos2(
                    rating_badge.left() + (13.5 + f32::from(index) * ICON_STEP) * scale,
                    rating_badge.center().y,
                ),
                11.0 * scale,
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
        painter.rect_filled(flag_badge, 6.0 * scale, Color32::from_black_alpha(150));
        paint_flag(
            &painter,
            flag_badge.center(),
            12.0 * scale,
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

fn review_editor(
    ui: &mut Ui,
    bounds: egui::Rect,
    id_salt: impl std::hash::Hash + std::fmt::Debug + Copy,
    review: PhotoReview,
    overlay: bool,
) -> Option<ReviewChange> {
    let painter = ui.painter_at(bounds);
    // Gallery stars and flags sit directly on the full-photo hover scrim.
    // In the Develop toolbar, use the same surface treatment as the app's
    // standard toolbar buttons so the review controls read as one compact group.
    if !overlay {
        let visuals = &ui.visuals().widgets.inactive;
        painter.rect_filled(bounds, visuals.corner_radius, visuals.weak_bg_fill);
        painter.rect_stroke(
            bounds,
            visuals.corner_radius,
            visuals.bg_stroke,
            StrokeKind::Inside,
        );
    }
    let edge = if overlay {
        bounds.width() / 5.0
    } else {
        (bounds.width() - 24.0) / 7.0
    };
    let mut change = None;
    let hovered_rating = (0..5)
        .find(|index| {
            let button = review_button_rect(bounds, edge, *index, overlay);
            ui.rect_contains_pointer(button)
        })
        .map(|index| index as u8 + 1);
    for index in 0..7 {
        let button = review_button_rect(bounds, edge, index, overlay);
        let response = ui.interact(
            button,
            ui.make_persistent_id(("photo-review", id_salt, index)),
            Sense::click(),
        );
        let active = match index {
            0..=4 => review.rating > index as u8,
            5 => review.flag == PhotoFlag::Picked,
            _ => review.flag == PhotoFlag::Rejected,
        };
        let highlight = button.shrink2(egui::vec2(1.0, if overlay { 1.0 } else { 4.0 }));
        if response.hovered()
            || response.has_focus()
            || response.is_pointer_button_down_on()
            || (!overlay && index >= 5 && active)
        {
            let visuals = ui.style().interact(&response);
            painter.rect_filled(highlight, visuals.corner_radius, visuals.weak_bg_fill);
            painter.rect_stroke(
                highlight,
                visuals.corner_radius,
                visuals.bg_stroke,
                StrokeKind::Inside,
            );
        }
        let icon_size = (edge * 0.62).min(15.0);
        let value = if index < 5 {
            paint_star(
                &painter,
                button.center(),
                icon_size,
                if active {
                    STAR_COLOR
                } else if hovered_rating.is_some_and(|rating| rating > index as u8) {
                    STAR_PREVIEW_COLOR
                } else if overlay {
                    MUTED
                } else {
                    ui.visuals().weak_text_color()
                },
                active,
            );
            let rating = index as u8 + 1;
            ReviewChange::Rating(if review.rating == rating { 0 } else { rating })
        } else {
            let flag = if index == 5 {
                PhotoFlag::Picked
            } else {
                PhotoFlag::Rejected
            };
            paint_flag(
                &painter,
                button.center(),
                icon_size,
                flag_color(flag),
                active,
            );
            ReviewChange::Flag(if review.flag == flag {
                PhotoFlag::Unflagged
            } else {
                flag
            })
        };
        if response.clicked() {
            change = Some(value);
        }
    }
    if !overlay {
        let x = bounds.left() + 6.0 + edge * 5.0 + 6.0;
        painter.line_segment(
            [
                egui::pos2(x, bounds.top() + 9.0),
                egui::pos2(x, bounds.bottom() - 9.0),
            ],
            Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
        );
    }
    change
}

fn review_button_rect(bounds: egui::Rect, edge: f32, index: usize, overlay: bool) -> egui::Rect {
    if overlay {
        let offset = if index < 5 {
            egui::vec2((index as f32 - 2.5) * edge, 0.0)
        } else {
            egui::vec2((index as f32 - 6.0) * edge, edge)
        };
        return egui::Rect::from_min_size(
            egui::pos2(bounds.center().x, bounds.top()) + offset,
            egui::Vec2::splat(edge),
        );
    }
    egui::Rect::from_min_size(
        egui::pos2(
            bounds.left() + 6.0 + edge * index as f32 + if index >= 5 { 12.0 } else { 0.0 },
            bounds.top(),
        ),
        egui::vec2(edge, bounds.height()),
    )
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

/// A button showing the rating and flag that opens the full editor.
pub(super) fn compact_review_editor(
    ui: &mut Ui,
    id_salt: impl std::hash::Hash + std::fmt::Debug + Copy,
    review: PhotoReview,
) -> Option<ReviewChange> {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(92.0, moduwu_design::CONTROL_HEIGHT),
        Sense::click(),
    );
    let painter = ui.painter_at(rect);
    let visuals = ui.style().interact(&response);
    painter.rect_filled(rect, visuals.corner_radius, visuals.weak_bg_fill);
    painter.rect_stroke(
        rect,
        visuals.corner_radius,
        visuals.bg_stroke,
        StrokeKind::Inside,
    );
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
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(EDITOR_WIDTH, moduwu_design::CONTROL_HEIGHT),
            Sense::hover(),
        );
        change = review_editor(ui, rect, id_salt, review, false);
        if change.is_some() {
            ui.close();
        }
    });
    change
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
        review_editor(ui, rect, ID, review, false)
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
    match asset_id {
        LibraryAssetId::Desktop(path) => Some(LibraryAsset::from_desktop_path(path, 0, 0, None)),
        LibraryAssetId::Android(uri) => {
            let Some(crate::sidecar::SidecarTarget::Android { display_name, .. }) =
                app.persistence.sidecar_target.as_ref()
            else {
                return None;
            };
            Some(LibraryAsset::from_android_document(
                crate::platform::LibraryDocument {
                    uri,
                    display_name: display_name.clone(),
                    display_path: display_name.clone(),
                    bytes: 0,
                    modified_seconds: 0,
                },
            ))
        }
    }
}
