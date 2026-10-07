//! The look and pointer reach of handles drawn over the photo, shared by the
//! mask shapes, the crop frame and effect lights. Every handle carries a
//! light outline and a dark halo so it stays visible on any part of an image.

use eframe::egui::{self, Color32, Pos2, Rect, Stroke};

/// Pointer reach of a point handle (a shape's center, end or axis handle, a
/// light), in points.
pub(super) const POINT_REACH: f32 = 22.0;
/// Pointer reach of a rotation knob.
pub(super) const ROTATION_REACH: f32 = 24.0;
/// Pointer reach of a path anchor point.
pub(super) const PATH_POINT_REACH: f32 = 20.0;
/// Pointer reach of a guide line or a curve handle.
pub(super) const GUIDE_REACH: f32 = 18.0;

/// Radius of a primary point handle.
pub(super) const POINT_RADIUS: f32 = 5.0;
/// Radius of a secondary point handle, such as a radial shape's axis handle.
pub(super) const SECONDARY_POINT_RADIUS: f32 = 4.0;
/// Radius of a rotation knob ring.
pub(super) const ROTATION_RADIUS: f32 = 6.0;

const OUTLINE: Color32 = Color32::WHITE;
const OUTLINE_WIDTH: f32 = 1.5;

fn halo() -> Color32 {
    Color32::from_black_alpha(130)
}

/// A filled point handle with an outline and a dark halo.
pub(super) fn paint_point(painter: &egui::Painter, center: Pos2, radius: f32, fill: Color32) {
    painter.circle_filled(center, radius + OUTLINE_WIDTH, halo());
    painter.circle_filled(center, radius, fill);
    painter.circle_stroke(center, radius, Stroke::new(OUTLINE_WIDTH, OUTLINE));
}

/// A hollow ring handle (a rotation knob, a curve handle, a depth cue) with a
/// dark halo on both sides of the stroke.
pub(super) fn paint_ring(painter: &egui::Painter, center: Pos2, radius: f32, stroke: Stroke) {
    painter.circle_stroke(center, radius, Stroke::new(stroke.width + 2.0, halo()));
    painter.circle_stroke(center, radius, stroke);
}

/// A thin connector from a shape to one of its handles.
pub(super) fn paint_stem(painter: &egui::Painter, from: Pos2, to: Pos2, color: Color32) {
    painter.line_segment([from, to], Stroke::new(1.0, color.gamma_multiply(0.72)));
}

/// A rounded value label centered below `anchor` by `offset` points, as shown
/// while a handle is hovered or dragged.
pub(super) fn paint_value_label(painter: &egui::Painter, anchor: Pos2, offset: f32, text: String) {
    let galley = painter.layout_no_wrap(text, egui::FontId::proportional(11.5), Color32::WHITE);
    let pill = Rect::from_center_size(
        anchor + egui::vec2(0.0, offset),
        galley.size() + egui::vec2(12.0, 6.0),
    );
    painter.rect_filled(pill, pill.height() * 0.5, Color32::from_black_alpha(190));
    painter.galley(pill.center() - galley.size() * 0.5, galley, Color32::WHITE);
}
