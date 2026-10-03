use super::{egui, float_param_slider, Ui};
use crate::app::CalibRawApp;
use crate::pipeline::effect_params::FloatParamSpec;
use crate::pipeline::{GeometryTransform, LensGeometryMap};
use crate::ui::preview::SourceProjection;
use std::sync::Arc;

/// Keep the detail sliders full-width even in narrow portrait cards.
pub(super) fn effect_details(ui: &mut Ui, label: &str, body: impl FnOnce(&mut Ui) -> bool) -> bool {
    ui.add_space(crate::ui::theme::SPACE_XS);
    egui::CollapsingHeader::new(label)
        .default_open(false)
        .show_unindented(ui, body)
        .body_returned
        .unwrap_or(false)
}

/// Which frame an effect's 0–100% position is measured in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PositionSpace {
    /// The full, uncropped source image.
    Source,
    /// The developed output, after crop and rotation.
    Output,
}

/// The developed image as the preview shows it (crop, rotation, lens
/// correction). Position pads take its shape and place their marker where the
/// effect lands in the preview.
#[derive(Clone)]
pub(crate) struct EffectFrame {
    pub(super) geometry: GeometryTransform,
    lens: Option<Arc<LensGeometryMap>>,
    source_width: u32,
    source_height: u32,
}

impl EffectFrame {
    pub(crate) fn of(app: &CalibRawApp) -> Self {
        let raw = app.develop.loaded_raw.as_ref();
        Self {
            geometry: app.develop.geometry,
            lens: raw.and_then(|raw| raw.lens_geometry.clone()),
            source_width: raw.map_or(3000, |raw| raw.width),
            source_height: raw.map_or(2000, |raw| raw.height),
        }
    }

    /// An uncropped, unrotated image of the given size.
    #[cfg(test)]
    pub(crate) fn uncropped(source_width: u32, source_height: u32) -> Self {
        Self {
            geometry: GeometryTransform::default(),
            lens: None,
            source_width,
            source_height,
        }
    }

    pub(super) fn output_aspect(&self) -> f32 {
        let (width, height) = self
            .geometry
            .crop_pixel_dimensions(self.source_width, self.source_height);
        width as f32 / height.max(1) as f32
    }

    fn projection(&self, canvas: egui::Rect) -> SourceProjection<'_> {
        SourceProjection::new(
            canvas,
            self.geometry,
            self.lens.as_deref(),
            self.source_width,
            self.source_height,
        )
    }

    /// Where `position` (percent of `space`) appears on a pad drawn in `canvas`.
    fn position_on_pad(
        &self,
        space: PositionSpace,
        canvas: egui::Rect,
        position: [f32; 2],
    ) -> egui::Pos2 {
        let uv = [position[0] / 100.0, position[1] / 100.0];
        match space {
            PositionSpace::Output => egui::pos2(
                egui::lerp(canvas.x_range(), uv[0]),
                egui::lerp(canvas.y_range(), uv[1]),
            ),
            PositionSpace::Source => self.projection(canvas).to_screen(uv),
        }
    }

    /// The position (percent of `space`) under `point` on a pad drawn in `canvas`.
    fn pad_to_position(
        &self,
        space: PositionSpace,
        canvas: egui::Rect,
        point: egui::Pos2,
    ) -> [f32; 2] {
        let uv = match space {
            PositionSpace::Output => [
                (point.x - canvas.left()) / canvas.width().max(1.0),
                (point.y - canvas.top()) / canvas.height().max(1.0),
            ],
            PositionSpace::Source => self.projection(canvas).to_source(point),
        };
        [uv[0] * 100.0, uv[1] * 100.0]
    }
}

pub(super) fn effect_position(
    ui: &mut Ui,
    label: &str,
    position: &mut [f32; 2],
    specs: [FloatParamSpec; 2],
    frame: &EffectFrame,
    space: PositionSpace,
) -> bool {
    ui.push_id(label, |ui| {
        ui.label(label);
        let (_, mut changed) = position_pad(ui, label, position, specs, frame, space);
        changed |= effect_details(ui, "Precise position", |ui| {
            ui.small(match space {
                PositionSpace::Source => {
                    "Percent of the full, uncropped image: left/top is 0, right/bottom is 100."
                }
                PositionSpace::Output => {
                    "Percent of the cropped image: left/top is 0, right/bottom is 100."
                }
            });
            float_param_slider(ui, &mut position[0], specs[0])
                | float_param_slider(ui, &mut position[1], specs[1])
        });
        changed
    })
    .inner
}

