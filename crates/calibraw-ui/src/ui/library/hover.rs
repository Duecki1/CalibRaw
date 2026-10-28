//! The details and review buttons shown over a library thumbnail under the
//! pointer.
//!
//! They sit on a card along the photo's bottom edge: the rating badge, grown
//! upwards, so the rest of the photo stays visible. The card stacks rows
//! bottom-up in a fixed priority: review buttons, name, capture settings,
//! file. It adds rows while it stays within `MAX_CARD_SHARE` of the tile
//! height, and the first row that does not fit ends the stack. A row is never
//! swapped for another, so tiles of one size show the same rows for every
//! photo. Tiles too narrow for the stars and flags in one row move Pick and
//! Reject to round buttons in the top-right corner.

use std::sync::Arc;

use eframe::egui::{self, pos2, vec2, Color32, FontId, Galley, Rect, Sense, Ui, UiBuilder};

use super::catalog::{
    paint_corner_toggle, thumbnail_capture_fields, thumbnail_file_fields, CHECK_HIT_EDGE,
    CORNER_TOGGLE_EDGE, CORNER_TOGGLE_INSET,
};
use super::review::{
    flag_button_id, flag_button_info, flag_buttons, flag_change, flag_color, paint_flag,
    paint_review_badge, star_buttons, ReviewChange, ReviewSurface, BADGE_FILL, BADGE_FLAG_SIZE,
    BADGE_HEIGHT, BADGE_ICON_INSET, BADGE_ICON_STEP, BADGE_MARGIN, BADGE_RADIUS, BADGE_STAR_SIZE,
    FLAG_BUTTONS,
};
use super::{LibraryAction, LibraryEntry};
use crate::pipeline::RenderedImageFormat;
use crate::sidecar::PhotoReview;

const ANIMATION_SECONDS: f32 = 0.18;
/// Share of the tile height the card may cover.
const MAX_CARD_SHARE: f32 = 0.45;
/// Above the card's top row, in points at full size.
const CARD_TOP_PADDING: f32 = 5.0;
/// Between text rows, in points.
const ROW_GAP: f32 = 1.0;
/// Between the corner flags, in points.
const CORNER_FLAG_GAP: f32 = 6.0;
const CORNER_FLAG_SIZE: f32 = 13.0;
/// Characters a shortened file name keeps of its stem, at least.
const MIN_NAME_STEM: usize = 4;
/// Longest suffix, dot included, a shortened name keeps whole as its extension.
const MAX_EXTENSION_LEN: usize = 6;
const FIELD_SEPARATOR: &str = " · ";

/// The hover card of the thumbnail at `rect`. The review badge fades into
/// the card's buttons in the same place, and the card stays while the
/// pointer moves from the photo onto a button. `corner_badges_bottom` is
/// where the tile's top-right badges end, which corner flags stay below.
pub(super) fn thumbnail_hover_overlay(
    ui: &mut Ui,
    rect: Rect,
    entry: &LibraryEntry,
    stacked_formats: &[RenderedImageFormat],
    corner_badges_bottom: f32,
) -> Option<LibraryAction> {
    let asset = &entry.asset;
    let hovered = ui.rect_contains_pointer(rect);
    let progress = ui.ctx().animate_bool_with_time_and_easing(
        ui.make_persistent_id(("thumbnail-hover", &asset.id)),
        hovered,
        ANIMATION_SECONDS,
        egui::emath::easing::cubic_out,
    );
    if progress <= 0.0 {
        paint_review_badge(ui, rect, entry.review());
        return None;
    }
    if progress < 1.0 {
        let mut badge = ui.new_child(UiBuilder::new());
        badge.multiply_opacity(1.0 - progress);
        paint_review_badge(&badge, rect, entry.review());
    }

    let mut overlay = ui.new_child(UiBuilder::new());
    overlay.multiply_opacity(progress);
    let card = CardLayout::new(rect);
    let text_rows = stack_text_rows(&overlay, rect, &card, entry, stacked_formats);
    let top_row = text_rows
        .last()
        .map_or(card.review_top, |(row_top, _)| *row_top);
    let painter = overlay.painter_at(rect);
    painter.rect_filled(
        card.rect(top_row - CARD_TOP_PADDING * card.scale),
        BADGE_RADIUS * card.scale,
        BADGE_FILL,
    );
    for (row_top, galley) in text_rows {
        painter.galley(
            pos2(rect.left() + card.text_inset, row_top),
            galley,
            Color32::WHITE,
        );
    }
    let review = entry.review();
    let surface = ReviewSurface::Photo;
    let rating = star_buttons(
        &overlay,
        card.stars,
        BADGE_STAR_SIZE * card.scale,
        &asset.id,
        review,
        surface,
    );
    let flag = match card.flags {
        Some(flags) => flag_buttons(
            &overlay,
            flags,
            BADGE_FLAG_SIZE * card.scale,
            &asset.id,
            review,
            surface,
        ),
        None => corner_flag_buttons(&overlay, rect, corner_badges_bottom, &asset.id, review),
    };
    rating
        .or(flag)
        .filter(|_| hovered)
        .map(|change| LibraryAction::Review(vec![asset.clone()], change))
}

