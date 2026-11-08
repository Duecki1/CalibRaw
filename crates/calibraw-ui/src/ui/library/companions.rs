//! RAW+JPEG pairing. Cameras that shoot RAW+JPEG (or RAW+HEIF) save a
//! rendered copy next to every RAW. The library shows each pair once, as the
//! RAW, unless the viewer chooses to see the rendered copies as separate photos.

use super::*;
use crate::pipeline::{is_camera_raw_path, RenderedImageFormat};

/// Rendered formats cameras record beside a RAW. A PNG with a RAW's name is
/// an export or screenshot the photographer made, so it is never stacked.
const IN_CAMERA_FORMATS: [RenderedImageFormat; 2] =
    [RenderedImageFormat::Jpeg, RenderedImageFormat::Heif];

/// In-camera JPEG/HEIF copies that share a folder and base name with a RAW.
#[derive(Debug, Default)]
pub(super) struct RawCompanions {
    companions: HashSet<LibraryAssetId>,
    paired_formats: HashMap<LibraryAssetId, Vec<RenderedImageFormat>>,
}

impl RawCompanions {
    pub(super) fn index<'a>(assets: impl IntoIterator<Item = &'a LibraryAsset>) -> Self {
        let mut raws = HashMap::new();
        let mut rendered = Vec::new();
        for asset in assets {
            let name = Path::new(&asset.display_name);
            if let Some(format) = RenderedImageFormat::from_path(name) {
                if IN_CAMERA_FORMATS.contains(&format) {
                    rendered.push((pair_key(asset), &asset.id, format));
                }
            } else if is_camera_raw_path(name) {
                raws.insert(pair_key(asset), &asset.id);
            }
        }

        let mut index = Self::default();
        for (key, companion, format) in rendered {
            let Some(raw) = raws.get(&key) else {
                continue;
            };
            index.companions.insert(companion.clone());
            let formats = index.paired_formats.entry((*raw).clone()).or_default();
            if !formats.contains(&format) {
                formats.push(format);
            }
        }
        index
    }

    /// Whether `asset` is the rendered copy of a RAW in the same folder.
    pub(super) fn is_companion(&self, asset: &LibraryAssetId) -> bool {
        self.companions.contains(asset)
    }

    /// Rendered formats saved alongside the RAW `asset`, e.g. `[Jpeg]`.
    pub(super) fn paired_formats(&self, asset: &LibraryAssetId) -> &[RenderedImageFormat] {
        self.paired_formats.get(asset).map_or(&[], Vec::as_slice)
    }
}

/// Folder and case-insensitive base name: `DSC_0001.NEF` pairs with
/// `DSC_0001.JPG` in the same folder only.
fn pair_key(asset: &LibraryAsset) -> (String, String) {
    let folder = Path::new(&asset.display_path)
        .parent()
        .map(|parent| parent.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stem = Path::new(&asset.display_name)
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    (folder, stem)
}

/// The short format name shown on library and filmstrip tiles of rendered
/// photos, so a JPEG is never mistaken for its RAW.
pub(crate) fn rendered_format_label(display_name: &str) -> Option<&'static str> {
    RenderedImageFormat::from_path(Path::new(display_name)).map(RenderedImageFormat::label)
}

// Look of the pills in a thumbnail's top-right corner, in points.
const CORNER_BADGE_HEIGHT: f32 = 18.0;
/// From the tile's top and right edges to the pills.
const CORNER_BADGE_MARGIN: f32 = 6.0;
/// Between neighbouring pills.
const CORNER_BADGE_GAP: f32 = 4.0;
const CORNER_BADGE_RADIUS: f32 = 5.0;
const CORNER_BADGE_FILL: Color32 = Color32::from_black_alpha(160);

/// Paints a small format pill (e.g. "JPEG") in the top-right corner of a
/// thumbnail. `right_inset` leaves room for other corner indicators. Returns
/// the pill, unless the thumbnail is too small for it.
pub(crate) fn paint_format_badge(
    ui: &Ui,
    rect: egui::Rect,
    label: &str,
    right_inset: f32,
) -> Option<egui::Rect> {
    let font = FontId::proportional(10.0);
    let painter = ui.painter_at(rect);
    let text = painter.layout_no_wrap(label.to_owned(), font, Color32::WHITE);
    let size = egui::vec2(text.size().x + 10.0, CORNER_BADGE_HEIGHT);
    if size.x + CORNER_BADGE_MARGIN * 2.0 + right_inset > rect.width()
        || CORNER_BADGE_HEIGHT + CORNER_BADGE_MARGIN * 2.0 > rect.height()
    {
        return None;
    }
    let badge = egui::Rect::from_min_size(
        egui::pos2(
            rect.right() - CORNER_BADGE_MARGIN - right_inset - size.x,
            rect.top() + CORNER_BADGE_MARGIN,
        ),
        size,
    );
    painter.rect_filled(badge, CORNER_BADGE_RADIUS, CORNER_BADGE_FILL);
    painter.galley(badge.center() - text.size() * 0.5, text, Color32::WHITE);
    Some(badge)
}

/// What a thumbnail's edit pill tells.
#[derive(Clone, Copy)]
pub(crate) struct EditBadge {
    /// The photo's saved edits differ from its start.
    pub(crate) edited: bool,
    /// The thumbnail does not show the saved edits yet.
    pub(crate) stale: bool,
}

/// Paints the edit pill in the top-right corner of a thumbnail, beside the
/// format pill and in its look: the Adjustments icon for an edited photo,
/// then a refresh arrow while the thumbnail is out of date. Returns the pill,
/// if it has anything to show.
pub(crate) fn paint_edit_badge(ui: &Ui, rect: egui::Rect, badge: EditBadge) -> Option<egui::Rect> {
    /// Between the centres of neighbouring icons.
    const ICON_STEP: f32 = 15.0;
    const ICON_SIZE: f32 = 12.0;

    let icons = [
        (badge.edited, egui_phosphor::regular::SLIDERS_HORIZONTAL),
        (badge.stale, egui_phosphor::regular::ARROWS_CLOCKWISE),
    ];
    let count = icons.iter().filter(|(shown, _)| *shown).count();
    if count == 0 {
        return None;
    }
    // Shrinks with tiny tiles rather than vanishing: the status always shows.
    let scale = (rect.width().min(rect.height()) * 0.3 / CORNER_BADGE_HEIGHT).clamp(0.0, 1.0);
    let height = CORNER_BADGE_HEIGHT * scale;
    let width = height + (count - 1) as f32 * ICON_STEP * scale;
    let pill = egui::Rect::from_min_size(
        egui::pos2(
            rect.right() - CORNER_BADGE_MARGIN - width,
            rect.top() + CORNER_BADGE_MARGIN,
        ),
        egui::vec2(width, height),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(pill, CORNER_BADGE_RADIUS * scale, CORNER_BADGE_FILL);
    let shown = icons.iter().filter(|(shown, _)| *shown);
    for (index, (_, icon)) in shown.enumerate() {
        painter.text(
            egui::pos2(
                pill.left() + height * 0.5 + index as f32 * ICON_STEP * scale,
                pill.center().y,
            ),
            Align2::CENTER_CENTER,
            *icon,
            FontId::proportional(ICON_SIZE * scale),
            Color32::WHITE,
        );
    }
    Some(pill)
}

/// The `right_inset` that places a format pill to the left of `pill`, a
/// corner pill of the tile at `rect`.
pub(crate) fn corner_inset_beside(rect: egui::Rect, pill: egui::Rect) -> f32 {
    rect.right() - CORNER_BADGE_MARGIN - pill.left() + CORNER_BADGE_GAP
}
