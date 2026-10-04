use crate::app::{
    CalibRawApp, CropDragState, CropHandle, MaskDragState, MaskOverlayBlink, OverlayRasterKey,
    SidebarTab, StraightenDragState,
};
use crate::pipeline::{
    BrushMode, GeometryTransform, LensGeometryMap, MaskCombineMode, MaskGeometry, MaskKind,
    RetouchStroke,
};
use crate::ui::mask_component_color;
use eframe::egui::{self, Color32, Mesh, Pos2, Rect, Sense, Shape, Stroke, Ui};
use std::sync::Arc;

pub(crate) const MIN_PREVIEW_ZOOM: f32 = if cfg!(target_os = "android") {
    0.25
} else {
    0.70
};
pub(crate) const MAX_PREVIEW_ZOOM: f32 = 32.0;

/// Borrow the active lens geometry; tools can clone the Arc before editing state.
pub(super) fn loaded_lens_geometry(app: &CalibRawApp) -> Option<&Arc<LensGeometryMap>> {
    app.develop
        .loaded_raw
        .as_ref()
        .and_then(|raw| raw.lens_geometry.as_ref())
}

fn physical_pixels_per_point(ctx: &egui::Context) -> f32 {
    let native = ctx.input(|input| input.viewport().native_pixels_per_point);
    native
        .unwrap_or_else(|| ctx.pixels_per_point())
        .max(ctx.pixels_per_point())
        .max(0.1)
}

fn white_balance_picker_owns_canvas(sidebar_tab: SidebarTab, picker_active: bool) -> bool {
    sidebar_tab == SidebarTab::Adjustments && picker_active
}

fn point_color_picker_owns_canvas(sidebar_tab: SidebarTab, picker_active: bool) -> bool {
    matches!(sidebar_tab, SidebarTab::Adjustments | SidebarTab::Masks) && picker_active
}

fn show_centered_preview_message(
    ui: &mut Ui,
    available: egui::Vec2,
    backdrop: Color32,
    title: &str,
    detail: Option<&str>,
    spinning: bool,
) {
    let (rect, _) = ui.allocate_exact_size(available, Sense::hover());
    let text_color = crate::ui::theme::text_on_backdrop(backdrop);
    let title_offset = if detail.is_some() { -11.0 } else { 8.0 };
    let spinner_offset = if detail.is_some() { -40.0 } else { -20.0 };
    if spinning {
        let spinner_rect = Rect::from_center_size(
            rect.center() + egui::vec2(0.0, spinner_offset),
            egui::Vec2::splat(20.0),
        );
        ui.put(spinner_rect, egui::Spinner::new().size(18.0));
    }
    ui.painter_at(rect).text(
        rect.center() + egui::vec2(0.0, title_offset),
        egui::Align2::CENTER_CENTER,
        title,
        egui::FontId::proportional(20.0),
        text_color,
    );
    if let Some(detail) = detail {
        ui.painter_at(rect).text(
            rect.center() + egui::vec2(0.0, 15.0),
            egui::Align2::CENTER_CENTER,
            detail,
            egui::FontId::proportional(14.0),
            text_color.gamma_multiply(0.74),
        );
    }
}

mod canvas;
mod interaction;
mod overlays;
mod tools;
mod transform;
mod viewport;

pub(crate) use transform::{shortest_angle_delta, source_angle_from, SourceProjection};
pub(crate) use viewport::{PreviewDetailImage, PreviewPlaceholder, PreviewViewportInput};

use canvas::*;
use interaction::*;
use overlays::*;
use transform::*;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod mask_regression_tests;

#[cfg(test)]
mod inpaint_regression_tests;

pub(crate) struct Preview;

/// Where the image sits on screen this frame, shared by the preview tools.
#[derive(Clone, Copy)]
pub(super) struct PreviewLayout {
    /// The whole image at the current zoom; it may extend past the viewport.
    image_rect: Rect,
    /// The part of the image inside the viewport.
    visible_rect: Rect,
    /// The whole preview viewport, including space around the image.
    viewport_rect: Rect,
    source_width: u32,
    source_height: u32,
}

impl PreviewLayout {
    fn projection<'a>(
        self,
        geometry: GeometryTransform,
        lens: Option<&'a LensGeometryMap>,
    ) -> SourceProjection<'a> {
        SourceProjection::new(
            self.image_rect,
            geometry,
            lens,
            self.source_width,
            self.source_height,
        )
    }
}

/// Marks the canvas while a rendered JPEG, PNG or HEIC is open, so it is never
/// mistaken for a RAW with its wider editing latitude.
fn paint_rendered_source_pill(
    painter: &egui::Painter,
    canvas: Rect,
    format: crate::pipeline::RenderedImageFormat,
) {
    let text = painter.layout_no_wrap(
        format!("Editing {}", format.label()),
        egui::FontId::proportional(11.5),
        Color32::WHITE,
    );
    let pill = Rect::from_min_size(
        canvas.left_top() + egui::vec2(12.0, 12.0),
        text.size() + egui::vec2(16.0, 8.0),
    );
    painter.rect_filled(pill, 6.0, Color32::from_black_alpha(150));
    painter.galley(pill.center() - text.size() * 0.5, text, Color32::WHITE);
}
