//! The details and review buttons shown over a library thumbnail under the
//! pointer.
//!
//! They sit in a band along the photo's bottom edge, on a scrim that ramps up
//! from clear, so the rest of the photo stays visible. The band stacks rows
//! bottom-up in a fixed priority: review buttons, name, capture settings,
//! file. It adds rows while they fit within `MAX_BAND_SHARE` of the tile
//! height, and the first row that does not fit ends the stack. A row is never
//! swapped for another, so tiles of one size show the same rows for every
//! photo.

use std::sync::Arc;

use eframe::egui::{
    self, pos2, vec2, Color32, ColorImage, FontId, Galley, Id, Rect, TextureHandle, TextureOptions,
    Ui, UiBuilder,
};

use super::catalog::{thumbnail_capture_fields, thumbnail_file_fields, THUMBNAIL_CARD_RADIUS};
use super::review::{
    paint_review_badge, review_buttons, ReviewSurface, BADGE_HEIGHT, BADGE_ICON_INSET,
    BADGE_ICON_STEP, BADGE_MARGIN, BADGE_STAR_SIZE,
};
use super::{LibraryAction, LibraryEntry};
use crate::pipeline::RenderedImageFormat;

const ANIMATION_SECONDS: f32 = 0.18;
/// Share of the tile height the rows may cover.
const MAX_BAND_SHARE: f32 = 0.45;
/// Height of the scrim's ramp from clear, above the top row, in points.
const SCRIM_FADE: f32 = 36.0;
/// Scrim opacity behind the rows.
const SCRIM_ALPHA: u8 = 176;
/// Texels in the scrim's ramp texture.
const SCRIM_TEXELS: usize = 64;
/// Between text rows, in points.
const ROW_GAP: f32 = 1.0;
/// Characters a shortened file name keeps of its stem, at least.
const MIN_NAME_STEM: usize = 4;
/// Longest suffix, dot included, a shortened name keeps whole as its extension.
const MAX_EXTENSION_LEN: usize = 6;
const FIELD_SEPARATOR: &str = " · ";

/// The hover band of the thumbnail at `rect`. The review badge fades into the
/// band's buttons in the same place, and the band stays while the pointer
/// moves from the photo onto a button.
pub(super) fn thumbnail_hover_overlay(
    ui: &mut Ui,
    rect: Rect,
    entry: &LibraryEntry,
    stacked_formats: &[RenderedImageFormat],
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

    let mut band = ui.new_child(UiBuilder::new());
    band.multiply_opacity(progress);
    let review_row = ReviewRow::new(rect);
    let text_rows = stack_text_rows(&band, rect, &review_row, entry, stacked_formats);
    let top = text_rows
        .last()
        .map_or(review_row.top, |(row_top, _)| *row_top);
    paint_scrim(&band, rect, top - SCRIM_FADE);
    let painter = band.painter_at(rect);
    for (row_top, galley) in text_rows {
        painter.galley(
            pos2(rect.left() + review_row.text_inset, row_top),
            galley,
            Color32::WHITE,
        );
    }
    review_buttons(
        &band,
        review_row.buttons,
        review_row.icon_size,
        &asset.id,
        entry.review(),
        ReviewSurface::Photo,
    )
    .filter(|_| hovered)
    .map(|change| LibraryAction::Review(vec![asset.clone()], change))
}

/// The review buttons over the resting badge: stars where its stars are,
/// Pick and Reject at the right edge where its flag is.
struct ReviewRow {
    buttons: [Rect; 7],
    icon_size: f32,
    /// Top of the buttons, where the text rows start.
    top: f32,
    /// From either tile edge to the text, in line with the outer icons.
    text_inset: f32,
}

