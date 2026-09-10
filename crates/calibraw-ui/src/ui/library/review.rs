use super::*;
use crate::sidecar::{PhotoFlag, PhotoReview};
use crate::ui::theme;

const STAR_COLOR: Color32 = Color32::from_rgb(234, 195, 112);
const STAR_PREVIEW_COLOR: Color32 = Color32::from_rgb(255, 232, 183);
const MUTED: Color32 = Color32::from_rgb(137, 145, 155);
const HOVER_ANIMATION_SECONDS: f32 = 0.18;
const EDITOR_WIDTH: f32 = 196.0;

#[derive(Clone, Copy, Debug)]
pub(crate) enum ReviewChange {
    Flag(PhotoFlag),
    Rating(u8),
}

/// One hover surface owns both photo details and review controls, including when
/// the pointer moves from the image onto a star or flag.
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

fn paint_review_badge(ui: &Ui, rect: egui::Rect, review: PhotoReview) {
    let has_flag = review.flag != PhotoFlag::Unflagged;
    let count = usize::from(review.rating) + usize::from(has_flag);
    if count == 0 {
        return;
    }
    let gap = if has_flag && review.rating > 0 {
        6.0
    } else {
        0.0
    };
    let width = count as f32 * 15.0 + 12.0 + gap;
    let badge = egui::Rect::from_min_size(
        egui::pos2(rect.left() + 8.0, rect.bottom() - 30.0),
        egui::vec2(width, 22.0),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(badge, 6.0, Color32::from_black_alpha(150));
    for index in 0..review.rating {
        paint_star(
            &painter,
            egui::pos2(
                badge.left() + 13.5 + f32::from(index) * 15.0,
                badge.center().y,
            ),
            11.0,
            STAR_COLOR,
            true,
        );
    }
    if has_flag {
        let center = egui::pos2(badge.right() - 13.5, badge.center().y);
        paint_flag(&painter, center, 12.0, flag_color(review.flag), true);
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
    // Only the Develop toolbar uses a separate control frame.
    if !overlay {
        painter.rect_filled(bounds, theme::CARD_RADIUS, ui.visuals().extreme_bg_color);
        painter.rect_stroke(
            bounds,
            theme::CARD_RADIUS,
            Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
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
        let (value, tooltip) = if index < 5 {
            paint_star(
                &painter,
                button.center(),
                icon_size,
                if active {
                    STAR_COLOR
                } else if hovered_rating.is_some_and(|rating| rating > index as u8) {
                    STAR_PREVIEW_COLOR
                } else {
                    MUTED
                },
                active,
            );
            let rating = index as u8 + 1;
            (
                ReviewChange::Rating(if review.rating == rating { 0 } else { rating }),
                format!("{rating} stars · click again to clear"),
            )
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
            (
                ReviewChange::Flag(if review.flag == flag {
                    PhotoFlag::Unflagged
                } else {
                    flag
                }),
                if index == 5 {
                    "Pick · click again to unflag"
                } else {
                    "Reject · click again to unflag"
                }
                .to_owned(),
            )
        };
        if response.on_hover_text(tooltip).clicked() {
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

pub(crate) fn show_current_photo_review(ui: &mut Ui, app: &mut CalibRawApp) {
    let Some(path) = app.develop.current_path.clone() else {
        return;
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
    let mut change = None;
    if ui.available_width() >= EDITOR_WIDTH + 176.0 {
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(EDITOR_WIDTH, theme::CONTROL_HEIGHT),
            Sense::hover(),
        );
        change = review_editor(ui, rect, &asset.id, review, false);
    } else {
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(92.0, theme::CONTROL_HEIGHT), Sense::click());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, theme::CARD_RADIUS, ui.visuals().extreme_bg_color);
        paint_star(
            &painter,
            rect.left_center() + egui::vec2(14.0, 0.0),
            14.0,
            STAR_COLOR,
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
            flag_color(review.flag),
            review.flag != PhotoFlag::Unflagged,
        );
        painter.text(
            rect.right_center() - egui::vec2(13.0, 0.0),
            Align2::CENTER_CENTER,
            egui_phosphor::regular::CARET_DOWN,
            FontId::proportional(12.0),
            ui.visuals().weak_text_color(),
        );
        theme::dropdown_menu(&response, |ui| {
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(EDITOR_WIDTH, theme::CONTROL_HEIGHT),
                Sense::hover(),
            );
            change = review_editor(ui, rect, &asset.id, review, false);
            if change.is_some() {
                ui.close();
            }
        });
        response.on_hover_text("Rate or flag the open photo");
    }
    if let Some(change) = change {
        apply_review(app, vec![asset.clone()], change);
        let mut updated = asset;
        if let Ok(review) = crate::sidecar::load_photo_review(&path) {
            updated.metadata.review = review;
        }
        ui.ctx()
            .data_mut(|data| data.insert_temp(cache_id, updated));
    }
}

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

#[cfg(test)]
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
