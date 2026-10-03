//! RAW+JPEG pairing. Cameras that shoot RAW+JPEG (or RAW+HEIF) save a
//! rendered copy next to every RAW. Like Lightroom and Capture One, the
//! library shows each pair once, as the RAW, unless the viewer chooses to see
//! the rendered copies as separate photos.

use super::*;
use crate::pipeline::{is_camera_raw_path, RenderedImageFormat};

/// Rendered images that share a folder and a base name with a camera RAW.
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
                rendered.push((pair_key(asset), &asset.id, format));
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

    pub(super) fn is_empty(&self) -> bool {
        self.companions.is_empty()
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

/// Paints a small format pill (e.g. "JPEG") in the top-right corner of a
/// thumbnail. `right_inset` leaves room for other corner indicators.
pub(crate) fn paint_format_badge(ui: &Ui, rect: egui::Rect, label: &str, right_inset: f32) {
    const HEIGHT: f32 = 18.0;
    const MARGIN: f32 = 6.0;
    let font = FontId::proportional(10.0);
    let painter = ui.painter_at(rect);
    let text = painter.layout_no_wrap(label.to_owned(), font, Color32::WHITE);
    let size = egui::vec2(text.size().x + 10.0, HEIGHT);
    if size.x + MARGIN * 2.0 + right_inset > rect.width() || HEIGHT + MARGIN * 2.0 > rect.height() {
        return;
    }
    let badge = egui::Rect::from_min_size(
        egui::pos2(
            rect.right() - MARGIN - right_inset - size.x,
            rect.top() + MARGIN,
        ),
        size,
    );
    painter.rect_filled(badge, 5.0, Color32::from_black_alpha(160));
    painter.galley(badge.center() - text.size() * 0.5, text, Color32::WHITE);
}