impl ReviewRow {
    fn new(tile: Rect) -> Self {
        // From the tile edge to the centre of the outermost icon.
        let icon_inset = BADGE_MARGIN + BADGE_ICON_INSET;
        // Tiles too narrow for five stars scale the buttons down, as the badge does.
        let scale = (tile.width() / (2.0 * icon_inset + 4.0 * BADGE_ICON_STEP)).min(1.0);
        // The flags share the stars' row while one empty step separates them.
        let one_row = tile.width() >= 2.0 * icon_inset + 7.0 * BADGE_ICON_STEP;
        let height = BADGE_HEIGHT * scale;
        let star_y = tile.bottom() - (BADGE_MARGIN + BADGE_HEIGHT / 2.0) * scale;
        let flag_y = if one_row { star_y } else { star_y - height };
        let button = |x: f32, y: f32| {
            Rect::from_center_size(pos2(x, y), vec2(BADGE_ICON_STEP * scale, height))
        };
        let buttons = std::array::from_fn(|index| match index {
            0..=4 => button(
                tile.left() + (icon_inset + BADGE_ICON_STEP * index as f32) * scale,
                star_y,
            ),
            // Pick, then Reject.
            _ => button(
                tile.right() - (icon_inset + BADGE_ICON_STEP * (6 - index) as f32) * scale,
                flag_y,
            ),
        });
        let icon_size = BADGE_STAR_SIZE * scale;
        Self {
            buttons,
            icon_size,
            top: flag_y - height / 2.0,
            text_inset: icon_inset * scale - icon_size / 2.0,
        }
    }
}

/// A text row of the band, from the bottom up in priority order.
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
    review_row: &ReviewRow,
    entry: &LibraryEntry,
    stacked_formats: &[RenderedImageFormat],
) -> Vec<(f32, Arc<Galley>)> {
    let width = tile.width() - 2.0 * review_row.text_inset;
    let highest_top = tile.bottom() - tile.height() * MAX_BAND_SHARE;
    let mut top = review_row.top;
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

/// Paints the scrim from `top` to the tile's bottom edge: clear at `top`,
/// ramping up over `SCRIM_FADE` points and solid below, with the card's
/// rounded bottom corners.
fn paint_scrim(ui: &Ui, tile: Rect, top: f32) {
    let band = Rect::from_min_max(pos2(tile.left(), top.max(tile.top())), tile.max);
    // Past v = 1, clamp-to-edge sampling repeats the ramp's solid last texel.
    let uv = Rect::from_min_max(
        pos2(0.0, (band.top() - top) / SCRIM_FADE),
        pos2(1.0, (band.bottom() - top) / SCRIM_FADE),
    );
    let corners = egui::epaint::CornerRadiusF32 {
        nw: 0.0,
        ne: 0.0,
        sw: THUMBNAIL_CARD_RADIUS,
        se: THUMBNAIL_CARD_RADIUS,
    };
    ui.painter_at(tile).add(
        egui::epaint::RectShape::filled(band, corners, Color32::from_black_alpha(SCRIM_ALPHA))
            .with_texture(scrim_texture(ui.ctx()).id(), uv),
    );
}

/// A one-texel-wide white ramp from clear to opaque, uploaded once per context
/// and kept in temporary memory. The scrim tints it, so the texture holds the
/// ramp's shape and `SCRIM_ALPHA` its strength.
fn scrim_texture(ctx: &egui::Context) -> TextureHandle {
    let id = Id::new("calibraw-library-hover-scrim");
    if let Some(texture) = ctx.data(|data| data.get_temp::<TextureHandle>(id)) {
        return texture;
    }
    let pixels = (0..SCRIM_TEXELS)
        .map(|texel| {
            let t = texel as f32 / (SCRIM_TEXELS - 1) as f32;
            // Smoothstep, so neither end of the ramp shows an edge.
            let alpha = t * t * (3.0 - 2.0 * t);
            Color32::from_white_alpha((alpha * 255.0).round() as u8)
        })
        .collect();
    let texture = ctx.load_texture(
        "library-hover-scrim",
        ColorImage::new([1, SCRIM_TEXELS], pixels),
        TextureOptions::LINEAR,
    );
    ctx.data_mut(|data| data.insert_temp(id, texture.clone()));
    texture
}