/// The largest rectangle of `aspect` (width / height) centered in `area`.
pub(super) fn fit_aspect(area: egui::Rect, aspect: f32) -> egui::Rect {
    let aspect = if aspect.is_finite() && aspect > 0.0 {
        aspect
    } else {
        1.0
    };
    let size = if area.width() > area.height() * aspect {
        egui::vec2(area.height() * aspect, area.height())
    } else {
        egui::vec2(area.width(), area.width() / aspect)
    };
    egui::Rect::from_center_size(area.center(), size)
}

/// The pad keeps a fixed footprint and draws the developed image inside it at
/// the preview's aspect ratio. Precise controls retain the full parameter
/// ranges, including off-image origins, without clamping them just because
/// the pad is drawn.
pub(super) fn position_pad(
    ui: &mut Ui,
    label: &str,
    position: &mut [f32; 2],
    specs: [FloatParamSpec; 2],
    frame: &EffectFrame,
    space: PositionSpace,
) -> (egui::Response, bool) {
    let width = ui.available_width().max(1.0);
    let height = (width * 0.45).clamp(80.0, 116.0);
    let (rect, mut response) =
        ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click_and_drag());
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, ui.is_enabled(), label));
    let canvas = fit_aspect(rect.shrink(8.0_f32.min(width * 0.1)), frame.output_aspect());
    let clamp_to_specs = |value: [f32; 2]| {
        [
            value[0].clamp(specs[0].min, specs[0].max),
            value[1].clamp(specs[1].min, specs[1].max),
        ]
    };
    let before = *position;
    if response.double_clicked() {
        *position = [specs[0].default, specs[1].default];
    } else if response.clicked() || response.dragged() {
        response.request_focus();
        if let Some(pointer) = response.interact_pointer_pos() {
            *position = clamp_to_specs(frame.pad_to_position(space, canvas, canvas.clamp(pointer)));
        }
    }
    if response.has_focus() && ui.is_enabled() {
        let (step, offset) = ui.input(|input| {
            let step = if input.modifiers.shift { 0.1 } else { 0.01 };
            let axis = |negative, positive| {
                f32::from(
                    i8::from(input.key_pressed(positive)) - i8::from(input.key_pressed(negative)),
                )
            };
            (
                step,
                egui::vec2(
                    axis(egui::Key::ArrowLeft, egui::Key::ArrowRight),
                    axis(egui::Key::ArrowUp, egui::Key::ArrowDown),
                ),
            )
        });
        // Arrows move the marker across the pad as drawn, whichever way the
        // crop rotates the underlying image.
        if offset != egui::Vec2::ZERO {
            let marker = frame.position_on_pad(space, canvas, *position);
            let moved = marker + offset * canvas.size() * step;
            // Snap away float noise so stepping lands on whole percents.
            let snapped = frame
                .pad_to_position(space, canvas, moved)
                .map(|value| (value * 100.0).round() / 100.0);
            *position = clamp_to_specs(snapped);
        }
    }
    let changed = *position != before;
    if changed {
        response.mark_changed();
    }
    let visuals = ui.style().interact(&response);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 6.0, ui.visuals().extreme_bg_color);
    painter.rect_stroke(rect, 6.0, visuals.bg_stroke, egui::StrokeKind::Inside);
    painter.rect_filled(canvas, 2.0, ui.visuals().faint_bg_color);
    let grid_stroke = ui.visuals().widgets.noninteractive.bg_stroke;
    painter.rect_stroke(canvas, 2.0, grid_stroke, egui::StrokeKind::Inside);
    for fraction in [0.25, 0.5, 0.75] {
        let x = egui::lerp(canvas.x_range(), fraction);
        let y = egui::lerp(canvas.y_range(), fraction);
        painter.line_segment(
            [egui::pos2(x, canvas.top()), egui::pos2(x, canvas.bottom())],
            grid_stroke,
        );
        painter.line_segment(
            [egui::pos2(canvas.left(), y), egui::pos2(canvas.right(), y)],
            grid_stroke,
        );
    }
    let marker = frame.position_on_pad(space, canvas, *position);
    let marker_color = if ui.is_enabled() {
        ui.visuals().selection.stroke.color
    } else {
        ui.visuals().weak_text_color()
    };
    let marker_inside = canvas.expand(0.5).contains(marker);
    let marker = canvas.clamp(marker);
    painter.circle_filled(marker, 5.0, marker_color);
    painter.circle_stroke(marker, 7.0, egui::Stroke::new(1.0, marker_color));
    if !marker_inside {
        painter.text(
            canvas.center(),
            egui::Align2::CENTER_CENTER,
            "Position outside frame",
            egui::TextStyle::Small.resolve(ui.style()),
            ui.visuals().text_color(),
        );
    }
    let response = response.on_hover_text(format!(
        "{label}: {:.1}%, {:.1}%\nDrag to position. Arrow keys move 1%; Shift moves 10%.\nDouble-click to reset. Use Precise position to enter coordinates.",
        position[0], position[1],
    ));
    (response, changed)
}
