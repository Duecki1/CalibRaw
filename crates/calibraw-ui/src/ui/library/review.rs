use super::*;
use crate::sidecar::{PhotoFlag, PhotoReview};
use crate::ui::theme;

const STAR_COLOR: Color32 = Color32::from_rgb(234, 195, 112);
const STAR_PREVIEW_COLOR: Color32 = Color32::from_rgb(255, 232, 183);
const MUTED: Color32 = Color32::from_rgb(137, 145, 155);
#[cfg(not(target_os = "android"))]
const HOVER_ANIMATION_SECONDS: f32 = 0.18;
const EDITOR_WIDTH: f32 = 196.0;

#[derive(Clone, Copy, Debug)]
pub(crate) enum ReviewChange {
    Flag(PhotoFlag),
    Rating(u8),
}

/// One hover surface owns both photo details and review controls, including when
/// the pointer moves from the image onto a star or flag.
#[cfg(not(target_os = "android"))]
pub(super) fn thumbnail_hover_overlay(
    ui: &mut Ui,
    rect: egui::Rect,
    entry: &LibraryEntry,
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
        paint_review_badge(ui, rect, entry.review);
        return None;
    }

    // Restore the original full-photo scrim and centered, gently sliding text.
    let mut painter = ui.painter_at(rect);
    painter.set_opacity(progress);
    painter.rect_filled(rect, theme::CARD_RADIUS, Color32::from_black_alpha(156));
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
            thumbnail_hover_details(asset)
        }
    });
    for (text, size, offset, alpha) in [
        (asset.display_name.clone(), 14.5, -48.0, 255),
        (thumbnail_capture_details(asset), 11.5, -27.0, 205),
        (detail, 10.5, -8.0, 175),
    ] {
        let size = size * scale;
        let chars = ((rect.width() - 24.0) / (size * 0.55)).floor().max(1.0) as usize;
        painter.text(
            center + egui::vec2(0.0, offset * scale),
            Align2::CENTER_CENTER,
            elide_middle(&text, chars),
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
            review_editor(ui, controls, &asset.id, entry.review, true)
        })
        .inner;
    change
        .filter(|_| hovered)
        .map(|change| LibraryAction::Review(vec![asset.clone()], change))
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
    asset_id: &LibraryAssetId,
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
            ui.make_persistent_id(("photo-review", asset_id, index)),
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

fn compact_review_editor(
    ui: &mut Ui,
    asset_id: &LibraryAssetId,
    review: PhotoReview,
) -> Option<ReviewChange> {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(92.0, theme::CONTROL_HEIGHT), Sense::click());
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
    theme::dropdown_menu(&response, |ui| {
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(EDITOR_WIDTH, theme::CONTROL_HEIGHT),
            Sense::hover(),
        );
        change = review_editor(ui, rect, asset_id, review, false);
        if change.is_some() {
            ui.close();
        }
    });
    change
}

#[cfg(not(target_os = "android"))]
pub(crate) fn show_current_photo_review(ui: &mut Ui, app: &mut CalibRawApp, compact: bool) -> bool {
    let Some(path) = app.develop.current_path.clone() else {
        return false;
    };
    // A RAW opened directly may not have a gallery entry. Cache its metadata instead
    // of reading the sidecar on every toolbar frame.
    let cache_id = egui::Id::new("develop-current-photo-review");
    let asset = app
        .library
        .filmstrip_index_for_path(&path)
        .and_then(|index| {
            app.library
                .entries
                .get(index)
                .map(|entry| entry.asset.clone())
        })
        .or_else(|| {
            ui.ctx()
                .data(|data| data.get_temp::<LibraryAsset>(cache_id))
                .filter(|asset| asset.desktop_path() == Some(path.as_path()))
        })
        .unwrap_or_else(|| LibraryAsset::from_desktop_path(path.clone(), 0, 0, None));
    let review = asset.metadata.review;
    ui.ctx()
        .data_mut(|data| data.insert_temp(cache_id, asset.clone()));
    let change = if !compact && ui.available_width() >= EDITOR_WIDTH {
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(EDITOR_WIDTH, theme::CONTROL_HEIGHT),
            Sense::hover(),
        );
        review_editor(ui, rect, &asset.id, review, false)
    } else {
        compact_review_editor(ui, &asset.id, review)
    };
    if let Some(change) = change {
        apply_review(app, vec![asset.clone()], change);
        let mut updated = asset;
        if let Ok(review) = crate::sidecar::load_photo_review(&path) {
            updated.metadata.review = review;
            app.develop.review = review;
        }
        ui.ctx()
            .data_mut(|data| data.insert_temp(cache_id, updated));
    }
    true
}