/// The card's place and its review buttons, over the resting badges: stars
/// where the rating badge's stars are, Pick and Reject at the right edge
/// where the flag badge is.
struct CardLayout {
    tile: Rect,
    /// Below 1 on tiles too narrow for five stars, which shrink the card as
    /// the badge shrinks.
    scale: f32,
    stars: [Rect; 5],
    /// `None` when the stars leave no room for the flags beside them.
    flags: Option<[Rect; 2]>,
    /// Top of the buttons, where the text rows start.
    review_top: f32,
    /// From either tile edge to the text, in line with the outer icons.
    text_inset: f32,
}

impl CardLayout {
    fn new(tile: Rect) -> Self {
        // From the tile edge to the centre of the outermost icon.
        let icon_inset = BADGE_MARGIN + BADGE_ICON_INSET;
        let scale = (tile.width() / (2.0 * icon_inset + 4.0 * BADGE_ICON_STEP)).min(1.0);
        let height = BADGE_HEIGHT * scale;
        let row_y = tile.bottom() - (BADGE_MARGIN + BADGE_HEIGHT / 2.0) * scale;
        let button =
            |x: f32| Rect::from_center_size(pos2(x, row_y), vec2(BADGE_ICON_STEP * scale, height));
        let stars = std::array::from_fn(|index| {
            button(tile.left() + (icon_inset + BADGE_ICON_STEP * index as f32) * scale)
        });
        // The flags share the row while one empty step separates them from
        // the stars.
        let flags = (tile.width() >= 2.0 * icon_inset + 7.0 * BADGE_ICON_STEP).then(|| {
            [
                button(tile.right() - icon_inset - BADGE_ICON_STEP),
                button(tile.right() - icon_inset),
            ]
        });
        Self {
            tile,
            scale,
            stars,
            flags,
            review_top: row_y - height / 2.0,
            text_inset: (icon_inset - BADGE_STAR_SIZE / 2.0) * scale,
        }
    }

    /// The card from `top` down, where the rating badge sits but full width.
    fn rect(&self, top: f32) -> Rect {
        let margin = BADGE_MARGIN * self.scale;
        Rect::from_min_max(
            pos2(self.tile.left() + margin, top),
            self.tile.max - vec2(margin, margin),
        )
    }
}

