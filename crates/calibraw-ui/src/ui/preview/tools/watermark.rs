//! The export watermark over the preview, shown only while the Export panel
//! is open. It is placed from the panel's card; the preview draws it in the
//! output frame as the export places it, clipped to the photo, which is where
//! the export cuts it off.

use super::super::*;
use crate::app::WatermarkView;

impl Preview {
    pub(in crate::ui::preview) fn paint_watermark(
        ui: &Ui,
        view: WatermarkView,
        layout: PreviewLayout,
    ) {
        let Some((texture, image_size)) = view.shown_image() else {
            return;
        };
        let output = layout.image_rect;
        let [left, top, width, height] = view
            .placement
            .rect(image_size, [output.width(), output.height()]);
        ui.painter_at(output.intersect(layout.viewport_rect)).image(
            texture,
            Rect::from_min_size(
                output.min + egui::vec2(left, top),
                egui::vec2(width, height),
            ),
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
    }
}