#[cfg(target_os = "android")]
pub(crate) fn show_current_photo_review(
    ui: &mut Ui,
    app: &mut CalibRawApp,
    _compact: bool,
) -> bool {
    let Some((raw_uri, asset_id)) =
        app.persistence
            .sidecar_target
            .as_ref()
            .and_then(|target| match target {
                crate::sidecar::SidecarTarget::Android { raw_uri, .. } => {
                    Some((raw_uri.clone(), LibraryAssetId::Android(raw_uri.clone())))
                }
                crate::sidecar::SidecarTarget::Desktop { .. } => None,
            })
    else {
        return false;
    };
    let Some(change) = compact_review_editor(ui, &asset_id, app.develop.review) else {
        return true;
    };
    match change {
        ReviewChange::Flag(flag) => app.develop.review.flag = flag,
        ReviewChange::Rating(rating) => app.develop.review.rating = rating,
    }
    if let Some(index) = app.library.entry_indices.get(&asset_id).copied() {
        app.library.entries[index].review = app.develop.review;
        app.library.entries[index].asset.metadata.review = app.develop.review;
        app.library.sort_entries();
    } else {
        log::debug!("reviewed Android RAW is not in the current library: {raw_uri}");
    }
    app.queue_explicit_sidecar_save();
    true
}

#[cfg(not(target_os = "android"))]
pub(super) fn apply_review(app: &mut CalibRawApp, assets: Vec<LibraryAsset>, change: ReviewChange) {
    let mut failures = Vec::new();
    for asset in assets {
        let Some(path) = asset.desktop_path() else {
            continue;
        };
        let result = crate::sidecar::load_photo_review(path).and_then(|mut review| {
            match change {
                ReviewChange::Flag(flag) => review.flag = flag,
                ReviewChange::Rating(rating) => review.rating = rating,
            }
            crate::sidecar::save_photo_review(path, review)?;
            if let Some(index) = app.library.entry_indices.get(&asset.id).copied() {
                app.library.entries[index].review = review;
                app.library.entries[index].asset.metadata.review = review;
            }
            Ok(())
        });
        if let Err(error) = result {
            failures.push(format!("{}: {error}", asset.display_name));
        }
    }
    app.library.sort_entries();
    if !failures.is_empty() {
        app.library.status = format!("Could not save review: {}", failures.join("; "));
        app.ui.status = app.library.status.clone();
    }
}

#[cfg(all(test, not(target_os = "android")))]
mod tests {
    use super::*;

    fn click_at(review: PhotoReview, pos: egui::Pos2) -> Option<LibraryAction> {
        let context = egui::Context::default();
        let asset =
            LibraryAsset::from_desktop_path(PathBuf::from("review-interaction.dng"), 0, 0, None);
        let mut action = None;
        // Allow hit testing to settle before pressing and releasing over a control.
        for pressed in [None, None, Some(true), Some(false)] {
            let mut events = vec![egui::Event::PointerMoved(pos)];
            if let Some(pressed) = pressed {
                events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                });
            }
            let _ = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(500.0, 400.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        let rect = egui::Rect::from_min_size(
                            egui::pos2(40.0, 60.0),
                            egui::vec2(300.0, 220.0),
                        );
                        let photo =
                            ui.interact(rect, ui.make_persistent_id("photo"), Sense::click());
                        let mut entry = new_library_entry(asset.clone());
                        entry.review = review;
                        action = thumbnail_hover_overlay(ui, rect, &entry);
                        if action.is_some() {
                            assert!(
                                !photo.clicked(),
                                "review controls must consume the click before the photo"
                            );
                        }
                    });
                },
            );
        }
        action
    }

    #[test]
    fn hovered_photo_controls_pick_reject_rate_and_clear() {
        let empty = PhotoReview::default();
        assert!(matches!(
            click_at(empty, egui::pos2(203.0, 219.0)),
            Some(LibraryAction::Review(
                _,
                ReviewChange::Flag(PhotoFlag::Rejected)
            ))
        ));
        assert!(matches!(
            click_at(empty, egui::pos2(177.0, 219.0)),
            Some(LibraryAction::Review(
                _,
                ReviewChange::Flag(PhotoFlag::Picked)
            ))
        ));
        assert!(matches!(
            click_at(empty, egui::pos2(216.0, 193.0)),
            Some(LibraryAction::Review(_, ReviewChange::Rating(4)))
        ));
        assert!(matches!(
            click_at(PhotoReview { rating: 4, ..empty }, egui::pos2(216.0, 193.0)),
            Some(LibraryAction::Review(_, ReviewChange::Rating(0)))
        ));
        assert!(matches!(
            click_at(
                PhotoReview {
                    flag: PhotoFlag::Rejected,
                    ..empty
                },
                egui::pos2(203.0, 219.0)
            ),
            Some(LibraryAction::Review(
                _,
                ReviewChange::Flag(PhotoFlag::Unflagged)
            ))
        ));
        assert!(click_at(empty, egui::pos2(20.0, 20.0)).is_none());
    }
}