/// Pick and Reject as round buttons in the tile's top-right corner, below
/// `badges_bottom`, styled like the select check in the opposite corner.
/// Side by side when that leaves the select check clear, else stacked.
fn corner_flag_buttons(
    ui: &Ui,
    tile: Rect,
    badges_bottom: f32,
    id_salt: impl std::hash::Hash + std::fmt::Debug + Copy,
    review: PhotoReview,
) -> Option<ReviewChange> {
    let radius = CORNER_TOGGLE_EDGE / 2.0;
    // Each button's hit square reaches halfway to its neighbour.
    let step = CORNER_TOGGLE_EDGE + CORNER_FLAG_GAP;
    let corner = pos2(
        tile.right() - CORNER_TOGGLE_INSET - radius,
        badges_bottom.max(tile.top()) + CORNER_TOGGLE_INSET + radius,
    );
    let side_by_side =
        tile.right() - CORNER_TOGGLE_INSET - radius - 1.5 * step >= tile.left() + CHECK_HIT_EDGE;
    let centers = if side_by_side {
        [corner - vec2(step, 0.0), corner]
    } else {
        [corner, corner + vec2(0.0, step)]
    };
    let painter = ui.painter_at(tile);
    let mut change = None;
    for (index, (flag, center)) in FLAG_BUTTONS.into_iter().zip(centers).enumerate() {
        let response = ui
            .interact(
                Rect::from_center_size(center, egui::Vec2::splat(step)),
                ui.make_persistent_id(flag_button_id(id_salt, index)),
                Sense::click(),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        let active = review.flag == flag;
        flag_button_info(&response, flag, active);
        paint_corner_toggle(
            &painter,
            center,
            active.then(|| flag_color(flag)),
            response.hovered(),
        );
        paint_flag(
            &painter,
            center,
            CORNER_FLAG_SIZE,
            if active {
                Color32::WHITE
            } else {
                flag_color(flag)
            },
            active,
        );
        if response.clicked() {
            change = Some(flag_change(review, flag));
        }
    }
    change
}

/// A text row of the card, from the bottom up in priority order.
#[derive(Clone, Copy)]
enum TextRow {
    Name,
    /// Aperture, shutter speed, ISO and focal length.
    Capture,
    /// Format and pixel size, or what is keeping the preview from being current.
    File,
}

impl TextRow {
    const STACK: [Self; 3] = [Self::Name, Self::Capture, Self::File];

    fn fit(
        self,
        ui: &Ui,
        entry: &LibraryEntry,
        stacked_formats: &[RenderedImageFormat],
        width: f32,
    ) -> RowFit {
        let (font, color) = match self {
            Self::Name => (FontId::proportional(13.0), Color32::WHITE),
            Self::Capture => (FontId::proportional(11.0), Color32::from_white_alpha(215)),
            Self::File => (FontId::proportional(11.0), Color32::from_white_alpha(180)),
        };
        let layout = |text: String| ui.painter().layout_no_wrap(text, font.clone(), color);
        match self {
            Self::Name => fit(name_candidates(&entry.asset.display_name), width, layout),
            Self::Capture => {
                let fields = thumbnail_capture_fields(&entry.asset);
                fit(field_candidates(&fields), width, layout)
            }
            Self::File => {
                let fields = preview_status(entry).map_or_else(
                    || thumbnail_file_fields(&entry.asset, stacked_formats),
                    |status| vec![status],
                );
                fit(field_candidates(&fields), width, layout)
            }
        }
    }
}

enum RowFit {
    /// The photo has nothing to show in the row.
    Empty,
    /// Not even the row's shortest form fits.
    TooNarrow,
    Fits(Arc<Galley>),
}

/// The text rows that fit above the review buttons, bottom-up, each with its
/// top edge.
fn stack_text_rows(
    ui: &Ui,
    tile: Rect,
    card: &CardLayout,
    entry: &LibraryEntry,
    stacked_formats: &[RenderedImageFormat],
) -> Vec<(f32, Arc<Galley>)> {
    let width = tile.width() - 2.0 * card.text_inset;
    let highest_top =
        tile.bottom() - tile.height() * MAX_CARD_SHARE + CARD_TOP_PADDING * card.scale;
    let mut top = card.review_top;
    let mut rows = Vec::new();
    for row in TextRow::STACK {
        let galley = match row.fit(ui, entry, stacked_formats, width) {
            RowFit::Empty => continue,
            RowFit::TooNarrow => break,
            RowFit::Fits(galley) => galley,
        };
        let row_top = top - ROW_GAP - galley.size().y;
        if row_top < highest_top {
            break;
        }
        top = row_top;
        rows.push((row_top, galley));
    }
    rows
}

/// Lays out the first of `candidates`, longest first, that fits `width`.
fn fit(
    candidates: impl Iterator<Item = String>,
    width: f32,
    layout: impl Fn(String) -> Arc<Galley>,
) -> RowFit {
    let mut candidates = candidates.peekable();
    if candidates.peek().is_none() {
        return RowFit::Empty;
    }
    candidates
        .map(layout)
        .find(|galley| galley.size().x <= width)
        .map_or(RowFit::TooNarrow, RowFit::Fits)
}

/// The file name, then ever shorter forms that cut the middle of its stem
/// but keep the extension and the stem's end, where cameras put frame
/// numbers: `IMG_20…4523.CR3`.
fn name_candidates(name: &str) -> impl Iterator<Item = String> + '_ {
    let (stem, extension) = name
        .rfind('.')
        .filter(|dot| *dot > 0)
        .map(|dot| name.split_at(dot))
        .filter(|(_, extension)| {
            extension.len() <= MAX_EXTENSION_LEN
                && extension[1..].chars().all(|c| c.is_ascii_alphanumeric())
        })
        .unwrap_or((name, ""));
    let stem: Vec<char> = stem.chars().collect();
    let shortened = (MIN_NAME_STEM..stem.len()).rev().map(move |kept| {
        let head = kept / 2;
        let mut text: String = stem[..head].iter().collect();
        text.push('…');
        text.extend(&stem[stem.len() - (kept - head)..]);
        text.push_str(extension);
        text
    });
    (!name.is_empty())
        .then(|| name.to_owned())
        .into_iter()
        .chain(shortened)
}

/// All `fields` joined, then without the last field, down to the first alone.
fn field_candidates(fields: &[String]) -> impl Iterator<Item = String> + '_ {
    (1..=fields.len())
        .rev()
        .map(|count| fields[..count].join(FIELD_SEPARATOR))
}

/// Why the preview does not show the photo's current state, if it does not.
fn preview_status(entry: &LibraryEntry) -> Option<String> {
    entry.thumbnail_error.clone().or_else(|| {
        entry.developed_thumbnail_pending.then(|| {
            if entry.thumbnail_queued {
                "Rendering edits…"
            } else {
                "Preview awaiting saved edits"
            }
            .to_owned()
        })
    })
}
